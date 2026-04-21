use clap::Parser;
use phase4::app::App;
use phase4::config::AppConfig;
use phase4::managers::audio::Input;
use phase4::Args;
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Defaults to "info" level, can be overridden via RUST_LOG env var. Init logging
    // so it will play nice with terminal raw mode when app interactive mode is enabled.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            // The \r ensures the cursor snaps back to the left side of the terminal.
            writeln!(buf, "[{}] {}\r", record.level(), record.args())
        })
        .init();

    let args = Args::parse();
    if args.list {
        Input::list_devices()?;
        return Ok(());
    }

    let config = match AppConfig::try_from(&args) {
        Ok(c) => c,
        Err(e) => {
            log::error!("{e}");
            std::process::exit(1);
        }
    };

    let app = App::new(config)?;
    app.run()?;

    // When run() returns Ok(()) Drop() is called, this will gracefully handle
    // shutdown, it will close any threads and write buffers to disk.
    Ok(())
}
