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
use crate::worker::WorkerThreads;
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Utf8Bytes;

/// Safety buffer for the record ringbuf, absorbs disk write jitter.
pub(crate) const RECORD_BUFFER_MS: u32 = 5000;

/// Safety buffer for the analyse ringbuf, headroom for analysis accumulation.
const ANALYSE_BUFFER_MS: u32 = 500;

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
        let stream_state = Arc::clone(&state);
        let recorder_state = Arc::clone(&state);
        let analyser_state = Arc::clone(&state);
        let mapper_state = Arc::clone(&state);
        let server_state = Arc::clone(&state);
        let generator_state = Arc::clone(&state);
        let controller_state = Arc::clone(&state);

        let mut input_device = Input::new();
        let calibration_mode = config.test_hz.is_some() || config.test_sweep.is_some();

        // Hardware & Device Resolution.
        let (hw_specs, device_handle) = if calibration_mode {
            (
                Specs {
                    sample_rate: 44100,
                    channels: 2,
                },
                None,
            )
        } else {
            let idx = config
                .device_index
                .expect("device_index required in hardware mode");
            let (d, c, s) = input_device.get_device(idx)?;
            (s, Some((d, c)))
        };
        validate_vocoder_sample_rate(config.vocoder_config.freq_high, hw_specs.sample_rate)?;

        // Specs & Channel Modes.
        let mut recorder_specs = hw_specs;
        let mut analyser_specs = hw_specs;

        let record_mode = ChannelMode::resolve(config.record_channels, &mut recorder_specs);
        let analyse_mode = ChannelMode::resolve(config.analyse_channels, &mut analyser_specs);

        // Allocate Inter-thread Channels (Ringbufs & Watch).
        let (record_tx, record_rx) =
            Input::create_audio_buffer_pair(recorder_specs, RECORD_BUFFER_MS);
        let (analyse_tx, analyse_rx) =
            Input::create_audio_buffer_pair(analyser_specs, ANALYSE_BUFFER_MS);

        let display_channels = analyser_specs.channels as usize;
        let (raw_tx, raw_rx) = watch::channel(RawPayload::new(display_channels, VOCODER_BANDS));

        let initial_display = serde_json::to_string(&DisplayPayload::new(display_channels))
            .expect("failed to serialise initial display payload");
        let (display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display));

        // Spawn Producers. Hardware or a Generator is spawned.
        let mut generator_thread = None;
        if calibration_mode {
            generator_thread = Some(Generator::spawn(
                config.test_hz,
                config.test_sweep,
                hw_specs.sample_rate,
                hw_specs.channels,
                record_tx,
                analyse_tx,
                generator_state,
            ));
        } else {
            let (device, stream_config) = device_handle.expect("device present in hardware mode");
            input_device.start_stream(
                &device,
                &stream_config,
                StreamSink {
                    tx: record_tx,
                    mode: record_mode,
                },
                StreamSink {
                    tx: analyse_tx,
                    mode: analyse_mode,
                },
                stream_state,
            )?;
        }

        // Spawn Worker Threads.
        let recorder = Writer::new(config.filename_pattern);
        let recorder_thread =
            Some(recorder.spawn(record_rx, config.bit_depth, recorder_specs, recorder_state));

        let analyser = Processor::new(config.vocoder_config);
        let analyser_thread =
            Some(analyser.spawn(analyse_rx, raw_tx, analyser_specs, analyser_state));

        let mapper_thread = Some(Mapper::spawn(
            raw_rx,
            display_tx,
            display_channels,
            mapper_state,
            config.broadcast_rate,
        ));

        let server = Server::new(config.addr, config.no_browser_origin, config.max_clients);
        let server_thread = Some(server.spawn(display_rx, server_state)?);

        Ok(Self {
            input_device: Some(input_device),
            state,
            workers: WorkerThreads::new(
                generator_thread,
                analyser_thread,
                mapper_thread,
                server_thread,
                recorder_thread,
            ),
            controller: Controller::new(controller_state),
            shutdown_started: false,
        })
    }

    /// Returns a reference to the shared application state.
    #[must_use]
    pub fn state(&self) -> &Arc<AppState> {
        &self.state
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
        log::info!("- Device shutdown complete");

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
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::thread;
    use std::time::Duration;

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
            workers: WorkerThreads::new(generator_thread, None, None, None, None),
            controller: Controller::new(state.clone()),
            shutdown_started: false,
        };

        app.shutdown();
        app.shutdown();

        assert!(app.shutdown_started);
        assert!(!state.keep_running.load(Ordering::Acquire));
        assert_eq!(exit_count.load(Ordering::Acquire), 1);
        assert!(app.workers.0.iter().all(Option::is_none));

        drop(app);

        assert_eq!(exit_count.load(Ordering::Acquire), 1);
    }
}
