# MCP client

`bricks-mcp` connects Bricks to Model Context Protocol (MCP) servers. Each tool of a server becomes an agent tool named `mcp__<server>__<tool>`.

* **Protocol.** It uses the official Rust SDK (`rmcp` 3.5.0).
* **Revisions.** `2026-07-28` (current) and `2025-11-25`. Bricks picks one for each connection.
* **Transports.** `stdio` and Streamable HTTP. The old HTTP+SSE transport is not supported.

## Adding servers

### Command line

```text
bricks mcp add files -- npx -y @modelcontextprotocol/server-filesystem .
bricks mcp add docs --url https://mcp.example.com/mcp --header "Authorization=Bearer ${DOCS_TOKEN}"
```

| Command | What it does |
|---|---|
| `bricks mcp list [--json]` | Shows your servers and the project's, with their state. |
| `bricks mcp add <name> ...` | Adds a server. Use `--url` for HTTP, or put the command after `--`. Other options: `--header NAME=VALUE`, `--env NAME=VALUE`, `--cwd DIR`. |
| `bricks mcp test <name>` | Connects and lists the server's tools. |
| `bricks mcp remove <name>` | Removes one of your servers. |
| `bricks mcp enable <name>`, `bricks mcp disable <name>` | Turns one of your servers on or off. It stays in the file. |
| `bricks mcp approve <name>`, `bricks mcp deny <name>` | Answers for a server the project declares. |

### Files

Your servers are in `~/.bricks/mcp.toml`:

```toml
[[servers]]
name = "files"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[[servers]]
name = "docs"
type = "http"
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer ${DOCS_TOKEN}" }
enabled = false
```

A project can declare servers in its `bricks.toml` under `[[mcp.servers]]`. Bricks connects to such a server only after you approve it. If the declaration changes (command, arguments, URL, headers), Bricks asks again. Approvals are stored in `~/.bricks/mcp-approvals.json`, never in the repository.

Bricks Desktop has the same list in its settings.

### Fields of a server

| Field | Meaning |
|---|---|
| `name` | The server name, used in tool names. |
| `type` | `stdio` (default) or `http`. `sse` is refused. |
| `command`, `args`, `env`, `cwd` | For a stdio server. |
| `url`, `headers` | For an HTTP server. Headers are fixed text. |
| `protocol` | `auto` (default), `modern` (2026-07-28 only) or `legacy` (2025-11-25 only). |
| `limits` | See below. |
| `enabled` | `true` by default. |

`${VAR}` and `${VAR:-default}` are replaced in the command, arguments, env, URL and headers. Values are never logged.

In Rust:

```rust
use bricks::mcp::McpServerConfig;
Agent::builder()
    .mcp_server(McpServerConfig::stdio("files", "mcp-files", &["--root", "."]))
    .mcp_server(McpServerConfig::http("docs", "https://mcp.example.com/mcp"))
```

### Limits

| Key | Default | Meaning |
|---|---|---|
| `connect_timeout_ms` | 20000 | Time to start the server, shake hands and list tools. |
| `request_timeout_ms` | 60000 | How long a request may stay idle. |
| `reset_on_progress` | true | Progress from the request restarts the idle timer. |
| `max_total_timeout_ms` | 300000 | Hard limit for one request. |
| `max_message_bytes` | 16 MiB | Largest message. A bigger one closes the connection. |
| `max_in_flight` | 16 | Requests at once on one connection. |
| `mrtr_max_rounds` | 4 | Rounds of extra input a single call may ask for. |

## Protocol versions

With `auto`, Bricks first sends `server/discover`. If the server answers, Bricks uses the modern revision. Any other answer or no answer means the older `initialize` handshake of 2025-11-25. On HTTP, a `4xx` error without a modern error body also means the older one.

The version is fixed when the connection opens. The two are never mixed on one connection.

## What Bricks declares and allows

* Bricks declares no client capability: no sampling, no elicitation, no roots. A server that asks for such input gets a clean refusal (`McpError::InputNotSupported`).
* An MCP tool's permission level comes from the server's hints: `readOnlyHint` means read-only, `destructiveHint` means dangerous, anything else means execute. These are hints, not guarantees. Every call goes through the agent's permission policy like any other tool.

## Calls

* **Extra input rounds.** If a result says `input_required` and only carries a state, Bricks retries with that state, up to `mrtr_max_rounds` times (then `McpError::RoundsExceeded`).
* **Progress.** Progress notifications reach the call that owns them and show as tool progress. They restart the idle timer for that call only. The total timeout always applies.
* **Cancel.** If a turn is cancelled, Bricks sends `notifications/cancelled` on stdio, or closes the response stream on HTTP.
* **No retry.** If the connection drops during a call, the call fails with `ConnectionLost` (the tool may have run). The next call reconnects. Nothing is sent twice by itself.
* **stderr.** The last 50 lines are kept and shown when a connection is lost. Output on stderr is not an error.
* **Results.** Text, structured content, resource links and the error flag are kept. Images are not sent to the model. Each one becomes a text note such as `[image <type>, <n> bytes - not shown]`. Only the built-in `Read` tool shows images.
* **Shutdown.** Bricks closes stdin, waits, then ends the server's process (TERM, then KILL on Unix).

## In the agent

* Servers connect at the same time, each within its connect timeout. Their tools are offered with the built-in tools.
* A server that cannot be reached is reported once (`MCP server 'x' unavailable: ...`). The others keep working.
* `Agent::close()` closes the connections.
* Sub-agents share the session's MCP connections.

## Tests and limits

Tests are in `crates/bricks-mcp/tests/` (against the official SDK server, scripted servers, and a real stdio process) and `crates/bricks-agent/tests/web_mcp_e2e.rs`.

Not covered: Windows. OAuth is not implemented, so only fixed headers work. `subscriptions/listen` and the tasks extension are not used.
