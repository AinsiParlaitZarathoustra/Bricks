//! Git view of a project folder, through the git CLI (owner: Haiku 7).
//!
//! `run` runs git (bounded time and output), `parse` reads its output
//! (pure), `ops` holds the operations, `commands` exposes them to Tauri.

pub mod commands;
mod ops;
mod parse;
mod run;

pub(crate) use ops::validate_relative_path;
pub(crate) use parse::is_binary_content;

#[cfg(test)]
mod tests;
