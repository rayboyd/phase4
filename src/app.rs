//! Top-level application struct that owns and coordinates all subsystems.
//!
//! [`App`] is assembled by the internal bootstrap function, which resolves
//! input and output configuration, validates DSP coefficients, sizes the
//! analysis ring buffer and starts the configured pipeline workers. `App` then
//! hands control to the [`Controller`] for interactive keyboard handling.
//!
//! Shared runtime state is carried by [`AppState`], which holds the shutdown
//! flag and MIDI atomics used by the controller and worker threads. Shutdown
//! drops the input stream, clears the flag, then joins each worker with a
//! grace period so one stalled worker cannot hang the main thread. The
//! stream drop itself runs inside the device driver and has no timeout.

use crate::bootstrap::bootstrap;
use crate::config::AppConfig;
use crate::config::{ConfigMidiInput, OutputConfig};
use crate::controller::Controller;
use crate::headless::{ReadyOutputs, ReadyReport};
use crate::managers::{Input, MIDI_TRANSPORT_NONE};
use crate::worker::WorkerThreads;
use anyhow::{Context, Result};
use signal_hook::consts::{SIGINT, SIGTERM};
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
    Arc,
};
use std::time::Duration;

/// How often the headless wait loop checks for a shutdown request.
const HEADLESS_POLL_RATE_MS: u64 = 50;

/// Shared application state flags for cross-thread synchronisation.
pub struct AppState {
    /// Signals every worker thread to exit.
    /// Set false by the controller (Ctrl+C) or `App::shutdown`.
    pub keep_running: AtomicBool,

    /// Last MIDI transport event seen, one of the `MIDI_TRANSPORT_*` codes.
    /// Written by the MIDI callback or synthetic clock, read and cleared by the mapper
    /// each time it broadcasts a frame.
    pub midi_last_transport: AtomicU8,

    /// MIDI 1/16 note steps derived from incoming MIDI clock ticks.
    ///
    /// Count since Start or initialisation, written by the MIDI callback or
    /// synthetic clock and read by the mapper. Resets on Start, wraps on u32
    /// overflow, and continues if clock ticks arrive while transport is stopped.
    pub midi_steps: AtomicU32,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            keep_running: AtomicBool::new(true),
            midi_last_transport: AtomicU8::new(MIDI_TRANSPORT_NONE),
            midi_steps: AtomicU32::new(0),
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

    /// Keyboard input handler for shutdown requests. `None` in headless mode,
    /// where there is no terminal to put into raw mode.
    controller: Option<Controller>,

    /// Tracks whether shutdown has already started, so drop remains idempotent.
    shutdown_started: bool,

    /// The WebSocket listener's actually bound address, obtained from the
    /// listener itself rather than the configured address so a `:0` port
    /// resolves to the real one. `None` when the WebSocket output is not
    /// configured.
    ws_bound_addr: Option<SocketAddr>,

    /// The facts resolved during construction, reported by the headless
    /// `ready` event.
    ready_report: ReadyReport,
}

impl App {
    /// Constructs the audio pipeline from the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if configuration violates structural or numeric runtime
    /// limits, an audio or MIDI device cannot be opened, the audio input stream
    /// cannot be started, or a configured output transport cannot bind to its
    /// given address. If an output fails after earlier workers have started,
    /// construction shuts those workers down before returning the error.
    ///
    /// # Panics
    ///
    /// Panics if worker thread startup fails internally.
    pub fn new(config: &AppConfig) -> Result<Self> {
        Self::build(config, true)
    }

    /// Constructs the audio pipeline without a terminal controller, for
    /// supervision by a host process under `--headless`.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`App::new`].
    ///
    /// # Panics
    ///
    /// Panics if worker thread startup fails internally.
    pub fn new_headless(config: &AppConfig) -> Result<Self> {
        Self::build(config, false)
    }

    fn build(config: &AppConfig, interactive: bool) -> Result<Self> {
        let bootstrapped = bootstrap(config)?;
        let controller = interactive.then(|| Controller::new(Arc::clone(&bootstrapped.state)));
        let osc = config.outputs.iter().find_map(|output| match output {
            OutputConfig::Osc { addr } => Some(*addr),
            OutputConfig::WebSocket { .. } => None,
        });
        let midi_device = match &config.midi_input {
            Some(ConfigMidiInput::Device(name)) => Some(name.clone()),
            Some(ConfigMidiInput::TestClock(_)) | None => None,
        };

        Ok(Self {
            input_device: Some(bootstrapped.input_device),
            state: bootstrapped.state,
            workers: bootstrapped.workers,
            controller,
            shutdown_started: false,
            ws_bound_addr: bootstrapped.ws_bound_addr,
            ready_report: ReadyReport {
                audio: bootstrapped.audio,
                midi_device,
                outputs: ReadyOutputs {
                    websocket: bootstrapped.ws_bound_addr,
                    osc,
                },
            },
        })
    }

    /// The facts resolved during construction, for the headless `ready` event.
    #[must_use]
    pub fn ready_report(&self) -> &ReadyReport {
        &self.ready_report
    }

    /// The WebSocket listener's actually bound address, obtained from
    /// `local_addr()` rather than the configured address, so `--ws-addr
    /// 127.0.0.1:0` reports the real OS-assigned port. `None` when the
    /// WebSocket output is not configured.
    #[must_use]
    pub fn ws_bound_addr(&self) -> Option<SocketAddr> {
        self.ws_bound_addr
    }

    /// Hands control to the interactive controller, blocking until shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if the controller encounters a terminal or I/O failure.
    pub fn run(&self) -> Result<()> {
        let controller = self
            .controller
            .as_ref()
            .context("App was constructed headless and has no interactive controller")?;
        controller.run()
    }

    /// Blocks until SIGINT or SIGTERM arrives, without a terminal or keyboard
    /// handling. Both signals request the same graceful shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if either signal handler cannot be registered.
    fn run_headless(&self) -> Result<()> {
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        for signal in [SIGINT, SIGTERM] {
            signal_hook::flag::register(signal, Arc::clone(&shutdown_requested))
                .with_context(|| format!("Failed to register signal handler for {signal}"))?;
        }

        log::info!("Ready. Send SIGINT or SIGTERM to exit.");

        while self.state.keep_running.load(Ordering::Acquire) {
            if shutdown_requested.load(Ordering::Acquire) {
                self.state.keep_running.store(false, Ordering::Release);
                break;
            }
            std::thread::sleep(Duration::from_millis(HEADLESS_POLL_RATE_MS));
        }

        Ok(())
    }

    /// Runs the headless wait loop and always performs shutdown afterwards.
    ///
    /// # Errors
    ///
    /// Returns an error if signal registration fails. Shutdown is still
    /// attempted before the error is returned.
    pub fn run_headless_until_shutdown(&mut self) -> Result<()> {
        let run_result = self.run_headless();
        self.shutdown();
        run_result
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
    // Fallback for callers that omit explicit shutdown.
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

        let generator_thread = thread::spawn(move || {
            while thread_state.keep_running.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            thread_exit_count.fetch_add(1, Ordering::AcqRel);
        });
        let mut workers = WorkerThreads::default();
        workers.register(crate::worker::WorkerKind::Generator, generator_thread);

        let mut app = App {
            input_device: None,
            state: state.clone(),
            workers,
            controller: Some(Controller::new(state.clone())),
            shutdown_started: false,
            ws_bound_addr: None,
            ready_report: ReadyReport {
                audio: crate::headless::ReadyAudio {
                    device: None,
                    sample_rate: 48_000,
                    channels: vec![0],
                },
                midi_device: None,
                outputs: ReadyOutputs {
                    websocket: None,
                    osc: None,
                },
            },
        };

        app.shutdown();
        app.shutdown();

        assert!(app.shutdown_started);
        assert!(!state.keep_running.load(Ordering::Acquire));
        assert_eq!(exit_count.load(Ordering::Acquire), 1);
        assert!(app.workers.is_empty());

        drop(app);

        assert_eq!(exit_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn midi_atomics_default_to_none_and_zero() {
        let state = AppState::new();
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            MIDI_TRANSPORT_NONE
        );
        assert_eq!(state.midi_steps.load(Ordering::Acquire), 0);
    }
}
