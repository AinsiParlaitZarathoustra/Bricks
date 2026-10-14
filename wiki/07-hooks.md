# Hooks

Hooks intercept events in the agent lifecycle. They can log, block or modify tool inputs. Several hooks can be registered; for a given event they run in registration order, and the first action other than `Continue` is the one returned.

## The Hook trait

```rust
#[async_trait]
pub trait Hook: Send + Sync {
    fn events(&self) -> &[HookEvent];
    async fn on_event(&self, ctx: &HookContext) -> HookAction;
    fn name(&self) -> &str { "unnamed-hook" }  // for logging
}
```

## Hook events

Only some events are fired by the engine in 0.6.1. The table says which.

| Event | Fired by the engine | What a non-`Continue` action does |
|-------|---------------------|-----------------------------------|
| `PreToolUse` | Yes: before each tool call the permission policy allowed, before execution | `Block`: the tool is not run; the model gets `Blocked by hook: <reason>` as a tool error. `ModifyInput`: the tool runs with the new input |
| `PostModelTurn` | Yes: after each model response | `Block`: the run fails with an error, `Blocked by hook: <reason>` |
| `TurnsElapsed` | Yes: every `turns_elapsed_cadence` turns (default 10) | Best effort; the action is ignored |
| `PostToolUse` | No: declared, never fired | — |
| `PreModelTurn` | No: declared, never fired | — |
| `Stop` | No: declared, never fired | — |
| `Error` | No: declared, never fired | — |

Tool-call hooks run after the permission check, so a call the policy denies never reaches `PreToolUse`.

## Hook actions

| Action | Effect |
|--------|--------|
| `Continue` | Proceed normally |
| `Block(reason)` | Stops the operation: on `PreToolUse`, the tool is skipped; on `PostModelTurn`, the run fails. Ignored elsewhere |
| `ModifyInput(Value)` | Replaces the tool's input JSON. Only meaningful on `PreToolUse` |
| `InjectMessage(Box<Message>)` | Declared, but the engine does not apply it in 0.6.1. It still counts as the first non-`Continue` action, so later hooks do not run for that event |

## HookContext

```rust
pub struct HookContext {
    pub event: HookEvent,
    pub tool_name: Option<String>,         // set for tool events
    pub tool_input: Option<Value>,         // set for PreToolUse
    pub tool_result: Option<String>,       // set only when PostToolUse fires (not in 0.6.1)
    pub tool_is_error: Option<bool>,       // set only when PostToolUse fires (not in 0.6.1)
    pub turn: u32,                         // current turn number
    pub cumulative_cost_usd: f64,          // running cost total (also available as a method)
    pub message_count: usize,              // messages in conversation
}
```

## Examples

### Cost guard

Stop the run when spending exceeds a budget. `Block` on `PostModelTurn` makes the run fail with an error:

```rust
struct CostGuard { max_usd: f64 }

#[async_trait]
impl Hook for CostGuard {
    fn events(&self) -> &[HookEvent] { &[HookEvent::PostModelTurn] }
    fn name(&self) -> &str { "cost-guard" }

    async fn on_event(&self, ctx: &HookContext) -> HookAction {
        if ctx.cumulative_cost_usd > self.max_usd {
            HookAction::Block(format!("Budget ${:.2} exceeded", self.max_usd))
        } else {
            HookAction::Continue
        }
    }
}
```

### Audit logger

Record every tool call before it runs. `PostToolUse` never fires, so results are not seen here; read them from `AgentEvent::ToolEnd` (see [Events](05-events-streaming.md)) or from `AgentOutput::tool_calls`:

```rust
struct AuditLogger;

#[async_trait]
impl Hook for AuditLogger {
    fn events(&self) -> &[HookEvent] { &[HookEvent::PreToolUse] }
    fn name(&self) -> &str { "audit" }

    async fn on_event(&self, ctx: &HookContext) -> HookAction {
        println!("[audit] PRE  {} turn={}", ctx.tool_name.as_deref().unwrap_or("?"), ctx.turn);
        HookAction::Continue
    }
}
```

### Tool blocker

Prevent specific tools from running:

```rust
struct ToolBlocker { blocked: Vec<String> }

#[async_trait]
impl Hook for ToolBlocker {
    fn events(&self) -> &[HookEvent] { &[HookEvent::PreToolUse] }
    fn name(&self) -> &str { "tool-blocker" }

    async fn on_event(&self, ctx: &HookContext) -> HookAction {
        if let Some(name) = &ctx.tool_name {
            if self.blocked.iter().any(|b| b == name) {
                return HookAction::Block(format!("Tool '{}' is blocked", name));
            }
        }
        HookAction::Continue
    }
}
```

### Input sanitizer

Refuse a shell command that contains sensitive keywords:

```rust
struct InputSanitizer;

#[async_trait]
impl Hook for InputSanitizer {
    fn events(&self) -> &[HookEvent] { &[HookEvent::PreToolUse] }

    async fn on_event(&self, ctx: &HookContext) -> HookAction {
        if let Some(input) = &ctx.tool_input {
            if let Some(cmd) = input["command"].as_str() {
                if cmd.contains("password") || cmd.contains("secret") {
                    return HookAction::Block("Command contains sensitive keywords".into());
                }
            }
        }
        HookAction::Continue
    }
}
```

A keyword check is a coarse filter, not a security boundary. Use the permission policy for real restrictions.

## Shell hook

`ShellHook` runs a shell command for an event, which is how Claude Code's `settings.json` hooks are reproduced:

```rust
use cersei::hooks::ShellHook;

Agent::builder()
    .hook(ShellHook::new(
        "./scripts/pre-tool.sh",           // command, run with `sh -c`
        &[HookEvent::PreToolUse],          // events
        true,                              // blocking: non-zero exit = Block
    ))
```

- The command receives `CERSEI_HOOK_CONTEXT`, a JSON object with `event`, `tool_name` and `turn`. The tool input is not included.
- The command runs in the process's current directory, not in the agent's `working_dir`.
- With `blocking = true`, a non-zero exit blocks the operation, with the command's stderr (or stdout) as the reason. With `blocking = false`, a non-zero exit is logged and ignored.
- A command that cannot be started is logged and ignored.

## Stacking hooks

```rust
Agent::builder()
    .hook(CostGuard { max_usd: 5.0 })   // checked first
    .hook(ToolBlocker { blocked: vec!["Write".into()] })
    .hook(AuditLogger)                  // returns Continue
    .build()?;
```

For each event, hooks run in registration order until one returns an action other than `Continue`. Hooks registered after that one do not run for the event.

## Hook execution

`cersei_hooks::run_hooks()` implements the loop:

```rust
pub async fn run_hooks(hooks: &[Arc<dyn Hook>], ctx: &HookContext) -> HookAction {
    for hook in hooks {
        if hook.events().contains(&ctx.event) {
            let action = hook.on_event(ctx).await;
            match &action {
                HookAction::Continue => continue,
                _ => return action,  // first non-Continue wins
            }
        }
    }
    HookAction::Continue
}
```
