//! Scripted MCP servers for the cases a well-behaved SDK server does not
//! produce: legacy-only servers, MRTR rounds, malformed answers, lost
//! connections, a real stdio process, Streamable HTTP in both eras.

use cersei_mcp::{
    Era, McpClient, McpContent, McpError, McpLimits, McpManager, McpServerConfig, ProtocolMode,
};
use cersei_testkit::{Reply, TestServer};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

type Script = Box<dyn FnMut(&Value) -> Option<Vec<Value>> + Send>;

/// A server on a byte stream: each received message goes to `script`, which
/// answers with messages, or `None` to close the connection. Every received
/// message is logged.
fn scripted(mut script: Script) -> (tokio::io::DuplexStream, Arc<parking_lot::Mutex<Vec<Value>>>) {
    let (server, client) = tokio::io::duplex(1 << 20);
    let log = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let l = log.clone();
    tokio::spawn(async move {
        let (r, mut w) = tokio::io::split(server);
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            l.lock().push(msg.clone());
            match script(&msg) {
                Some(out) => {
                    for m in out {
                        let mut s = serde_json::to_string(&m).unwrap();
                        s.push('\n');
                        if w.write_all(s.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                }
                None => return,
            }
        }
    });
    (client, log)
}

fn method(m: &Value) -> &str {
    m.get("method").and_then(Value::as_str).unwrap_or("")
}

fn reply(m: &Value, result: Value) -> Vec<Value> {
    vec![json!({"jsonrpc": "2.0", "id": m["id"], "result": result})]
}

fn error(m: &Value, code: i64, message: &str) -> Vec<Value> {
    vec![json!({"jsonrpc": "2.0", "id": m["id"], "error": {"code": code, "message": message}})]
}

fn tool_list() -> Value {
    json!({"resultType": "complete", "tools": [{"name": "t", "inputSchema": {"type": "object"}}], "ttlMs": 0, "cacheScope": "private"})
}

fn discover() -> Value {
    // `ttlMs` and `cacheScope` are required (CacheableResult).
    json!({"resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": {"tools": {}},
           "ttlMs": 0, "cacheScope": "private",
           "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "scripted", "version": "1"}}})
}

fn legacy_server() -> Script {
    Box::new(|m| {
        Some(match method(m) {
            // A legacy server knows nothing of discovery.
            "server/discover" => error(m, -32601, "Method not found"),
            "initialize" => reply(
                m,
                json!({"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                                             "serverInfo": {"name": "old", "version": "0.9"}}),
            ),
            "notifications/initialized" => vec![],
            "tools/list" => reply(
                m,
                json!({"tools": [{"name": "t", "inputSchema": {"type": "object"}}]}),
            ),
            "tools/call" => reply(
                m,
                json!({"content": [{"type": "text", "text": "legacy ok"}]}),
            ),
            _ => vec![],
        })
    })
}

fn config(protocol: ProtocolMode) -> McpServerConfig {
    let limits = McpLimits {
        connect_timeout_ms: 3000,
        request_timeout_ms: 1000,
        max_total_timeout_ms: 2000,
        ..Default::default()
    };
    McpServerConfig::stdio("scripted", "unused", &[])
        .with_protocol(protocol)
        .with_limits(limits)
}

#[tokio::test]
async fn auto_falls_back_to_initialize_for_a_legacy_server_and_modern_refuses_it() {
    let (io, log) = scripted(legacy_server());
    let c = McpClient::connect_with(config(ProtocolMode::Auto), io)
        .await
        .unwrap();
    assert_eq!(c.info().era, Era::Legacy);
    assert_eq!(c.info().protocol_version, "2025-11-25");
    assert_eq!(c.info().server_name.as_deref(), Some("old"));
    let r = c.call_tool("t", None, None).await.unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "legacy ok".into()
        }]
    );
    let methods: Vec<String> = log.lock().iter().map(|m| method(m).to_string()).collect();
    assert_eq!(
        methods,
        [
            "server/discover",
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/call"
        ]
    );
    // Legacy requests carry no per-request protocol metadata.
    let call = log
        .lock()
        .iter()
        .find(|m| method(m) == "tools/call")
        .cloned()
        .unwrap();
    assert!(
        call["params"]["_meta"]
            .get("io.modelcontextprotocol/protocolVersion")
            .is_none(),
        "{call}"
    );

    let (io, _) = scripted(legacy_server());
    match McpClient::connect_with(config(ProtocolMode::Modern), io).await {
        Err(McpError::Connect { .. }) => {}
        Err(other) => panic!("expected a connection refusal: {other:?}"),
        Ok(_) => panic!("a modern-only client must not accept a legacy server"),
    }
}

fn modern(mut on_call: impl FnMut(&Value) -> Option<Vec<Value>> + Send + 'static) -> Script {
    Box::new(move |m| match method(m) {
        "server/discover" => Some(reply(m, discover())),
        "tools/list" => Some(reply(m, tool_list())),
        "tools/call" => on_call(m),
        _ => Some(vec![]),
    })
}

#[tokio::test]
async fn modern_requests_carry_their_metadata() {
    let (io, log) = scripted(modern(|m| {
        Some(reply(
            m,
            json!({"resultType": "complete", "content": [{"type": "text", "text": "ok"}]}),
        ))
    }));
    let c = McpClient::connect_with(config(ProtocolMode::Auto), io)
        .await
        .unwrap();
    assert_eq!(c.info().era, Era::Modern);
    c.call_tool("t", Some(json!({"x": 1})), None).await.unwrap();
    let call = log
        .lock()
        .iter()
        .find(|m| method(m) == "tools/call")
        .cloned()
        .unwrap();
    let meta = &call["params"]["_meta"];
    assert_eq!(
        meta["io.modelcontextprotocol/protocolVersion"], "2026-07-28",
        "{call}"
    );
    assert!(
        meta.get("io.modelcontextprotocol/clientCapabilities")
            .is_some(),
        "{call}"
    );
    assert_eq!(meta["io.modelcontextprotocol/clientInfo"]["name"], "bricks");
    // No capability is declared that Bricks would not handle.
    assert_eq!(
        meta["io.modelcontextprotocol/clientCapabilities"],
        json!({}),
        "{call}"
    );
    assert!(log.lock().iter().all(|m| method(m) != "initialize"));
}

#[tokio::test]
async fn input_required_rounds_echo_state_and_are_bounded() {
    let (io, log) = scripted(modern(|m| {
        let state = m["params"]
            .get("requestState")
            .and_then(Value::as_str)
            .map(str::to_string);
        Some(match state.as_deref() {
            None => reply(
                m,
                json!({"resultType": "input_required", "requestState": "s1"}),
            ),
            Some("s1") => reply(
                m,
                json!({"resultType": "input_required", "requestState": "s2"}),
            ),
            Some("s2") => reply(
                m,
                json!({"resultType": "complete", "content": [{"type": "text", "text": "after 2 rounds"}]}),
            ),
            _ => error(m, -32602, "bad state"),
        })
    }));
    let c = McpClient::connect_with(config(ProtocolMode::Auto), io)
        .await
        .unwrap();
    let r = c.call_tool("t", None, None).await.unwrap();
    assert_eq!(r.rounds, 2);
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "after 2 rounds".into()
        }]
    );
    let calls: Vec<Value> = log
        .lock()
        .iter()
        .filter(|m| method(m) == "tools/call")
        .cloned()
        .collect();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1]["params"]["requestState"], "s1");
    assert_eq!(calls[2]["params"]["requestState"], "s2");
    let ids: std::collections::HashSet<String> =
        calls.iter().map(|c| c["id"].to_string()).collect();
    assert_eq!(ids.len(), 3, "each retry is a new request");

    // A server that never completes: bounded.
    let (io, log) = scripted(modern(|m| {
        Some(reply(
            m,
            json!({"resultType": "input_required", "requestState": "again"}),
        ))
    }));
    let c = McpClient::connect_with(config(ProtocolMode::Auto), io)
        .await
        .unwrap();
    assert!(matches!(
        c.call_tool("t", None, None).await,
        Err(McpError::RoundsExceeded(4))
    ));
    assert_eq!(
        log.lock()
            .iter()
            .filter(|m| method(m) == "tools/call")
            .count(),
        5
    );

    // Input Bricks did not declare is refused, without a retry.
    let (io, log) = scripted(modern(|m| {
        Some(reply(
            m,
            json!({"resultType": "input_required", "inputRequests": {
            "who": {"method": "elicitation/create", "params": {"mode": "form", "message": "Name?",
                     "requestedSchema": {"type": "object", "properties": {}}}}}}),
        ))
    }));
    let c = McpClient::connect_with(config(ProtocolMode::Auto), io)
        .await
        .unwrap();
    match c.call_tool("t", None, None).await {
        Err(McpError::InputNotSupported(m)) => assert_eq!(m, "elicitation/create"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        log.lock()
            .iter()
            .filter(|m| method(m) == "tools/call")
            .count(),
        1
    );
}

#[tokio::test]
async fn malformed_answers_fail_the_call_not_the_client() {
    let (io, _) = scripted(modern(|m| {
        if m["params"]["name"] == "t" && m["params"]["arguments"]["bad"] == true {
            Some(vec![
                json!({"jsonrpc": "2.0", "id": m["id"], "result": "not an object"}),
            ])
        } else {
            Some(reply(
                m,
                json!({"resultType": "complete", "content": [{"type": "text", "text": "fine"}]}),
            ))
        }
    }));
    let c = McpClient::connect_with(config(ProtocolMode::Auto), io)
        .await
        .unwrap();
    let r = c.call_tool("t", Some(json!({"bad": true})), None).await;
    assert!(r.is_err(), "{r:?}");
    println!("malformed answer → {}", r.unwrap_err());
    let ok = c.call_tool("t", None, None).await.unwrap();
    assert_eq!(
        ok.content,
        vec![McpContent::Text {
            text: "fine".into()
        }]
    );
}

#[tokio::test]
async fn a_connection_lost_during_a_call_is_reported_and_not_replayed() {
    let (io, log) = scripted(modern(|_| None));
    let c = McpClient::connect_with(config(ProtocolMode::Auto), io)
        .await
        .unwrap();
    match c.call_tool("t", None, None).await {
        Err(
            e @ McpError::ConnectionLost {
                in_flight: true, ..
            },
        ) => {
            assert!(e.to_string().contains("not retried"), "{e}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        log.lock()
            .iter()
            .filter(|m| method(m) == "tools/call")
            .count(),
        1
    );
    assert!(matches!(
        c.call_tool("t", None, None).await,
        Err(McpError::ConnectionLost {
            in_flight: false,
            ..
        })
    ));
}

// ─── A real stdio process ────────────────────────────────────────────────────

#[cfg(unix)]
const LEGACY_SH: &str = r#"
# A legacy (2025-11-25) MCP server in bash: answers by method, logs to stderr.
calls="$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  echo "got $method" >&2
  case "$method" in
    server/discover) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"Method not found"}}\n' "$id" ;;
    initialize) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"bash","version":"1"}}}\n' "$id" ;;
    tools/list) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}},{"name":"die","inputSchema":{"type":"object"}},{"name":"big","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    tools/call)
      name=$(printf '%s' "$line" | sed -n 's/.*"name":"\([^"]*\)".*/\1/p')
      echo "$name" >> "$calls"
      case "$name" in
        die) echo "fatal: exiting" >&2; exit 3 ;;
        big) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"' "$id"; head -c 9000 /dev/zero | tr '\0' 'x'; printf '"}]}}\n' ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"bash ok"}]}}\n' "$id" ;;
      esac ;;
  esac
done
"#;

#[cfg(unix)]
#[tokio::test]
async fn a_real_stdio_server_logs_on_stderr_dies_and_is_restarted_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.sh");
    std::fs::write(&script, LEGACY_SH).unwrap();
    let calls = dir.path().join("calls.log");
    let mut cfg = McpServerConfig::stdio(
        "bash",
        "bash",
        &[script.to_str().unwrap(), calls.to_str().unwrap()],
    );
    cfg.limits.max_message_bytes = 4096;
    cfg.limits.connect_timeout_ms = 5000;
    let m = McpManager::connect(std::slice::from_ref(&cfg)).await;
    let info = &m.infos().await[0];
    assert_eq!(info.era, Era::Legacy);
    assert_eq!(info.transport, "stdio");

    let r = m.call("bash", "echo", None, None).await.unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "bash ok".into()
        }]
    );
    // stderr is the server's log, not an error.
    let client = m.client("bash").await.unwrap();
    assert!(
        client.stderr().iter().any(|l| l == "got tools/call"),
        "{:?}",
        client.stderr()
    );

    // The process dies during a call: reported, not replayed.
    match m.call("bash", "die", None, None).await {
        Err(
            e @ McpError::ConnectionLost {
                in_flight: true, ..
            },
        ) => {
            println!("{e}");
        }
        other => panic!("{other:?}"),
    }
    // The next call starts a new process; the dead call was not re-sent.
    let r = m.call("bash", "echo", None, None).await.unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "bash ok".into()
        }]
    );
    let log = std::fs::read_to_string(&calls).unwrap();
    assert_eq!(log.lines().collect::<Vec<_>>(), ["echo", "die", "echo"]);

    // A message over the limit closes the connection, with the reason.
    match m.call("bash", "big", None, None).await {
        Err(e @ McpError::ConnectionLost { .. }) => {
            assert!(e.to_string().contains("longer than 4096 bytes"), "{e}");
        }
        other => panic!("{other:?}"),
    }
    m.close().await;
}

// ─── Streamable HTTP ─────────────────────────────────────────────────────────

fn rpc(req: &cersei_testkit::Request) -> Value {
    serde_json::from_slice(&req.body).unwrap_or(Value::Null)
}

fn sse(events: &[Value]) -> Reply {
    let mut r = Reply::new(200).header("content-type", "text/event-stream");
    for e in events {
        r = r.chunk(
            Duration::from_millis(20),
            format!("event: message\ndata: {e}\n\n").into_bytes(),
        );
    }
    r.length(cersei_testkit::Length::None)
}

#[tokio::test]
async fn streamable_http_modern_with_headers_and_a_progress_stream() {
    let server = TestServer::start(|req| async move {
        if req.method != "POST" {
            return Reply::new(405);
        }
        let m = rpc(&req);
        let id = m["id"].clone();
        match method(&m) {
            "server/discover" => Reply::json(200, &json!({"jsonrpc": "2.0", "id": id, "result": discover()}).to_string()),
            "tools/list" => Reply::json(200, &json!({"jsonrpc": "2.0", "id": id, "result": tool_list()}).to_string()),
            "tools/call" => {
                let token = m["params"]["_meta"]["progressToken"].clone();
                sse(&[
                    json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": token, "progress": 1, "total": 2, "message": "half"}}),
                    json!({"jsonrpc": "2.0", "id": id, "result": {"resultType": "complete", "content": [{"type": "text", "text": "streamed"}], "structuredContent": {"n": 2}}}),
                ])
            }
            _ => Reply::new(202),
        }
    })
    .await;
    let cfg = McpServerConfig::http("web", server.url("/mcp"));
    let c = McpClient::connect(cfg).await.unwrap();
    assert_eq!(c.info().era, Era::Modern);
    assert_eq!(c.info().transport, "streamable_http");
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let r = c
        .call_tool(
            "t",
            Some(json!({"q": 1})),
            Some(Arc::new(move |p, _, m| s2.lock().push((p, m)))),
        )
        .await
        .unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "streamed".into()
        }]
    );
    assert_eq!(r.structured, Some(json!({"n": 2})));
    assert_eq!(seen.lock().clone(), vec![(1.0, Some("half".to_string()))]);
    let call = server
        .requests()
        .into_iter()
        .find(|r| rpc(r)["method"] == "tools/call")
        .unwrap();
    assert_eq!(call.header("mcp-protocol-version"), Some("2026-07-28"));
    assert_eq!(call.header("mcp-method"), Some("tools/call"));
    assert_eq!(call.header("mcp-name"), Some("t"));
    assert!(call.header("accept").unwrap().contains("text/event-stream"));
    assert!(server
        .requests()
        .iter()
        .all(|r| r.header("mcp-session-id").is_none()));
}

#[tokio::test]
async fn streamable_http_falls_back_to_a_2025_11_25_session() {
    let server = TestServer::start(|req| async move {
        match req.method.as_str() {
            "GET" => return Reply::new(405),
            "DELETE" => return Reply::new(200),
            _ => {}
        }
        let m = rpc(&req);
        let id = m["id"].clone();
        match method(&m) {
            // No discovery: a plain 400 the client must read as "legacy".
            "server/discover" => Reply::text(400, "text/plain", "Bad Request: no valid session"),
            "initialize" => Reply::json(200, &json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": "2025-11-25", "capabilities": {"tools": {}}, "serverInfo": {"name": "sess", "version": "1"}}}).to_string())
                .header("mcp-session-id", "sess-42"),
            "notifications/initialized" => Reply::new(202),
            _ if req.header("mcp-session-id") != Some("sess-42") => Reply::text(400, "text/plain", "missing session"),
            "tools/list" => Reply::json(200, &json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "t", "inputSchema": {"type": "object"}}]}}).to_string()),
            "tools/call" => Reply::json(200, &json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "session ok"}]}}).to_string()),
            _ => Reply::new(202),
        }
    })
    .await;
    let c = McpClient::connect(McpServerConfig::http("old-web", server.url("/mcp")))
        .await
        .unwrap();
    assert_eq!(c.info().era, Era::Legacy);
    assert_eq!(c.info().protocol_version, "2025-11-25");
    let r = c.call_tool("t", None, None).await.unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "session ok".into()
        }]
    );
    c.close().await;
}

#[tokio::test]
async fn streamable_http_messages_are_bounded() {
    let server = TestServer::start(|req| async move {
        let m = rpc(&req);
        let id = m["id"].clone();
        match method(&m) {
            "server/discover" => Reply::json(
                200,
                &json!({"jsonrpc": "2.0", "id": id, "result": discover()}).to_string(),
            ),
            "tools/list" => Reply::json(
                200,
                &json!({"jsonrpc": "2.0", "id": id, "result": tool_list()}).to_string(),
            ),
            "tools/call" => Reply::json(
                200,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"resultType": "complete",
                "content": [{"type": "text", "text": "x".repeat(20_000)}]}})
                .to_string(),
            ),
            _ => Reply::new(202),
        }
    })
    .await;
    let mut cfg = McpServerConfig::http("big", server.url("/mcp"));
    cfg.limits.max_message_bytes = 8192;
    cfg.limits.request_timeout_ms = 2000;
    let c = McpClient::connect(cfg).await.unwrap();
    let r = c.call_tool("t", None, None).await;
    let e = r.unwrap_err();
    println!("oversized HTTP answer → {e}");
    assert!(e.to_string().contains("8192"), "{e}");
}

#[test]
fn the_deprecated_http_sse_transport_is_refused_with_a_reason() {
    let mut c = McpServerConfig::http("old", "https://mcp.example.com/sse");
    c.server_type = "sse".into();
    let e = c.validate().unwrap_err();
    assert!(
        e.contains("2024-11-05") && e.contains("not supported"),
        "{e}"
    );
}
