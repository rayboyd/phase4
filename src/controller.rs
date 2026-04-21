//! Keyboard-driven runtime controller.
//!
//! [`Controller`] puts the terminal into raw mode for the duration of its
//! lifetime and polls for key events at a fixed `POLL_RATE_MS` interval.
//! Key presses toggle the corresponding atomic flags on [`AppState`], which the
//! worker threads observe to start or stop recording, analysis, and broadcasting.

use crate::app::AppState;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

/// Keyboard input polling interval.
const POLL_RATE_MS: u64 = 100;

pub struct Controller {
    state: Arc<AppState>,
}

impl Controller {
    /// Creates an [`Controller`] bound to the given shared application state.
    #[must_use]
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Enters raw mode and polls for key events until shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if raw mode cannot be enabled or a terminal event
    /// cannot be read.
    pub fn run(&self) -> Result<()> {
        enable_raw_mode()?;

        log::info!("'A' to analyse, 'B' to broadcast, 'R' to record, Ctrl+C to exit");

        let mut last_record_ring_overflow_events = 0usize;

        while self.state.keep_running.load(Ordering::Acquire) {
            if event::poll(Duration::from_millis(POLL_RATE_MS))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key_event(key);
                }
            }

            let ring_overflow_events = self
                .state
                .record_ring_overflow_events
                .load(Ordering::Relaxed);
            if ring_overflow_events > last_record_ring_overflow_events {
                log::warn!(
                    "Record ring full: {} event(s) since last poll (total: {}). Recorder fell behind, audio loss may have occurred.",
                    ring_overflow_events - last_record_ring_overflow_events,
                    ring_overflow_events
                );
                last_record_ring_overflow_events = ring_overflow_events;
            }
        }

        Ok(())
    }

    fn handle_key_event(&self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('b' | 'B') => {
                let was_broadcasting = self.state.is_broadcasting.load(Ordering::Acquire);
                self.state
                    .is_broadcasting
                    .store(!was_broadcasting, Ordering::Release);
                let status = if was_broadcasting { "OFF" } else { "ON" };
                log::info!("Broadcasting: {status}");
            }

            KeyCode::Char('r' | 'R') => {
                let was_recording = self.state.is_recording.load(Ordering::Acquire);
                self.state
                    .is_recording
                    .store(!was_recording, Ordering::Release);
                let status = if was_recording { "OFF" } else { "ON" };
                log::info!("Recording: {status}");
            }

            KeyCode::Char('a' | 'A') => {
                let was_analysing = self.state.is_analysing.load(Ordering::Acquire);
                self.state
                    .is_analysing
                    .store(!was_analysing, Ordering::Release);
                let status = if was_analysing { "OFF" } else { "ON" };
                log::info!("Analysis: {status}");
            }

            KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.keep_running.store(false, Ordering::Release);
            }

            _ => {}
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}
