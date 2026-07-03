//! Top-level application struct that owns and coordinates all subsystems.
//!
//! [`App`] is responsible for constructing the audio pipeline: it queries the
//! selected input device for its hardware capabilities, sizes the ringbufs
//! accordingly, spawns the analyser, WebSocket server, and (in calibration
//! mode) the synthetic generator threads, then hands control to the
//! [`Controller`] for interactive keyboard handling.
//!
//! Shared runtime state is carried by [`AppState`], which holds a set of
//! [`std::sync::atomic`] flags that the controller writes and the worker threads
//! observe. On shutdown, all threads are signalled to stop and given a bounded
//! grace period to exit, which prevents one stalled worker from hanging the
//! main thread indefinitely.

use crate::config::{validate_vocoder_sample_rate, AppConfig, AppConfigError};
use crate::controller::Controller;
use crate::dsp::{vocoder::VOCODER_BANDS, DisplayPayload, RawPayload};
use crate::managers::audio::{ChannelMode, StreamSink};
use crate::managers::{Generator, Input, Mapper, OscSender, Processor, Server, Specs};
use crate::worker::WorkerThreads;
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::watch;

/// Safety buffer for the analyse ringbuf, headroom for analysis accumulation.
const ANALYSE_BUFFER_MS: u32 = 500;

/// Builds the calibration mode announcement for the given test signal
/// configuration. Exactly one of `test_hz` or `test_sweep` is expected to
/// be `Some` when this is called, calibration mode is only entered when
/// at least one is set.
fn calibration_announcement(test_hz: Option<f32>, test_sweep: Option<f32>) -> String {
    if let Some(hz) = test_hz {
        format!("Calibration mode: fixed tone at {hz} Hz")
    } else if let Some(sweep) = test_sweep {
        format!("Calibration mode: sweep at {sweep} Hz LFO rate")
    } else {
        "Calibration mode".to_string()
    }
}

/// The resolved input source for the audio pipeline: either a real hardware
/// device or a synthetic calibration generator. Resolved once in `App::new`
/// from `AppConfig`'s calibration fields, then matched on wherever behaviour
/// previously branched on a `calibration_mode` bool re-derived from those
/// same fields.
enum InputSource {
    Calibration,
    Hardware(cpal::Device, cpal::SupportedStreamConfig),
}

/// Shared application state flags for cross-thread synchronisation.
pub struct AppState {
    pub is_active: AtomicBool,
    pub keep_running: AtomicBool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            is_active: AtomicBool::new(true),
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
    /// Panics if worker thread startup fails internally.
    pub fn new(config: AppConfig) -> Result<Self> {
        let state = Arc::new(AppState::new());
        let stream_state = Arc::clone(&state);
        let analyser_state = Arc::clone(&state);
        let mapper_state = Arc::clone(&state);
        let server_state = Arc::clone(&state);
        let osc_state = Arc::clone(&state);
        let generator_state = Arc::clone(&state);
        let controller_state = Arc::clone(&state);

        let mut input_device = Input::new();
        let calibration_mode = config.test_hz.is_some() || config.test_sweep.is_some();
        let (hw_specs, input_source) =
            Self::resolve_hardware(&config, calibration_mode, &mut input_device)?;
        validate_vocoder_sample_rate(config.vocoder_config.freq_high, hw_specs.sample_rate)?;

        // Validate that all requested channel indices are within the hardware's capacity.
        // This check does not apply in calibration mode, where no real device is involved.
        match &input_source {
            InputSource::Calibration => {}
            InputSource::Hardware(..) => {
                if let Some(&idx) = config
                    .analyse_channels
                    .as_deref()
                    .map(<[u16]>::iter)
                    .and_then(Iterator::max)
                {
                    if idx >= hw_specs.channels {
                        anyhow::bail!(AppConfigError::ChannelIndexOutOfRange {
                            idx,
                            channels: hw_specs.channels,
                        });
                    }
                }
            }
        }

        // Specs & Channel Modes.
        let mut analyser_specs = hw_specs;

        let analyse_mode = ChannelMode::resolve(config.analyse_channels, &mut analyser_specs);

        // Allocate Inter-thread Channels (Ringbufs & Watch).
        let (analyse_tx, analyse_rx) =
            Input::create_audio_buffer_pair(analyser_specs, ANALYSE_BUFFER_MS);

        let display_channels = analyser_specs.channels as usize;
        let (raw_tx, raw_rx) = watch::channel(RawPayload::new(display_channels, VOCODER_BANDS));
        let (display_tx, display_rx) = watch::channel(DisplayPayload::new(display_channels));

        // Spawn Producers. Hardware or a Generator is spawned.
        let mut generator_thread = None;
        match input_source {
            InputSource::Calibration => {
                log::info!(
                    "{}",
                    calibration_announcement(config.test_hz, config.test_sweep)
                );
                generator_thread = Some(Generator::spawn(
                    config.test_hz,
                    config.test_sweep,
                    hw_specs.sample_rate,
                    hw_specs.channels,
                    analyse_tx,
                    generator_state,
                ));
            }
            InputSource::Hardware(device, stream_config) => {
                input_device.start_stream(
                    &device,
                    &stream_config,
                    StreamSink {
                        tx: analyse_tx,
                        mode: analyse_mode,
                    },
                    &stream_state,
                )?;
            }
        }

        // Spawn Worker Threads.
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
        let server_thread = Some(server.spawn(display_rx.clone(), server_state)?);
        log::info!("WebSocket server listening on ws://{}", config.addr);

        // Spawn the OSC sender when a target address is configured.
        let osc_sender_thread = if let Some(addr) = config.osc_addr {
            let sender = OscSender::new(addr);
            let thread = Some(sender.spawn(display_rx, display_channels, osc_state)?);
            log::info!("OSC sender transmitting to udp://{addr}");
            thread
        } else {
            None
        };

        Ok(Self {
            input_device: Some(input_device),
            state,
            workers: WorkerThreads::new(
                generator_thread,
                analyser_thread,
                mapper_thread,
                server_thread,
                osc_sender_thread,
            ),
            controller: Controller::new(config.controller_mode, controller_state),
            shutdown_started: false,
        })
    }

    /// Returns hardware specs and a resolved [`InputSource`], either calibration-mode
    /// defaults or a real device handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the device cannot be resolved or queried.
    ///
    /// # Panics
    ///
    /// Panics if `device_name_match` is `None` when not in calibration mode.
    /// This is guarded by `AppConfig::TryFrom`, so it should never occur in practice.
    fn resolve_hardware(
        config: &AppConfig,
        calibration_mode: bool,
        input: &mut Input,
    ) -> Result<(Specs, InputSource)> {
        if calibration_mode {
            return Ok((
                Specs {
                    sample_rate: 44100,
                    channels: 2,
                },
                InputSource::Calibration,
            ));
        }

        let name_query = config
            .device_name_match
            .as_deref()
            .expect("device_name_match is required in hardware mode");
        let (device, stream_config, specs) = input.get_device(name_query)?;

        Ok((specs, InputSource::Hardware(device, stream_config)))
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
    use crate::ControllerMode;
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
            controller: Controller::new(ControllerMode::Term, state.clone()),
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

    #[test]
    fn calibration_announcement_describes_fixed_tone() {
        assert_eq!(
            calibration_announcement(Some(440.0), None),
            "Calibration mode: fixed tone at 440 Hz"
        );
    }

    #[test]
    fn calibration_announcement_describes_sweep() {
        assert_eq!(
            calibration_announcement(None, Some(0.1)),
            "Calibration mode: sweep at 0.1 Hz LFO rate"
        );
    }

    #[test]
    fn resolve_hardware_in_calibration_mode_returns_defaults() {
        let config = AppConfig::default();
        let mut input = Input::new();

        let (specs, input_source) = App::resolve_hardware(&config, true, &mut input)
            .expect("resolve_hardware should succeed in calibration mode");

        assert_eq!(specs.sample_rate, 44100);
        assert_eq!(specs.channels, 2);
        assert!(matches!(input_source, InputSource::Calibration));
    }
}
