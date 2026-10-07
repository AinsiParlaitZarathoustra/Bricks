<div align="center">

# Bricks

**An engine for agentic development. Built in Rust. At home in your terminal.**

![Version](https://img.shields.io/badge/version-0.3.6-blue)
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

**Bricks 0.3.6** has a substantial, stable foundation. Development continues,
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

**Measured again on 2026-10-06.** The machine is an Apple A18 Pro (6
cores, 8 GB) running macOS 27.2. The build uses the workspace's release
profile (`opt-level = "z"`, thin LTO). No model is called by any benchmark
below.

Pacifio's figures were published with Cersei 0.1.6 on Apple Silicon, on a
different machine. The comparison shows orders of magnitude and
regressions, not a controlled A/B test.

### Tool I/O

```bash
cargo run --release -p cersei --example benchmark_io
```

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

### Memory I/O

```bash
cargo run --release -p cersei-memory --features graph --example memory_bench
```

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

Each agent carries a context manager, a tool-output compressor with its
store, a web context and an approval-ready permission path. These additional
capabilities explain the higher instantiation cost and memory footprint.
Built-in compression rules are parsed once per process and shared.

The Python frameworks (Agno, LangGraph, PydanticAI, CrewAI) have **not yet been
re-run**: their published results in `bench/general-agents/results/` date
from Pacifio's run. Re-running them downloads those frameworks from PyPI:
`./bench/general-agents/run.sh`.

### CLI startup

```bash
python3 scripts/bench_cli.py --bricks <path> --iterations 50
```

The command is `--version`, with no model call: 50 runs after warm-up.

| CLI | Startup (mean) | Executed file | Peak RSS |
|---|---|---|---|
| bricks 0.2.6 | 5.4 ms | 27.1 MB | 7.4 MB |
| Claude Code 2.1.289 | 7.4 ms | 219.0 MB | 24.6 MB |
| Codex CLI 0.150.1 | 9.0 ms | 218.4 MB | 16.8 MB |

Pacifio's earlier comparison (269 ms for Claude Code) measured the former
Node.js Claude Code; the current one is a native binary. The "Abstract CLI"
it compared against no longer exists; `bricks` replaces it.

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
