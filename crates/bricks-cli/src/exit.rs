//! Exit codes of `bricks` (documented in docs/cli.md).

/// The run succeeded.
pub const OK: i32 = 0;
/// The run failed (provider, tool infrastructure, engine error).
pub const FAILED: i32 = 1;
/// Invalid arguments or configuration; nothing was run.
pub const USAGE: i32 = 2;
/// A step needed an approval and nobody could give it (non-interactive).
pub const APPROVAL: i32 = 3;
/// The answer was delivered, but the long-term memory maintenance failed.
pub const MEMORY: i32 = 4;
/// The run stopped at a limit before a final answer (turns, output tokens,
/// no progress, content filter, empty response); the partial answer was
/// printed.
pub const INCOMPLETE: i32 = 5;
/// The answer was delivered, but a background sub-agent did not complete
/// (incomplete, failed, cancelled, or stopped at the drain deadline).
pub const BACKGROUND: i32 = 6;
/// Cancelled (Ctrl+C), like a shell's 128 + SIGINT.
pub const CANCELLED: i32 = 130;
