//! Engine assembly shared by the Bricks frontends (CLI, terminal interface,
//! desktop): the Bricks home, the provider registry, the project loader and
//! the engine configuration. Extracted from `bricks-cli/src/setup.rs`
//! (Sprint 14). Nothing here depends on command-line arguments: a frontend
//! resolves its own options into plain paths and flags first.

mod engine;
mod memory;
mod paths;
mod projects;
mod providers;

pub use engine::engine_config;
pub use paths::{bricks_home, providers_file_from_env, sessions_dir};
pub use projects::{Projects, announced_tools};
pub use providers::load_registry;
