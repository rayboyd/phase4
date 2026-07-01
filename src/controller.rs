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

        log::info!("'T' to toggle engine, Ctrl+C to exit");

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
            KeyCode::Char('t' | 'T') => {
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

/// Restores the terminal to cooperative mode and logs the panic message.
///
/// Called from the installed panic hook before the process aborts. Terminal
/// restoration is best effort, since `disable_raw_mode` can itself fail if the
/// terminal is already gone, and that failure is deliberately swallowed here.
fn handle_panic(message: &str) {
    let _ = disable_raw_mode();
    log::error!("{message}");
}

/// Installs a process-wide panic hook that restores the terminal before the
/// process aborts.
///
/// With `panic = "abort"` set in the release profile, `Drop` implementations
/// do not run on panic, so `Controller`'s `Drop` impl never fires on this
/// path. This hook is the only opportunity to leave the terminal in a
/// cooperative state before the process terminates. The previous hook, which
/// prints the panic message and backtrace, is preserved and still runs.
pub fn install_panic_hook() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        handle_panic(&info.to_string());
        previous_hook(info);
    }));
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
            KeyCode::Char('t'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        ));

        // is_active starts true; pressing 't' toggles it to false.
        assert!(!state.is_active.load(Ordering::Acquire));
    }

    #[test]
    fn release_event_does_not_toggle_broadcasting() {
        let (controller, state) = controller_with_state();

        controller.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Char('t'),
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
            KeyCode::Char('t'),
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

    #[test]
    fn handle_panic_logs_the_panic_message() {
        testing_logger::setup();

        handle_panic("deliberate test panic message");

        testing_logger::validate(|captured_logs| {
            assert_eq!(captured_logs.len(), 1, "expected exactly one log entry");
            assert_eq!(captured_logs[0].level, log::Level::Error);
            assert!(
                captured_logs[0]
                    .body
                    .contains("deliberate test panic message"),
                "log body should include the panic message, got: {}",
                captured_logs[0].body
            );
        });
    }
}
