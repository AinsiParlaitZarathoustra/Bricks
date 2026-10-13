# Events & Streaming

Three mechanisms observe agent activity, from simple to sophisticated: a callback, a broadcast channel and an event stream. Reporters are a structured consumer built on the same events. The variants of `AgentEvent` are listed at the end of this page.

## 1. Callback (simple)

A single closure attached at build time. Good for logging or simple TUI updates.

```rust
let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .tools(cersei::tools::coding())
    .on_event(|event: &AgentEvent| {
        match event {
            AgentEvent::TextDelta(t) => print!("{}", t),
            AgentEvent::ToolStart { name, .. } => eprintln!("[tool] {}...", name),
            AgentEvent::TurnComplete { turn, usage, .. } => {
                eprintln!("[turn {}] {} tokens", turn, usage.total());
            }
            AgentEvent::Error(e) => eprintln!("[error] {}", e),
            _ => {}
        }
    })
    .build()?;
```

The callback runs synchronously in the agent loop, so keep it fast (no blocking I/O).

## 2. Broadcast channel (multi-consumer)

Several independent listeners, each receiving every event (after the event filter, if any). Uses `tokio::sync::broadcast` internally.

```rust
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

let agent = Arc::new(
    Agent::builder()
        .provider(provider_from_config(None, "my-provider/my-model")?)
        .enable_broadcast(256)                 // buffer capacity
        .build()?,
);

// Listener 1: cost tracker
let mut costs = agent.subscribe().expect("enable_broadcast was called");
tokio::spawn(async move {
    loop {
        match costs.recv().await {
            Ok(AgentEvent::CostUpdate { cumulative_cost, .. }) => {
                println!("Cost: ${:.4}", cumulative_cost);
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => eprintln!("skipped {n} events"),
            Err(RecvError::Closed) => break,
        }
    }
});

agent.run("...").await?;
```

A listener that falls behind receives `RecvError::Lagged`; the channel is lossy by design so the agent is never blocked. Handle `Lagged` and keep receiving, as above. Returning from the loop on any error stops the listener.

`agent.subscribe()` returns `None` unless `enable_broadcast` was called.

## 3. Event stream (async iterator)

`run_stream` takes `self: &Arc<Self>`, so the agent must be held in an `Arc`. It returns an `AgentStream`:

```rust
use std::sync::Arc;

let agent = Arc::new(
    Agent::builder()
        .provider(provider_from_config(None, "my-provider/my-model")?)
        .tools(cersei::tools::coding())
        .build()?,
);

let mut stream = agent.run_stream("Deploy to staging");

while let Some(event) = stream.next().await {
    match event {
        AgentEvent::TextDelta(t) => print!("{}", t),

        AgentEvent::Complete(output) => {
            println!("\nDone: {} turns", output.turns);
            break;
        }

        AgentEvent::Error(e) => {
            eprintln!("Error: {}", e);
            break;
        }

        _ => {}
    }
}
```

The stream ends with `Complete` or `Error`; memory maintenance events, if any, come after it.

### AgentStream methods

```rust
impl AgentStream {
    pub async fn next(&mut self) -> Option<AgentEvent>;
    pub async fn collect(self) -> Result<AgentOutput>;
    pub async fn collect_text(self) -> Result<String>;
    pub fn cancel(&self);                                    // takes effect
    pub fn respond_permission(&self, request_id: String, decision: PermissionDecision); // see limitation
    pub fn inject_message(&self, message: String);           // see limitation
}
```

> **Current limitation (repository at 0.6.0).** `run_stream` acts on `cancel()` only. `respond_permission()` and `inject_message()` are accepted but ignored, and the engine never emits `AgentEvent::PermissionRequired`. `InteractivePolicy::via_stream()` returns a policy that allows every call. For approvals, use the approval policy described in [Permissions](08-permissions.md).

The stream receives every event. `event_filter` does not apply to it (see below).

## Reporters

Structured event consumers implementing the `Reporter` trait:

```rust
#[async_trait]
pub trait Reporter: Send + Sync {
    async fn on_event(&self, event: &AgentEvent);
    async fn on_complete(&self, output: &AgentOutput) {}
    async fn on_error(&self, error: &CerseiError) {}
}
```

### Built-in reporters

| Reporter | Description |
|----------|-------------|
| `ConsoleReporter { verbose }` | Prints text and tool activity to stdout/stderr |
| `JsonReporter<W>` | Writes JSON lines to any `Write` (`JsonReporter::new(w)`) |
| `CollectorReporter` | Collects events into a `Vec` (`events()`) for post-hoc analysis |
| `MetricsReporter` | Aggregates into `AgentMetrics` and hands them to a closure every interval |

```rust
use std::{fs::File, time::Duration};
use cersei::reporters::*;

let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .reporter(ConsoleReporter { verbose: true })
    .reporter(JsonReporter::new(File::create("agent.jsonl")?))
    .reporter(MetricsReporter::new(Duration::from_secs(10), |m| {
        println!("Turns: {}, Cost: ${:.4}", m.total_turns, m.total_cost_usd);
    }))
    .build()?;
```

### Custom reporter

```rust
struct SlackNotifier { webhook_url: String }

#[async_trait]
impl Reporter for SlackNotifier {
    async fn on_event(&self, event: &AgentEvent) {
        if let AgentEvent::Error(e) = event {
            // POST `e` to self.webhook_url
        }
    }

    async fn on_complete(&self, output: &AgentOutput) {
        // Notify that the agent finished
    }
}
```

Register it with `.reporter(SlackNotifier { webhook_url })`.

### AgentMetrics

```rust
pub struct AgentMetrics {
    pub total_turns: u32,
    pub total_tool_calls: u32,
    pub total_cost_usd: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub avg_turn_duration: Duration,
    pub tool_call_histogram: HashMap<String, u32>,
}
```

## Event filtering

`event_filter` keeps only the events a predicate accepts. It applies to the callback, to broadcast listeners and to reporters. It does not apply to `AgentStream`, which receives every event.

```rust
Agent::builder()
    .event_filter(|e| {
        // Only forward tool and cost events
        matches!(e,
            AgentEvent::ToolStart { .. } |
            AgentEvent::ToolEnd { .. } |
            AgentEvent::CostUpdate { .. }
        )
    })
```

## Full AgentEvent enum

The enum is defined in [`crates/cersei-agent/src/events.rs`](../crates/cersei-agent/src/events.rs). Its variants, grouped by theme:

- **Streaming:** `TextDelta`, `ThinkingDelta`
- **Tools:** `ToolStart`, `ToolProgress`, `ToolPermissionCheck`, `ToolEnd` (with `duration` and compression stats)
- **Permissions and approvals:** `PermissionRequired` (declared, not emitted by the engine), `ApprovalRequested`, `ApprovalResolved` (with `decision` and `by`)
- **Turns and model requests:** `TurnStart`, `TurnComplete`, `ModelRequestStart`, `ModelResponseStart`
- **Context:** `TokenWarning`, `CompactStart`, `CompactEnd`, `CompactionResult`, `ContextUpdate`
- **Sessions:** `SessionLoaded`, `SessionSaved`
- **Cost:** `CostUpdate`
- **Sub-agents and jobs:** `SubAgentSpawned`, `SubAgentComplete`, `SubAgent`, `Job`
- **Hooks:** `HookFired`, `HookBlocked`
- **Memory:** `MemoryRecalled`, `MemoryMaintenanceStarted`, `MemoryMaintenanceFinished`
- **Edits and skills:** `EditApplied`, `SkillActivated`, `SkillNotActivated`
- **Terminal:** `Status`, `Error`, `Complete`

Consult the file for the fields of each variant; the enum evolves between releases.
