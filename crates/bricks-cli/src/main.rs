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
    // The folder Bricks was started from: the base of relative paths given
    // on the command line (the process never changes it afterwards).
    let launch = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("bricks: current folder: {e}");
            std::process::exit(exit::USAGE);
        }
    };
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
            Some(args::Cmd::Run(run)) => headless::run(&cli.global, &launch, run).await,
            Some(args::Cmd::Sessions { json, all }) => {
                sessions::run(&cli.global, &launch, json, all).await
            }
            None | Some(args::Cmd::Tui) => tui::run(&cli.global, &launch, None, false).await,
            Some(args::Cmd::Resume { session_id }) => {
                let pick = session_id.is_none();
                tui::run(&cli.global, &launch, session_id, pick).await
            }
        }
    });
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    std::process::exit(code);
}
