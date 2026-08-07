use anyhow::Result;
use clap::Parser;
use phase4::app::App;
use phase4::config::AppConfig;
use phase4::managers::audio::Input;
use phase4::managers::MidiListener;
use phase4::Args;
use std::io::{IsTerminal, Write};

const TERMINAL_LOG_LINE_ENDING: &str = "\r";

fn main() -> Result<()> {
    let args = Args::parse();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            writeln!(
                buf,
                "[{}] {}{TERMINAL_LOG_LINE_ENDING}",
                record.level(),
                record.args()
            )
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

    if !std::io::stdin().is_terminal() {
        anyhow::bail!("Phase4 requires an interactive terminal. Run it directly from a terminal.");
    }

    let config = AppConfig::try_from(&args)?;
    let mut app = App::new(&config)?;

    app.run_until_shutdown()?;

    Ok(())
}
