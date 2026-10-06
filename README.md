# Bricks

A coding agent written in Rust: an embeddable engine (the `cersei-*` crates)
and the `bricks` command, with a headless mode for scripts and CI and an
interactive terminal interface.

Bricks is a fork of [Cersei](https://github.com/pacifio/cersei) by Adib
Mohsin (Pacifio), released under the MIT license. The crates keep their
`cersei-*` names.

```bash
cargo install --path crates/bricks-cli
bricks                                                     # interactive terminal interface
bricks run "Explain the failing test in src/parser.rs"     # one prompt, answer on stdout
bricks run --json --non-interactive "Analyse the build errors"   # scripts and CI: JSONL events
bricks resume                                              # pick a stored session
```

---

## What it does

* **Models are configuration.** `~/.bricks/providers.toml` lists
  providers, models, limits, capabilities, reasoning profiles and prices.
  * A model is selected explicitly as `provider_id/model_id`; nothing is
    guessed from a name and no catalogue is built in.
  * Three wire protocols are supported: `chat_completions`, `responses` and
    `anthropic_messages`. Any compatible server (a hosted API, a gateway,
    Ollama, vLLM, llama.cpp) is added by editing the file. See
    [docs/providers.md](docs/providers.md).
* **Context management.** Occupation comes from the server's reported usage,
  with explicit estimates in between.
  * A request that cannot fit is compacted first, or not sent.
  * Compaction keeps the user's messages verbatim and verifies its gain.
  * The raw history is never shortened. See [docs/context.md](docs/context.md).
* **Tool-output compression.** Diagnostics are kept first, and rules exist
  for cargo, go, pytest, npm/pnpm/yarn, eslint, docker, kubectl, terraform,
  and more.
  * Tree-sitter skeletons summarise long files.
  * Every reduced output names its saved original. See
    [docs/compression.md](docs/compression.md).
* **Shell and files.**
  * The shell is persistent: directory, variables, aliases and functions
    survive between commands.
  * Execution is supervised: timeouts, process-tree termination,
    cancellation and background tasks.
  * Edits are tolerant but safe: they are re-validated before an atomic
    write, and a patch tool rolls back on failure. See
    [docs/shell.md](docs/shell.md).
* **Web.** Web search with ordered provider fallback, page reading in
  Markdown, citable passages, and a private-network policy. See
  [docs/web.md](docs/web.md).
* **MCP.** A client on the official SDK (`rmcp`), over stdio and Streamable
  HTTP. See [docs/mcp.md](docs/mcp.md).
* **Long-term memory.** Sourced episodes, entities scoped by
  user/project/space, and temporal facts with evidence.
  * Hybrid recall combines vector, BM25 and a bounded graph expansion,
    fused with weighted RRF.
  * Maintenance (extraction, embeddings) runs after the answer: it is
    resumable and cancellable. See [docs/memory.md](docs/memory.md).
* **Approvals.** `[permissions]` in `bricks.toml` sets the policy; the
  engine applies it to every frontend.
  * File changes are shown as a diff before anything is written.
  * A change approved for a file that changed meanwhile is recomputed and
    asked again.
* **The `bricks` command.** A thin layer over the engine's command/event
  contract. JSONL events are versioned, with documented exit codes.
  * The interface is inline, so the transcript stays in your terminal
    scrollback.
  * It has `@` file mentions, `/` commands, approvals with diff,
    inspectors and session resume. See [docs/cli.md](docs/cli.md).

## Configuration

```toml
# ~/.bricks/providers.toml — see docs/providers.example.toml
schema_version = 1

[[providers]]
id = "local"
name = "Local server"
endpoint = "http://127.0.0.1:11434/v1"
protocol = "chat_completions"
auth = "none"

[[providers.models]]
id = "coder"
name = "My coding model"
api_model = "qwen2.5-coder:32b"
streaming = true
tool_calls = true

[providers.models.limits]
max_input_tokens = 32000
max_output_tokens = 4096
```

```toml
# ./bricks.toml — see docs/bricks.example.toml
[agent]
model = "local/coder"

[permissions]
write = "ask"      # file changes: shown as a diff, then asked
execute = "ask"    # shell commands
```

## The engine as a library

```rust
use cersei::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let output = Agent::builder()
        .provider(provider_from_config(None, "local/coder")?) // ~/.bricks/providers.toml
        .tools(cersei::tools::coding())                        // files, shell, web
        .permission_policy(AllowReadOnly)
        .run_with("Explain what src/main.rs does")
        .await?;
    println!("{}", output.text());
    Ok(())
}
```

Frontends use the command/event contract (`cersei_agent::control`). It
offers typed commands (submit, cancel, set_model, compact, resume,
approve…) and versioned events with one `run_finished` per run. Delivery is
bounded and never silently drops an event. See [docs/cli.md](docs/cli.md).

## Architecture

```text
bricks-cli            the `bricks` binary: arguments, configuration, headless / JSONL
  bricks-tui          terminal interface (ratatui): renders events, turns gestures into commands
cersei                facade crate — use cersei::prelude::*
  cersei-agent        agent builder, agentic loop, context manager, compaction,
                      command/event contract (control), approvals
  cersei-provider     provider registry from configuration, 3 protocol adapters
  cersei-tools        file, shell, web, planning, orchestration tools; permissions; previews
  cersei-compression  tool-output reduction, rules, tree-sitter skeletons
  cersei-memory       sessions, memdir, graph memory, structured long-term memory
  cersei-embeddings   embedding providers, USearch vector index
  cersei-web          search, fetching, extraction, passages
  cersei-mcp          MCP client (rmcp)
  cersei-types, cersei-hooks, cersei-skills, cersei-lsp, cersei-workflows, …
```

---

## Benchmarks

**Measured again on 2026-10-06.** The machine is an Apple A18 Pro (6
cores, 8 GB) running macOS 27.2. The build uses the workspace's release
profile (`opt-level = "z"`, thin LTO). No model is called by any benchmark
below.

Pacifio's figures were published with Cersei 0.1.6 on Apple Silicon, on a
different machine. The comparison shows orders of magnitude and
regressions, not a controlled A/B test.

### Tool I/O (`cargo run --release -p cersei --example benchmark_io`)

50 iterations per tool, in-process dispatch, average and range.

| Tool | Now | min – max | Cersei 0.1.6 (Pacifio) |
|---|---|---|---|
| Edit | 0.05 ms | 0.05 – 0.13 ms | 0.04 ms |
| Glob | 0.07 ms | 0.06 – 0.08 ms | 0.05 ms |
| Write | 0.14 ms | 0.04 – 1.68 ms | 0.09 ms |
| Read | 0.25 ms | 0.19 – 0.43 ms | 0.09 ms |
| Grep | 2.42 ms | 1.43 – 2.84 ms | 5.85 ms |
| Bash | 37.6 ms | 35.2 – 41.5 ms | 15.64 ms |

Why some tools got slower:

* **Read** now detects binary and non-UTF-8 files, handles BOMs, counts the
  exact total and checks that the file did not change during the read.
* **Bash** now runs in a persistent shell that keeps state, supervises the
  whole process tree and spills large outputs to disk. Each command
  therefore pays a request/response round-trip and a short output-settling
  window (15 ms).
* **Bash fix.** While re-running this benchmark, a 1 s penalty per command
  on macOS was found and fixed: the kernel never reports the end of the
  output FIFOs, and each command waited two 500 ms windows (1 051 ms
  measured before the fix).

### Memory I/O (`cargo run --release -p cersei-memory --features graph --example memory_bench`)

| Operation | Now (mean) | Cersei 0.1.6 (Pacifio) |
|---|---|---|
| Scan 100 memory files (frontmatter) | 1.71 ms | 1.2 ms |
| Load MEMORY.md | 16.4 µs | 9.6 µs |
| Memory recall, text (100 files) | 1.88 ms | 1.3 ms |
| Memory recall, graph (1 000 nodes) | 1.34 ms | 98 µs (graph size not stated) |
| Graph store | 114 µs per node | 30 µs per node |
| Topic query (graph) | 118 µs | 77 µs |
| Session write | 37 µs per entry | 27 µs per entry |
| Session load (100 entries) | 279 µs | 268 µs |

Notes on the graph rows:

* **Recall.** Graph recall matches the query as a substring of every
  stored memory, so it grows with the graph size. Pacifio's figure gives
  no size, so the two numbers are not comparable.
* **Writes.** Graph writes are slower because every query and write is now
  parameterised: nothing written by a user or a model is spliced into a
  query anymore.

The structured long-term memory (hybrid recall, Sprint 6) has its own
measurements in [docs/memory.md](docs/memory.md#measurements): recall
p50 2.0 / 3.5 / 6.8 ms at 500 / 2 000 / 10 000 episodes.

### Agent framework overhead (`bench/general-agents`, Cersei side)

The workload is build → one turn → shut down, with a stub model and one
echo tool (`cargo run --release -p cersei-agent --example general_agent_bench --features bench-full`).

| Axis | Now | Cersei 0.1.6 (Pacifio) |
|---|---|---|
| Instantiation (mean) | 33.9 µs | 8.5 µs |
| Memory per agent (jemalloc) | 71.8 KB | 704 B |
| 10 000 concurrent agents: turn p50 / p99 | 0.08 / 0.81 ms | 0.056 / 0.155 ms |
| 10 000 concurrent agents: RSS | 695 MB | 22 MB |
| Graph recall under load, 10 000 nodes (p50) | 90.2 ms | 94.0 ms |
| Semantic search under load, 10 000 chunks (p50) | 65.6 µs | 50.7 µs |

Each agent now carries a context manager, a tool-output compressor with its
store, a web context and an approval-ready permission path, which explains
the heavier instantiation and footprint. This re-run also found and fixed a
much larger regression from that work: the built-in compression rules were
re-parsed from TOML for every agent. Instantiation had reached 28.9 ms and
2.3 MB per agent; the rules are now parsed once per process and shared.

The Python frameworks (Agno, LangGraph, PydanticAI, CrewAI) were **not
re-run**: their published results in `bench/general-agents/results/` date
from Pacifio's run. Re-running them downloads those frameworks from PyPI:
`./bench/general-agents/run.sh`.

### CLI startup (`python3 scripts/bench_cli.py --bricks <path> --iterations 50`)

The command is `--version`, with no model call: 50 runs after warm-up.

| CLI | Startup (mean) | Executed file | Peak RSS |
|---|---|---|---|
| bricks 0.2.6 | 5.4 ms | 27.1 MB | 7.4 MB |
| Claude Code 2.1.289 | 7.4 ms | 219.0 MB | 24.6 MB |
| Codex CLI 0.150.1 | 9.0 ms | 218.4 MB | 16.8 MB |

Pacifio's earlier comparison (269 ms for Claude Code) measured the former
Node.js Claude Code; the current one is a native binary. The "Abstract CLI"
it compared against no longer exists; `bricks` replaces it.

### Not re-run (paid or external services)

| Suite | Why | Command |
|---|---|---|
| LongMemEval | Needs the dataset (download), an answerer, a judge and embeddings: paid calls | `bench/long-mem` — evidence recall is free with local embeddings: `cargo run --release -p longmem-bench --bin longmem-recall -- --dataset oracle` |
| Terminal-Bench 2.0 | Daytona sandboxes and a model: paid | `./bench/term-bench/run.sh` |
| Compression savings with a real model | One paid call | `BRICKS_LIVE_MODEL=provider_id/model_id cargo test -p cersei-agent --test e2e_live_compression -- --ignored --nocapture` |
| Memory recall vs Claude Code / Codex (LLM-based recall) | Runs those agents with their models | — |

No figure from these suites is claimed here.

### Stress checks

All five stress suites pass (`cargo run --release -p cersei --example stress_<name>`):

| Suite | Checks |
|---|---|
| core infrastructure | 46 / 46 |
| tools | 47 / 47 |
| orchestration | 33 / 33 |
| skills | 47 / 47 |
| memory | 85 / 85 |

Before this run, 8 checks failed. Most had expectations outdated by later
changes: tool counts, the compaction prompt's wording, the message of a
removed tool result, and a stub model too small for the current tool
definitions. One check (`orchestration() = 3 tools`) already failed in
Cersei 0.1.6, which had 9 orchestration tools.

## Tests

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The workspace has 930+ tests. The model-facing paths are tested against
scripted models and local test servers; no test makes a paid call (the
live ones are `#[ignore]`). Tests ran on macOS (arm64); Linux and Windows
were not run.

## Documentation

| | |
|---|---|
| [docs/cli.md](docs/cli.md) | the `bricks` command, JSONL schema, keys, approvals, sessions |
| [docs/providers.md](docs/providers.md) | providers configuration |
| [docs/context.md](docs/context.md) | context management and compaction |
| [docs/compression.md](docs/compression.md) | tool-output compression and rules |
| [docs/shell.md](docs/shell.md) | shell, file tools, result format |
| [docs/web.md](docs/web.md) | web search and reading |
| [docs/mcp.md](docs/mcp.md) | MCP client |
| [docs/memory.md](docs/memory.md) | long-term memory and hybrid recall |
| [docs/bricks.example.toml](docs/bricks.example.toml) | every `bricks.toml` section, annotated |
| [CHANGELOG.md](CHANGELOG.md) | changes since the fork |

## License

MIT. Copyright (c) 2025-2026 Adib Mohsin (Cersei), and the Bricks
contributors.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
