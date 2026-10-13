# Permissions

Permissions decide whether a tool call runs. Two layers are involved:

1. The agent's **permission policy** (`PermissionPolicy`), asked for every tool call. If none is set, the agent uses `AllowAll`. The session engine (the `bricks` CLI and TUI) installs an **approval policy**, `ApprovalGate`, configured by the `[permissions]` section of `bricks.toml`. It can ask a person before a call runs.
2. The **PreToolUse hooks** ([Hooks](07-hooks.md)), which run only for calls the policy allowed.

## The PermissionPolicy trait

```rust
#[async_trait]
pub trait PermissionPolicy: Send + Sync {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision;
}
```

### PermissionRequest

```rust
pub struct PermissionRequest {
    pub tool_name: String,                 // which tool
    pub tool_input: serde_json::Value,     // what arguments
    pub permission_level: PermissionLevel, // level for this input
    pub description: String,               // human-readable description
    pub id: String,                        // ID of the tool call
    pub preview: Option<ChangePreview>,    // file changes the call would make, when computable
    pub agent_id: Option<String>,          // the sub-agent making the call (None: top level)
}
```

### PermissionDecision

```rust
pub enum PermissionDecision {
    Allow,                 // run the call
    AllowOnce,             // run this call
    AllowForSession,       // run the call; see note below
    Deny(String),          // refuse; the model receives "Permission denied: <reason>"
}
```

The engine treats the three allow variants alike. Nothing in the engine remembers `AllowForSession`: only `ApprovalGate` does, for its own asks. A custom policy that wants session-wide approvals must keep its own state.

## Built-in policies

### AllowAll

Allow everything. Use for CI, headless agents and trusted environments.

```rust
Agent::builder().permission_policy(AllowAll)
```

### AllowReadOnly

Allow only tools at `None` or `ReadOnly`. Other calls are denied with "requires … permission (read-only mode)".

```rust
Agent::builder().permission_policy(AllowReadOnly)
```

### DenyAll

Deny every tool call. The model can still answer in text.

```rust
Agent::builder().permission_policy(DenyAll)
```

### RuleBased

Rules are checked in order; the first matching rule decides. A rule matches a tool when its `tool_name` is `None`, is `"all"`, or equals the tool's name. If no rule matches, the call is allowed. `path_pattern` is part of the struct but is not evaluated in 0.6.0.

```rust
use cersei::tools::permissions::{PermissionAction, PermissionRule, RuleBased};

Agent::builder().permission_policy(RuleBased {
    rules: vec![
        PermissionRule { tool_name: Some("Read".into()), path_pattern: None, action: PermissionAction::Allow },
        PermissionRule { tool_name: Some("Glob".into()), path_pattern: None, action: PermissionAction::Allow },
        PermissionRule { tool_name: Some("Grep".into()), path_pattern: None, action: PermissionAction::Allow },
        // Deny everything else
        PermissionRule { tool_name: None, path_pattern: None, action: PermissionAction::Deny },
    ],
})
```

### InteractivePolicy

Calls a synchronous callback for each call. It cannot wait for a person asynchronously, so use it only for simple prompts on a terminal:

```rust
use cersei::prelude::*;

Agent::builder().permission_policy(InteractivePolicy::new(|req| {
    println!("\nTool '{}' wants to run: {}", req.tool_name, req.tool_input);
    // read the answer from the terminal here
    PermissionDecision::Allow
}))
```

`InteractivePolicy::via_stream()` returns a `StreamDeferredPolicy`, which allows every call. No event asks the stream consumer, and `AgentEvent::PermissionRequired` is never emitted in 0.6.0. Do not use it for approvals.

## Permission levels

Each tool declares a level (see [Tools](03-tools.md) for the level of every built-in tool):

| Level | Risk | Examples |
|-------|------|----------|
| `None` | Safe | Task tools, TodoWrite, Sleep |
| `ReadOnly` | Low | Read, Glob, Grep, WebFetch, Skill |
| `Write` | Medium | Write, Edit, MultiEdit, ApplyPatch |
| `Execute` | High | Bash, PowerShell, CronCreate |
| `Dangerous` | Very high | MCP tools annotated `destructiveHint` |
| `Forbidden` | Never | No built-in tool; refused by the approval policy |

MCP tools take their level from their annotations: `readOnlyHint: true` gives `ReadOnly` (checked first), `destructiveHint: true` gives `Dangerous`, and any other MCP tool gets `Execute`.

## Approvals (`[permissions]` in bricks.toml)

The session engine applies the approval policy: rules from the `[permissions]` section of the project's `bricks.toml`. When a rule says `ask`, a person decides, through the terminal interface or a frontend built on `cersei_agent::control`.

```toml
[permissions]              # the values below are the defaults
none = "allow"
read_only = "allow"
write = "ask"
execute = "ask"
dangerous = "ask"
default = "ask"            # parsed; see note below

[permissions.tools]        # by tool name; checked before the levels
WebFetch = "allow"
Bash = "ask"
```

- **Actions:** `allow` (run without asking), `ask` (a person decides), `deny` (refuse).
- **Order:** a `Forbidden` tool is always refused. Then the `[permissions.tools]` entry for the tool's name, if any. Then the entry for its level.
- **Invalid section:** unknown keys make the whole section invalid. The defaults are used and a diagnostic is shown when the session opens.
- **`default`:** accepted, but every level has its own key, so `default` never decides a call in 0.6.0. `docs/cli.md` says it applies after the levels; the code does not do that.

### What happens when a call is asked about

1. `ApprovalRequested` is emitted with `approval_id`, `tool_call_id`, `tool`, `level`, `description`, `input`, the `preview` when the tool can compute one, and `agent_id`.
2. The person answers `allow`, `allow_for_session` (applies to later calls of the same tool in this session) or `deny`.
3. `ApprovalResolved` is emitted with `decision` and `by` (`user`, `session`, `non_interactive` or `cancelled`).

File changes (`Write`, `Edit`, `MultiEdit`, `ApplyPatch`) are computed without writing, and nothing is written before the decision. If a file changes while the approval is pending, the change is recomputed and asked again; after three changes in a row it is refused. Shell commands and MCP calls have no preview, so the approval shows the command and its context.

With `--non-interactive`, or without a terminal, nobody can be asked: the call is refused, and the run stops with `failure = "approval_required"` and exit code 3. `--non-interactive` grants nothing by itself.

### Using the approval gate from Rust

The gate lives in `cersei_agent::control`. The `cersei` facade does not re-export that module, so depend on `cersei-agent` directly:

```rust
use cersei_agent::bricks_config::BricksConfig;
use cersei_agent::control::{ApprovalBroker, ApprovalGate};

let working_dir = std::env::current_dir()?;
let bricks = BricksConfig::load(&working_dir);   // reads <working_dir>/bricks.toml and ~/.bricks/rules/*.toml
let broker = ApprovalBroker::new(false);         // nobody to ask: "ask" becomes a refusal

let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .tools(cersei::tools::coding())
    .working_dir(working_dir.clone())
    .permission_policy(ApprovalGate::new(bricks.permissions.clone(), broker))
    .build()?;
```

A broker created this way refuses every call the rules say to ask about. Interactive approval needs the session controller, because it is the one that attaches a run to the broker. That attachment is crate-private, so a standalone broker cannot receive requests.

## Custom policy

```rust
use chrono::{Local, Timelike};

struct TimeBasedPolicy;

#[async_trait]
impl PermissionPolicy for TimeBasedPolicy {
    async fn check(&self, req: &PermissionRequest) -> PermissionDecision {
        let hour = Local::now().hour();

        // Only allow writes during business hours
        if req.permission_level == PermissionLevel::Write && !(9..17).contains(&hour) {
            PermissionDecision::Deny("Write operations only allowed 9am-5pm".into())
        } else {
            PermissionDecision::Allow
        }
    }
}
```

## How permissions flow

```
Model requests tool_use("Bash", {"command": "rm -rf /tmp/old"})
    │
    ▼
policy.check(PermissionRequest {
    tool_name: "Bash",
    permission_level: Execute,
    ...
})
    │
    ├── Allow / AllowOnce / AllowForSession
    │       │
    │       ▼
    │   PreToolUse hooks ── Block(reason) ──► tool error "Blocked by hook: reason"
    │       │
    │       ▼
    │   execute tool → result to model
    │
    └── Deny("reason") → tool error "Permission denied: reason"
                          model sees the denial and can adjust
```

When a call is denied, the model receives the reason as a tool error. This lets it try another approach, for example `Read` instead of `Bash` with `cat`.
