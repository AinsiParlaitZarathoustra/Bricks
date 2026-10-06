//! A scripted in-process HTTP server for end-to-end provider tests.
//!
//! No mock-HTTP crate: the contract under test is what goes on the wire, and a
//! literal response is the least indirect way to state it. Every connection is
//! served one scripted [`Reply`] and closed (`Connection: close`), and every
//! request is recorded so tests can assert on path, headers and JSON body.

#![allow(dead_code)]

use cersei_provider::{ProviderRegistry, ResolvedModel};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Recorded {
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Value,
}

pub enum Reply {
    /// A complete JSON response.
    Json {
        status: &'static str,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    },
    /// A streamed `text/event-stream` body written in the given fragments.
    Sse { chunks: Vec<Vec<u8>> },
    /// Declares a long body, sends part of it, then drops the connection.
    Truncated { chunks: Vec<Vec<u8>> },
}

pub struct Mock {
    pub base: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Mock {
    pub fn requests(&self) -> std::sync::MutexGuard<'_, Vec<Recorded>> {
        self.requests.lock().unwrap()
    }
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn read_request(sock: &mut std::net::TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        match sock.read(&mut tmp) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let path = lines.next()?.split_whitespace().nth(1)?.to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        match sock.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let body = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
    Some(Recorded {
        path,
        headers,
        body,
    })
}

/// Serve `replies` in order, one per connection.
pub fn serve(replies: Vec<Reply>) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&requests);
    std::thread::spawn(move || {
        for reply in replies {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            let Some(rec) = read_request(&mut sock) else {
                continue;
            };
            log.lock().unwrap().push(rec);
            match reply {
                Reply::Json {
                    status,
                    headers,
                    body,
                } => {
                    let extra: String = headers
                        .iter()
                        .map(|(k, v)| format!("{k}: {v}\r\n"))
                        .collect();
                    let payload = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(payload.as_bytes());
                }
                Reply::Sse { chunks } => {
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                    for c in chunks {
                        let _ = sock.write_all(&c);
                        let _ = sock.flush();
                        std::thread::sleep(Duration::from_millis(3));
                    }
                }
                Reply::Truncated { chunks } => {
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n");
                    for c in chunks {
                        let _ = sock.write_all(&c);
                        let _ = sock.flush();
                        std::thread::sleep(Duration::from_millis(3));
                    }
                }
            }
            let _ = sock.flush();
        }
    });
    Mock {
        base: format!("http://127.0.0.1:{port}"),
        requests,
    }
}

/// Fragment a stream at arbitrary byte offsets (including inside a multi-byte
/// UTF-8 character) so a decoder that works per chunk would corrupt it.
pub fn fragment(text: &str, size: usize) -> Vec<Vec<u8>> {
    text.as_bytes().chunks(size).map(|c| c.to_vec()).collect()
}

pub fn sse_events(events: &[(Option<&str>, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| match name {
            Some(n) => format!("event: {n}\ndata: {data}\n\n"),
            None => format!("data: {data}\n\n"),
        })
        .collect()
}

/// One provider with one model per protocol, pointed at `base`.
pub fn registry_for(base: &str, extra_model_toml: &str) -> ProviderRegistry {
    let text = format!(
        r#"
schema_version = 1

[[providers]]
id = "mock"
name = "Mock"
endpoint = "{base}/v1"
protocol = "chat_completions"
auth = "bearer"
api_key_env = "BRICKS_MOCK_API_KEY"

[[providers.models]]
id = "chat"
name = "Chat"
api_model = "vendor-chat"
streaming = true
tool_calls = true
input_modalities = ["text", "image", "audio", "document"]
document_mime_types = ["application/pdf"]
[providers.models.limits]
max_input_tokens = 96000
max_output_tokens = 4000
context_window_tokens = 100000
[providers.models.pricing]
currency = "USD"
per_tokens = 1000000
input = "1"
output = "2"
cache_read = "0.1"

[[providers.models]]
id = "resp"
name = "Resp"
api_model = "vendor-resp"
protocol = "responses"
streaming = true
tool_calls = true
input_modalities = ["text", "image", "document"]
document_mime_types = ["application/pdf"]
[providers.models.limits]
max_input_tokens = 64000
max_output_tokens = 4000

[[providers.models]]
id = "anth"
name = "Anth"
api_model = "vendor-anth"
protocol = "anthropic_messages"
streaming = true
tool_calls = true
input_modalities = ["text", "image", "document"]
document_mime_types = ["application/pdf"]
[providers.models.limits]
max_input_tokens = 64000
max_output_tokens = 4000
[providers.models.pricing]
currency = "USD"
per_tokens = 1000000
input = "3"
output = "15"
cache_read = "0.3"
cache_write = "3.75"
[providers.models.pricing.cache_write_variants]
"1h" = "6"

{extra_model_toml}
"#
    );
    ProviderRegistry::from_toml_str(&text, "mock.toml").unwrap()
}

pub fn model(reg: &ProviderRegistry, sel: &str) -> ResolvedModel {
    reg.resolve(sel).unwrap()
}
