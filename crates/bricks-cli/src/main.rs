//! `bricks`: loads the configuration, starts the engine and presents it.
//! The engine (cersei-agent) runs everything; this binary only translates
//! arguments into commands and events into output.

mod args;
mod exit;
mod headless;
mod sessions;
mod setup;
mod tui;

use clap::Parser;

fn main() {
    let cli = args::Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bricks: cannot start: {e}");
            std::process::exit(exit::FAILED);
        }
    };
    let code = runtime.block_on(async move {
        match cli.command {
            Some(args::Cmd::Run(run)) => headless::run(&cli.global, run).await,
            Some(args::Cmd::Sessions { json }) => sessions::run(json).await,
            None | Some(args::Cmd::Tui) => tui::run(&cli.global, None, false).await,
            Some(args::Cmd::Resume { session_id }) => {
                let pick = session_id.is_none();
                tui::run(&cli.global, session_id, pick).await
            }
        }
    });
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    std::process::exit(code);
}
