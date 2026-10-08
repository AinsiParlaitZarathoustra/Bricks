//! Long-term memory through a real agent: a scripted model, the structured
//! memory with a scripted extractor and deterministic embeddings.

mod common;

use async_trait::async_trait;
use cersei_agent::events::AgentEvent;
use cersei_agent::Agent;
use cersei_embeddings::HashingEmbeddings;
use cersei_memory::structured::extract::{ExtractError, ExtractionRequest, Extractor};
use cersei_memory::structured::{MemoryConfig, StructuredMemory};
use cersei_memory::LongTermMemory;
use serde_json::{json, Value};
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
    let tokens = cersei_types::tokens::estimate_text(block).tokens;
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
            cersei_types::tokens::estimate_text(&system[i..]).tokens <= 60,
            "{system}"
        );
    }
}
