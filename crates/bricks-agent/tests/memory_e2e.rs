//! Long-term memory through a real agent: a scripted model, the structured
//! memory with a scripted extractor and deterministic embeddings.

mod common;

use async_trait::async_trait;
use bricks_agent::Agent;
use bricks_agent::events::AgentEvent;
use bricks_embeddings::HashingEmbeddings;
use bricks_memory::LongTermMemory;
use bricks_memory::structured::extract::{ExtractError, ExtractionRequest, Extractor};
use bricks_memory::structured::{MemoryConfig, StructuredMemory};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn text(t: &str) -> String {
    let first =
        json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "content": t } }] });
    let last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] });
    format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n")
}

/// A model that answers `replies` in order and records request bodies.
fn model(replies: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
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
            if let Ok(v) = serde_json::from_slice::<Value>(&buf[start..]) {
                s2.lock().unwrap().push(v);
            }
            let body = it.next().unwrap_or_else(|| text("ok"));
            let out = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(out.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), seen)
}

struct BricksExtractor;

#[async_trait]
impl Extractor for BricksExtractor {
    fn id(&self) -> String {
        "scripted/test".into()
    }
    async fn extract(
        &self,
        r: &ExtractionRequest,
        _: &CancellationToken,
    ) -> Result<String, ExtractError> {
        if r.role == "user" && r.content.contains("utilise Axum") {
            return Ok(json!({
                "entities": [{"ref": "e1", "name": "Bricks", "type": "project"}, {"ref": "e2", "name": "Axum", "type": "library"}],
                "facts": [{"subject": "e1", "predicate": "uses_framework", "value": "Axum", "object": "e2",
                           "quote": "Le projet Bricks utilise Axum"}]
            })
            .to_string());
        }
        Ok(r#"{"entities":[],"facts":[]}"#.into())
    }
}

fn system_of(request: &Value) -> String {
    request["messages"]
        .as_array()
        .and_then(|m| m.iter().find(|x| x["role"] == "system"))
        .and_then(|x| x["content"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_remembered_fact_is_recalled_into_the_next_session_within_budget() {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        StructuredMemory::builder(Arc::new(HashingEmbeddings::new(256)))
            .path(dir.path().join("memory.grafeo"))
            .config(MemoryConfig::default().with_space("project:bricks"))
            .extractor(Arc::new(BricksExtractor))
            .open()
            .unwrap(),
    );
    let ltm: Arc<dyn LongTermMemory> = memory.clone();

    // Session 1: the user states a fact.
    let (url, _) = model(vec![text("Noté.")]);
    let phases: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let a = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .system_prompt("You are Bricks.")
        .session_id("s1")
        .long_term_memory(ltm.clone())
        .on_event({
            let p = phases.clone();
            move |e| match e {
                AgentEvent::Complete(_) => p.lock().unwrap().push("answer".to_string()),
                AgentEvent::MemoryMaintenanceStarted => {
                    p.lock().unwrap().push("maintenance".into())
                }
                AgentEvent::MemoryMaintenanceFinished(Ok(r)) => p.lock().unwrap().push(format!(
                    "maintained: {} fact(s), pending {}",
                    r.facts_created, r.pending
                )),
                _ => {}
            }
        })
        .build()
        .unwrap();
    a.run("Le projet Bricks utilise Axum pour son serveur HTTP.")
        .await
        .unwrap();
    let facts = memory.current_facts("project:bricks").unwrap();
    assert_eq!(facts.len(), 1, "the exchange was remembered");
    // The answer comes first; the maintenance is its own phase after it.
    assert_eq!(
        *phases.lock().unwrap(),
        vec!["answer", "maintenance", "maintained: 1 fact(s), pending 0"]
    );
    assert_eq!(memory.stats().episodes, 2, "user turn and assistant reply");

    // Session 2: another agent, same memory.
    let (url, seen) = model(vec![text("Axum.")]);
    let notes = Arc::new(Mutex::new(Vec::new()));
    let n2 = notes.clone();
    let b = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .system_prompt("You are Bricks.")
        .session_id("s2")
        .long_term_memory(ltm.clone())
        .memory_recall_tokens(400)
        .on_event(move |e| {
            if let AgentEvent::MemoryRecalled { items, tokens, .. } = e {
                n2.lock().unwrap().push((*items, *tokens));
            }
        })
        .build()
        .unwrap();
    b.run("Quel framework web utilise le projet Bricks ?")
        .await
        .unwrap();
    let request = seen.lock().unwrap()[0].clone();
    let system = system_of(&request);
    assert!(system.starts_with("You are Bricks."), "{system}");
    assert!(system.contains("<long_term_memory"), "{system}");
    assert!(system.contains("uses framework: Axum"), "{system}");
    assert!(
        system.contains("source: user"),
        "the fact carries its source: {system}"
    );
    let block = &system[system.find("<long_term_memory").unwrap()..];
    let tokens = bricks_types::tokens::estimate_text(block).tokens;
    assert!(tokens <= 400, "{tokens} tokens for a 400-token budget");
    let recalled = notes.lock().unwrap().clone();
    assert_eq!(recalled.len(), 1, "one recall per run: {recalled:?}");
    assert!(recalled[0].0 >= 1 && recalled[0].1 <= 400, "{recalled:?}");

    // A tiny budget: the memory never exceeds it (it may then recall nothing).
    let (url, seen) = model(vec![text("?")]);
    let c = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .session_id("s3")
        .long_term_memory(ltm)
        .memory_recall_tokens(60)
        .build()
        .unwrap();
    c.run("Quel framework web utilise le projet Bricks ?")
        .await
        .unwrap();
    let system = system_of(&seen.lock().unwrap()[0]);
    if let Some(i) = system.find("<long_term_memory") {
        assert!(
            bricks_types::tokens::estimate_text(&system[i..]).tokens <= 60,
            "{system}"
        );
    }
}

#[tokio::test]
async fn contextualized_recall_and_post_compaction_refresh_work_in_multi_turn_runs() {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        StructuredMemory::builder(Arc::new(HashingEmbeddings::new(256)))
            .path(dir.path().join("memory.grafeo"))
            .config(MemoryConfig::default().with_space("project:acme"))
            .open()
            .unwrap(),
    );
    memory
        .write_note(
            bricks_memory::structured::NoteInput {
                space: "project:acme".into(),
                project_id: None,
                session_id: Some("seed".into()),
                text: "Acme Gateway listens on port 8443 with mTLS enabled".into(),
            },
            false,
        )
        .await
        .unwrap();
    let ltm: Arc<dyn LongTermMemory> = memory.clone();

    // Multi-turn history where the new prompt alone doesn't name "Acme Gateway",
    // so contextualized recall pulls it from recent session history.
    let (url, seen) = model(vec![text("Port 8443.")]);
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .system_prompt("You are Bricks.")
        .session_id("s_ctx")
        .with_messages(vec![
            bricks_types::Message::user("Let's inspect Acme Gateway configuration"),
            bricks_types::Message::assistant("Sure, checking Acme Gateway TLS and listeners."),
        ])
        .long_term_memory(ltm)
        .build()
        .unwrap();
    agent.run("What port does it listen on?").await.unwrap();
    let sys = system_of(&seen.lock().unwrap()[0]);
    assert!(
        sys.contains("Acme Gateway listens on port 8443"),
        "expected contextualized recall block in system prompt, got: {sys}"
    );
}

#[tokio::test]
async fn tool_outputs_and_cancelled_runs_still_capture_and_maintain_memory() {
    use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
    use bricks_memory::structured::EntryFilter;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct DummyTool(Arc<AtomicBool>);
    #[async_trait]
    impl bricks_tools::Tool for DummyTool {
        fn name(&self) -> &str {
            "Inspect"
        }
        fn description(&self) -> &str {
            "Inspect a component"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object", "properties": {"path": {"type": "string"}}})
        }
        async fn execute(
            &self,
            _input: Value,
            _ctx: &bricks_tools::ToolContext,
        ) -> bricks_tools::ToolResult {
            self.0.store(true, Ordering::SeqCst);
            bricks_tools::ToolResult::success("postgres pool_size = 32 configured cleanly")
        }
    }

    struct ToolExtractor;
    #[async_trait]
    impl Extractor for ToolExtractor {
        fn id(&self) -> String {
            "scripted/tool".into()
        }
        async fn extract(
            &self,
            r: &ExtractionRequest,
            _: &CancellationToken,
        ) -> Result<String, ExtractError> {
            if r.role == "tool" && r.content.contains("pool_size = 32") {
                return Ok(json!({
                    "entities": [{"ref": "e1", "name": "Postgres", "type": "database"}],
                    "facts": [{"subject": "e1", "predicate": "pool_size", "value": "32",
                               "quote": "pool_size = 32"}]
                })
                .to_string());
            }
            Ok(r#"{"entities":[],"facts":[]}"#.into())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        StructuredMemory::builder(Arc::new(HashingEmbeddings::new(256)))
            .path(dir.path().join("tool_cancel.grafeo"))
            .config(MemoryConfig::default().with_space("project:acme"))
            .extractor(Arc::new(ToolExtractor))
            .open()
            .unwrap(),
    );
    let ltm: Arc<dyn LongTermMemory> = memory.clone();

    // First run completes after a tool call -> captures Origin::ToolOutput ("tool" role)
    // and extracts an active fact from tool output.
    let ran_tool = Arc::new(AtomicBool::new(false));
    let script = Script::new(vec![
        Reply::tool("t1", "Inspect", json!({"path": "db.toml"})),
        Reply::text("Checked db.toml."),
    ]);
    let provider = ScriptedProvider::new("a", script);
    let agent = Agent::builder()
        .provider(provider)
        .tool(DummyTool(Arc::clone(&ran_tool)))
        .system_prompt("You are Bricks.")
        .session_id("s_tool")
        .long_term_memory(ltm.clone())
        .build()
        .unwrap();
    agent.run("Check the database config").await.unwrap();
    assert!(ran_tool.load(Ordering::SeqCst));
    let exchanges = memory
        .entries(EntryFilter::Exchanges, "", 0, 50)
        .unwrap();
    assert!(
        exchanges
            .entries
            .iter()
            .any(|e| e.role.as_deref() == Some("tool") && e.text.contains("pool_size = 32")),
        "expected tool episode, got {:?}",
        exchanges.entries
    );
    assert!(
        memory
            .current_facts("project:acme")
            .unwrap()
            .iter()
            .any(|f| f.predicate == "pool_size" && f.value == "32"),
        "expected fact extracted from Origin::ToolOutput"
    );

    // Second run executes a valid tool turn and is cancelled on turn 2:
    // capture and maintenance MUST still run cleanly.
    let cancel_after_tool = Arc::new(AtomicBool::new(false));
    let tool2 = DummyTool(Arc::clone(&cancel_after_tool));
    let script2 = Script::new(vec![
        Reply::tool("t2", "Inspect", json!({"path": "cache.toml"})),
        Reply::hang(),
    ]);
    let agent2 = Arc::new(
        Agent::builder()
            .provider(ScriptedProvider::new("a", script2))
            .tool(tool2)
            .system_prompt("You are Bricks.")
            .session_id("s_cancelled")
            .long_term_memory(ltm)
            .build()
            .unwrap(),
    );
    let cancel_handle = Arc::clone(&agent2);
    let waiter = tokio::spawn(async move {
        while !cancel_after_tool.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_handle.cancel();
    });
    let err = agent2.run("Check cache config").await.unwrap_err();
    waiter.await.unwrap();
    assert!(err.to_string().to_lowercase().contains("cancel"), "{err}");

    let after = memory
        .entries(EntryFilter::Exchanges, "cache.toml", 0, 50)
        .unwrap();
    assert!(
        after
            .entries
            .iter()
            .any(|e| e.role.as_deref() == Some("tool") && e.text.contains("cache.toml")),
        "expected cancelled run after valid tool turn to capture tool output: {:?}",
        after.entries
    );
    assert_eq!(memory.pending_work().unwrap(), 0, "maintenance should have processed cancelled run capture");
}
