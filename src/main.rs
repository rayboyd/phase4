use anyhow::Result;
use clap::Parser;
use phase4::app::App;
use phase4::config::{AppConfig, OutputConfig};
use phase4::events::{map_config_error, map_startup_error, Emitter, Event};
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

    let emitter = Emitter::new(args.network.stdout_events);

    let config = match AppConfig::try_from(&args) {
        Ok(c) => c,
        Err(e) => {
            emitter.emit(&Event::Fatal {
                reason: map_config_error(&e),
                detail: format!("{e}"),
            });
            log::error!("{e}");
            std::process::exit(1);
        }
    };

    let osc_addr = config.outputs.iter().find_map(|output| match output {
        OutputConfig::Osc { addr } => Some(*addr),
        OutputConfig::WebSocket { .. } => None,
    });

    let mut app = match App::new(config) {
        Ok(app) => app,
        Err(e) => {
            emitter.emit(&Event::Fatal {
                reason: map_startup_error(&e),
                detail: format!("{e:#}"),
            });
            log::error!("{e:#}");
            std::process::exit(1);
        }
    };

    emitter.emit(&Event::Ready {
        pid: std::process::id(),
        ws_addr: app.ws_bound_addr(),
        osc_addr,
    });

    app.run_until_shutdown()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_line_ending_appends_carriage_return_in_term_mode() {
        assert_eq!(log_line_ending(ControllerMode::Term), "\r");
    }

    #[test]
    fn log_line_ending_is_empty_in_headless_mode() {
        assert_eq!(log_line_ending(ControllerMode::Headless), "");
    }
}
