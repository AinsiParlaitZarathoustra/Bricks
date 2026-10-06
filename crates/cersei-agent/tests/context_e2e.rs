//! Context management through the real runner, against scripted servers.
//! No network beyond 127.0.0.1, no paid call.

mod common;

use cersei_agent::events::AgentEvent;
use cersei_agent::{Agent, CompactionOutcome, ContextPolicy, Provenance};
use cersei_compression::CompressionLevel;
use cersei_provider::ProviderRegistry;
use cersei_types::*;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

// ─── Scripted HTTP server ────────────────────────────────────────────────────

struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl Reply {
    fn sse(body: String) -> Self {
        Reply {
            status: 200,
            content_type: "text/event-stream",
            body,
        }
    }
    fn json(status: u16, body: Value) -> Self {
        Reply {
            status,
            content_type: "application/json",
            body: body.to_string(),
        }
    }
}

/// Chat Completions stream saying `text`, with `usage` when given.
fn chat_text(text: &str, usage: Option<Value>) -> Reply {
    let first =
        json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "content": text } }] });
    let mut last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] });
    if let Some(u) = usage {
        last["usage"] = u;
    }
    Reply::sse(format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n"))
}

fn chat_tool_call(id: &str, tool: &str, args: &Value) -> Reply {
    let first = json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "tool_calls": [{
        "index": 0, "id": id, "type": "function",
        "function": { "name": tool, "arguments": args.to_string() } }] } }] });
    let last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
        "usage": { "prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110 } });
    Reply::sse(format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n"))
}

fn anthropic_text(text: &str, input_tokens: u64) -> Reply {
    let events = [
        (
            "message_start",
            json!({ "type": "message_start", "message": { "id": "m1", "type": "message", "role": "assistant", "model": "x", "content": [], "usage": { "input_tokens": input_tokens, "output_tokens": 1 } } }),
        ),
        (
            "content_block_start",
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
        ),
        (
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "message_delta",
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 7 } }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ];
    Reply::sse(
        events
            .iter()
            .map(|(e, d)| format!("event: {e}\ndata: {d}\n\n"))
            .collect(),
    )
}

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    body: String,
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn read_request(sock: &mut TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let end = loop {
        match sock.read(&mut tmp) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
    let len = head
        .to_lowercase()
        .lines()
        .find_map(|l| {
            l.strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < end + len {
        match sock.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    Some(Seen {
        path,
        body: String::from_utf8_lossy(&buf[end..]).to_string(),
    })
}

fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rec = seen.clone();
    std::thread::spawn(move || {
        let mut it = replies.into_iter();
        while let Ok((mut sock, _)) = listener.accept() {
            let Some(req) = read_request(&mut sock) else {
                continue;
            };
            rec.lock().unwrap().push(req);
            let reply = it.next().unwrap_or(Reply {
                status: 500,
                content_type: "text/plain",
                body: "script exhausted".into(),
            });
            let reason = if reply.status == 200 { "OK" } else { "Error" };
            let out = format!(
                "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.status,
                reply.content_type,
                reply.body.len(),
                reply.body
            );
            let _ = sock.write_all(out.as_bytes());
            let _ = sock.flush();
            let _ = sock.shutdown(std::net::Shutdown::Write);
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), seen)
}

fn events() -> (
    Arc<Mutex<Vec<AgentEvent>>>,
    impl Fn(&AgentEvent) + Send + Sync + 'static,
) {
    let store = Arc::new(Mutex::new(Vec::new()));
    let s = store.clone();
    (store, move |e: &AgentEvent| {
        s.lock().unwrap().push(e.clone())
    })
}

fn raw_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reported_usage_is_the_measured_base_and_totals_add_cache() {
    let (url, _) = serve(vec![chat_text(
        "hello",
        Some(
            json!({ "prompt_tokens": 1234, "completion_tokens": 50, "total_tokens": 1284,
                     "prompt_tokens_details": { "cached_tokens": 200 } }),
        ),
    )]);
    let (log, on_event) = events();
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .raw_output_dir(dir.path())
        .on_event(on_event)
        .build()
        .unwrap();
    agent.run("say hello").await.unwrap();

    let st = agent.context_status();
    assert_eq!(
        st.context_used.measured_tokens, 1234,
        "uncached + cached prompt"
    );
    assert_eq!(
        st.context_used.provenance,
        Provenance::Mixed,
        "the kept reply is added on top"
    );
    assert_eq!(
        st.context_used.estimated_tokens, 54,
        "reply output + framing, from its usage"
    );
    assert_eq!(st.totals.cache_read_tokens, 200);
    assert_eq!(st.total_tokens, 1034 + 200 + 50);
    assert_eq!(st.context_window.total, None);
    assert!(st
        .notes
        .iter()
        .any(|n| n.contains("no total context window")));
    assert!(log
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, AgentEvent::ContextUpdate(_))));
}

#[tokio::test]
async fn without_usage_the_occupation_stays_an_explicit_estimate() {
    let (url, _) = serve(vec![chat_text("hello", None)]);
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .raw_output_dir(dir.path())
        .build()
        .unwrap();
    agent.run("say hello").await.unwrap();
    let st = agent.context_status();
    assert_eq!(st.context_used.provenance, Provenance::Estimated);
    assert!(st.context_used.tokens > 0, "an estimate, never a fake zero");
    assert_eq!(st.totals.requests_without_usage, 1);
    assert_eq!(st.total_tokens, 0);
}

fn long_history() -> Vec<Message> {
    let mut v = Vec::new();
    for i in 0..12 {
        v.push(Message::user(format!(
            "step {i}: {}",
            "detail ".repeat(300)
        )));
        v.push(Message::assistant(format!(
            "done {i}: {}",
            "result ".repeat(300)
        )));
    }
    v
}

#[tokio::test]
async fn a_context_overflow_refusal_is_compacted_and_retried_once() {
    let (url, seen) = serve(vec![
        Reply::json(
            400,
            json!({ "error": { "code": "context_length_exceeded",
            "message": "This model's maximum context length is 8192 tokens." } }),
        ),
        chat_text(
            "SUMMARY: steps 0-7 done",
            Some(json!({ "prompt_tokens": 900, "completion_tokens": 40 })),
        ),
        chat_text(
            "final answer",
            Some(json!({ "prompt_tokens": 600, "completion_tokens": 5 })),
        ),
    ]);
    let (log, on_event) = events();
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .with_messages(long_history())
        .raw_output_dir(dir.path())
        .on_event(on_event)
        .build()
        .unwrap();
    let out = agent.run("continue").await.unwrap();
    assert_eq!(out.text(), "final answer");

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.len(),
        3,
        "refused request, summary call, retried request"
    );
    assert!(seen[1].body.contains("Transcript of the session so far"));
    assert!(seen[2].body.contains("SUMMARY: steps 0-7 done"));
    assert!(
        seen[2].body.contains("step 0: detail"),
        "the first user message is kept verbatim (shortened)"
    );
    assert!(
        seen[2].body.contains("continue"),
        "the prompt survives in the kept tail"
    );

    let st = agent.context_status();
    assert_eq!(st.totals.compaction_requests, 1);
    assert_eq!(
        st.total_tokens,
        900 + 40 + 600 + 5,
        "each executed call counted once"
    );
    let outcomes: Vec<_> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::CompactionResult { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .collect();
    assert!(
        matches!(outcomes[..], [CompactionOutcome::Compacted { .. }]),
        "{outcomes:?}"
    );
    assert_eq!(agent.raw_history().len(), long_history().len() + 2);
}

#[tokio::test]
async fn a_second_refusal_is_not_retried_in_a_loop() {
    let refusal = || {
        Reply::json(
            400,
            json!({ "error": { "message": "prompt is too long: 9000 tokens > 8192 maximum" } }),
        )
    };
    let (url, seen) = serve(vec![
        refusal(),
        chat_text(
            "SUMMARY",
            Some(json!({ "prompt_tokens": 900, "completion_tokens": 40 })),
        ),
        refusal(),
    ]);
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .with_messages(long_history())
        .raw_output_dir(dir.path())
        .build()
        .unwrap();
    let err = agent.run("continue").await.unwrap_err();
    assert!(err.is_context_overflow(), "{err}");
    assert_eq!(
        seen.lock().unwrap().len(),
        3,
        "one recovery per turn, no loop"
    );
}

#[tokio::test]
async fn a_request_that_cannot_fit_is_never_sent() {
    let (url, seen) = serve(vec![]);
    let (log, on_event) = events();
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 2_000))
        .raw_output_dir(dir.path())
        .on_event(on_event)
        .build()
        .unwrap();
    // One huge prompt: nothing older to compact.
    let err = agent.run(&"word ".repeat(10_000)).await.unwrap_err();
    assert!(matches!(err, CerseiError::ContextOverflow { .. }), "{err}");
    assert!(
        seen.lock().unwrap().is_empty(),
        "no request reached the server"
    );
    let log = log.lock().unwrap();
    assert!(log.iter().any(|e| matches!(
        e,
        AgentEvent::CompactionResult {
            outcome: CompactionOutcome::Skipped { .. },
            ..
        }
    )));
    assert!(log
        .iter()
        .any(|e| matches!(e, AgentEvent::Status(s) if s.contains("Request not sent"))));
}

#[tokio::test]
async fn the_output_reserve_counts_against_a_shared_window() {
    let toml = |endpoint: &str| {
        format!(
            r#"
schema_version = 1
[[providers]]
id = "w"
name = "W"
endpoint = "{endpoint}"
protocol = "chat_completions"
auth = "none"
[[providers.models]]
id = "m"
name = "M"
api_model = "m"
streaming = true
[providers.models.limits]
max_input_tokens = 8000
max_output_tokens = 6000
context_window_tokens = 8000
"#
        )
    };
    let (url, seen) = serve(vec![chat_text("ok", None)]);
    let provider = ProviderRegistry::from_toml_str(&toml(&url), "w.toml")
        .unwrap()
        .resolve("w/m")
        .unwrap()
        .provider()
        .build()
        .unwrap();
    let dir = raw_dir();
    // ~3k tokens of prompt: fits 8000 alone, not with 6000 reserved for output.
    let agent = Agent::builder()
        .provider(provider)
        .max_tokens(6_000)
        .auto_compact(false)
        .raw_output_dir(dir.path())
        .build()
        .unwrap();
    let err = agent.run(&"word ".repeat(2_500)).await.unwrap_err();
    assert!(
        matches!(err, CerseiError::ContextOverflow { limit, .. } if limit < 2_000),
        "{err}"
    );
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn preflight_uses_a_configured_counting_endpoint() {
    let (url, seen) = serve(vec![
        Reply::json(200, json!({ "input_tokens": 321 })),
        anthropic_text("counted", 321),
    ]);
    let toml = format!(
        r#"
schema_version = 1
[[providers]]
id = "a"
name = "A"
endpoint = "{url}"
protocol = "anthropic_messages"
auth = "api_key_header"
api_key_env = "BRICKS_TEST_API_KEY"
[[providers.models]]
id = "m"
name = "M"
api_model = "m"
streaming = true
[providers.models.limits]
max_input_tokens = 50000
max_output_tokens = 4000
[providers.models.token_counting]
"#
    );
    let provider = ProviderRegistry::from_toml_str(&toml, "a.toml")
        .unwrap()
        .resolve("a/m")
        .unwrap()
        .provider()
        .env(|_| Some("test-key".into()))
        .build()
        .unwrap();
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(provider)
        // Pre-flight on every request, to exercise the endpoint.
        .context_policy(ContextPolicy {
            preflight_ratio: 0.0,
            ..Default::default()
        })
        .raw_output_dir(dir.path())
        .build()
        .unwrap();
    agent.run("hello").await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].path, "/v1/messages/count_tokens");
    let count_body: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert!(count_body.get("max_tokens").is_none() && count_body.get("stream").is_none());
    assert_eq!(count_body["messages"][0]["role"], "user");
    assert_eq!(seen[1].path, "/v1/messages");
}

#[tokio::test]
async fn an_unconfigured_counting_route_is_never_called() {
    let (url, seen) = serve(vec![chat_text("ok", None)]);
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .context_policy(ContextPolicy {
            preflight_ratio: 0.0,
            ..Default::default()
        })
        .raw_output_dir(dir.path())
        .build()
        .unwrap();
    agent.run("hello").await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/chat/completions");
}

#[tokio::test]
async fn a_skeleton_view_does_not_count_as_having_read_the_file() {
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("lib.rs");
    let mut src = String::new();
    for f in 0..30 {
        src.push_str(&format!("/// Doc {f}.\npub fn f{f}(x: u32) -> u32 {{\n"));
        for l in 0..6 {
            src.push_str(&format!("    let v{l} = x + {l};\n"));
        }
        src.push_str("    x\n}\n\n");
    }
    std::fs::write(&file, &src).unwrap();
    let path = file.to_str().unwrap();
    let (url, _) = serve(vec![
        chat_tool_call("r1", "Read", &json!({ "file_path": path })),
        chat_tool_call(
            "e1",
            "Edit",
            &json!({ "file_path": path, "old_string": "let v3 = x + 3;", "new_string": "let v3 = 0;" }),
        ),
        chat_text("stopping", None),
        chat_text("stopping", None),
    ]);
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .tool(cersei_tools::file_read::FileReadTool)
        .tool(cersei_tools::file_edit::FileEditTool)
        .compression_level(CompressionLevel::Aggressive)
        .working_dir(work.path())
        .raw_output_dir(dir.path())
        .max_turns(4)
        .build()
        .unwrap();
    agent.run("edit the file").await.unwrap();

    let results: Vec<String> = agent
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
        .collect();
    // The uniform header first, then the compression header.
    let mut lines = results[0].lines();
    assert!(
        lines.next().unwrap().starts_with("✓ [Read] Succès"),
        "{}",
        &results[0][..200]
    );
    assert!(
        lines
            .next()
            .unwrap()
            .starts_with("[bricks: skeleton view of"),
        "{}",
        &results[0][..200]
    );
    assert!(
        results[1].contains("was not run"),
        "the edit must be refused: {}",
        results[1]
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        src,
        "nothing written"
    );
    // The raw history keeps the exact text the tool returned.
    let raw_read = agent
        .raw_history()
        .iter()
        .flat_map(|m| m.content_blocks())
        .find_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content: ToolResultContent::Text(t),
                ..
            } if tool_use_id == "r1" => Some(t),
            _ => None,
        });
    assert!(raw_read.unwrap().contains("let v3 = x + 3;"));
}

#[tokio::test]
async fn a_reduced_tool_output_can_be_read_back_from_the_store() {
    let mut log = String::new();
    for i in 0..400 {
        log.push_str(&format!("   Compiling crate{i} v0.1.0\n"));
        if i == 200 {
            log.push_str(
                "error[E0425]: cannot find value `zz` in this scope\n  --> src/a.rs:9:5\n",
            );
        }
    }
    struct Fake(String);
    #[async_trait::async_trait]
    impl cersei_tools::Tool for Fake {
        fn name(&self) -> &str {
            "Bash"
        }
        fn description(&self) -> &str {
            "fake shell"
        }
        fn permission_level(&self) -> cersei_tools::PermissionLevel {
            cersei_tools::PermissionLevel::None
        }
        fn category(&self) -> cersei_tools::ToolCategory {
            cersei_tools::ToolCategory::Shell
        }
        fn input_schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(
            &self,
            _: Value,
            _: &cersei_tools::ToolContext,
        ) -> cersei_tools::ToolResult {
            cersei_tools::ToolResult::error(format!("Exit code 101\n{}", self.0))
        }
    }
    let (url, _) = serve(vec![
        chat_tool_call("b1", "Bash", &json!({ "command": "cargo build" })),
        chat_text("seen", None),
        chat_text("seen", None),
    ]);
    let dir = raw_dir();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .tool(Fake(log.clone()))
        .compression_level(CompressionLevel::Minimal)
        .raw_output_dir(dir.path())
        .max_turns(3)
        .build()
        .unwrap();
    agent.run("build").await.unwrap();
    let active = agent
        .messages()
        .iter()
        .flat_map(|m| m.content_blocks())
        .find_map(|b| match b {
            ContentBlock::ToolResult {
                content: ToolResultContent::Text(t),
                ..
            } => Some(t),
            _ => None,
        })
        .unwrap();
    assert!(
        active.contains("error[E0425]: cannot find value `zz`"),
        "{active}"
    );
    assert!(active.contains("Exit code 101"));
    let path = active
        .split("Read file_path=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("the header names the saved original");
    assert!(path.starts_with(dir.path().to_str().unwrap()));
    // The original exactly as the tool returned it (with the runner's
    // repeated-failure note, which is part of that result).
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.starts_with(&format!("Exit code 101\n{log}")));
}

#[tokio::test]
async fn a_restored_session_brings_back_its_raw_history_and_is_estimated() {
    let mem_dir = tempfile::tempdir().unwrap();
    let (url, _) = serve(vec![
        chat_text(
            "first",
            Some(json!({ "prompt_tokens": 50, "completion_tokens": 2 })),
        ),
        chat_text(
            "second",
            Some(json!({ "prompt_tokens": 80, "completion_tokens": 2 })),
        ),
    ]);
    let dir = raw_dir();
    let build = || {
        Agent::builder()
            .provider(common::provider(&url, "chat_completions", 100_000))
            .memory(cersei_memory::JsonlMemory::new(mem_dir.path()))
            .session_id("s1")
            .raw_output_dir(dir.path())
            .build()
            .unwrap()
    };
    let a = build();
    a.run("one").await.unwrap();
    assert!(mem_dir.path().join("s1.raw.jsonl").exists());

    let b = build();
    // Before any request: restored history, nothing measured yet.
    b.run("two").await.unwrap();
    assert_eq!(b.raw_history().len(), 4);
    assert_eq!(b.raw_history()[0].get_text(), Some("one"));
}

#[tokio::test]
async fn a_different_model_starts_from_an_estimate() {
    let (url, _) = serve(vec![chat_text(
        "hi",
        Some(json!({ "prompt_tokens": 40, "completion_tokens": 2 })),
    )]);
    let dir = raw_dir();
    let a = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .raw_output_dir(dir.path())
        .build()
        .unwrap();
    a.run("hello").await.unwrap();
    assert_ne!(
        a.context_status().context_used.provenance,
        Provenance::Estimated
    );

    // Same history, another model (`responses` protocol here): the
    // measurement made with the first one does not carry over.
    let b = Agent::builder()
        .provider(common::provider(&url, "responses", 64_000))
        .with_messages(a.messages())
        .raw_output_dir(dir.path())
        .build()
        .unwrap();
    let st = b.context_status();
    assert_eq!(st.context_used.provenance, Provenance::Estimated);
    assert_eq!(st.context_window.max_input_tokens, 64_000);
}

/// A shell tool whose failing output is long enough to be reduced.
struct LongFailingBash(String);

#[async_trait::async_trait]
impl cersei_tools::Tool for LongFailingBash {
    fn name(&self) -> &str {
        "Bash"
    }
    fn description(&self) -> &str {
        "fake shell"
    }
    fn permission_level(&self) -> cersei_tools::PermissionLevel {
        cersei_tools::PermissionLevel::None
    }
    fn category(&self) -> cersei_tools::ToolCategory {
        cersei_tools::ToolCategory::Shell
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _: Value, _: &cersei_tools::ToolContext) -> cersei_tools::ToolResult {
        cersei_tools::ToolResult::error(format!("Exit code 101\n{}", self.0))
    }
}

#[tokio::test]
async fn originals_raw_history_and_snapshots_survive_a_restore() {
    use cersei_memory::{JsonlMemory, Memory};

    let mut log = String::new();
    for i in 0..400 {
        log.push_str(&format!("   Compiling crate{i} v0.1.0\n"));
        if i == 200 {
            log.push_str(
                "error[E0425]: cannot find value `zz` in this scope\n  --> src/a.rs:9:5\n",
            );
        }
    }
    let (url, _) = serve(vec![
        chat_tool_call("b1", "Bash", &json!({ "command": "cargo build" })),
        chat_text("seen", None),
        chat_text("seen", None),
        chat_text("SUMMARY of the build session", None),
        chat_text("restored", None),
    ]);
    let mem_dir = tempfile::tempdir().unwrap();

    // ── first process: reduced output, then a compaction ──
    {
        let a = Agent::builder()
            .provider(common::provider(&url, "chat_completions", 100_000))
            .memory(JsonlMemory::new(mem_dir.path()))
            .session_id("s1")
            .with_messages(long_history())
            .tool(LongFailingBash(log.clone()))
            .compression_level(CompressionLevel::Minimal)
            .max_turns(4)
            .build()
            .unwrap();
        a.run("build it").await.unwrap();
        assert!(a.compact().await.is_compacted());
    } // agent and its storage handle dropped

    // ── second process: a fresh storage handle on the same directory ──
    let memory = JsonlMemory::new(mem_dir.path());
    let b = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .memory(JsonlMemory::new(mem_dir.path()))
        .session_id("s1")
        .build()
        .unwrap();
    b.run("continue").await.unwrap();

    // The snapshot is back, and the reference it holds still resolves.
    let snaps = b.compaction_snapshots();
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].memory_key.as_deref(), Some("s1.compaction-1"));
    let reduced = snaps[0]
        .messages
        .iter()
        .flat_map(|m| m.content_blocks())
        .find_map(|blk| match blk {
            ContentBlock::ToolResult {
                content: ToolResultContent::Text(t),
                ..
            } if t.lines().nth(1).is_some_and(|l| l.starts_with("[bricks:")) => Some(t),
            _ => None,
        })
        .expect("the reduced output is in the snapshot");
    let path = reduced
        .split("Read file_path=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap()
        .to_string();
    let files_dir = memory.session_files_dir("s1").unwrap();
    assert!(path.starts_with(files_dir.to_str().unwrap()), "{path}");
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .starts_with(&format!("Exit code 101\n{log}")));

    // The raw history holds the unreduced output and the earlier messages.
    let raw = b.raw_history();
    assert!(raw.iter().flat_map(|m| m.content_blocks()).any(|blk| matches!(blk,
        ContentBlock::ToolResult { content: ToolResultContent::Text(t), .. } if t.contains("Compiling crate399"))));
    assert_eq!(raw[0].get_text(), long_history()[0].get_text());

    // Internal files are not sessions, and they go with the session.
    let ids: Vec<String> = memory
        .sessions()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, ["s1"]);
    memory.delete("s1").await.unwrap();
    assert!(!std::path::Path::new(&path).exists());
    assert_eq!(std::fs::read_dir(mem_dir.path()).unwrap().count(), 0);
}
