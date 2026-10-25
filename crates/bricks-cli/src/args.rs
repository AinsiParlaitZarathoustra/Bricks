//! Command line.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "bricks",
    version,
    about = "Bricks — a coding agent in the terminal.",
    long_about = "Bricks — a coding agent in the terminal.\n\n\
        Without a command, prints this help. `bricks run` runs one prompt (scripts, CI); \
        `bricks run --session <id>` continues a stored session. The desktop application \
        resumes sessions from its sidebar and the `/resume` command. Models come from \
        ~/.bricks/providers.toml (or --providers); settings from the project's bricks.toml. \
        See docs/cli.md.\n\n\
        Examples:\n  \
        bricks --workspace ./atlas sessions\n  \
        bricks sessions --all\n  \
        bricks --workspace ./atlas run \"Describe this project.\"\n  \
        bricks --workspace ./atlas run --session <id> \"Continue where we stopped.\""
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
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Run one prompt (`--session <id>` continues a stored session).
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
    /// The permission profile of this run's session: `ask` (reads run,
    /// writes, commands, network and sub-agents are asked) or `execute`
    /// (everything runs). Without it, the session keeps its own policy, or
    /// starts from the default chosen in Bricks (none chosen: as `ask`).
    #[arg(long, value_enum, value_name = "PROFILE")]
    pub permissions: Option<PermissionsArg>,
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
    /// Activate this skill before the prompt (repeatable, in order). Every
    /// one is checked before anything is sent; one that cannot be used
    /// stops the command (exit code 2).
    #[arg(long = "skill", value_name = "NAME")]
    pub skills: Vec<String>,
    /// Allow the project skills of explicitly configured sources for this
    /// run's session only (Bricks reads none by default). Grants no other
    /// permission.
    #[arg(long)]
    pub trust_project_skills: bool,
}

/// The profiles `--permissions` accepts (CUSTOM is set in Bricks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PermissionsArg {
    Ask,
    Execute,
}

impl From<PermissionsArg> for bricks_agent::permission::PermissionProfile {
    fn from(a: PermissionsArg) -> Self {
        match a {
            PermissionsArg::Ask => Self::Ask,
            PermissionsArg::Execute => Self::Execute,
        }
    }
}
