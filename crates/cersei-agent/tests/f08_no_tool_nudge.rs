//! F-08, revised: a final answer ends the run.
//!
//! The runner used to answer a prose-only first turn with a synthetic
//! "[system] You answered without using any tools" message and force
//! `tool_choice: required` on the retry, for every session. Tools being
//! available is not an obligation to use them: a conversational answer is a
//! legitimate final answer. These drive the real runner against a scripted
//! SSE socket (the `p0_wiring.rs` harness pattern) and assert on the literal
//! request bodies: the number of requests, and what each one carries.

use async_trait::async_trait;
use cersei_agent::Agent;
mod common;
use cersei_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

// ─── Canned SSE server (recording variant, as in p0_wiring.rs) ───────────────

struct Canned {
    body: String,
}

impl Canned {
    /// A complete SSE stream saying `text`, terminated properly.
    fn sse_text(text: &str) -> Self {
        let first = json!({
            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": text } }]
        });
        let last = json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        });
        Canned {
            body: format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n"),
        }
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn drain_http_request(sock: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        match sock.read(&mut tmp) {
            Ok(0) | Err(_) => return String::new(),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if let Some(p) = find_subslice(&buf, b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
    let content_length = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        match sock.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    String::from_utf8_lossy(&buf[header_end..]).to_string()
}

/// Serve `responses` in order, recording every request body. Exhausted scripts
/// answer 500 so an unexpected extra request fails the test loudly.
fn serve_recording(responses: Vec<Canned>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let recorder = bodies.clone();

    std::thread::spawn(move || {
        let mut scripted = responses.into_iter();
        while let Ok((mut sock, _)) = listener.accept() {
            let body = drain_http_request(&mut sock);
            recorder.lock().unwrap().push(body);
            let payload = match scripted.next() {
                Some(c) => format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    c.body.len(),
                    c.body
                ),
                None => "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\
                         Connection: close\r\n\r\n"
                    .to_string(),
            };
            let _ = sock.write_all(payload.as_bytes());
            let _ = sock.flush();
            let _ = sock.shutdown(std::net::Shutdown::Write);
        }
    });

    (format!("http://127.0.0.1:{port}/v1"), bodies)
}

fn provider_against(base_url: &str, model: &str) -> cersei_provider::ConfiguredProvider {
    // "gpt-4" used to resolve to a small 8_192-token window through a model-name
    // table; the window is now configuration, so the test states it.
    let window = if model == "gpt-4" { 8_192 } else { 128_000 };
    common::provider(base_url, "chat_completions", window)
}

/// A trivial registered tool, so `tools_available` is true.
struct PingTool;

#[async_trait]
impl Tool for PingTool {
    fn name(&self) -> &str {
        "Ping"
    }
    fn description(&self) -> &str {
        "Replies with pong."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn execute(&self, _input: Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::success("pong")
    }
}

// ─── Cases ───────────────────────────────────────────────────────────────────

fn agent_with_ping(url: &str) -> Agent {
    Agent::builder()
        .provider(provider_against(url, "gpt-4o"))
        .tool(PingTool)
        .model("gpt-4o")
        .max_turns(6)
        .max_tokens(64)
        .build()
        .expect("build agent")
}

/// Turn 1 is prose and tools exist: that answer is final. One request, no
/// nudge, no forced tool choice.
#[tokio::test]
async fn prose_answer_with_tools_available_is_final() {
    let (url, bodies) = serve_recording(vec![Canned::sse_text(
        "Bonjour ! Comment puis-je t'aider ?",
    )]);
    let out = agent_with_ping(&url)
        .run("Salut")
        .await
        .expect("run must complete");
    assert!(out.is_complete(), "{:?}", out.termination);
    assert_eq!(out.turns, 1);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1, "a final answer is not relaunched");
    assert!(!bodies[0].contains("tool_choice"), "{}", bodies[0]);
}

/// Without tools, a prose answer is the only possible answer. One request.
#[tokio::test]
async fn prose_answer_without_tools_is_final() {
    let (url, bodies) = serve_recording(vec![Canned::sse_text("Hello!")]);

    let agent = Agent::builder()
        .provider(provider_against(&url, "gpt-4o"))
        .model("gpt-4o")
        .max_turns(6)
        .max_tokens(64)
        .build()
        .expect("build agent");

    agent.run("hi").await.expect("run must complete");

    assert_eq!(bodies.lock().unwrap().len(), 1);
}

/// Two runs in a row on one agent: each is one request, and nothing from
/// the first (a nudge, a forced choice) is carried into the second.
#[tokio::test]
async fn successive_runs_carry_no_stale_nudge() {
    let (url, bodies) = serve_recording(vec![
        Canned::sse_text("Premier."),
        Canned::sse_text("Second."),
    ]);
    let agent = agent_with_ping(&url);
    agent.run("un").await.unwrap();
    let out = agent.reply("deux").await.unwrap();
    assert_eq!(out.text(), "Second.");
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    for b in bodies.iter() {
        assert!(!b.contains("tool_choice"), "{b}");
        assert!(!b.contains("[system]"), "{b}");
    }
}

/// `finish_reason: "tool_calls"` with no call in the response: the text is
/// the answer. Never an empty round that would relaunch the model.
#[tokio::test]
async fn tool_calls_reason_without_a_call_is_an_answer() {
    let first = json!({
        "choices": [{ "index": 0, "delta": { "role": "assistant", "content": "Fait." } }]
    });
    let last = json!({
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
    });
    let (url, bodies) = serve_recording(vec![Canned {
        body: format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n"),
    }]);
    let out = agent_with_ping(&url).run("go").await.unwrap();
    assert!(out.is_complete());
    assert_eq!(out.text(), "Fait.");
    assert_eq!(bodies.lock().unwrap().len(), 1);
}

/// An answer cut by the output limit is continued, visibly, a bounded
/// number of times; the continuation names its cause.
#[tokio::test]
async fn a_cut_answer_is_continued_then_ends() {
    let cut = |text: &str| {
        let first = json!({
            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": text } }]
        });
        let last = json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 64, "total_tokens": 74 }
        });
        Canned {
            body: format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n"),
        }
    };
    let (url, bodies) = serve_recording(vec![cut("Début"), Canned::sse_text(" et fin.")]);
    let out = agent_with_ping(&url).run("écris").await.unwrap();
    assert!(out.is_complete());
    assert_eq!(out.turns, 2);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies[1].contains("cut by the output-token limit"),
        "{}",
        bodies[1]
    );
}
