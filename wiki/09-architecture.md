# Architecture

## Workspace Layers (Bricks 1.0.0-alpha.1)

```text
bricks-cli            `bricks` binary: CLI arguments, config loading, headless / JSONL runs
bricks-config         shared engine assembly for the CLI and desktop app
apps/bricks-desktop   desktop application (Tauri 2 + Svelte 5)
bricks-agent          runtime: agent loop, controller, permissions, compaction, sub-agents
  bricks-provider     model registry and HTTP/SSE protocol clients
  bricks-tools        built-in tools and PermissionPolicy
  bricks-memory       session storage and structured long-term memory
  bricks-hooks        lifecycle hooks (Hook, ShellHook)
  bricks-mcp          Model Context Protocol client
  bricks-skills       SKILL.md discovery, loading, and activation
  bricks-lsp          Language Server Protocol client
  bricks-semantic     symbol and code intelligence over LSP
  bricks-embeddings   embedding providers and hashing fallback
  bricks-compression  tool output compression
  bricks-web          web fetch and search runtime
  bricks-vms          sandboxed VM support (optional `vms` feature)
bricks-types          shared types, BricksError, and shared HTTP client
```

The workspace in `Cargo.toml` has 25 members, including `bricks` (the facade crate), `bricks-tools-derive` (`#[derive(Tool)]`), `bricks-agentlang`, `bricks-agentrl`, `bricks-workflows`, `bricks-tbench`, `bricks-testkit`, and `bench/long-mem`. See `docs/content/docs/crate-map.mdx` for the full list.

## Dependency Flow

Internal dependencies declared in each crate's `Cargo.toml`:

```text
bricks-types          shared types, BricksError, shared HTTP client (no internal deps)
    ^
    |
bricks-provider       depends on bricks-types
bricks-hooks          depends on bricks-types
bricks-mcp            depends on bricks-types
bricks-compression    depends on bricks-types
bricks-web            depends on bricks-types
bricks-embeddings     depends on bricks-types
bricks-vms            depends on bricks-types
bricks-memory         depends on bricks-types (feature `structured` adds bricks-provider, bricks-embeddings)
bricks-lsp            no internal dependencies
bricks-semantic       depends on bricks-lsp, bricks-types
bricks-skills         no internal dependencies
    ^
    |
bricks-tools          depends on bricks-mcp, bricks-web, bricks-compression, bricks-embeddings,
                      bricks-semantic, bricks-skills, bricks-lsp, bricks-types,
                      and bricks-vms (optional `vms` feature)
    ^
    |
bricks-agent          depends on bricks-hooks, bricks-memory, bricks-provider, bricks-tools,
                      bricks-mcp, bricks-compression, bricks-web, bricks-semantic,
                      bricks-skills, bricks-types
    ^
    |
bricks                facade + prelude: re-exports the core crates above, with features
                      `vms` (default), `agentlang`, `agentrl`, and `workflows`
```

The `bricks` facade re-exports sub-crates as modules (for example `bricks::hooks`, `bricks::memory`, `bricks::tools`).

## Data Flow

```text
User prompt
    |
    v
Agent::run("prompt")
    |
    +-- Load session from Memory (if configured)
    +-- Recall from LongTermMemory (if configured)
    |
    v
+-- AGENTIC LOOP ---------------------------------------+
|                                                       |
|  Build CompletionRequest                              |
|    |                                                  |
|    v                                                  |
|  Provider.complete(request) -> CompletionStream       |
|    |                                                  |
|    v                                                  |
|  StreamAccumulator collects events                    |
|    +-- TextDelta -> emit to listeners                 |
|    +-- ThinkingDelta -> emit                          |
|    +-- ToolUse -> queue for dispatch                  |
|    |                                                  |
|    v                                                  |
|  CompletionResponse                                   |
|    |                                                  |
|    +-- PostModelTurn hooks                            |
|    |                                                  |
|    v                                                  |
|  Answer has tool calls?                               |
|    +-- none: final answer, BREAK                      |
|    +-- some: dispatch tools                           |
|    |     For each tool:                               |
|    |       Permission check                           |
|    |       PreToolUse hooks                           |
|    |       Tool.execute()                             |
|    |       Record ToolCallRecord                      |
|    |     Append results and CONTINUE loop             |
|    |                                                  |
|    +-- MaxTokens -> retry up to limit, or end as      |
|          OutputTruncated                              |
|                                                       |
+-------------------------------------------------------+
    |
    +-- Save session to Memory (if configured)
    +-- Capture turn to LongTermMemory (if configured)
    |
    v
AgentOutput { message, usage, stop_reason, turns, tool_calls, termination }
```

A denied call returns `Permission denied: <reason>` to the model as a tool error. A call stopped by a `PreToolUse` hook returns `Blocked by hook: <reason>`. `PostToolUse` is declared in `HookEvent` but is not fired by the current runner.

## Event Distribution

```text
Agent loop emits AgentEvent
    |
    +-- on_event callback (if set): synchronous, in-loop
    |
    +-- broadcast channel (if enabled): async, multi-consumer
    |     +-- subscriber 1
    |     +-- subscriber 2
    |     +-- subscriber N
    |
    +-- reporters (Vec<Arc<dyn Reporter>>)
          +-- ConsoleReporter
          +-- JsonReporter
          +-- CollectorReporter
          +-- MetricsReporter
```

## Design Principles

1. **Trait-based components:** Core pieces are traits (`Provider`, `Tool`, `Memory`, `LongTermMemory`, `Hook`, `PermissionPolicy`, `Reporter`), so you can swap any part.
2. **Builder pattern:** `AgentBuilder` uses `self -> Self` chaining and validates configuration at `build()` time.
3. **Scoped session services:** Tools like `TodoWrite` write to a per-session `PlanService` (`plan.json` in the session files folder) rather than a global singleton. `ShellSessions` tracks shell state per session id and cleans up when the session closes.
4. **Streaming-first:** Providers emit `StreamEvent` values; `StreamAccumulator` turns them into responses while listeners receive deltas right away.
5. **Headless-first:** The SDK has no terminal UI dependency. Frontends (`bricks-cli`, `apps/bricks-desktop`) drive the engine and consume events.
6. **Zero-cost defaults:** Hooks, memory, and broadcast channels are off until you configure them.
