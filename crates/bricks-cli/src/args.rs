//! Command line.

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "bricks",
    version,
    about = "Bricks — a coding agent in the terminal.",
    long_about = "Bricks — a coding agent in the terminal.\n\n\
        Without a command, opens the interactive interface. `bricks run` runs one prompt \
        without it (scripts, CI). Models come from ~/.bricks/providers.toml (or --providers); \
        settings from the project's bricks.toml. See docs/cli.md.\n\n\
        Examples:\n  \
        bricks --workspace ~/Documents/atlas\n  \
        bricks --workspace \"~/Documents/My project\"\n  \
        bricks --workspace ./atlas sessions\n  \
        bricks sessions --all\n  \
        bricks --workspace ./atlas run \"Describe this project.\""
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Debug, Clone, Args)]
pub struct Global {
    /// Project folder to open. Default: the current folder. `~` and `~/…`
    /// are expanded (also when quoted); relative paths start from the
    /// current folder. A resumed session keeps its own folder.
    #[arg(
        long = "workspace",
        visible_alias = "cd",
        global = true,
        value_name = "DIR"
    )]
    pub workspace: Option<PathBuf>,
    /// Providers file (default: ~/.bricks/providers.toml). A relative path
    /// starts from the current folder.
    #[arg(long, global = true, value_name = "FILE")]
    pub providers: Option<PathBuf>,
    /// Model, as `provider_id/model_id` from the providers file.
    #[arg(long, global = true, value_name = "PROVIDER/MODEL")]
    pub model: Option<String>,
    /// Reasoning profile of that model (as configured).
    #[arg(long, global = true, value_name = "PROFILE")]
    pub reasoning: Option<String>,
    /// Clickable web links in the interface (OSC 8): `auto` when the
    /// terminal is known to support them, `always`, or `never` (the link's
    /// address stays written next to it in every mode).
    #[arg(
        long,
        global = true,
        value_enum,
        default_value_t = Hyperlinks::Auto,
        value_name = "WHEN"
    )]
    pub hyperlinks: Hyperlinks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Hyperlinks {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Open the interactive interface (the default).
    Tui,
    /// Run one prompt without the interface.
    Run(RunArgs),
    /// List the stored sessions of the project (`--all`: every project).
    Sessions {
        /// One JSON object per line (same selection as the text listing).
        #[arg(long)]
        json: bool,
        /// Every stored session, whatever its project.
        #[arg(long)]
        all: bool,
    },
    /// Resume a session in the interface (without an id: choose it among
    /// the project's sessions, or all of them). A session resumes in its
    /// own folder, with that project's settings.
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
