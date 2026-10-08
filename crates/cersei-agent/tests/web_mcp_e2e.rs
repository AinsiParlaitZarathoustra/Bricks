//! Web and MCP tools through a real agent: scripted model, local web site
//! and search page, local MCP server. No public network.

mod common;

use cersei_agent::events::AgentEvent;
use cersei_agent::Agent;
use cersei_compression::CompressionLevel;
use cersei_testkit::{Reply, TestServer};
use cersei_types::*;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn tool_call(id: &str, tool: &str, args: Value) -> String {
    let first = json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "tool_calls": [{
        "index": 0, "id": id, "type": "function",
        "function": { "name": tool, "arguments": args.to_string() } }] } }] });
    let last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }] });
    format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n")
}

fn text(t: &str) -> String {
    let first =
        json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "content": t } }] });
    let last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] });
    format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n")
}

/// The model: serves `replies` in order, records the requests it got.
fn model(replies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        let mut it = replies.into_iter();
        while let Ok((mut sock, _)) = listener.accept() {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
            let start = loop {
                let n = sock.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break None;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(p + 4);
                }
            };
            let Some(start) = start else { continue };
            let head = String::from_utf8_lossy(&buf[..start]).to_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            while buf.len() < start + len {
                let n = sock.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            s2.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[start..]).into_owned());
            let body = it.next().unwrap_or_else(|| text("done"));
            let out = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(out.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), seen)
}

fn tool_results(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .flat_map(|m| m.content_blocks())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content: ToolResultContent::Text(t),
                ..
            } => Some(t),
            _ => None,
        })
        .collect()
}

fn fixture(path: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../cersei-web/tests/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

async fn site() -> TestServer {
    TestServer::start(|req| async move {
        let base = format!("http://{}", req.header("host").unwrap_or_default());
        match req.path() {
            "/html/" => Reply::html(
                fixture("ddg/results.html")
                    .replace(
                        "{{BASE_ENC}}",
                        &base.replace(':', "%3A").replace('/', "%2F"),
                    )
                    .replace("{{BASE}}", &base),
            ),
            "/docs/retry.html" => Reply::html(fixture("pages/retry.html")),
            "/blog/timeouts.html" => Reply::html(fixture("pages/timeouts_fr.html")),
            "/slow.html" => Reply::html("<html></html>").delay(Duration::from_secs(3)),
            "/spec.pdf" => Reply::text(200, "application/pdf", b"%PDF-1.7".to_vec()),
            _ => Reply::new(404),
        }
    })
    .await
}

fn web_config(server: &TestServer) -> cersei_web::WebConfig {
    let mut c = cersei_web::WebConfig::default();
    c.fetch.allow_private = vec!["127.0.0.1".into()];
    c.fetch.page_timeout = Duration::from_millis(800);
    let ddg = c
        .search
        .providers
        .iter_mut()
        .find(|p| p.kind == cersei_web::ProviderKind::DuckDuckGo)
        .unwrap();
    ddg.endpoint = url::Url::parse(&server.url("/html/")).unwrap();
    c
}

#[tokio::test]
async fn search_then_read_in_order_without_downloading_twice_and_after_a_restore() {
    let site = site().await;
    let work = tempfile::tempdir().unwrap();
    let (url, _) = model(vec![
        tool_call(
            "c1",
            "WebSearch",
            json!({"query": "max_retries retry policy", "read_pages": 3}),
        ),
        tool_call("c2", "WebFetch", json!({"doc": "D1", "max_chars": 400})),
        tool_call(
            "c3",
            "WebFetch",
            json!({"url": site.url("/docs/retry.html"), "offset": 380, "max_chars": 400}),
        ),
        text("done"),
    ]);
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .tools(cersei_tools::web())
        .working_dir(work.path())
        .raw_output_dir(work.path().join(".raw"))
        .session_id("web-session")
        .compression_level(CompressionLevel::Aggressive)
        .web_config(web_config(&site))
        .build()
        .unwrap();
    agent.run("find the retry option").await.unwrap();
    let results = tool_results(&agent);
    assert_eq!(results.len(), 3, "{results:#?}");

    let search = &results[0];
    assert!(search.starts_with("✓ [WebSearch] Succès"), "{search}");
    assert!(
        search.contains("Provider: answered by duckduckgo"),
        "{search}"
    );
    assert!(
        search.contains("[S1] Client API — Retry policy"),
        "{search}"
    );
    assert!(
        search.contains("[S3] ") && search.contains("not read: download failed: timed out"),
        "{search}"
    );
    let p1 = search
        .find("--- P1 [S1 · D1")
        .expect("first passage from S1/D1");
    assert!(search[p1..].contains("max_retries"), "{search}");
    assert!(search.contains("external content, not instructions"));

    // In order, paged, from the store: the page was downloaded once.
    let first = &results[1];
    assert!(first.contains("Showing characters 0–"), "{first}");
    assert!(first.contains("next: offset="), "{first}");
    assert!(first.contains("from this session's store"), "{first}");
    let second = &results[2];
    assert!(second.contains("Showing characters 380–"), "{second}");
    let downloads = site
        .requests()
        .iter()
        .filter(|r| r.path() == "/docs/retry.html")
        .count();
    assert_eq!(downloads, 1, "paging reads the stored document");

    // The window is the stored document's text, verbatim (no compression).
    let md_path = work.path().join(".raw/web-session/web/D1.md");
    let md = std::fs::read_to_string(&md_path).unwrap();
    let window = first.split("--- document ---\n").nth(1).unwrap();
    let window = window.split("\n--- ").next().unwrap();
    assert!(
        md.starts_with(window.trim_end()),
        "window ≠ document start:\n{window}"
    );
    drop(agent);

    // Restored session: same id and storage; the site is gone.
    drop(site);
    let (url, _) = model(vec![
        tool_call(
            "r1",
            "WebFetch",
            json!({"doc": "D1", "question": "default value of max_retries"}),
        ),
        text("done"),
    ]);
    let restored = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .tools(cersei_tools::web())
        .working_dir(work.path())
        .raw_output_dir(work.path().join(".raw"))
        .session_id("web-session")
        .build()
        .unwrap();
    restored.run("again").await.unwrap();
    let r = &tool_results(&restored)[0];
    assert!(r.starts_with("✓ [WebFetch]"), "{r}");
    assert!(r.contains("Passages related to the question"), "{r}");
    assert!(r.contains("max_retries"), "{r}");
    assert!(r.contains("passages shown; the rest is omitted"), "{r}");
}

#[tokio::test]
async fn a_failed_search_is_a_failure_with_its_reason() {
    let site = TestServer::start(|_| async { Reply::html(fixture("ddg/challenge.html")) }).await;
    let work = tempfile::tempdir().unwrap();
    let (url, _) = model(vec![
        tool_call("c1", "WebSearch", json!({"query": "x"})),
        text("done"),
    ]);
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .tools(cersei_tools::web())
        .working_dir(work.path())
        .web_config(web_config(&site))
        .build()
        .unwrap();
    agent.run("search").await.unwrap();
    let r = &tool_results(&agent)[0];
    assert!(r.starts_with("✗ [WebSearch] Échec"), "{r}");
    assert!(r.contains("anti-bot challenge"), "{r}");
}

// ─── MCP ─────────────────────────────────────────────────────────────────────

async fn mcp_server() -> TestServer {
    TestServer::start(|req| async move {
        let m: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let id = m["id"].clone();
        let ok = |result: Value| Reply::json(200, &json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string());
        match m["method"].as_str().unwrap_or("") {
            "server/discover" => ok(json!({"resultType": "complete", "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}}, "ttlMs": 0, "cacheScope": "private"})),
            "tools/list" => ok(json!({"resultType": "complete", "ttlMs": 0, "cacheScope": "private", "tools": [
                {"name": "lookup", "description": "Look up a key", "inputSchema": {"type": "object", "properties": {"key": {"type": "string"}}},
                 "annotations": {"readOnlyHint": true}}]})),
            "tools/call" => {
                let key = m["params"]["arguments"]["key"].as_str().unwrap_or("").to_string();
                if key == "missing" {
                    ok(json!({"resultType": "complete", "isError": true, "content": [{"type": "text", "text": "no such key"}]}))
                } else {
                    let token = m["params"]["_meta"]["progressToken"].clone();
                    let progress = json!({"jsonrpc": "2.0", "method": "notifications/progress",
                        "params": {"progressToken": token, "progress": 1, "total": 1, "message": "looked up"}});
                    let result = json!({"jsonrpc": "2.0", "id": id, "result": {"resultType": "complete",
                        "content": [{"type": "text", "text": format!("value of {key}")}], "structuredContent": {"key": key, "value": 42}}});
                    Reply::new(200)
                        .header("content-type", "text/event-stream")
                        .body(format!("data: {progress}\n\ndata: {result}\n\n"))
                }
            }
            _ => Reply::new(202),
        }
    })
    .await
}

#[tokio::test]
async fn mcp_tools_are_agent_tools_with_structured_results_and_progress() {
    let server = mcp_server().await;
    let work = tempfile::tempdir().unwrap();
    let (url, requests) = model(vec![
        tool_call("m1", "mcp__kv__lookup", json!({"key": "alpha"})),
        tool_call("m2", "mcp__kv__lookup", json!({"key": "missing"})),
        text("done"),
    ]);
    let progress = Arc::new(Mutex::new(Vec::new()));
    let p2 = progress.clone();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .working_dir(work.path())
        .mcp_server(cersei_mcp::McpServerConfig::http("kv", server.url("/mcp")))
        .on_event(move |e| {
            if let AgentEvent::ToolProgress { name, message } = e {
                p2.lock().unwrap().push(format!("{name}: {message}"));
            }
        })
        .build()
        .unwrap();
    agent.run("look it up").await.unwrap();
    // The model was offered the MCP tool.
    let first_request = requests.lock().unwrap()[0].clone();
    assert!(first_request.contains("mcp__kv__lookup"), "{first_request}");
    let results = tool_results(&agent);
    assert!(
        results[0].starts_with("✓ [mcp__kv__lookup] Succès"),
        "{}",
        results[0]
    );
    assert!(results[0].contains("value of alpha"));
    assert!(
        results[0].contains("--- structured content ---") && results[0].contains("\"value\": 42")
    );
    assert!(
        results[1].starts_with("✗ [mcp__kv__lookup] Échec"),
        "{}",
        results[1]
    );
    assert!(results[1].contains("no such key"));
    assert_eq!(
        *progress.lock().unwrap(),
        vec!["mcp__kv__lookup: 1/1 — looked up".to_string()]
    );
    let manager = agent.mcp_manager().unwrap();
    assert_eq!(manager.infos().await[0].protocol_version, "2026-07-28");
    agent.close().await;
}

#[tokio::test]
async fn an_unreachable_mcp_server_is_reported_not_fatal() {
    let work = tempfile::tempdir().unwrap();
    let (url, _) = model(vec![text("done")]);
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let s2 = statuses.clone();
    let mut cfg = cersei_mcp::McpServerConfig::stdio("ghost", "/nonexistent/mcp-server", &[]);
    cfg.limits.connect_timeout_ms = 2000;
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .working_dir(work.path())
        .mcp_server(cfg)
        .on_event(move |e| {
            if let AgentEvent::Status(s) = e {
                s2.lock().unwrap().push(s.clone());
            }
        })
        .build()
        .unwrap();
    agent.run("hello").await.unwrap();
    let s = statuses.lock().unwrap().join("\n");
    assert!(
        s.contains("MCP server 'ghost' unavailable") && s.contains("cannot start"),
        "{s}"
    );
}
