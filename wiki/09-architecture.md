# Architecture

## Current layers (Bricks 0.6.1)

```
bricks-cli            the `bricks` binary: arguments, configuration, headless / JSONL
bricks-config         engine assembly shared by the CLI and the desktop
apps/bricks-desktop   desktop application (Tauri 2 + Svelte 5) over the controller
bricks-agent          engine: agent loop, controller (commands/events), approvals, compaction
  bricks-provider, bricks-tools, bricks-memory, bricks-compression, bricks-web,
  bricks-semantic, bricks-skills, bricks-lsp, bricks-mcp, bricks-embeddings, …
bricks-types          shared types and the shared HTTP client
```

The complete list is in the README (Architecture) and in
`docs/content/docs/crate-map.mdx`.

## Crate Map (Cersei-era snapshot)

Historical: the layout below is the original Cersei workspace (before
`bricks-cli`, `bricks-tui` and the provider registry). File names such as
`anthropic.rs` / `openai.rs` no longer exist.

```
src-cersei/
├── Cargo.toml                    workspace manifest
├── README.md                     project README with benchmark results
├── docs/                         this documentation
├── examples/
│   └── benchmark/                standalone benchmark binary
│       ├── Cargo.toml
│       ├── src/main.rs
│       └── README.md
└── crates/
    ├── cersei/                   facade — re-exports everything
    │   ├── src/lib.rs            prelude, convenience re-exports
    │   └── examples/             8 runnable examples
    │
    ├── cersei-types/             provider-agnostic types
    │   └── src/lib.rs            Message, ContentBlock, Usage, CerseiError, StreamEvent
    │
    ├── cersei-provider/          LLM provider abstraction
    │   └── src/
    │       ├── lib.rs            Provider trait, Auth, CompletionRequest/Stream
    │       ├── stream.rs         StreamAccumulator
    │       ├── anthropic.rs      Anthropic provider (SSE streaming, OAuth)
    │       └── openai.rs         OpenAI-compatible provider
    │
    ├── cersei-tools/             tool system
    │   └── src/
    │       ├── lib.rs            Tool trait, ToolContext, ToolResult, built-in sets
    │       ├── permissions.rs    PermissionPolicy trait + built-in policies
    │       ├── bash.rs           Bash tool (shell state persistence)
    │       ├── file_read.rs      Read tool
    │       ├── file_write.rs     Write tool
    │       ├── file_edit.rs      Edit tool (string replacement)
    │       ├── glob_tool.rs      Glob tool
    │       └── grep_tool.rs      Grep tool (rg/grep)
    │
    ├── cersei-tools-derive/      proc-macro crate
    │   └── src/lib.rs            #[derive(Tool)] implementation
    │
    ├── cersei-agent/             agent builder and runtime
    │   └── src/
    │       ├── lib.rs            Agent, AgentBuilder, AgentOutput
    │       ├── events.rs         AgentEvent (36 variants), AgentStream, AgentControl
    │       ├── reporters.rs      Reporter trait + Console/Json/Collector/Metrics
    │       └── runner.rs         Agentic loop implementation
    │
    ├── cersei-memory/            session persistence
    │   └── src/lib.rs            Memory trait, JsonlMemory, InMemory
    │
    ├── cersei-hooks/             middleware system
    │   └── src/lib.rs            Hook trait, ShellHook, run_hooks()
    │
    └── cersei-mcp/               Model Context Protocol
        └── src/lib.rs            McpManager, McpServerConfig
```

## Dependency Flow

The internal dependencies, as declared in each `Cargo.toml`. Optional dependencies (behind a feature) are listed where they apply.

```
bricks-types          shared types, the error type, the shared HTTP client
    ↑                 (no internal dependency)
    │
bricks-provider       LLM communication: depends on bricks-types only
bricks-hooks          lifecycle interception: depends on bricks-types only
bricks-mcp            MCP client: depends on bricks-types only
bricks-compression, bricks-web, bricks-embeddings, bricks-memory
                      depend on bricks-types only (bricks-memory's `structured`
                      and `embed` features add bricks-provider and bricks-embeddings)
bricks-lsp            no internal dependency
bricks-semantic       depends on bricks-lsp and bricks-types
bricks-skills         no internal dependency
bricks-vms            depends on bricks-types only
    ↑
bricks-tools          tool system: depends on bricks-mcp, bricks-web, bricks-compression,
                      bricks-embeddings, bricks-semantic, bricks-skills, bricks-lsp,
                      bricks-types, and bricks-vms with the `vms` feature
    ↑
bricks-agent          the runtime: depends on bricks-hooks, bricks-memory, bricks-provider,
                      bricks-tools, bricks-mcp, bricks-compression, bricks-web,
                      bricks-semantic, bricks-skills, bricks-types
bricks-agentlang      AgentTemplate: depends on bricks-tools, bricks-types, bricks-vms
bricks-agentrl        self-evolving loop: depends on bricks-agent, bricks-agentlang, bricks-tools,
                      bricks-memory, bricks-embeddings, bricks-vms, bricks-provider, bricks-compression, bricks-types
bricks-workflows      depends on bricks-agent, bricks-tools, bricks-types
    ↑
bricks                facade + prelude: re-exports the crates above, with the features
                      `vms` (default), `agentlang`, `agentrl` and `workflows`
```

The `bricks` facade re-exports the crates as modules (for example `bricks::hooks`, `bricks::memory`, `bricks::tools`).

## Data Flow

```
User prompt
    │
    ▼
Agent::run("prompt")
    │
    ├── Load session from Memory (if configured)
    │
    ▼
┌─ AGENTIC LOOP ──────────────────────────────────┐
│                                                   │
│  Build CompletionRequest                          │
│    │                                              │
│    ▼                                              │
│  Provider.complete(request) → CompletionStream    │
│    │                                              │
│    ▼                                              │
│  StreamAccumulator collects events                │
│    │  ├── TextDelta → emit to listeners           │
│    │  ├── ThinkingDelta → emit                    │
│    │  └── ToolUse → queue for dispatch            │
│    │                                              │
│    ▼                                              │
│  CompletionResponse                               │
│    │                                              │
│    ├── PostModelTurn hooks                        │
│    │                                              │
│    ▼                                              │
│  Answer has tool calls?                           │
│    ├── none: final answer, BREAK                  │
│    ├── some: dispatch tools ──┐                   │
│    │                              │               │
│    │   For each tool:             │               │
│    │     PermissionPolicy check   │               │
│    │     PreToolUse hooks         │               │
│    │     Tool.execute()           │               │
│    │     Record ToolCallRecord    │               │
│    │                              │               │
│    │   Append results ────────────┘               │
│    │   CONTINUE loop                              │
│    │                                              │
│    └── MaxTokens → retry up to a limit, then the  │
│         run ends as OutputTruncated               │
│                                                   │
└───────────────────────────────────────────────────┘
    │
    ├── Save session to Memory (if configured)
    │
    ▼
AgentOutput { message, usage, stop_reason, turns, tool_calls, termination }
```

A denied call returns `Permission denied: <reason>` to the model as a tool error. A call stopped by a `PreToolUse` hook returns `Blocked by hook: <reason>`. `PostToolUse` is declared but never fired in 0.6.1 (see [Hooks](07-hooks.md)), so it does not appear in the loop.

## Event Distribution

```
Agent loop emits AgentEvent
    │
    ├── on_event callback (if set)     — synchronous, in-loop
    │
    ├── broadcast channel (if enabled) — async, multi-consumer
    │   ├── subscriber 1
    │   ├── subscriber 2
    │   └── subscriber N
    │
    └── reporters (Vec<Arc<dyn Reporter>>)
        ├── ConsoleReporter
        ├── JsonReporter
        └── MetricsReporter
```

## Design Principles

1. **Trait-based extensibility** — every major component is a trait (`Provider`, `Tool`, `Memory`, `Hook`, `PermissionPolicy`, `Reporter`). Swap any component by implementing the trait.

2. **Builder pattern** — `AgentBuilder` with `self -> Self` chaining. Validates at `build()` time, not at method call time.

3. **Few process-wide registries** — `bricks-tools` keeps some global registries (for example for shell sessions, tasks, todos, cron jobs and inboxes). `ShellSessions` holds one entry per tool-context session id, and its entry is removed by the cleanup functions. Agent configuration flows through the builder.

4. **Streaming-first** — providers emit `StreamEvent`s. The accumulator is a separate concern. Consumers (callback, broadcast, stream) get events as they arrive.

5. **Headless-first** — no TUI dependency in the SDK. The TUI is an application-layer concern that consumes the engine via events.

6. **Zero-cost defaults** — `AllowAll` permission policy, no hooks, no memory, no broadcast. You only pay for what you configure.
