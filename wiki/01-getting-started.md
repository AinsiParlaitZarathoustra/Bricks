# Getting Started

## Installation

The workspace version is 0.6.0 (`Cargo.toml`). Depend on a checkout of this repository, the `cersei` crate in `crates/cersei`:

```toml
[dependencies]
cersei = { path = "../bricks/crates/cersei" }   # adjust to your checkout
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

To use the `bricks` command-line tool instead, install it from the checkout: `cargo install --path crates/bricks-cli`.

## Prerequisites

- A recent stable Rust: edition 2024, 1.88 or later (let chains); tested with Rust 1.98.1
- A provider declared in `~/.bricks/providers.toml` (see [Providers](02-providers.md)). Start from `docs/providers.example.toml`. Its API key is read from the environment variable named by `api_key_env`.
- No external tools for `Grep`: it runs in-process. `Bash` uses the `bash` found on `PATH`; `PowerShell` needs `pwsh` or `powershell`.

## Your First Agent

```rust
use cersei::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let output = Agent::builder()
        .provider(provider_from_config(None, "my-provider/my-model")?) // ~/.bricks/providers.toml
        .tools(cersei::tools::coding())                                 // files, shell, web
        .permission_policy(AllowAll)                                    // allow all tool calls
        .run_with("Count the number of .rs files in this directory")
        .await?;

    println!("{}", output.text());
    println!("Turns: {}, Tools used: {}", output.turns, output.tool_calls.len());
    Ok(())
}
```

`my-provider/my-model` is a `provider_id/model_id` pair from your `providers.toml`. Nothing is inferred from the model name, and a missing or invalid file is an error.

Set the key variable named in the file, then run:

```bash
export BRICKS_MY_PROVIDER_KEY=...   # whatever api_key_env names in providers.toml
cargo run
```

## How It Works

1. **You provide a prompt** to the agent
2. **The agent sends it to the LLM** (via the Provider) with available tool definitions
3. **The LLM responds** — either with text (done) or with tool calls
4. **Tools execute locally** — Bash commands, file reads, grep searches, etc.
5. **Results feed back** to the LLM for the next turn
6. **Loop repeats** until the LLM says "end_turn" (there is no turn limit; cancellation, errors and the no-progress guard also stop it)
7. **You get `AgentOutput`** with the final message, usage stats, and tool call history

## Next Steps

- [Providers](02-providers.md) — declare providers and models, protocols, custom providers
- [Tools](03-tools.md) — built-in tools, permission levels, and how to create custom ones
- [Agent Lifecycle](04-agent-lifecycle.md) — the agentic loop in detail
- [Events & Streaming](05-events-streaming.md) — real-time observation
- [Memory](06-memory.md) — session persistence
- [Hooks](07-hooks.md) — middleware and interception
- [Permissions](08-permissions.md) — controlling what tools can do, and approvals
- [Benchmarks](10-benchmarks.md) — performance figures and their dates
