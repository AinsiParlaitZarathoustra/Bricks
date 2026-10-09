//! A server-sent event over the decoder's 16 MiB bound, through the real
//! provider and runner: after a first text delta has been shown, the run ends
//! with an explicit protocol error, and the provider is not called again (no
//! retry, no replay).

mod common;

use cersei_agent::events::AgentEvent;
use cersei_agent::Agent;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Answers every request with a text delta, then one `data:` line that never
/// ends, 17 MiB long. Counts the requests.
fn serve_oversized(calls: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((mut sock, _)) = listener.accept() {
            calls.fetch_add(1, Ordering::SeqCst);
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
            // The request: headers, then a body of Content-Length bytes.
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
            let delta = json!({ "choices": [{ "index": 0,
                "delta": { "role": "assistant", "content": "partial " } }] });
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {delta}\n\ndata: "
                )
                .as_bytes(),
            );
            let block = vec![b'a'; 1024 * 1024];
            for _ in 0..17 {
                if sock.write_all(&block).is_err() {
                    break; // the client gave up, as it should
                }
            }
            let _ = sock.shutdown(std::net::Shutdown::Both);
        }
    });
    format!("http://127.0.0.1:{port}/v1")
}

#[tokio::test]
async fn an_oversized_event_ends_the_run_without_a_second_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let url = serve_oversized(calls.clone());
    let seen = Arc::new(Mutex::new(String::new()));
    let s = seen.clone();
    let work = tempfile::tempdir().unwrap();
    let agent = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .working_dir(work.path())
        .on_event(move |e| {
            if let AgentEvent::TextDelta(t) = e {
                s.lock().unwrap().push_str(t);
            }
        })
        .build()
        .unwrap();

    let err = agent.run("go").await.unwrap_err().to_string();
    assert!(err.contains("exceeds 16 MiB"), "{err}");
    assert_eq!(seen.lock().unwrap().as_str(), "partial ");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the response was not replayed"
    );
}
