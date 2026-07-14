use anyhow::Result;
use clap::Parser;
use phase4::app::App;
use phase4::config::AppConfig;
use phase4::managers::audio::Input;
use phase4::managers::MidiListener;
use phase4::{Args, ControllerMode};
use std::io::Write;

/// Returns the line ending appended to each log line for the given controller mode.
///
/// Term mode enables terminal raw mode, so a carriage return is appended to keep the
/// cursor snapped back to the left of the terminal. Headless mode is intended for a
/// wrapper process consuming stderr as plain, line-oriented text, so no carriage
/// return is appended there, leaving each line ending in a plain `\n`.
fn log_line_ending(mode: ControllerMode) -> &'static str {
    match mode {
        ControllerMode::Term => "\r",
        ControllerMode::Headless => "",
    }
}

/// Returns whether the startup banner should be shown for the given mode.
fn should_show_banner(mode: ControllerMode) -> bool {
    matches!(mode, ControllerMode::Term)
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Defaults to "info" level, can be overridden via RUST_LOG env var. The line
    // ending depends on the controller mode: \r keeps output aligned under raw mode
    // in term mode, and is omitted in headless mode so wrapper processes receive
    // plain \n line endings.
    let line_ending = log_line_ending(args.runtime.controller_mode);
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(move |buf, record| {
            writeln!(buf, "[{}] {}{line_ending}", record.level(), record.args())
        })
        .init();

    phase4::controller::install_panic_hook();

    if args.input.audio_list {
        Input::list_devices(args.input.audio_list_format)?;
        return Ok(());
    }

    if args.midi.midi_list {
        MidiListener::list_devices(args.midi.midi_list_format)?;
        return Ok(());
    }

    if should_show_banner(args.runtime.controller_mode) {
        log::info!("Welcome to phase4.");
    }

    let config = match AppConfig::try_from(&args) {
        Ok(c) => c,
        Err(e) => {
            log::error!("{e}");
            std::process::exit(1);
        }
    };

    let mut app = App::new(config)?;
    app.run_until_shutdown()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_show_banner_is_true_in_term_mode() {
        assert!(should_show_banner(ControllerMode::Term));
    }

    #[test]
    fn should_show_banner_is_false_in_headless_mode() {
        assert!(!should_show_banner(ControllerMode::Headless));
    }

    #[test]
    fn log_line_ending_appends_carriage_return_in_term_mode() {
        assert_eq!(log_line_ending(ControllerMode::Term), "\r");
    }

    #[test]
    fn log_line_ending_is_empty_in_headless_mode() {
        assert_eq!(log_line_ending(ControllerMode::Headless), "");
    }
}
