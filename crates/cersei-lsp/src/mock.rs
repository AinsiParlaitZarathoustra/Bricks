//! An in-process, scripted language server for tests (feature `mock`).
//!
//! It speaks real JSON-RPC framing over in-memory pipes, keeps the
//! documents it receives (`didOpen` / `didChange`, versions included),
//! records every message, answers requests through a handler, can delay
//! answers, publish diagnostics and "crash" (close its pipes).

use crate::client::normalize_uri;
use crate::jsonrpc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::Mutex;

/// Documents as the server knows them: uri → (version, text).
pub type MockDocs = HashMap<String, (i64, String)>;

/// Answers a request: `Ok(result)` or `Err((code, message))`.
pub type Handler =
    Arc<dyn Fn(&str, &Value, &MockDocs) -> Result<Value, (i64, String)> + Send + Sync>;

/// Decides what to publish after a document change: `(version, items)`.
pub type Publisher =
    Arc<dyn Fn(&str, i64, &str) -> Option<(Option<i64>, Vec<Value>)> + Send + Sync>;

#[derive(Clone)]
pub struct MockConfig {
    /// `capabilities` of the `initialize` result.
    pub capabilities: Value,
    pub handler: Handler,
    pub publisher: Option<Publisher>,
    /// Delay before answering, by method.
    pub delays: HashMap<String, Duration>,
}

impl MockConfig {
    pub fn new(capabilities: Value, handler: Handler) -> Self {
        Self {
            capabilities,
            handler,
            publisher: None,
            delays: HashMap::new(),
        }
    }
}

#[derive(Default)]
struct State {
    docs: MockDocs,
    received: Vec<(String, Value)>,
    crashed: bool,
}

/// Control and inspection of a running mock server.
#[derive(Clone)]
pub struct MockHandle {
    state: Arc<parking_lot_like::Mutex<State>>,
    writer: Arc<Mutex<Option<DuplexStream>>>,
}

/// Minimal sync mutex (std) wrapper, kept private.
mod parking_lot_like {
    pub struct Mutex<T>(std::sync::Mutex<T>);
    impl<T> Mutex<T> {
        pub fn new(t: T) -> Self {
            Self(std::sync::Mutex::new(t))
        }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap_or_else(|e| e.into_inner())
        }
    }
}

impl MockHandle {
    /// Every message received with this method, in order.
    pub fn received(&self, method: &str) -> Vec<Value> {
        self.state
            .lock()
            .received
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }

    pub fn count(&self, method: &str) -> usize {
        self.received(method).len()
    }

    /// Methods received, in order.
    pub fn methods(&self) -> Vec<String> {
        self.state
            .lock()
            .received
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }

    pub fn doc(&self, uri: &str) -> Option<(i64, String)> {
        self.state.lock().docs.get(&normalize_uri(uri)).cloned()
    }

    /// Publish diagnostics now.
    pub async fn publish(&self, uri: &str, version: Option<i64>, items: Vec<Value>) {
        let mut params = json!({ "uri": uri, "diagnostics": items });
        if let Some(v) = version {
            params["version"] = json!(v);
        }
        send(
            &self.writer,
            json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params}),
        )
        .await;
    }

    /// Send any notification (`$/progress`, `experimental/serverStatus`...).
    pub async fn notify(&self, method: &str, params: Value) {
        send(
            &self.writer,
            json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
        .await;
    }

    /// Close the pipes: the client sees the server exit.
    pub async fn crash(&self) {
        self.state.lock().crashed = true;
        if let Some(mut w) = self.writer.lock().await.take() {
            let _ = w.shutdown().await;
        }
    }
}

async fn send(writer: &Arc<Mutex<Option<DuplexStream>>>, msg: Value) {
    let body = serde_json::to_vec(&msg).unwrap_or_default();
    if let Some(w) = writer.lock().await.as_mut() {
        let _ = jsonrpc::send_message(w, &body).await;
    }
}

/// Start a mock server. Returns its handle and the client's ends of the
/// pipes (`reader`, `writer`) to pass to [`crate::LspClient::connect`].
pub fn spawn(config: MockConfig) -> (MockHandle, DuplexStream, DuplexStream) {
    // Client → server and server → client pipes.
    let (client_writer, srv_reader) = tokio::io::duplex(1 << 20);
    let (srv_writer, client_reader) = tokio::io::duplex(1 << 20);

    let handle = MockHandle {
        state: Arc::new(parking_lot_like::Mutex::new(State::default())),
        writer: Arc::new(Mutex::new(Some(srv_writer))),
    };
    let h = handle.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(srv_reader);
        while let Ok(Some(data)) = jsonrpc::read_message(&mut reader).await {
            if h.state.lock().crashed {
                break;
            }
            let Ok(msg) = serde_json::from_slice::<Value>(&data) else {
                continue;
            };
            let method = msg["method"].as_str().unwrap_or("").to_string();
            let params = msg["params"].clone();
            if method.is_empty() {
                continue; // a reply to a server request
            }
            h.state
                .lock()
                .received
                .push((method.clone(), params.clone()));
            let id = msg.get("id").cloned();
            match (id, method.as_str()) {
                (Some(id), "initialize") => {
                    send(
                        &h.writer,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": config.capabilities}}),
                    )
                    .await;
                }
                (Some(id), _) => {
                    let delay = config.delays.get(&method).copied();
                    let docs = h.state.lock().docs.clone();
                    let handler = Arc::clone(&config.handler);
                    let writer = Arc::clone(&h.writer);
                    tokio::spawn(async move {
                        if let Some(d) = delay {
                            tokio::time::sleep(d).await;
                        }
                        let reply = match handler(&method, &params, &docs) {
                            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                            Err((code, message)) => {
                                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
                            }
                        };
                        send(&writer, reply).await;
                    });
                }
                (None, "textDocument/didOpen") => {
                    let td = &params["textDocument"];
                    let uri = normalize_uri(td["uri"].as_str().unwrap_or(""));
                    let version = td["version"].as_i64().unwrap_or(0);
                    let text = td["text"].as_str().unwrap_or("").to_string();
                    h.state
                        .lock()
                        .docs
                        .insert(uri.clone(), (version, text.clone()));
                    publish_after_change(&h, &config, &uri, version, &text).await;
                }
                (None, "textDocument/didChange") => {
                    let td = &params["textDocument"];
                    let uri = normalize_uri(td["uri"].as_str().unwrap_or(""));
                    let version = td["version"].as_i64().unwrap_or(0);
                    // Full-content changes only (what the client sends).
                    let text = params["contentChanges"][0]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    h.state
                        .lock()
                        .docs
                        .insert(uri.clone(), (version, text.clone()));
                    publish_after_change(&h, &config, &uri, version, &text).await;
                }
                (None, "textDocument/didClose") => {
                    let uri = normalize_uri(params["textDocument"]["uri"].as_str().unwrap_or(""));
                    h.state.lock().docs.remove(&uri);
                }
                (None, "exit") => break,
                _ => {}
            }
        }
        if let Some(mut w) = h.writer.lock().await.take() {
            let _ = w.shutdown().await;
        }
    });
    (handle, client_reader, client_writer)
}

async fn publish_after_change(
    h: &MockHandle,
    config: &MockConfig,
    uri: &str,
    version: i64,
    text: &str,
) {
    if let Some(p) = &config.publisher {
        if let Some((v, items)) = p(uri, version, text) {
            h.publish(uri, v, items).await;
        }
    }
}
