//! Where Bricks keeps its files.

use std::path::PathBuf;

/// `~/.bricks`, or `$BRICKS_HOME`.
pub fn bricks_home() -> PathBuf {
    if let Some(h) = std::env::var_os("BRICKS_HOME") {
        return PathBuf::from(h);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".bricks")
}

/// The skill store: `<bricks_home>/skills`, one standard skill folder per
/// installed skill.
pub fn skills_dir() -> PathBuf {
    bricks_home().join("skills")
}

/// The session store: `<bricks_home>/sessions`.
pub fn sessions_dir() -> PathBuf {
    bricks_home().join("sessions")
}

/// The providers file named by `$BRICKS_HOME` (`$BRICKS_HOME/providers.toml`),
/// or `None` when `BRICKS_HOME` is not set: the registry then reads its
/// default location (`~/.bricks/providers.toml`).
pub fn providers_file_from_env() -> Option<PathBuf> {
    std::env::var_os("BRICKS_HOME").map(|h| PathBuf::from(h).join("providers.toml"))
}
