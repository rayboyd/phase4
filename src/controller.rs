//! Keyboard-driven runtime controller.
//!
//! [`Controller`] puts the terminal into raw mode for the duration of its
//! lifetime and polls for key events at a fixed `POLL_RATE_MS` interval.
//! Key presses toggle the corresponding atomic flags on [`AppState`], which the
//! worker threads observe to start or stop analysis and broadcasting.

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

        log::info!("'A' or 'B' to toggle engine, Ctrl+C to exit");

        while self.state.keep_running.load(Ordering::Acquire) {
            if event::poll(Duration::from_millis(POLL_RATE_MS))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key_event(key);
                }
            }
        }

        Ok(())
    }

    fn handle_key_event(&self, key: KeyEvent) {
        if !key.is_press() {
            return;
        }

        match key.code {
            KeyCode::Char('a' | 'A' | 'b' | 'B') => {
                let was_active = self.state.is_active.load(Ordering::Acquire);
                self.state.is_active.store(!was_active, Ordering::Release);
                let status = if was_active { "PAUSED" } else { "ACTIVE" };
                log::info!("Engine Status: {status}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn controller_with_state() -> (Controller, Arc<AppState>) {
        let state = Arc::new(AppState::new());
        (Controller::new(state.clone()), state)
    }

    #[test]
    fn press_event_toggles_broadcasting_once() {
        let (controller, state) = controller_with_state();

        controller.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Char('b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        ));

        // is_active starts true; pressing 'b' toggles it to false.
        assert!(!state.is_active.load(Ordering::Acquire));
    }

    #[test]
    fn release_event_does_not_toggle_broadcasting() {
        let (controller, state) = controller_with_state();

        controller.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Char('b'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));

        // Release events do not toggle; is_active remains true.
        assert!(state.is_active.load(Ordering::Acquire));
    }

    #[test]
    fn repeat_event_does_not_toggle_broadcasting() {
        let (controller, state) = controller_with_state();

        controller.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Char('b'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ));

        // Repeat events do not toggle; is_active remains true.
        assert!(state.is_active.load(Ordering::Acquire));
    }

    #[test]
    fn press_ctrl_c_signals_shutdown() {
        let (controller, state) = controller_with_state();

        // Sanity check that it starts as true.
        assert!(state.keep_running.load(Ordering::Acquire));

        controller.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        ));

        // The keep_running flag should now be false.
        assert!(!state.keep_running.load(Ordering::Acquire));
    }
}
