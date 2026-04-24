//! Top-level application struct that owns and coordinates all subsystems.
//!
//! [`App`] is responsible for constructing the audio pipeline: it queries the
//! selected input device for its hardware capabilities, sizes the ringbufs
//! accordingly, spawns the recorder, analyser, WebSocket server, and (in
//! calibration mode) the synthetic generator threads, then hands control to
//! the [`Controller`] for interactive keyboard handling.
//!
//! Shared runtime state is carried by [`AppState`], which holds a set of
//! [`std::sync::atomic`] flags that the controller writes and the worker threads
//! observe. On shutdown, all threads are signalled to stop and given a bounded
//! grace period to exit, which prevents one stalled worker from hanging the
//! main thread indefinitely.

use crate::config::{validate_vocoder_sample_rate, AppConfig};
use crate::controller::Controller;
use crate::dsp::{vocoder::VOCODER_BANDS, DisplayPayload, RawPayload};
use crate::managers::audio::{ChannelMode, StreamSink};
use crate::managers::{Generator, Input, Mapper, Processor, Server, Specs, Writer};
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Utf8Bytes;

/// Safety buffer for the record ringbuf, absorbs disk write jitter.
const RECORD_BUFFER_MS: u32 = 5000;

/// Safety buffer for the analyse ringbuf, headroom for analysis accumulation.
const ANALYSE_BUFFER_MS: u32 = 500;

/// Poll interval while waiting for a worker thread to finish.
const SHUTDOWN_POLL_MS: u64 = 10;

/// Grace period for the generator thread, which wakes on a 10 ms cadence.
const GENERATOR_SHUTDOWN_TIMEOUT_MS: u64 = 250;

/// Grace period for the analyser thread to drain and release the mapper input.
const ANALYSER_SHUTDOWN_TIMEOUT_MS: u64 = 1_000;

/// Grace period for the mapper thread to observe analyser channel closure.
const MAPPER_SHUTDOWN_TIMEOUT_MS: u64 = 1_000;

/// Grace period for the server thread to finish its bounded accept and client shutdown.
const SERVER_SHUTDOWN_TIMEOUT_MS: u64 = 1_500;

/// Grace period for the recorder to drain the record ringbuf and finalise the WAV file.
const RECORDER_SHUTDOWN_TIMEOUT_MS: u64 = RECORD_BUFFER_MS as u64 + 2_000;

/// Shared application state flags for cross-thread synchronisation.
pub struct AppState {
    pub record_ring_overflow_events: AtomicUsize,
    pub is_broadcasting_websocket: AtomicBool,
    pub is_recording: AtomicBool,
    pub is_analysing: AtomicBool,
    pub keep_running: AtomicBool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            record_ring_overflow_events: AtomicUsize::new(0),
            is_broadcasting_websocket: AtomicBool::new(false),
            is_recording: AtomicBool::new(false),
            is_analysing: AtomicBool::new(false),
            keep_running: AtomicBool::new(true),
        }
    }
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinOutcome {
    Joined,
    TimedOut,
    Panicked,
}

#[derive(Debug, Clone, Copy)]
struct ShutdownWorker {
    name: &'static str,
    success_message: &'static str,
    timeout_ms: u64,
}

impl ShutdownWorker {
    const GENERATOR: Self = Self {
        name: "generator",
        success_message: ">> Generator shutdown complete",
        timeout_ms: GENERATOR_SHUTDOWN_TIMEOUT_MS,
    };

    const ANALYSER: Self = Self {
        name: "analyser",
        success_message: ">> Analyser shutdown complete",
        timeout_ms: ANALYSER_SHUTDOWN_TIMEOUT_MS,
    };

    const MAPPER: Self = Self {
        name: "mapper",
        success_message: ">> Mapper shutdown complete",
        timeout_ms: MAPPER_SHUTDOWN_TIMEOUT_MS,
    };

    const SERVER: Self = Self {
        name: "server",
        success_message: ">> Server shutdown complete",
        timeout_ms: SERVER_SHUTDOWN_TIMEOUT_MS,
    };

    const RECORDER: Self = Self {
        name: "recorder",
        success_message: ">> Recorder shutdown complete",
        timeout_ms: RECORDER_SHUTDOWN_TIMEOUT_MS,
    };

    fn timeout(self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

#[derive(Default)]
struct WorkerThreads {
    recorder: Option<JoinHandle<()>>,
    analyser: Option<JoinHandle<()>>,
    mapper: Option<JoinHandle<()>>,
    server: Option<JoinHandle<()>>,
    generator: Option<JoinHandle<()>>,
}

impl WorkerThreads {
    fn shutdown(&mut self) {
        Self::shutdown_worker(ShutdownWorker::GENERATOR, &mut self.generator);
        Self::shutdown_worker(ShutdownWorker::ANALYSER, &mut self.analyser);
        Self::shutdown_worker(ShutdownWorker::MAPPER, &mut self.mapper);
        Self::shutdown_worker(ShutdownWorker::SERVER, &mut self.server);
        Self::shutdown_worker(ShutdownWorker::RECORDER, &mut self.recorder);
    }

    fn shutdown_worker(worker: ShutdownWorker, handle: &mut Option<JoinHandle<()>>) {
        let Some(handle) = handle.take() else {
            return;
        };

        if Self::join_with_timeout(worker, handle) == JoinOutcome::Joined {
            log::info!("{}", worker.success_message);
        }
    }

    fn join_with_timeout(worker: ShutdownWorker, handle: JoinHandle<()>) -> JoinOutcome {
        let deadline = Instant::now() + worker.timeout();

        loop {
            if handle.is_finished() {
                return if let Ok(()) = handle.join() {
                    JoinOutcome::Joined
                } else {
                    log::error!("Worker thread '{}' panicked during shutdown", worker.name);
                    JoinOutcome::Panicked
                };
            }

            let now = Instant::now();
            if now >= deadline {
                log::error!(
                    "Worker thread '{}' did not stop within {} ms, detaching",
                    worker.name,
                    worker.timeout_ms
                );
                return JoinOutcome::TimedOut;
            }

            let remaining = deadline.saturating_duration_since(now);
            thread::sleep(remaining.min(Duration::from_millis(SHUTDOWN_POLL_MS)));
        }
    }
}

pub struct App {
    // Kept alive until dropped. Dropping the stream stops audio capture,
    // and wraps the device in an Option so we can drop it on command.
    input_device: Option<Input>,

    /// Shared atomic flags for cross-thread coordination.
    state: Arc<AppState>,

    /// All worker threads owned by the application runtime.
    workers: WorkerThreads,

    /// Keyboard input handler, drives all runtime state transitions.
    controller: Controller,

    /// Tracks whether shutdown has already started, so drop remains idempotent.
    shutdown_started: bool,
}

impl App {
    /// Constructs the audio pipeline from the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the audio device cannot be opened, the input stream
    /// cannot be started, or the WebSocket server cannot bind to its address.
    ///
    /// # Panics
    ///
    /// Panics if `device_index` is `None` when not in calibration mode. This is
    /// guarded by `AppConfig::TryFrom`, so it should never occur in practice.
    pub fn new(config: AppConfig) -> Result<Self> {
        let state = Arc::new(AppState::new());
        let mut input_device = Input::new();

        let addr = config.addr;
        let max_clients = config.max_clients;
        let bit_depth = config.bit_depth;
        let filename_pattern = config.filename_pattern;
        let test_hz = config.test_hz;
        let test_sweep = config.test_sweep;
        let device_index = config.device_index;
        let vocoder_config = config.vocoder_config;
        let no_browser_origin = config.no_browser_origin;
        let broadcast_rate = config.broadcast_rate;

        let calibration_mode = test_hz.is_some() || test_sweep.is_some();

        // In calibration mode no hardware is needed. Use standard CD-quality defaults.
        // Otherwise query the device for its native sample rate and channel count.
        let (specs, device_handle) = if calibration_mode {
            (
                Specs {
                    sample_rate: 44100,
                    channels: 2,
                },
                None,
            )
        } else {
            // device_index is guaranteed Some when not in calibration mode by AppConfig::TryFrom.
            let idx = device_index.expect("device_index required in hardware mode");
            let (d, c, s) = input_device.get_device(idx)?;
            (s, Some((d, c)))
        };
        validate_vocoder_sample_rate(vocoder_config.freq_high, specs.sample_rate)?;

        let (record_tx, record_rx) = Input::create_audio_buffer_pair(specs, RECORD_BUFFER_MS);
        let (analyse_tx, analyse_rx) = Input::create_audio_buffer_pair(specs, ANALYSE_BUFFER_MS);
        let channels = specs.channels as usize;
        let (raw_tx, raw_rx) = watch::channel(RawPayload::new(channels, VOCODER_BANDS));
        let initial_display = serde_json::to_string(&DisplayPayload::new(channels))
            .expect("failed to serialise initial display payload");
        let (display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display));

        let mut generator_thread = None;

        if calibration_mode {
            generator_thread = Some(Generator::spawn(
                test_hz,
                test_sweep,
                specs.sample_rate,
                specs.channels,
                record_tx,
                analyse_tx,
                state.clone(),
            ));
        } else {
            // device_handle is guaranteed Some when not in calibration mode.
            let (device, config) = device_handle.expect("device present in hardware mode");
            input_device.start_stream(
                &device,
                &config,
                StreamSink {
                    tx: record_tx,
                    mode: ChannelMode::All,
                },
                StreamSink {
                    tx: analyse_tx,
                    mode: ChannelMode::All,
                },
                state.clone(),
            )?;
        }

        // Spin up the recorder thread to drain the record ringbuf to disk.
        let recorder = Writer::new(filename_pattern);
        let recorder_thread = Some(recorder.spawn(record_rx, bit_depth, specs, state.clone()));

        // Spin up the analyser thread to drain the analyse ringbuf and publish DSP results.
        let analyser = Processor::new(vocoder_config);
        let analyser_thread = Some(analyser.spawn(analyse_rx, raw_tx, specs, state.clone()));

        // Spin up the mapper thread to map raw vocoder bins to display resolution.
        let mapper_thread = Some(Mapper::spawn(
            raw_rx,
            display_tx,
            channels,
            state.clone(),
            broadcast_rate,
        ));

        // Spin up the WebSocket server thread.
        let server = Server::new(addr, no_browser_origin, max_clients);
        let server_thread = Some(server.spawn(display_rx, state.clone())?);

        let controller = Controller::new(state.clone());

        Ok(Self {
            input_device: Some(input_device),
            state,
            workers: WorkerThreads {
                recorder: recorder_thread,
                analyser: analyser_thread,
                mapper: mapper_thread,
                server: server_thread,
                generator: generator_thread,
            },
            controller,
            shutdown_started: false,
        })
    }

    /// Hands control to the interactive controller, blocking until shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if the controller encounters a terminal or I/O failure.
    pub fn run(&self) -> Result<()> {
        self.controller.run()
    }

    /// Runs the controller loop and always performs shutdown afterwards.
    ///
    /// This keeps the main entry point linear while ensuring teardown still
    /// happens when the controller exits with an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the controller loop exits with a terminal or I/O
    /// failure. Shutdown is still attempted before the error is returned.
    pub fn run_until_shutdown(&mut self) -> Result<()> {
        let run_result = self.run();
        self.shutdown();
        run_result
    }

    /// Signals all workers to stop and waits a bounded time for each one.
    ///
    /// This method is idempotent. It should be called explicitly from the main
    /// execution path, while [`Drop`] remains as a best effort fallback.
    pub fn shutdown(&mut self) {
        if self.shutdown_started {
            return;
        }
        self.shutdown_started = true;

        log::info!("Shutdown started");

        self.input_device.take();
        log::info!("> Device shutdown complete");

        // Signal every worker before waiting on any of them.
        self.state.keep_running.store(false, Ordering::Release);

        self.workers.shutdown();

        log::info!("Shutdown complete");
    }
}

impl Drop for App {
    // Keep drop lightweight and idempotent by delegating to the explicit
    // shutdown path. This still gives callers a best effort fallback when
    // they do not call shutdown() themselves.
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    };

    #[test]
    fn join_with_timeout_joins_completed_thread() {
        let handle = thread::spawn(|| {});

        assert_eq!(
            WorkerThreads::join_with_timeout(ShutdownWorker::GENERATOR, handle),
            JoinOutcome::Joined
        );
    }

    #[test]
    fn join_with_timeout_reports_panic() {
        let handle = thread::spawn(|| panic!("boom"));

        assert_eq!(
            WorkerThreads::join_with_timeout(ShutdownWorker::ANALYSER, handle),
            JoinOutcome::Panicked
        );
    }

    #[test]
    fn join_with_timeout_times_out_without_blocking_forever() {
        let keep_running = Arc::new(AtomicBool::new(true));
        let thread_state = keep_running.clone();
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            while thread_state.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            tx.send(()).expect("thread exit signal should be delivered");
        });

        assert_eq!(
            WorkerThreads::join_with_timeout(
                ShutdownWorker {
                    name: "blocking",
                    success_message: "",
                    timeout_ms: 20,
                },
                handle,
            ),
            JoinOutcome::TimedOut
        );

        keep_running.store(false, Ordering::Release);
        rx.recv_timeout(Duration::from_millis(200))
            .expect("detached thread should still exit once signalled");
    }

    #[test]
    fn shutdown_is_idempotent_and_drop_safe() {
        let state = Arc::new(AppState::new());
        let exit_count = Arc::new(AtomicUsize::new(0));
        let thread_state = state.clone();
        let thread_exit_count = exit_count.clone();

        let generator_thread = Some(thread::spawn(move || {
            while thread_state.keep_running.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            thread_exit_count.fetch_add(1, Ordering::AcqRel);
        }));

        let mut app = App {
            input_device: None,
            state: state.clone(),
            workers: WorkerThreads {
                generator: generator_thread,
                ..WorkerThreads::default()
            },
            controller: Controller::new(state.clone()),
            shutdown_started: false,
        };

        app.shutdown();
        app.shutdown();

        assert!(app.shutdown_started);
        assert!(!state.keep_running.load(Ordering::Acquire));
        assert_eq!(exit_count.load(Ordering::Acquire), 1);
        assert!(app.workers.generator.is_none());

        drop(app);

        assert_eq!(exit_count.load(Ordering::Acquire), 1);
    }
}
