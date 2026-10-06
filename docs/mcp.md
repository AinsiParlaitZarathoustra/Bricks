# MCP client

`cersei-mcp` connects Bricks to Model Context Protocol servers. Each
server's tools become agent tools named `mcp__<server>__<tool>`.

* **Protocol.** It is built on the official Rust SDK (`rmcp` 3.5.0, MSRV
  1.88) behind Bricks' types. The hand-written JSON-RPC client it replaces
  spoke `2024-11-05` over stdio only.
* **Revisions.** **2026-07-28** (current) and **2025-11-25** (the
  `initialize`-based revision), detected per connection.
* **Transports.** **stdio** and **Streamable HTTP**. The HTTP+SSE transport
  of 2024-11-05 (two endpoints, deprecated) is not supported. It never
  worked here: the former code rejected it as "not yet implemented".

## Configuration

```rust
Agent::builder()
    .mcp_server(McpServerConfig::stdio("files", "mcp-files", &["--root", "."]))
    .mcp_server(McpServerConfig::http("docs", "https://mcp.example.com/mcp"))
```

| field | |
|---|---|
| `type` | `stdio` (default) or `http` (`streamable-http`); `sse` is refused with an explanation |
| `command`, `args`, `env`, `cwd` | stdio server |
| `url`, `headers` | Streamable HTTP endpoint and static headers (e.g. `Authorization = "Bearer ${DOCS_TOKEN}"`) |
| `protocol` | `auto` (default), `modern` (2026-07-28 only), `legacy` (2025-11-25 only) |
| `limits` | see below |

`${VAR}` and `${VAR:-default}` are expanded in command, arguments, env, URL
and headers. Values are never logged.

| `limits` | default | |
|---|---|---|
| `connect_timeout_ms` | 20 000 | spawn or first request, discovery or handshake, tool list |
| `request_timeout_ms` | 60 000 | idle time of a request |
| `reset_on_progress` | true | the request's own progress resets the idle timer |
| `max_total_timeout_ms` | 300 000 | hard limit, whatever the progress |
| `max_message_bytes` | 16 MiB | stdio line, HTTP JSON body, SSE event |
| `max_in_flight` | 16 | requests at once per connection |
| `mrtr_max_rounds` | 4 | `input_required` rounds per call |

## Lifecycle and versions

* **`auto`.** Sends `server/discover` first, with `2026-07-28` in `_meta`:
  * a `DiscoverResult` → modern;
  * a recognised modern error (unsupported version) → modern, with a
    supported version;
  * any other error or no answer → legacy, and the `initialize` handshake
    of 2025-11-25.

  On HTTP, a `4xx` without a recognised modern error body means legacy.
* **One era per connection.** The era is fixed at connection time and the
  two contracts are never mixed on it. Modern requests carry
  `io.modelcontextprotocol/protocolVersion`, `clientCapabilities` and
  `clientInfo` in `_meta`; on HTTP they also carry the
  `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers. Legacy
  requests do not.
* **Results.** A result without `resultType` (older servers) counts as
  `complete`.

## Capabilities

* **None declared.** Bricks declares no client capability: no sampling, no
  elicitation, no roots. Roots and sampling are deprecated in 2026-07-28 and
  were never implemented in Bricks. The former code declared `roots`
  without handling it; that is fixed.
* **Refusals.** A server that still asks for such input gets a clean
  refusal: `McpError::InputNotSupported("elicitation/create")`, either as a
  legacy server-to-client request or inside an `input_required` result.
  No capability is announced without a handler.
* **Discovery.** It never widens permissions. An MCP tool's permission level
  comes from its annotations (`readOnlyHint` → read-only, `destructiveHint`
  → dangerous, otherwise "execute"). These are hints from the server, not
  guarantees, and they go through the agent's permission policy like any
  tool.

## Calls

* **Multi round-trip requests (MRTR).** An `input_required` result with only
  `requestState` is retried with a new request id and the state echoed
  verbatim, at most `mrtr_max_rounds` times
  (`McpError::RoundsExceeded`). Input requests are refused as above.
* **Progress.** `notifications/progress` reach the call that owns the token
  and are forwarded as `AgentEvent::ToolProgress`. They reset the idle
  timer only for that call: a notification with another token extends
  nothing. The total timeout always applies.
* **Cancellation.** A call whose future is dropped (a cancelled turn) sends
  `notifications/cancelled` on stdio. On HTTP the response stream is
  closed.
* **No replay.** A call cut by a lost connection fails with
  `ConnectionLost { in_flight: true }`: "not retried (the tool may have
  run)". The next call reconnects (a new process on stdio). Nothing is
  re-sent automatically. The legacy HTTP client's session re-initialisation
  and stream reconnection are disabled for the same reason.
* **stderr is a log.** The last 50 lines are kept and shown when a
  connection is lost. Output on stderr is not an error.
* **Results.** Results keep text blocks in order, `structuredContent` as
  JSON, media described by type and size, resource links, and `isError` as
  the tool's failure status, in the uniform rendering
  (`✓ [mcp__kv__lookup] Succès (0.01s)`, `--- structured content ---`).
* **Messages over `max_message_bytes`.** They close the connection, with the
  reason in the error. HTTP JSON bodies and SSE events are read through a
  bounded client (the workspace's `reqwest`, plugged into `rmcp`).
* **Shutdown.** Closing stops the transport (stdin closed), waits, then
  terminates the stdio server's process group (TERM, then KILL on Unix).
  On other systems the process is killed after the wait.

## Agent integration

* **First run.** Servers are connected concurrently, each within its
  connect timeout. Their tools are offered to the model with the built-in
  tools.
* **Failures.** A server that cannot be reached is reported once as a
  status event (`MCP server 'x' unavailable: …`); the others work.
* **Shutdown.** `Agent::close()` closes the connections.

## Verified

| | how |
|---|---|
| 2026-07-28 and 2025-11-25 against the official SDK's server | in process (stdio framing over a byte stream) and over Streamable HTTP (hyper, stateless and session modes) |
| legacy-only server, `server/discover` fallback | scripted stdio-framed and HTTP servers |
| real stdio process | bash script server: stderr logs, death during a call, restart without replay, oversized line |
| progress, idle and total timeouts, another call's token, cancellation | SDK server |
| MRTR (state rounds, bound, refused elicitation), malformed answer, lost connection | scripted servers |
| agent wiring | `crates/cersei-agent/tests/web_mcp_e2e.rs` |

* **Not verified.** Windows. Authorization flows (OAuth) are not
  implemented: static headers only. `subscriptions/listen` is not used.
  The tasks extension is not declared.
