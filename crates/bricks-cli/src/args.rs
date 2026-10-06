//! Command line.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "bricks",
    version,
    about = "Bricks — a coding agent in the terminal.",
    long_about = "Bricks — a coding agent in the terminal.\n\n\
        Without a command, opens the interactive interface. `bricks run` runs one prompt \
        without it (scripts, CI). Models come from ~/.bricks/providers.toml (or --providers); \
        settings from ./bricks.toml. See docs/cli.md."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Debug, Clone, Args)]
pub struct Global {
    /// Working directory (default: the current directory).
    #[arg(long = "cd", global = true, value_name = "DIR")]
    pub cd: Option<PathBuf>,
    /// Providers file (default: ~/.bricks/providers.toml).
    #[arg(long, global = true, value_name = "FILE")]
    pub providers: Option<PathBuf>,
    /// Model, as `provider_id/model_id` from the providers file.
    #[arg(long, global = true, value_name = "PROVIDER/MODEL")]
    pub model: Option<String>,
    /// Reasoning profile of that model (as configured).
    #[arg(long, global = true, value_name = "PROFILE")]
    pub reasoning: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Open the interactive interface (the default).
    Tui,
    /// Run one prompt without the interface.
    Run(RunArgs),
    /// List stored sessions.
    Sessions {
        /// One JSON object per line.
        #[arg(long)]
        json: bool,
    },
    /// Resume a session in the interface (without an id: choose it).
    Resume { session_id: Option<String> },
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// The prompt. `-` reads it from stdin. When omitted, stdin is read if
    /// it is not a terminal.
    pub prompt: Option<String>,
    /// Write versioned JSONL events to stdout (diagnostics go to stderr).
    #[arg(long)]
    pub json: bool,
    /// Never wait for a person: a step that needs an approval stops the run
    /// (exit code 3). Does not grant any permission.
    #[arg(long)]
    pub non_interactive: bool,
    /// Also read stdin and attach it after the positional prompt.
    #[arg(long)]
    pub stdin: bool,
    /// Continue a stored session instead of starting a new one.
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// Attach a file (its content is captured when the run starts).
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<String>,
    /// Attach an image (the model must accept images).
    #[arg(long = "image", value_name = "PATH")]
    pub images: Vec<String>,
    /// Do not wait for the long-term memory maintenance after the answer
    /// (its work stays pending and resumes next time).
    #[arg(long)]
    pub no_wait_memory: bool,
}
