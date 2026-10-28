<div align="center">

# Bricks

**A desktop coding agent built in Rust.**

![Version](https://img.shields.io/badge/version-0.6.1-blue)
![Rust](https://img.shields.io/badge/built_with-Rust-orange)

[Features](#features) · [Getting started](#getting-started) · [Settings](#settings) · [CLI](#cli) · [Rust SDK](#rust-sdk) · [Development](#development) · [Documentation](#documentation) · [License](#license)

</div>

Bricks is a desktop coding agent built in Rust. Explore your codebase, edit
files, run commands and delegate work to sub-agents from one workspace.
Configure your models and tools directly in the application’s settings.

The desktop application, command-line interface and Rust SDK share the same
agent engine: model connections, tool execution, permissions, context
management and sessions.

**Status:** in active development. The current workspace version is **0.6.1**.
Development and the recorded validation runs use macOS on Apple Silicon;
Linux and Windows have not yet been validated in those runs.

## Features

- **Desktop workspace.** Projects, conversations, file attachments, `@` file
  mentions, `/` commands, tool activity and session resume.
- **Your choice of models.** Add providers and models in Settings, with
  support for Chat Completions, Responses and Anthropic Messages. Set model
  limits, reasoning profiles and pricing from the interface.
- **Concurrent tools.** Independent tool calls can run in parallel. Each
  model has an option to start completed tool calls while the rest of the
  response is still streaming.
- **Code and shell tools.** Search code with `CodeScout`, read and edit files,
  run supervised commands and manage background jobs. File edits are
  revalidated before atomic writes.
- **Sub-agents.** Delegate tasks with profiles and separate contexts. Parallel
  writers can work in isolated Git worktrees, with changes returned for
  inspection and application.
- **Permissions.** Each session uses ASK, EXECUTE or CUSTOM. Review file diffs
  before approving changes; approvals cover individual calls.
- **Context management.** Track usage, compact conversations and reduce tool
  outputs while keeping their originals accessible.
- **Memory.** Structured long-term memory with sourced records, project
  scopes and hybrid recall.
- **Skills and MCP.** Install Agent Skills in Bricks’ own store and connect
  MCP servers over stdio or Streamable HTTP.
- **Web tools.** Search, read pages as Markdown and retrieve citable passages.

## Getting started

To run Bricks Desktop from source, install a recent stable Rust toolchain,
Node.js, npm and the native build dependencies for Tauri. The vector index
also requires a C++17 toolchain.

From the repository root:

```bash
cd apps/bricks-desktop
npm ci
npm run tauri dev
```

Once Bricks is open:

1. Open **Settings** and add your provider.
2. Add a model and choose its reasoning and execution options.
3. Open a project and start a conversation.

## Settings

Set up Bricks directly in the desktop application:

- **Providers and models:** connections, credentials, model selection,
  limits, reasoning profiles and tool execution options.
- **Permissions:** choose how new conversations ask for approval.
- **Skills:** import, inspect and remove installed skills.
- **Memory:** manage the memories available to your projects.
- **Appearance:** adjust the theme and fonts.

Each conversation keeps its own permission policy. File changes can be
reviewed as diffs before approval, and running work can be interrupted from
the conversation.

## CLI

Install the command-line frontend from the repository root:

```bash
cargo install --locked --path crates/bricks-cli
```

```bash
# Run one task.
bricks run "Explain the failing test in src/parser.rs"

# Stream JSONL events for scripts and CI.
bricks run --json --non-interactive "Analyse the build errors"

# List and continue stored sessions.
bricks sessions
bricks run --session <id> "Continue where we stopped"
```

Use `--permissions ask` or `--permissions execute` to choose a profile for
a CLI session. See the [CLI reference](docs/cli.md) for configuration,
events and exit codes.

## Rust SDK

Bricks also exposes its engine through the `bricks` facade and `bricks-*`
crates. Build custom agent applications with the same model connections,
tools, context management, sessions and permission engine used by the
desktop application.

See the [agent API](docs/content/docs/api-agent.mdx),
[architecture](docs/content/docs/architecture.mdx) and
[crate map](docs/content/docs/crate-map.mdx) for integration details.

## Development

Rust tests use [Nextest](https://nexte.st/):

```bash
cargo nextest run --workspace --locked
```

For the workspace validation, including compilation of all targets:

```bash
./scripts/run_tests.sh
```

Desktop checks, from `apps/bricks-desktop`:

```bash
npm run check
npm run test:unit
npm run build
```

Model-facing tests use scripted providers and local servers. Live API tests
are opt-in. Builds and test runs regenerate `target`; keep an eye on its
size during development.

## Documentation

| Guide | Contents |
|---|---|
| [CLI and desktop](docs/cli.md) | Sessions, permissions, commands and events |
| [Providers](docs/providers.md) | Models, protocols and reasoning profiles |
| [Context](docs/context.md) | Usage accounting and compaction |
| [Compression](docs/compression.md) | Tool-output reduction and originals |
| [Shell and files](docs/shell.md) | Commands, edits and background jobs |
| [Sub-agents](docs/agents.md) | Profiles, delegation and worktrees |
| [Code understanding](docs/semantic.md) | CodeScout, syntax and optional LSP |
| [Memory](docs/memory.md) | Long-term storage and hybrid recall |
| [Skills](docs/skills.md) | Skill installation and activation |
| [MCP](docs/mcp.md) | External tool connections |
| [Web](docs/web.md) | Search and page reading |
| [Maintenance](docs/maintenance.md) | Builds, publishing and dependencies |
| [Migration](docs/content/docs/migration.mdx) | Removed APIs and configuration changes |
| [Changelog](CHANGELOG.md) | Release history |

The documentation website lives in [docs/](docs/README.md).

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
