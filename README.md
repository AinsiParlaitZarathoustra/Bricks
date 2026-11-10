<div align="center">

# Bricks

**A desktop coding agent built in Rust.**

![Version](https://img.shields.io/badge/version-1.0.0--alpha.1-blue)
![Rust](https://img.shields.io/badge/built_with-Rust-orange)

[Features](#features) · [Getting started](#getting-started) · [Settings](#settings) · [CLI](#cli) · [Rust SDK](#rust-sdk) · [Development](#development) · [Documentation](#documentation) · [License](#license)

<img width="1200" height="712" alt="bricks_demo" src="https://github.com/user-attachments/assets/80ce4162-456b-4102-886a-941e47c3d200" />

</div>

Bricks is a desktop coding agent built in Rust. Explore your codebase, edit
files, run commands and delegate work to sub-agents from one workspace.
Configure your models and tools directly in the application’s settings.

The desktop application, command-line interface and Rust SDK share the same
agent engine: model connections, tool execution, permissions, context
management and sessions.

**Status:** Bricks 1.0 Alpha, in active development. The current workspace version is **1.0.0-alpha.1**.
Development and the recorded validation runs use macOS on Apple Silicon;
Linux and Windows have not yet been validated in those runs.

## Features

### Desktop application

- **Projects and conversations.** A project is a folder. Search across
  conversation titles and messages, rename, delete and resume sessions.
  Bricks asks for the folder again when a project has moved.
- **Composer.** Attach files, folders and images, mention files with `@`,
  run `/` commands, recall earlier requests, and choose the model and
  reasoning effort for each conversation.
- **Readable runs.** Tool activity folds into each run. A run ends with a card
  showing its duration, changed files and commands. The plan stays visible,
  the model can ask you questions with an optional timer, and you can edit a
  message and run it again.
- **Rich answers.** Markdown, syntax-highlighted code, diffs, KaTeX formulas,
  footnotes and GitHub-style alerts.
- **Figures.** Mermaid diagrams, SVG drawings and Vega-Lite charts render
  sanitized and sandboxed, with nothing loaded from the network. Zoom, pan,
  open full screen, view the source, export SVG or PNG, or copy the image.
- **Reader.** Open project files in tabs, with a source view, a Markdown and
  SVG preview, and the recorded changes of each file.
- **Changes and Git.** See what a conversation changed, the project's change
  history, and Git status, commits and diffs.
- **Terminal.** Several terminals per project, in the project folder.
- **Context panel.** Context usage by source, pending approvals, project
  environment, and views for tools, skills, MCP, memory, jobs and cost.
- **Bilingual interface.** English by default, French in Settings. This
  language is independent of the conversations.
- **Personal touches.** Light, dark or system theme, custom colours with
  contrast checks, a choice of app icon, and an option to keep the computer
  awake.

### Agent engine

- **Your choice of models.** Add providers and models in Settings, with
  support for Chat Completions, Responses and Anthropic Messages. Set model
  limits, modalities, reasoning profiles and pricing from the interface.
- **Built-in tools.** Every session starts with:

  | Area | Tools |
  |---|---|
  | Files | `Read`, `ViewImage`, `Write`, `Edit`, `ApplyPatch` |
  | Search and code | `Glob`, `Grep`, `CodeSearch`, `CodeScout`, `CodeStats` |
  | Commands | `Bash`, `PowerShell`, `Job` |
  | Web | `WebFetch`, `WebSearch` |
  | Planning and skills | `TodoWrite`, `Skill`, `Ask` (interactive sessions) |
  | Sub-agents | `Agent`, `AgentControl`, `AgentProfiles`, `Messagent` |

- **Concurrent tools.** Independent tool calls can run in parallel. Each
  model has an option to start completed tool calls while the rest of the
  response is still streaming.
- **Safe edits and commands.** File edits are revalidated before atomic
  writes. Commands are supervised, and long ones continue as background jobs.
- **Code understanding.** Lexical search, Tree-sitter syntax context for Rust,
  TypeScript, JavaScript, Python and Go, and optional language servers.
- **Sub-agents.** Launch one or more missions in the background, from built-in,
  user or project profiles, each with its own context. They coordinate through
  `Messagent`. Parallel writers can work in isolated Git worktrees, and their
  changes come back for inspection before they are applied. On the desktop,
  each sub-agent opens as a read-only branch of the conversation.
- **Permissions.** Each session uses ASK, EXECUTE or CUSTOM, with rules per
  action and per tool. Review file diffs before approving changes; approvals
  cover individual calls.
- **Context management.** Track usage, compact or clear a conversation, and
  reduce tool outputs while keeping their originals accessible.
- **Memory.** Optional long-term memory with sourced records, global and
  project scopes, and hybrid recall.
- **Skills.** Install Agent Skills in Bricks’ own store. Project skills run
  only once you have trusted them.
- **Web tools.** Search with DuckDuckGo, Brave, Tavily or Exa, read pages as
  Markdown and retrieve citable passages.
- **Computer Use.** An optional built-in extension on macOS, through Cua Driver.

### For developers

- **CLI.** One-shot runs, JSONL events for scripts and CI, a non-interactive
  mode, stored sessions, attachments and skills.
- **Rust SDK.** The same engine as a library, with a `Tool` trait and derive
  macro, an MCP client for stdio and Streamable HTTP, and lifecycle hooks.
- **Further crates.** Workflows (`bricks-workflows`), sandboxes
  (`bricks-vms`), embeddings (`bricks-embeddings`), an agent template language
  (`bricks-agentlang`) and self-evolving orchestration (`bricks-agentrl`).

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
- **General:** interface language, permissions for new conversations,
  keeping the computer awake, diagrams and the question timer.
- **Memory:** turn long-term memory on and manage it, globally or per project.
- **Skills:** import, inspect and remove installed skills.
- **Extensions:** turn on Computer Use and check its macOS permissions.
- **Appearance:** theme, colours and app icon.
- **Application:** version, platform and Bricks’ folders.

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
a CLI session, `--file` and `--image` to attach content, and `--skill` to
activate a skill. See the [CLI reference](docs/cli.md) for configuration,
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
| [Built-in tools](docs/content/docs/built-in-tools.mdx) | Every tool of the SDK |
| [Shell and files](docs/shell.md) | Commands, edits and background jobs |
| [Sub-agents](docs/agents.md) | Profiles, delegation and worktrees |
| [Code understanding](docs/semantic.md) | CodeScout, syntax and optional LSP |
| [Memory](docs/memory.md) | Long-term storage and hybrid recall |
| [Skills](docs/skills.md) | Skill installation and activation |
| [MCP](docs/mcp.md) | The SDK's MCP client |
| [Web](docs/web.md) | Search and page reading |
| [Maintenance](docs/maintenance.md) | Builds, publishing and dependencies |
| [Migration](docs/content/docs/migration.mdx) | Removed APIs and configuration changes |
| [Changelog](CHANGELOG.md) | Release history |

The documentation website lives in [docs/](docs/README.md).

## License

New contributions to Bricks are licensed under the
[Mozilla Public License 2.0](LICENSE), file by file. Copyright © 2026
AinsiParlaitZarathoustra.

Code inherited from Cersei, up to commit
[`fa7af2b8`](https://github.com/AinsiParlaitZarathoustra/Bricks/commit/fa7af2b8e7d6d6fb89f25dab2f6f4a1d0cf2885e),
stays under the MIT license: Copyright (c) 2025 Adib Mohsin and contributors.
Its notices must be kept when that code is reused or modified.
