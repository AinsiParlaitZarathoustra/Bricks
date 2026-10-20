//! `bricks`: loads the configuration, starts the engine and presents it.
//! The engine (bricks-agent) runs everything; this binary only translates
//! arguments into commands and events into output.

mod args;
mod exit;
mod headless;
mod sessions;
mod setup;

use clap::{CommandFactory, Parser};

fn main() {
    let args::Cli { global, command } = args::Cli::parse();
    let Some(command) = command else {
        // No command: show the help. There is no interactive interface here.
        let _ = args::Cli::command().print_help();
        println!();
        std::process::exit(exit::OK);
    };
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
        match command {
            args::Cmd::Run(run) => headless::run(&global, &launch, run).await,
            args::Cmd::Sessions { json, all } => {
                sessions::run(&global, &launch, json, all).await
            }
        }
    });
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    std::process::exit(code);
}
