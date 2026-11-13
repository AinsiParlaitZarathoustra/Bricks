# Getting Started

## Installation

The workspace version is `1.0.0-alpha.1` (`Cargo.toml`). To use the SDK in a Rust project, depend on the `bricks` facade crate in `crates/bricks`:

```toml
[dependencies]
bricks = { path = "../bricks/crates/bricks" }   # adjust to your checkout
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

To use the `bricks` command-line tool instead, install it from the checkout:

```bash
cargo install --path crates/bricks-cli
```

The CLI provides `bricks run` (run one prompt or continue a session with `--session <id>`), `bricks mcp` (manage MCP servers), and `bricks sessions` (list stored sessions).

## Prerequisites

- A recent stable Rust toolchain for edition 2024 (see `Cargo.toml`).
- A provider configured in `~/.bricks/providers.toml` (or `$BRICKS_HOME/providers.toml` when `BRICKS_HOME` is set). Start from `docs/providers.example.toml` and see [Providers](02-providers.md).
- `Grep` runs in-process and needs no external binary. `Bash` uses `bash` on `PATH`, and `PowerShell` uses `pwsh` (or Windows PowerShell).

## Your First Agent

```rust
use bricks::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let output = Agent::builder()
        .provider(provider_from_config(None, "my-provider/my-model")?) // ~/.bricks/providers.toml
        .tools(bricks::tools::coding())                                 // files, shell, web
        .permission_policy(AllowAll)                                    // approves every tool call
        .run_with("Count the number of .rs files in this directory")
        .await?;

    println!("{}", output.text());
    println!("Turns: {}, Tools used: {}", output.turns, output.tool_calls.len());
    Ok(())
}
```

`my-provider/my-model` is a `provider_id/model_id` pair from your `providers.toml`. Nothing is guessed from the model name, and a missing or invalid file returns an error.

Run your program with:

```bash
cargo run
```

## How It Works

1. You give a prompt to the agent.
2. The agent sends the prompt and available tool definitions to the model through the provider.
3. The model replies with either final text or one or more tool calls.
4. Tools run locally (for example `Bash`, `Read`, or `Grep`).
5. Tool results go back to the model for the next turn.
6. The loop repeats until the model finishes its turn, the run is cancelled, an error occurs, or the no-progress guard stops it. There is no fixed turn limit.
7. `run_with` returns an `AgentOutput` with the final message, token usage, and tool call history.

## Next Steps

- [Providers](02-providers.md): configure providers, models, and API keys
- [Tools](03-tools.md): built-in tool sets, permission levels, and custom tools
- [Agent Lifecycle](04-agent-lifecycle.md): how the agent loop runs and compacts history
- [Events & Streaming](05-events-streaming.md): callbacks, broadcast channels, streams, and reporters
- [Memory](06-memory.md): session transcripts and structured long-term memory
- [Hooks](07-hooks.md): intercepting turns and tool calls
- [Permissions](08-permissions.md): controlling what tools can do
- [Architecture](09-architecture.md): workspace crates and data flow
- [Benchmarks](10-benchmarks.md): measured timings and how to run the suites
