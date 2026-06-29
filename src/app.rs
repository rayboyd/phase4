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
use tokio_tungstenite::tungstenite::Utf8Bytes;

/// Safety buffer for the analyse ringbuf, headroom for analysis accumulation.
const ANALYSE_BUFFER_MS: u32 = 500;

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
    /// Panics if the initial display payload cannot be serialised to JSON.
    /// This is a programmer error and should never occur in practice.
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
        let (hw_specs, device_handle) =
            Self::resolve_hardware(&config, calibration_mode, &mut input_device)?;
        validate_vocoder_sample_rate(config.vocoder_config.freq_high, hw_specs.sample_rate)?;

        // Validate that all requested channel indices are within the hardware's capacity.
        // This check is skipped in calibration mode, where no real device is involved.
        if !calibration_mode {
            if let Some(indices) = &config.analyse_channels {
                if let Some(&idx) = indices.iter().max() {
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

        let initial_display = serde_json::to_string(&DisplayPayload::new(display_channels))
            .expect("failed to serialise initial display payload");
        let (display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display));

        // OSC typed channel, only allocated when an OSC target address is configured.
        let (osc_tx, osc_display_rx) = if config.osc_addr.is_some() {
            let (tx, rx) = watch::channel(DisplayPayload::new(display_channels));
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        // Spawn Producers. Hardware or a Generator is spawned.
        let mut generator_thread = None;
        if calibration_mode {
            generator_thread = Some(Generator::spawn(
                config.test_hz,
                config.test_sweep,
                hw_specs.sample_rate,
                hw_specs.channels,
                analyse_tx,
                generator_state,
            ));
        } else {
            let (device, stream_config) = device_handle.expect("device present in hardware mode");
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

        // Spawn Worker Threads.
        let analyser = Processor::new(config.vocoder_config);
        let analyser_thread =
            Some(analyser.spawn(analyse_rx, raw_tx, analyser_specs, analyser_state));

        let mapper_thread = Some(Mapper::spawn(
            raw_rx,
            display_tx,
            osc_tx,
            display_channels,
            mapper_state,
            config.broadcast_rate,
        ));

        let server = Server::new(config.addr, config.no_browser_origin, config.max_clients);
        let server_thread = Some(server.spawn(display_rx, server_state)?);

        // Spawn the OSC sender when a target address is configured.
        let osc_sender_thread = if let (Some(addr), Some(rx)) = (config.osc_addr, osc_display_rx) {
            let sender = OscSender::new(addr);
            Some(sender.spawn(rx, display_channels, osc_state)?)
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
            controller: Controller::new(controller_state),
            shutdown_started: false,
        })
    }

    /// Returns hardware specs and a device handle, or calibration-mode defaults
    /// when no real device is needed.
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
    ) -> Result<(Specs, Option<(cpal::Device, cpal::SupportedStreamConfig)>)> {
        if calibration_mode {
            return Ok((
                Specs {
                    sample_rate: 44100,
                    channels: 2,
                },
                None,
            ));
        }

        let name_query = config
            .device_name_match
            .as_deref()
            .expect("device_name_match is required in hardware mode");
        let (device, stream_config, specs) = input.get_device(name_query)?;

        Ok((specs, Some((device, stream_config))))
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
