# Cersei

The complete Rust SDK for building coding agents.

Cersei gives you every building block of a production coding agent — tool execution, LLM streaming, sub-agent orchestration, persistent memory, skills, MCP integration — as composable library functions. Build a Claude Code replacement, embed an agent in your app, or create something entirely new.

```rust
use cersei::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let output = Agent::builder()
        // Models are configuration: ~/.bricks/providers.toml, selected as provider_id/model_id.
        .provider(provider_from_config(None, "my-provider/my-model")?)
        .tools(cersei::tools::coding())
        .permission_policy(AllowAll)
        .run_with("Fix the failing tests in src/")
        .await?;

    println!("{}", output.text());
    Ok(())
}
```

**MIT License** | Built by [Adib Mohsin](https://github.com/pacifio) | [Docs](https://cersei.pacifio.dev/docs) | [GitHub](https://github.com/pacifio/cersei)

---

### The `bricks` command

```bash
cargo install --path crates/bricks-cli
bricks                                   # interactive terminal interface
bricks run --json --non-interactive "Analyse the build errors"   # scripts and CI: JSONL events
bricks resume                            # pick a stored session
```

The CLI and its terminal interface are thin frontends over the engine's
command/event contract (`cersei_agent::control`): models come from
`providers.toml`, settings from `bricks.toml`, approvals from its
`[permissions]` policy. See [docs/cli.md](docs/cli.md).

---

## Why Cersei

| | Claude Code | OpenCode | **Cersei SDK** | **Abstract CLI** |
|---|---|---|---|---|
| Form factor | CLI app | CLI app | **Library** | **CLI app** |
| Embeddable | No | No | **Yes** | No (uses SDK) |
| Provider | Anthropic only | Multi-provider | **Configurable (no code)** | **Configurable (no code)** |
| Language | TypeScript | TypeScript | **Rust** | **Rust** |
| Custom tools | Plugins | Plugins | **`impl Tool` / `#[derive(Tool)]`** | Via SDK |
| Startup | ~269ms | ~300ms | N/A (library) | **~34ms** |
| Binary / RSS | 174MB / 330MB | — | N/A | **5.8MB / 4.9MB** |
| Memory | File-based | SQLite | **File + Graph** | **File + Graph** |
| Skills | `.claude/commands/` | `.claude/skills/` | **Both formats** | **Both formats** |

Cersei is built from the architecture of Claude Code (reverse-engineered Rust port) and designed so that anyone can build a complete, drop-in replacement for Claude Code, OpenCode, or any coding agent — as a library call.

---

## Install

```toml
[dependencies]
cersei = { git = "https://github.com/pacifio/cersei" }
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

For graph-backed memory (optional):
```toml
cersei-memory = { git = "https://github.com/pacifio/cersei", features = ["graph"] }
```

---

## Architecture

```
cersei                    Facade crate — use cersei::prelude::*;
  cersei-types            Provider-agnostic messages, errors, stream events
  cersei-provider         Provider trait + configuration-driven registry, 3 protocol adapters
  cersei-tools            30+ tools, permissions, bash classifier, skills, git utils
  cersei-tools-derive     #[derive(Tool)] proc macro
  cersei-agent            Agent builder, agentic loop, compact, coordinator, effort
  cersei-memory           Memory trait, memdir, CLAUDE.md, sessions, Grafeo graph
  cersei-hooks            Hook/middleware system
  cersei-mcp              MCP client (JSON-RPC 2.0, stdio transport)
```

---

## Core Concepts

### Provider

Providers and models are **configuration**, not code. A file (`~/.bricks/providers.toml`, or any
`.toml`/`.json` path you pass) lists providers, their models, limits, capabilities, reasoning profiles and
prices; a model is selected explicitly as `provider_id/model_id`. Three wire protocols are built in —
`chat_completions`, `responses` and `anthropic_messages` — so any compatible server (Ollama, vLLM, a
gateway, a hosted API) is added by editing the file, with no recompilation.

```rust
let provider = cersei::provider_from_config(None, "my-provider/my-model")?;   // ~/.bricks/providers.toml
let provider = cersei::provider_from_config(Some("./providers.json".as_ref()), "local/coder")?;
Agent::builder().provider(provider).reasoning_profile("deep")                  // a profile *you* defined
Agent::builder().provider(MyCustomProvider)                                    // impl Provider
```

See [`docs/providers.md`](docs/providers.md) (schema, capability matrix, migration) and the annotated
[`docs/providers.example.toml`](docs/providers.example.toml).

### Tools (30+)

Every tool a coding agent needs, organized into sets:

```rust
cersei::tools::all()           // 30+ tools
cersei::tools::coding()        // filesystem + shell + web
cersei::tools::filesystem()    // Read, Write, Edit, Glob, Grep, NotebookEdit
cersei::tools::shell()         // Bash, PowerShell
cersei::tools::web()           // WebFetch, WebSearch
cersei::tools::planning()      // EnterPlanMode, ExitPlanMode, TodoWrite
cersei::tools::scheduling()    // CronCreate/List/Delete, Sleep, RemoteTrigger
cersei::tools::orchestration() // SendMessage, Tasks (6 tools), Worktree
```

Custom tools in 10 lines:

> The `#[derive(Tool)]` macro generates code with `#[async_trait::async_trait]` and `cercei-tools`, to make it work add both of it to depending on your project.
>  ```toml
> async-trait = "0.1"
> cersei = { path = "path/to/cersei" } # or git
> cersei-tools = { path = "path/to/cersei/crates/cersei-tools" }
> ```
> or write `use cersei::tools as cersei_tools;` when using `derive(Tool)`;
    

```rust
#[derive(Tool)]
#[tool(name = "search", description = "Search docs", permission = "read_only")]
struct SearchTool;

#[async_trait]
impl ToolExecute for SearchTool {
    type Input = SearchInput; // derives Deserialize + JsonSchema
    async fn run(&self, input: SearchInput, ctx: &ToolContext) -> ToolResult {
        ToolResult::success(format!("Found: {}", input.query))
    }
}
```

### Sub-Agent Orchestration

Spawn parallel workers, coordinate tasks, pass messages between agents:

```rust
// AgentTool — model spawns sub-agents autonomously
Agent::builder()
    .tool(AgentTool::new(|| provider_factory(), cersei::tools::coding()))   // a closure returning your configured provider

// Coordinator mode — orchestrate parallel workers
Agent::builder()
    .tools(cersei::tools::all())  // includes Agent, Tasks, SendMessage
    // Workers get filtered tools (no Agent — prevents recursion)

// Task system
// TaskCreate → TaskUpdate → TaskGet → TaskList → TaskStop → TaskOutput
```

### Memory (Three-Tier)

```rust
use cersei::memory::manager::MemoryManager;

let mm = MemoryManager::new(project_root)
    .with_graph(Path::new("./memory.grafeo"))?;  // optional graph layer

// Tier 1: Flat files (~/.claude/projects/<root>/memory/)
let metas = mm.scan();                    // scan .md files with frontmatter
let content = mm.build_context();         // build system prompt injection

// Tier 2: CLAUDE.md hierarchy (managed > user > project > local)
// Automatically merged into build_context()

// Tier 3: Graph memory (Grafeo, optional)
let id = mm.store_memory("User prefers Rust", MemoryType::User, 0.9)?;
mm.tag_memory(&id, "preferences");
let results = mm.recall("Rust", 5);       // graph query with fallback to text match

// Session persistence (JSONL, append-only, tombstone soft-delete)
mm.write_user_message("session-1", Message::user("Hello"))?;
let messages = mm.load_session_messages("session-1")?;
```

### Skills (Claude Code + OpenCode Compatible)

```rust
// Auto-discovers skills from:
//   .claude/commands/*.md      (Claude Code format)
//   .claude/skills/*/SKILL.md  (OpenCode format)
//   ~/.claude/commands/*.md    (user-level)
//   Bundled skills             (simplify, debug, commit, verify, stuck, remember, loop)

let skill_tool = SkillTool::new().with_project_root(".");
// skill="list" → lists all available skills
// skill="debug" args="tests are flaky" → expands $ARGUMENTS template
```

### Realtime Events

Three observation mechanisms:

```rust
// 1. Callback
Agent::builder().on_event(|e| match e {
    AgentEvent::TextDelta(t) => print!("{}", t),
    AgentEvent::ToolStart { name, .. } => eprintln!("[{}]", name),
    _ => {}
})

// 2. Broadcast (multi-consumer)
let agent = Agent::builder().enable_broadcast(256).build()?;
let mut rx = agent.subscribe().unwrap();
tokio::spawn(async move { while let Ok(e) = rx.recv().await { /* ... */ } });

// 3. Stream (bidirectional control)
let mut stream = agent.run_stream("Deploy");
while let Some(e) = stream.next().await {
    if let AgentEvent::PermissionRequired(req) = e {
        stream.respond_permission(req.id, PermissionDecision::Allow);
    }
}
```

### Context Management

```rust
Agent::builder()
    .bricks_config(BricksConfig::load(&working_dir)) // bricks.toml + ~/.bricks/rules
    .auto_compact(true)                   // compact near the limit (see docs/context.md)
    .compact_threshold(0.85)              // fraction of the prompt budget
    .compression_level(CompressionLevel::Minimal)
    .tool_result_budget(50_000)           // old tool results removed above 50K chars
    .reasoning_profile("deep")            // a profile defined in providers.toml

agent.context_status();   // context used (measured/estimated), window, session totals
agent.raw_history();      // every message, tool results unreduced
```

Occupation comes from the server's reported usage, with explicit estimates in
between; a request that cannot fit is compacted first or not sent. Tool outputs
are reduced with diagnostics kept first, and every reduced output names its
saved original. See [docs/context.md](docs/context.md),
[docs/shell.md](docs/shell.md) (shell, file tools, result format),
[docs/web.md](docs/web.md) (web search and reading), [docs/mcp.md](docs/mcp.md) (MCP client),
[docs/memory.md](docs/memory.md) (long-term memory and recall), [docs/cli.md](docs/cli.md) (the `bricks` command, events and terminal interface) and
[docs/compression.md](docs/compression.md).

### MCP (Model Context Protocol)

```rust
let mcp = McpManager::connect(&[
    McpServerConfig::stdio("db", "npx", &["-y", "@my/db-mcp"]),
    McpServerConfig::sse("docs", "https://mcp.example.com"),
    // Remote Streamable HTTP MCP via a stdio bridge; no account or API key required.
    McpServerConfig::stdio(
        "parallel-search",
        "npx",
        &["-y", "mcp-remote", "https://search.parallel.ai/mcp"],
    ),
]).await?;

Agent::builder().tools(mcp.tool_definitions().await)
```

---

## Agent Builder — Complete API

```rust
Agent::builder()
    // Provider (required): a configured model (see docs/providers.md)
    .provider(provider_from_config(None, "my-provider/my-model")?)

    // Tools
    .tool(MyTool)
    .tools(cersei::tools::coding())

    // Model & generation
    .model("claude-sonnet-4-6")
    .max_turns(10)
    .max_tokens(16384)
    .temperature(0.7)
    .thinking_budget(8192)

    // Prompt
    .system_prompt("You are a helpful assistant.")
    .append_system_prompt("Extra context.")

    // Environment
    .working_dir("./my-project")
    .permission_policy(AllowAll)          // or AllowReadOnly, DenyAll, RuleBased, Interactive

    // Memory
    .memory(JsonlMemory::new("./sessions"))
    .session_id("my-session")

    // Hooks & events
    .hook(CostGuard { max_usd: 5.0 })
    .on_event(|e| { /* ... */ })
    .enable_broadcast(256)
    .reporter(ConsoleReporter { verbose: true })

    // Context management
    .auto_compact(true)
    .compact_threshold(0.9)
    .tool_result_budget(50_000)

    // Execute
    .build()?                             // -> Agent
    .run_with("Fix the tests")            // -> AgentOutput (shorthand)
```

---

## Benchmarks

Measured on Apple Silicon, release build, 100 iterations with 3 warmup runs.

### Tool I/O

| Tool | Avg | Min | Max |
|------|-----|-----|-----|
| Edit | 0.04ms | 0.02ms | 0.05ms |
| Glob | 0.05ms | 0.05ms | 0.07ms |
| Write | 0.09ms | 0.07ms | 0.11ms |
| Read | 0.09ms | 0.08ms | 0.11ms |
| Grep | 5.85ms | 5.34ms | 8.51ms |
| Bash | 15.64ms | 14.50ms | 16.19ms |

### vs Claude Code CLI

> **Note:** Cersei is a library — tool dispatch happens in-process. Claude Code is a CLI where
> each sub-agent fork pays full startup cost. These are different layers; the comparison below
> shows the gap between in-process dispatch and CLI process overhead.

| Metric | Cersei (SDK) | Claude Code (CLI) | Notes |
|--------|-------------|-------------------|-------|
| Tool dispatch (Read) | 0.09ms | ~5-15ms (est.) | In-process vs Node.js fs |
| CLI startup | N/A (library) | 269ms | Claude `--version` warm avg |
| Sub-agent spawn | ~1ms (in-process) | ~300ms (fork) | Agent tool overhead |

### Memory I/O

| Operation | Abstract (Cersei) | Claude Code (measured) | Ratio |
|-----------|------------------|----------------------|-------|
| Scan 100 files | **1.2ms** | 26.6ms (`find`) | 22x |
| Load MEMORY.md | **9.6μs** | 17.1ms | 1,781x |
| Memory recall (graph) | **98μs** | 7,545ms (LLM call) | 77,000x |
| Memory recall (text) | **1.3ms** | 17.5ms (`grep`) | 13x |
| Session write | **27μs/entry** | N/A | — |
| Session load (100) | **268μs** | N/A | — |
| Graph store | **30μs/node** | N/A (no graph) | — |
| Topic query | **77μs** | N/A (no graph) | — |

### Benchmark suites

Each bench lives in its own self-contained directory with its own runner and result schema. Add new benches as siblings.

| Suite | Path | What it measures | Runner |
|---|---|---|---|
| **General-agent frameworks** | [`bench/general-agents/`](bench/general-agents/) | Per-agent memory, instantiation time, max concurrent agents — Cersei vs Agno / PydanticAI / LangGraph / CrewAI. | `./bench/general-agents/run.sh` |
| **Terminal Bench 2.0** | [`bench/term-bench/`](bench/term-bench/) | End-to-end coding tasks inside Daytona sandboxes using the full `abstract` CLI (Linux x86_64 / arm64 binaries shipped in-tree). | `./bench/term-bench/run.sh` |
| **LongMemEval (long-term memory)** | [`bench/long-mem/`](bench/long-mem/) | Recall accuracy on the ICLR-25 LongMemEval 500-question benchmark — head-to-head vs Mastra / Zep / Supermemory with identical prompts and LLM-as-judge rubric. Four Cersei configs: full-context baseline, usearch-HNSW semantic, grafeo-graph substring, hybrid w/ LLM fact extraction + RRF fusion. | `cargo run --release -p longmem-bench -- --dataset s --config all` |
| **Compression (real LLMs)** | `crates/cersei-agent/tests/e2e_live_compression.rs` | Input-token savings from `cersei-compression` on a model of your providers configuration. `#[ignore]`, paid call; set `BRICKS_LIVE_MODEL=provider_id/model_id`. | `BRICKS_LIVE_MODEL=… cargo test -p cersei-agent --test e2e_live_compression -- --ignored --nocapture` |
| **SDK Tool I/O** | `examples/benchmark_io.rs` | In-process tool dispatch latency for Read / Write / Edit / Grep / Bash / Glob. | `cargo run --example benchmark_io --release` |
| **vs Claude Code CLI** | `run_tool_bench_claude.sh` · `run_tool_bench_codex.sh` | CLI-vs-CLI startup, memory, and dispatch overhead. | `./run_tool_bench.sh --iterations 20 --full` |

### Run benchmarks

```bash
# Rust-side SDK benches (no external services)
cargo run --example benchmark_io --release

# vs Claude Code / Codex CLIs
./run_tool_bench.sh --iterations 20 --full

# Python-harness benches (uv-managed; each dir self-contained)
./bench/general-agents/run.sh          # Cersei vs Agno / PydanticAI / LangGraph / CrewAI
./bench/term-bench/run.sh              # Terminal Bench 2.0 via Daytona

# LongMemEval memory benchmark (head-to-head vs Mastra / Zep / Supermemory)
./bench/long-mem/setup.sh              # downloads oracle + s datasets
OPENAI_API_KEY=sk-… cargo run --release -p longmem-bench -- \
  --answerer-model provider_id/model_id --judge-model provider_id/model_id \
  --embeddings openai --dataset s --config all --concurrency 8   # models from ~/.bricks/providers.toml

# Real-LLM compression savings (a paid call; key = the api_key_env of your configuration)
BRICKS_LIVE_MODEL=provider_id/model_id cargo test -p cersei-agent \
  --test e2e_live_compression -- --ignored --nocapture
```

---

## Stress Tests

```bash
cargo run --example stress_core_infrastructure --release  # system prompt, compact, context, bash classifier
cargo run --example stress_tools --release                 # all 30+ tools, registry, performance
cargo run --example stress_orchestration --release         # sub-agents, coordinator, tasks, messaging
cargo run --example stress_skills --release                # bundled + disk skills, Claude Code + OpenCode format
cargo run --example stress_memory --release                # memdir, CLAUDE.md, sessions, extraction, auto-dream
```

---

## Examples

| Example | Description |
|---------|-------------|
| [`simple_agent`](examples/simple_agent.rs) | Minimal agent in 3 lines |
| [`custom_tools`](examples/custom_tools.rs) | Define and register custom tools |
| [`streaming_events`](examples/streaming_events.rs) | Real-time `run_stream()` with colored output |
| [`multi_listener`](examples/multi_listener.rs) | Broadcast channel with multiple consumers |
| [`resumable_session`](examples/resumable_session.rs) | Persist and resume with `JsonlMemory` |
| [`custom_provider`](examples/custom_provider.rs) | Echo provider + configuring a compatible endpoint |
| [`hooks_middleware`](examples/hooks_middleware.rs) | Cost guard + audit logger + tool blocker |
| [`benchmark_io`](examples/benchmark_io.rs) | Full I/O benchmark suite |
| [`usage_report`](examples/usage_report.rs) | Token/cost tracking and billing estimates |
| [`coding_agent`](examples/coding_agent.rs) | Build a Python todo CLI (end-to-end) |

```bash
cargo run --example simple_agent --release
```

---

## Test Suite

```bash
# Run all 160 unit tests
cargo test --workspace

# Run with graph memory (requires grafeo)
cargo test --workspace --features graph

# Run specific crate
cargo test -p cersei-tools
cargo test -p cersei-agent
cargo test -p cersei-memory
cargo test -p cersei-mcp
```

**160 unit tests** | **262 stress checks** | **0 failures** | **Zero I/O regression**

---

## Extension Points

| What | How | Example |
|------|-----|---------|
| Custom provider | `impl Provider` | Local LLM, Azure, Bedrock |
| Custom tool | `#[derive(Tool)]` or `impl Tool` | DB query, deploy, search |
| Custom permissions | `impl PermissionPolicy` | RBAC, OAuth-scoped |
| Custom memory | `impl Memory` | PostgreSQL, Redis, S3 |
| Custom hooks | `impl Hook` | Cost gating, audit logging |
| Custom reporters | `impl Reporter` | Dashboards, WebSocket relay |
| MCP servers | `McpServerConfig` via builder | Any MCP-compatible server |
| Skills | `.claude/commands/*.md` | Custom prompt templates |
| Graph memory | `features = ["graph"]` | Grafeo relationship tracking |

---

## Documentation

**[cersei.pacifio.dev/docs](https://cersei.pacifio.dev/docs)** — full docs with API reference, architecture, cookbooks, benchmarks, and llms.txt support.

| Section | Content |
|---------|---------|
| [Quick Start](https://cersei.pacifio.dev/docs/quick-start) | First agent in 10 lines |
| [API Reference](https://cersei.pacifio.dev/docs/api-agent) | Agent, Provider, Tools, Memory, Hooks, MCP |
| [Architecture](https://cersei.pacifio.dev/docs/architecture) | Crate map, data flow, design principles |
| [Cookbooks](https://cersei.pacifio.dev/docs/cookbook-custom-tools) | Custom tools, deployment, embedding |
| [Abstract CLI](https://cersei.pacifio.dev/docs/abstract) | Reference CLI built on Cersei |
| [Benchmarks](https://cersei.pacifio.dev/docs/bench-vs-claude-code) | vs Claude Code vs Codex |

---

## License

MIT License

Copyright (c) 2025-2026 Adib Mohsin

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:


The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
