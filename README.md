<div align="center">

# Bricks

**An engine for agentic development. Built in Rust. At home in your terminal.**

![Version](https://img.shields.io/badge/version-0.4.6-blue)
![Rust](https://img.shields.io/badge/built_with-Rust-orange)

[Quick start](#quick-start) · [What it does](#what-it-does) · [Configuration](#configuration) · [Documentation](#documentation) · [License](#license)

</div>

Bricks is a new tool for coding with AI. Built on the technology behind
[Cersei](https://github.com/pacifio/cersei), it brings new capabilities and
a redesigned, more robust architecture to the terminal, with one goal:
give AI agents the means to do their best work.

It combines an interactive terminal interface, a headless command for scripts
and CI, and an embeddable Rust engine for building your own applications.

### Two ideas behind Bricks

Bricks brings together two concepts developed by AinsiParlaitZarathoustra:

- **ADK — Agentic Development Kit.** A broad set of purpose-built, optimised
  tools that help AI agents inspect code, make changes, run commands and
  work effectively across a project.
- **ADE — Agentic Development Engine.** An engine designed to enhance the
  capabilities of any agent by providing the tools, context and execution
  infrastructure it needs to perform well.

**Engine** is the defining word. Where a *harness* suggests restraint,
reduced agility and dependence on whoever holds it, an engine supplies the
means to act. Bricks is built around that ambition: empower the agent and
make its work more effective.

### Project status

**Bricks 0.4.6** has a substantial, stable foundation. Development continues,
with further features and code optimisations still ahead.

Bricks is a fork of Cersei, created by **Adib Mohsin (Pacifio)**. The inherited
Rust crates retain their `cersei-*` names; Bricks extends that foundation
with its own architecture and capabilities.

## Quick start

From a checkout of this repository:

```bash
cargo install --path crates/bricks-cli
```

Configure a provider and model using the [configuration example](#configuration),
then choose how to work:

```bash
bricks                                                        # interactive terminal interface
bricks run "Explain the failing test in src/parser.rs"          # one prompt, answer on stdout
bricks run --json --non-interactive "Analyse the build errors"  # scripts and CI: JSONL events
bricks resume                                                 # pick a stored session
```

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
* **Code understanding.** `CodeScout` (and `/search`) answer "where is it
  defined, who uses it, what is here" in one call with exact, sourced and
  bounded results: text search, Tree-sitter, and optional language servers
  (never installed by Bricks). See [docs/semantic.md](docs/semantic.md).
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
  bricks-semantic     shared code understanding engine: lexical, syntax, LSP
  cersei-web          search, fetching, extraction, passages
  cersei-mcp          MCP client (rmcp)
  cersei-types, cersei-hooks, cersei-skills, cersei-lsp, cersei-workflows, …
```

---

## Benchmarks

**Bricks 0.4.6, measured on 2026-10-08** (commit `dee981e`), compared with
the previous run of 2026-10-06 (commit `7315d82`, the code that became
0.3.6 — the sub-agent and code-engine work landed after it) and with
Pacifio's published figures for Cersei 0.1.6.

The machine is an Apple A18 Pro (6 cores, 8 GiB) running macOS 27.2,
Rust 1.98.1, workspace release profile (`opt-level = "z"`, thin LTO). No
model is called by any benchmark below. Pacifio's figures come from a
different Apple Silicon machine: that column shows orders of magnitude and
regressions, not a controlled A/B test. Full report, raw notes and harness
fixes: [docs/benchmarks-bricks-2026-10-08.md](docs/benchmarks-bricks-2026-10-08.md).

### Tool I/O

```bash
cargo run --release -p cersei --example benchmark_io
```

50 iterations per tool after warm-up, in-process dispatch, mean. Every call
is now checked for success.

| Tool | 0.4.6 | 0.3.6 cycle | Cersei 0.1.6 (Pacifio) |
|---|---|---|---|
| Read | 0.11 ms | 0.25 ms | 0.09 ms |
| Write | 0.05 ms | 0.14 ms | 0.09 ms |
| Edit | 0.81 ms | *(0.05 ms)* | *(0.04 ms)* |
| Glob | 0.05 ms | 0.07 ms | 0.05 ms |
| Grep | 1.86 ms | 2.42 ms | 5.85 ms |
| Bash | 39.8 ms | 37.6 ms | 15.64 ms |

* **Edit.** The earlier harness timed an ambiguous call (the text appears
  several times) whose error was ignored: it measured a refusal. The
  harness now uses `replace_all` and checks the result, so 0.81 ms is the
  first real figure for a repeated edit; the figures in brackets are not
  comparable.
* **Read** detects binary and non-UTF-8 files, handles BOMs, counts the
  exact total and checks that the file did not change during the read.
* **Bash** runs in a persistent shell that keeps state, supervises the
  whole process tree and spills large outputs to disk: each command pays a
  request/response round-trip and a short output-settling window (15 ms).

The standalone suite (`examples/benchmark`, 100 iterations) gives the same
picture: Read 0.118 ms, Write 0.050 ms, Edit 0.759 ms, Glob 0.066 ms,
Grep 2.017 ms, Bash 39.7 ms.

### Memory I/O

```bash
cargo run --release -p cersei-memory --features graph --example memory_bench
```

| Operation (mean) | 0.4.6 | 0.3.6 cycle | Cersei 0.1.6 (Pacifio) |
|---|---|---|---|
| Scan 100 memory files (frontmatter) | 1.84 ms | 1.71 ms | 1.2 ms |
| Load MEMORY.md | 15.4 µs | 16.4 µs | 9.6 µs |
| Memory recall, text (100 files) | 2.03 ms | 1.88 ms | 1.3 ms |
| Memory recall, graph (1 000 nodes) | 1.36 ms | 1.34 ms | 98 µs (graph size not stated) |
| Graph store | 76.2 µs per node | 114 µs per node | 30 µs per node |
| Topic query (graph) | 114.5 µs | 118 µs | 77 µs |
| Session write | 35.9 µs per entry | 37 µs per entry | 27 µs per entry |
| Session load (100 entries) | 279 µs | 279 µs | 268 µs |

* **Recall.** Graph recall matches the query as a substring of every stored
  memory, so it grows with the graph size; Pacifio's figure gives no size.
* **Writes.** Every graph query and write is parameterised: nothing written
  by a user or a model is spliced into a query.

The structured long-term memory (hybrid recall) has its own measurements in
[docs/memory.md](docs/memory.md#measurements): recall p50 2.0 / 3.5 / 6.8 ms
at 500 / 2 000 / 10 000 episodes.

### Agent framework overhead (`bench/general-agents`)

```bash
CERSEI_BENCH_AXES=1,2,3,4 cargo run --release -p cersei-agent --example general_agent_bench --features bench-full
```

A minimal agent with one tool is built, held and dropped; no model turn is
run (parity with the Python harnesses).

| Axis | 0.4.6 | 0.3.6 cycle | Cersei 0.1.6 (Pacifio) |
|---|---|---|---|
| Instantiation (mean) | 21.3 µs | 33.9 µs | 8.5 µs |
| Memory per agent (jemalloc) | 71.7 KB | 71.8 KB | 704 B |
| 10 000 agents built concurrently: per-agent p50 / p99 | 0.066 / 1.05 ms | 0.08 / 0.81 ms | 0.056 / 0.155 ms |
| 10 000 agents built concurrently: total | 197 ms | — | — |
| 10 000 agents held: peak RSS | 647 MiB | 695 MiB | 22 MB |
| Graph recall under load, 10 000 nodes (p50) | 83.0 ms | 90.2 ms | 94.0 ms |
| Semantic search under load, 10 000 chunks (p50) | not run | 65.6 µs | 50.7 µs |

Each agent carries a context manager, a tool-output compressor with its
store, a web context and an approval-ready permission path; these explain
the higher footprint than Cersei 0.1.6. 10 000 is the largest step tested,
not a limit found. Graph recall under load is 10 workers × 100 queries
(p95 130 ms, p99 226 ms).

**Python frameworks, re-run on the same machine** (Python 3.12.13, versions
from `uv.lock`; Rust memory measured with jemalloc, Python with
tracemalloc, which misses native allocations — the allocation column is not
an equal comparison of total RAM):

| Framework | Construction (mean) | Allocation per agent | 1 000 agents: total | 1 000 agents: peak RSS |
|---|---|---|---|---|
| **Bricks 0.4.6** | 21.3 µs | 71 703 B | 20.4 ms | 98.5 MiB |
| Agno 2.5.17 | 6.0 µs | 5 394 B | 19.0 ms | 103.6 MiB |
| PydanticAI 1.22.0 | 228.6 µs | 8 196 B | 262.1 ms | 107.2 MiB |
| LangGraph 1.1.8 | 1 950.7 µs | 30 344 B | 2 101.1 ms | 168.7 MiB |
| CrewAI 1.14.2 | 7 476.2 µs | 17 900 B | 3 024.0 ms | 1 526.2 MiB |

### CLI startup

```bash
python3 scripts/bench_cli.py --bricks target/release/bricks --iterations 50
```

`--version`, no model call: 50 runs after 3 warm-up runs. Sizes and RSS in
MiB.

| CLI | Startup (mean) | min – max | Executed file | Peak RSS |
|---|---|---|---|---|
| **bricks 0.4.6** | 4.8 ms | 4.3 – 6.1 ms | 29.4 MiB | 7.5 MiB |
| bricks, 0.3.6 cycle | 5.4 ms | — | 27.1 MiB | 7.4 MiB |
| Claude Code 2.1.289 | 7.3 ms | 6.4 – 11.2 ms | 219.0 MiB | 24.6 MiB |
| Codex CLI 0.150.1 | 8.6 ms | 7.5 – 11.9 ms | 218.4 MiB | 16.9 MiB |

This measures startup, not the time to answer a prompt. Pacifio's earlier
comparison (269 ms for Claude Code) measured the former Node.js Claude
Code; the "Abstract CLI" it compared against no longer exists.

### Stress suites and token accounting

The five `stress_*` examples pass **256 checks, 0 failures**
(infrastructure 46, tools 47, orchestration 33, skills 47, memory 83).
`usage_report`, with a simulated provider, reports 5 017 input and 714
output tokens for two successful tool calls, and its three consistency
checks pass; these are simulator figures, not real consumption.

### Not re-run yet

The following evaluations have not yet been re-run for Bricks. The priority
is reliable validation of real-model behaviour and reproducible results;
API cost is not the reason these evaluations remain pending.

| Evaluation | Requirements | Entry point |
|---|---|---|
| LongMemEval | Dataset, answerer, judge and embeddings | `bench/long-mem` |
| Terminal-Bench 2.0 | Daytona sandboxes and a configured model | `./bench/term-bench/run.sh` |
| Compression savings with a real model | A configured live model | Command below |
| Memory recall vs Claude Code / Codex | Each agent running with its configured model | Comparative evaluation pending |

LongMemEval evidence recall can also run with local embeddings:

```bash
cargo run --release -p longmem-bench --bin longmem-recall -- --dataset oracle
```

To evaluate compression with a live model:

```bash
BRICKS_LIVE_MODEL=provider_id/model_id cargo test -p cersei-agent --test e2e_live_compression -- --ignored --nocapture
```

No results are claimed for these pending evaluations. The measurements above
cover local tools and engine overhead; they do not measure coding quality
with a live model.

## Tests

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The published validation snapshot covers 930+ workspace tests. The model-facing paths are tested against
scripted models and local test servers. Live API tests are opt-in
(`#[ignore]`). The reported test runs used macOS (arm64); Linux and Windows
have not yet been validated in those runs.

## Documentation

| Guide | Contents |
|---|---|
| [docs/cli.md](docs/cli.md) | the `bricks` command, JSONL schema, keys, approvals, sessions |
| [docs/providers.md](docs/providers.md) | providers configuration |
| [docs/context.md](docs/context.md) | context management and compaction |
| [docs/compression.md](docs/compression.md) | tool-output compression and rules |
| [docs/shell.md](docs/shell.md) | shell, file tools, result format |
| [docs/web.md](docs/web.md) | web search and reading |
| [docs/semantic.md](docs/semantic.md) | code understanding engine, CodeScout, benchmark |
| [docs/agents.md](docs/agents.md) | sub-agents, profiles, parallel and background runs, worktrees, jobs |
| [docs/mcp.md](docs/mcp.md) | MCP client |
| [docs/memory.md](docs/memory.md) | long-term memory and hybrid recall |
| [docs/bricks.example.toml](docs/bricks.example.toml) | every `bricks.toml` section, annotated |
| [CHANGELOG.md](CHANGELOG.md) | changes since the fork |

## License

Bricks combines code under **MPL 2.0** and inherited code under **MIT**.

- **New Bricks contributions after the 107th commit**,
  [`fa7af2b8 — Bricks release`](https://github.com/AinsiParlaitZarathoustra/Bricks/commit/fa7af2b8e7d6d6fb89f25dab2f6f4a1d0cf2885e),
  are licensed under the [Mozilla Public License 2.0](https://www.mozilla.org/en-US/MPL/2.0/).
  Copyright © 2026 AinsiParlaitZarathoustra, for their contributions to Bricks.
- **Code already present at that commit**, including Cersei code by
  Adib Mohsin (Pacifio) and other contributors, retains its MIT license
  and the applicable original copyright and permission notices.

The transition does not retroactively change the license of earlier code
or transfer ownership of other contributors' work. Original MIT notices
must be preserved when that code is reused or modified.

MPL 2.0 applies at the file level: distributed files containing MPL-covered
code, including modifications to that code, remain subject to MPL 2.0.
Inherited MIT notices remain applicable to the original portions of mixed
files. See the [MPL 2.0 FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)
for details.

Cersei's original notice: **Copyright (c) 2025 Adib Mohsin**.
