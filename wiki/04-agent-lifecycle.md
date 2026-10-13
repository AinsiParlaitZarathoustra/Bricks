# Agent Lifecycle

## Build → Configure → Run → Result

```rust
let agent = Agent::builder()      // 1. Create builder
    .provider(cersei::provider_from_config(None, "my-provider/my-model")?)   // 2. Configure
    .tools(cersei::tools::coding())
    .build()?;                          // 3. Build (validates config)

let output = agent.run("prompt").await?;  // 4. Run (agentic loop)
println!("{}", output.text());            // 5. Use result
```

## The Agentic Loop

When you call `agent.run("prompt")`, this happens:

```
1. LOAD SESSION
   If memory + session_id configured:
     Load previous messages from Memory backend

2. APPEND USER MESSAGE
   Add Message::user(prompt) to conversation

3. LOOP (no turn limit; until a final answer or another stop):
   a. BUILD REQUEST
      - Collect conversation messages
      - Attach system prompt
      - Attach tool definitions
      - Set model, max_tokens, temperature, reasoning profile

   b. SEND TO PROVIDER (streaming)
      - Provider.complete(request) → CompletionStream
      - Emit: ModelRequestStart, ModelResponseStart

   c. PROCESS STREAM
      - Accumulate text deltas → emit TextDelta events
      - Accumulate thinking deltas → emit ThinkingDelta events
      - Accumulate tool use blocks
      - Track usage/cost → emit CostUpdate events

   d. POST-MODEL HOOKS
      - Fire PostModelTurn hooks
      - If any hook returns Block → the run returns an error (`Blocked by hook: ...`)

   e. HANDLE STOP REASON
      ┌─ EndTurn → break (model is done)
      ├─ ToolUse → dispatch tools (see below)
      ├─ MaxTokens → inject a continue message and retry (at most 3 times, then the run ends with `Termination::OutputTruncated`)
      └─ Other → break

   f. DISPATCH TOOLS (if ToolUse)
      For each tool_use block:
        - Fire PreToolUse hooks (can Block or ModifyInput)
        - Check PermissionPolicy
        - Execute tool
        - Fire PostToolUse hooks
        - Record ToolCallRecord
        - Emit: ToolStart, ToolEnd events
      Append all ToolResults as a user message
      Continue loop

   g. AUTO-COMPACT (if enabled)
      If context usage ≥ compact_threshold:
        Emit CompactStart, summarize old messages, emit CompactEnd

4. PERSIST SESSION
   If memory + session_id configured:
     Save conversation to Memory backend
     Emit: SessionSaved

5. RETURN AgentOutput
   - Final assistant message
   - Cumulative usage/cost
   - Stop reason
   - Turn count
   - All tool call records
```

## AgentOutput

```rust
pub struct AgentOutput {
    pub message: Message,                // final assistant message
    pub usage: Usage,                    // cumulative token usage
    pub stop_reason: StopReason,         // provider's reason for its last response
    pub turns: u32,                      // generation turns actually run
    pub tool_calls: Vec<ToolCallRecord>, // all tool calls made
    pub termination: Termination,        // how the run ended (Completed, OutputTruncated, NoProgress, ...)
}

impl AgentOutput {
    pub fn text(&self) -> &str;        // convenience: extract text
}
```

### ToolCallRecord

```rust
pub struct ToolCallRecord {
    pub name: String,                  // tool name
    pub id: String,                    // tool use ID
    pub input: serde_json::Value,      // what was passed
    pub result: String,                // what the tool returned
    pub is_error: bool,                // whether it errored
    pub duration: Duration,            // wall-clock time
}
```

## Multi-Turn Conversations

Use `agent.reply()` to continue a conversation:

```rust
let agent = Agent::builder()
    .provider(cersei::provider_from_config(None, "my-provider/my-model")?)
    .build()?;

// First turn
let out1 = agent.run("What files are in src/?").await?;
println!("{}", out1.text());

// Follow-up (same conversation context)
let out2 = agent.reply("Now read the largest one.").await?;
println!("{}", out2.text());

// Check full history
println!("Total messages: {}", agent.messages().len());
```

## Cancellation

```rust
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();
let agent = Agent::builder()
    .provider(cersei::provider_from_config(None, "my-provider/my-model")?)
    .cancel_token(token.clone())
    .build()?;

// Cancel from another task
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(30)).await;
    token.cancel();  // agent.run() will return Err(Cancelled)
});

// Or cancel directly
agent.cancel();
```

## Context Management

### Auto-Compact

When the conversation approaches the context window limit, Cersei automatically compacts older messages:

```rust
Agent::builder()
    .auto_compact(true)          // enabled by default
    .compact_threshold(0.9)      // trigger at 90% context usage
    .tool_result_budget(50_000)  // max chars of tool results before eviction
```

### Tool Result Budget

Large tool outputs (e.g., reading a 10K line file) are tracked against a character budget (default 50,000). When the budget is exceeded, older tool results are removed from the active context and replaced by a placeholder that states their size.

## Shorthand: run_with

For one-shot agents, skip the `build()` step:

```rust
let output = Agent::builder()
    .provider(cersei::provider_from_config(None, "my-provider/my-model")?)
    .tools(cersei::tools::coding())
    .run_with("Fix the tests")  // build + run in one call
    .await?;
```
