//! TUI entry point for Phase4.
//!
//! `phase4-tui` is an interactive front-end for the same audio pipeline as the
//! headless `phase4` binary. It presents a device selection screen before
//! construction and a live control surface whilst the pipeline is running.
//! Both binaries share the same library; no IPC is involved.

mod app_screen;
mod device_screen;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use device_screen::DeviceScreen;
use phase4::app::App;
use phase4::config::AppConfig;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn main() -> Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    let result = run();

    // Ignore cleanup errors: the process is exiting and both calls must run.
    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);

    result
}

fn run() -> Result<()> {
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut screen = DeviceScreen::load()?;

    // Device selection loop.
    let device_index = loop {
        terminal.draw(|f| device_screen::render(f, &mut screen))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if !key.is_press() {
                    continue;
                }
                match key.code {
                    KeyCode::Up => screen.move_up(),
                    KeyCode::Down => screen.move_down(),
                    KeyCode::Enter => {
                        if let Some(idx) = screen.selected_device_index() {
                            break idx;
                        }
                    }
                    KeyCode::Char('q' | 'Q') => return Ok(()),
                    KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    };

    // Build config from the selected device using all other defaults.
    let config = AppConfig {
        device_index: Some(device_index),
        ..AppConfig::default()
    };

    let mut app = App::new(config)?;
    let state = app.state().clone();

    // Control surface loop.
    loop {
        terminal.draw(|f| app_screen::render(f, &state))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if !key.is_press() {
                    continue;
                }
                match key.code {
                    KeyCode::Char('a' | 'A') => {
                        state.is_analysing.fetch_xor(true, Ordering::AcqRel);
                    }
                    KeyCode::Char('b' | 'B') => {
                        state
                            .is_broadcasting_websocket
                            .fetch_xor(true, Ordering::AcqRel);
                    }
                    KeyCode::Char('r' | 'R') => {
                        state.is_recording.fetch_xor(true, Ordering::AcqRel);
                    }
                    KeyCode::Char('q' | 'Q') => break,
                    KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        break;
                    }
                    _ => {}
                }
            }
        }

        if !state.keep_running.load(Ordering::Acquire) {
            break;
        }
    }

    app.shutdown();
    Ok(())
}
