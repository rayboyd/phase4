use anyhow::Result;
use clap::Parser;
use phase4::app::App;
use phase4::config::AppConfig;
use phase4::headless::{write_event, Event, ShutdownReason};
use phase4::managers::audio::Input;
use phase4::managers::MidiListener;
use phase4::Args;
use std::io::{IsTerminal, Write};
use std::process::ExitCode;

const TERMINAL_LOG_LINE_ENDING: &str = "\r";

fn main() -> ExitCode {
    let args = Args::parse();

    init_logging(args.headless);

    if args.headless {
        phase4::headless::install_panic_hook();
    } else {
        phase4::controller::install_panic_hook();
    }

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if args.headless {
                let _ = write_event(&mut std::io::stdout().lock(), &Event::from_anyhow(&error));
            } else {
                eprintln!("Error: {error:#}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Initialises logging on stderr. The carriage return suffix is a raw-mode
/// artefact and is not emitted when there is no terminal.
fn init_logging(headless: bool) {
    let line_ending = if headless {
        ""
    } else {
        TERMINAL_LOG_LINE_ENDING
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(move |buf, record| {
            writeln!(buf, "[{}] {}{line_ending}", record.level(), record.args())
        })
        .init();
}

/// Runs the requested mode, returning an error rather than exiting, so the
/// caller can report it through the right channel.
fn run(args: &Args) -> Result<()> {
    if args.input.audio_list {
        Input::list_devices(args.input.audio_list_format)?;
        return Ok(());
    }

    if args.midi.midi_list {
        MidiListener::list_devices(args.midi.midi_list_format)?;
        return Ok(());
    }

    if args.headless {
        return run_headless(args);
    }

    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "Phase4 requires an interactive terminal. Run it directly from a terminal, or pass --headless."
        );
    }

    let config = AppConfig::try_from(args)?;
    let mut app = App::new(&config)?;
    app.run_until_shutdown()
}

/// Runs under host supervision, writing the event stream to stdout.
fn run_headless(args: &Args) -> Result<()> {
    let config = AppConfig::try_from(args)?;
    let mut app = App::new_headless(&config)?;

    let mut stdout = std::io::stdout().lock();
    write_event(&mut stdout, &Event::Ready(app.ready_report().clone()))?;
    drop(stdout);

    app.run_headless_until_shutdown()?;

    write_event(
        &mut std::io::stdout().lock(),
        &Event::Shutdown {
            reason: ShutdownReason::Signal,
        },
    )
}
