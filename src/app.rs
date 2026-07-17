//! Top-level application struct that owns and coordinates all subsystems.
//!
//! [`App`] is assembled from the pieces resolved by [`crate::bootstrap::bootstrap`],
//! which queries the selected input device for its hardware capabilities,
//! sizes the ringbufs accordingly, and spawns the analyser, WebSocket server,
//! and (in calibration mode) the synthetic generator threads. `App` then
//! hands control to the [`Controller`] for interactive keyboard handling.
//!
//! Shared runtime state is carried by [`AppState`], which holds a set of
//! [`std::sync::atomic`] flags that the controller writes and the worker threads
//! observe. On shutdown, all threads are signalled to stop and given a bounded
//! grace period to exit, which prevents one stalled worker from hanging the
//! main thread indefinitely.

use crate::bootstrap::bootstrap;
use crate::config::AppConfig;
use crate::controller::Controller;
use crate::managers::{Input, MIDI_TRANSPORT_NONE};
use crate::worker::WorkerThreads;
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
    Arc,
};

/// Shared application state flags for cross-thread synchronisation.
pub struct AppState {
    /// Whether the analyser is currently processing samples.
    /// Toggled by the controller (T key), read by the analyser thread.
    pub is_active: AtomicBool,

    /// Signals every worker thread to exit.
    /// Set false by the controller (Ctrl+C) or `App::shutdown`.
    pub keep_running: AtomicBool,

    /// Last MIDI transport event seen, one of the `MIDI_TRANSPORT_*` codes.
    /// Written by the MIDI listener thread, read and cleared by the mapper
    /// each time it broadcasts a frame.
    pub midi_last_transport: AtomicU8,

    /// MIDI 1/16 note steps derived from incoming MIDI clock ticks.
    ///
    /// Absolute monotonic count since the most recent Start event. Written by
    /// the MIDI listener thread, read by the mapper as a snapshot, and reset
    /// only by Start.
    pub midi_steps: AtomicU32,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            is_active: AtomicBool::new(true),
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
    /// cannot be started, or a configured output transport cannot bind to its
    /// given address.
    ///
    /// # Panics
    ///
    /// Panics if worker thread startup fails internally.
    pub fn new(config: AppConfig) -> Result<Self> {
        let bootstrapped = bootstrap(config)?;
        let controller_state = Arc::clone(&bootstrapped.state);

        Ok(Self {
            input_device: Some(bootstrapped.input_device),
            state: bootstrapped.state,
            workers: bootstrapped.workers,
            controller: Controller::new(bootstrapped.controller_mode, controller_state),
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
        log::info!("- Device shutdown complete");

        // Signal every worker before waiting on any of them.
        self.state.keep_running.store(false, Ordering::Release);
        self.workers.shutdown();

        log::info!("Shutdown complete");
    }
}

impl Drop for App {
    // Keep drop lightweight and idempotent by delegating to the explicit
    // shutdown path. This gives callers a best effort fallback when they
    // do not call shutdown() themselves.
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
            workers: WorkerThreads::new(generator_thread, None, None, None, Vec::new()),
            controller: Controller::new(ControllerMode::Term, state.clone()),
            shutdown_started: false,
        };

        app.shutdown();
        app.shutdown();

        assert!(app.shutdown_started);
        assert!(!state.keep_running.load(Ordering::Acquire));
        assert_eq!(exit_count.load(Ordering::Acquire), 1);
        assert!(app.workers.pipeline.iter().all(Option::is_none));
        assert!(app.workers.outputs.is_empty());

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
