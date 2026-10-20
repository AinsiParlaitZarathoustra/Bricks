//! The builtin namespace → operation mapping.
//!
//! Resolves a dotted call path (`io.read`, `agent.send`, …) to an [`Op`] that
//! the interpreter knows how to execute, along with the permission level the
//! call requires.

use bricks_tools::PermissionLevel;

/// What a resolved builtin call does.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Dispatch to an underlying named `Tool`. Positional args fill `positional`
    /// keys in order; named args override by key; `pipe_target` (if set) is the
    /// key that a chained `$_` value fills when not given explicitly.
    Tool {
        name: &'static str,
        positional: &'static [&'static str],
        pipe_target: Option<&'static str>,
        perm: PermissionLevel,
    },
    /// `io.delete($f)` — routed through Bash `rm`. First positional / `$_` is the path.
    DeleteViaBash,
    /// `agent.send(to: id, payload)` — publish to a Mailbox topic.
    AgentSend,
    /// `agent.tools.call(name, args...)` — dispatch a tool dynamically.
    ToolsCall,
    /// `agent.tools.list()` — list dispatchable tool names.
    ToolsList,
    /// `agent.tools.register(...)` — register a new tool (needs the agentrl registry).
    ToolsRegister,
    /// `agent.permission.ask('rw')` — ask the permission policy; returns a bool.
    PermissionAsk,
    /// `kv.get($k)` — read shared state.
    KvGet,
    /// `kv.set($k, $v)` — write shared state.
    KvSet,
}

/// Resolve a dotted path to its operation, or `None` if unknown.
pub fn resolve(path: &[String]) -> Option<Op> {
    let joined = path.join(".");
    Some(match joined.as_str() {
        "io.read" => Op::Tool {
            name: "Read",
            positional: &["file_path"],
            pipe_target: None,
            perm: PermissionLevel::ReadOnly,
        },
        "io.write" => Op::Tool {
            name: "Write",
            positional: &["file_path", "content"],
            pipe_target: Some("content"),
            perm: PermissionLevel::Write,
        },
        "io.edit" => Op::Tool {
            name: "Edit",
            positional: &["file_path"],
            pipe_target: None,
            perm: PermissionLevel::Write,
        },
        "io.glob" => Op::Tool {
            name: "Glob",
            positional: &["pattern"],
            pipe_target: None,
            perm: PermissionLevel::ReadOnly,
        },
        "io.grep" => Op::Tool {
            name: "Grep",
            positional: &["pattern"],
            pipe_target: None,
            perm: PermissionLevel::ReadOnly,
        },
        "io.delete" => Op::DeleteViaBash,
        "net.get" => Op::Tool {
            name: "WebFetch",
            positional: &["url"],
            pipe_target: None,
            perm: PermissionLevel::ReadOnly,
        },
        "net.search" => Op::Tool {
            name: "WebSearch",
            positional: &["query"],
            pipe_target: None,
            perm: PermissionLevel::ReadOnly,
        },
        "agent.send" => Op::AgentSend,
        "agent.tools.call" => Op::ToolsCall,
        "agent.tools.list" => Op::ToolsList,
        "agent.tools.register" => Op::ToolsRegister,
        "agent.permission.ask" => Op::PermissionAsk,
        "kv.get" => Op::KvGet,
        "kv.set" => Op::KvSet,
        _ => return None,
    })
}

/// The permission level a resolved op requires (for the pre-execution gate).
pub fn op_permission(op: &Op) -> PermissionLevel {
    match op {
        Op::Tool { perm, .. } => *perm,
        Op::DeleteViaBash => PermissionLevel::Write,
        Op::AgentSend => PermissionLevel::Write,
        Op::ToolsCall => PermissionLevel::Execute,
        Op::ToolsList => PermissionLevel::None,
        Op::ToolsRegister => PermissionLevel::Dangerous,
        Op::PermissionAsk => PermissionLevel::None,
        Op::KvGet => PermissionLevel::ReadOnly,
        Op::KvSet => PermissionLevel::Write,
    }
}
