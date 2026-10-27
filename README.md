<div align="center">

# Bricks

**An engine for agentic development. Built in Rust. At home in your terminal.**

![Version](https://img.shields.io/badge/version-0.6.1-blue)
![Rust](https://img.shields.io/badge/built_with-Rust-orange)

[Quick start](#quick-start) · [What it does](#what-it-does) · [Configuration](#configuration) · [The engine as a library](#the-engine-as-a-library) · [Documentation](#documentation) · [License](#license)


**<ins>The README is outdated and will be updated very soon.</ins>**

</div>

Bricks is a tool for coding with AI agents. It has three parts:

- **Bricks ADK** (Agentic Development Kit): the Rust crates (`bricks` and the
  `bricks-*` crates) that you use to build agent applications. It provides the
  agent loop, providers configured by file, tools, approvals, context
  management, sessions, memory, skills and MCP.
- **`bricks`**, the command-line frontend for one prompt, scripts and CI, with
  text output or versioned JSONL events.
- **Bricks Desktop** (`apps/bricks-desktop`), the interactive application. It
  runs on the same engine contract as the command line.

### Two ideas behind Bricks

Bricks brings together two concepts developed by AinsiParlaitZarathoustra:

- **ADK, the Agentic Development Kit (Bricks ADK).** A broad set of
  purpose-built, optimised tools that help AI agents inspect code, make
  changes, run commands and work effectively across a project.
- **ADE, the Agentic Development Engine.** An engine designed to enhance the
  capabilities of any agent by providing the tools, context and execution
  infrastructure it needs to perform well.

**Engine** is the defining word. Where a *harness* suggests restraint,
reduced agility and dependence on whoever holds it, an engine supplies the
means to act. Bricks is built around that ambition: empower the agent and
make its work more effective.

### Project status

**Bricks 0.6.1** (the workspace version) has a substantial foundation.
Development continues, with further features and code optimisations still
ahead.

Bricks is a fork of Cersei, created by **Adib Mohsin (Pacifio)**. Its crates
are named `bricks-*`. A few identifiers keep their Cersei names because other
code depends on them, for example the error type `CerseiError`. See the
[migration guide](docs/content/docs/migration.mdx) for the changes since the
Cersei names and behaviour.

## Quick start

Build the `bricks` command from a checkout of this repository. You need a
recent stable Rust (1.88 or later) and a C++17 toolchain, because the vector
index (USearch) is compiled with the engine:

```bash
cargo install --path crates/bricks-cli
```

Declare a provider and a model. Copy [`docs/providers.example.toml`](docs/providers.example.toml)
to `~/.bricks/providers.toml` and adapt it (the example's endpoints, limits and
prices are illustrative). Then select the model in the project's `bricks.toml`,
as shown in [Configuration](#configuration):

```bash
bricks run "Explain the failing test in src/parser.rs"          # one prompt, answer on stdout
bricks run --json --non-interactive "Analyse the build errors"  # scripts and CI: JSONL events
bricks sessions                                                 # the project's stored sessions
bricks run --session <id> "Continue where we stopped"           # continue one, headless
```

The interactive interface is Bricks Desktop. From a checkout, with Node.js and
npm installed:

```bash
cd apps/bricks-desktop
npm ci
npm run tauri dev
```

Open a project, then pick a conversation in the sidebar or use `/resume`.

## What it does

* **Models are configuration.** `~/.bricks/providers.toml` lists providers,
  models, limits, capabilities, reasoning profiles and prices. Nothing is built
  in and nothing is guessed from a model name.
  * A model is selected explicitly as `provider_id/model_id`.
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
  * Each `Bash` call is its own process. The session's state (working
    directory, exported variables, aliases and functions) carries over to the
    next call when a call ends normally. Calls can run side by side, and a
    long command can be handed over as a background task.
  * Execution is supervised: timeouts, process-tree termination,
    cancellation and background tasks.
  * `Edit` is one tool: a single replacement, or several in `edits`. Edits are
    re-validated before an atomic write. See [docs/shell.md](docs/shell.md).
* **Sub-agents and jobs.** The native `Agent`, `Agents`, `AgentControl` and
  `AgentProfiles` tools run sub-agents with a fresh context. They get the
  parent's permissions and at most its tools. Parallel writers each get their
  own git worktree, and their changes come back as ChangeSets to inspect and
  apply. Background commands are supervised jobs. See
  [docs/agents.md](docs/agents.md).
* **Web.** Web search with ordered provider fallback, page reading in
  Markdown, citable passages, and a private-network policy. See
  [docs/web.md](docs/web.md).
* **Code understanding.** `CodeScout` (and `/search`) answer "where is it
  defined, who uses it, what is here" in one call with exact, sourced and
  bounded results: text search, Tree-sitter, and optional language servers
  (never installed by Bricks). See [docs/semantic.md](docs/semantic.md).
* **MCP.** A client on the official SDK (`rmcp`), over stdio and Streamable
  HTTP. See [docs/mcp.md](docs/mcp.md).
* **Skills.** Standard [Agent Skills](https://agentskills.io/specification)
  (`SKILL.md`) from `.agents/skills` and `.claude/skills` (project and home)
  plus built-in ones.
  * The model sees a catalog and activates a skill with the `Skill` tool;
    you can choose one with `/skill` or `--skill`.
  * A project's skills need your approval for the session.
  * Activating a skill gives instructions only: it runs nothing and grants
    no permission. See [docs/skills.md](docs/skills.md).
* **Long-term memory.** Sourced episodes, entities scoped by
  user/project/space, and temporal facts with evidence.
  * Hybrid recall combines vector, BM25 and a bounded graph expansion,
    fused with weighted RRF.
  * Maintenance (extraction, embeddings) runs after the answer: it is
    resumable and cancellable. Memory is off until `[memory] enabled = true`.
    See [docs/memory.md](docs/memory.md).
* **Approvals.** `[permissions]` in `bricks.toml` sets the policy; the
  engine applies it to every frontend.
  * File changes are shown as a diff before anything is written.
  * A change approved for a file that changed meanwhile is recomputed and
    asked again.
* **The `bricks` command.** A thin layer over the engine's command/event
  contract, for scripts and CI. JSONL events are versioned, with documented
  exit codes. Sessions can be listed and continued (`bricks sessions`,
  `bricks run --session <id>`). See [docs/cli.md](docs/cli.md).
* **Bricks Desktop** (`apps/bricks-desktop`). The interactive interface:
  projects and conversations in a sidebar, `@` file mentions, `/` commands,
  approvals with the diff, inspectors and session resume. See
  [the desktop section of docs/cli.md](docs/cli.md#the-desktop-application).

## Configuration

Two files configure a run. Neither is optional for a model: `bricks` never
chooses a model silently.

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
# ./bricks.toml — see docs/bricks.example.toml for every section
[agent]
model = "local/coder"            # provider_id/model_id

[permissions]
write = "ask"                    # file changes: shown as a diff, then asked
execute = "ask"                  # shell commands
```

Bricks keeps its files in `~/.bricks`: the providers file, the sessions
(`~/.bricks/sessions`) and the memory. Set `BRICKS_HOME` to move that folder,
and pass `--providers <file>` to read another providers file.

## The engine as a library

Add the facade crate from a checkout, by path:

```toml
[dependencies]
bricks = { path = "/path/to/bricks/crates/bricks" }
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

or from the Git repository, replacing the revision with the one you want to
build against:

```toml
[dependencies]
bricks = { git = "https://github.com/AinsiParlaitZarathoustra/Bricks", rev = "<revision>" }
```

```rust
use bricks::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let output = Agent::builder()
        .provider(provider_from_config(None, "local/coder")?) // ~/.bricks/providers.toml
        .tools(bricks::tools::coding())                        // files, shell, web
        .permission_policy(AllowReadOnly)
        .run_with("Explain what src/main.rs does")
        .await?;
    println!("{}", output.text());
    Ok(())
}
```

`bricks::tools::coding()` is a set of tools, not a default for every session:
see [the tools of a session](docs/cli.md#tools-of-a-session) for what the
`bricks` command and Bricks Desktop give the model.

Frontends use the command/event contract (`bricks_agent::control`). It offers
typed commands (submit, cancel, set_model, compact, resume, approve…) and
versioned events with one `run_finished` per run. Delivery is bounded and never
silently drops an event. See [docs/cli.md](docs/cli.md).

## Architecture

```text
bricks-cli            the `bricks` binary: arguments, configuration, headless / JSONL
apps/bricks-desktop   Bricks Desktop (Tauri): the interactive interface
bricks                facade crate — use bricks::prelude::*
  bricks-agent        agent builder, agentic loop, context manager, compaction,
                      command/event contract (control), approvals, sub-agents
  bricks-provider     provider registry from configuration, 3 protocol adapters
  bricks-tools        file, shell, web, planning, orchestration tools; permissions; previews
  bricks-compression  tool-output reduction, rules, tree-sitter skeletons
  bricks-memory       sessions, memdir, graph memory, structured long-term memory
  bricks-embeddings   embedding providers, USearch vector index
  bricks-semantic     shared code understanding engine: lexical, syntax, LSP
  bricks-web          search, fetching, extraction, passages
  bricks-mcp          MCP client (rmcp)
  bricks-skills       Agent Skills (SKILL.md): sources, trust, activation
  bricks-config       engine assembly shared by the frontends: home, providers, projects
  bricks-types, bricks-hooks, bricks-lsp, bricks-workflows, …
```

## Measurements

The measurements recorded so far (Bricks 0.4.6, 2026-10-08) are in
[docs/benchmarks-bricks-2026-10-08.md](docs/benchmarks-bricks-2026-10-08.md).
They predate the current architecture. They cover local tools and engine
overhead, and no live model was called. They are not a performance promise.
Evaluations that need a live model have not been re-run, and no result is
claimed for them.

## Tests

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The model-facing paths are tested against scripted models and local test
servers. Live API tests are opt-in (`#[ignore]`). The reported test runs used
macOS (arm64); Linux and Windows have not yet been validated in those runs.

## Documentation

| Guide | Contents |
|---|---|
| [docs/cli.md](docs/cli.md) | the `bricks` command, JSONL schema, approvals, sessions, tools of a session, the desktop application |
| [docs/maintenance.md](docs/maintenance.md) | build, publishing, dependency advisories |
| [docs/providers.md](docs/providers.md) | providers configuration |
| [docs/providers.example.toml](docs/providers.example.toml) | every `providers.toml` section, annotated |
| [docs/context.md](docs/context.md) | context management and compaction |
| [docs/compression.md](docs/compression.md) | tool-output compression and rules |
| [docs/shell.md](docs/shell.md) | shell, file tools, result format |
| [docs/web.md](docs/web.md) | web search and reading |
| [docs/semantic.md](docs/semantic.md) | code understanding engine, CodeScout |
| [docs/agents.md](docs/agents.md) | sub-agents, profiles, parallel and background runs, worktrees, jobs |
| [docs/mcp.md](docs/mcp.md) | MCP client |
| [docs/memory.md](docs/memory.md) | long-term memory and hybrid recall |
| [docs/skills.md](docs/skills.md) | Agent Skills |
| [docs/bricks.example.toml](docs/bricks.example.toml) | every `bricks.toml` section, annotated |
| [docs/content/docs/migration.mdx](docs/content/docs/migration.mdx) | migration from the Cersei names and behaviour |
| [CHANGELOG.md](CHANGELOG.md) | changes since the fork |

The documentation site is in [`docs/`](docs/README.md).

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
