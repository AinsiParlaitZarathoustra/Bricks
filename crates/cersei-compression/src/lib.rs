//! cersei-compression — reduction of tool outputs for the Cersei SDK, without
//! losing access to the originals.
//!
//! * [`log`]: command logs — diagnostics found first and kept, then
//!   declarative [`rules`] (built-in, `~/.bricks/rules/*.toml`, `bricks.toml`),
//!   then a budget that shows every omission in place.
//! * [`skeleton`]: Tree-sitter skeletons of source files (an exploration view).
//! * [`json_view`]: structured summaries of large JSON documents.
//! * [`dispatch`]: the [`Compressor`] that routes a tool output, adds a
//!   `[bricks: …]` header to anything it reduces, and saves the original in
//!   a [`raw::RawStore`] so it can be read back with the `Read` tool.
//!
//! Credits: the rule engine and ANSI handling started as a port of rtk (Rust
//! Token Killer) by Patrick Szymkowiak, <https://github.com/rtk-ai/rtk>, MIT
//! licensed. See LICENSE.

pub mod ansi;
pub mod command;
pub mod config;
pub mod dispatch;
pub mod errors;
pub mod json_view;
pub mod level;
pub mod log;
pub mod raw;
pub mod rules;
pub mod skeleton;
pub mod structured;

pub use config::{CompressionConfig, CompressionSection};
pub use dispatch::{
    compress_tool_output, compress_tool_output_with_stats, CompressionStats, Compressor, Processed,
    ToolOutput,
};
pub use level::CompressionLevel;
pub use raw::{RawRef, RawStore};
pub use rules::{RuleDiagnostic, RuleSet, RuleSources};
