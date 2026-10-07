//! LSP client: manages a single language server process.
//!
//! Communicates via JSON-RPC 2.0 over stdio with Content-Length framing.
//!
//! * The capabilities the server returns from `initialize` are kept
//!   ([`ServerCaps`]), position encoding and synchronization included.
//! * Server requests (`workspace/configuration`, `client/registerCapability`,
//!   ...) are answered; they are never mistaken for responses.
//! * A request can time out or be cancelled: it is then removed and the
//!   server gets `$/cancelRequest`. When the server exits, every pending
//!   request fails at once with [`LspError::ServerExited`].
//! * Diagnostics are kept with the document version they were published
//!   for, and a sequence number, so a caller can tell a fresh report from
//!   an old one.

use crate::config::LspServerConfig;
use crate::jsonrpc::{self, Notification, Reply, Request, Response, RpcError};
use crate::types::*;
use dashmap::DashMap;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::process::Child;
use tokio::sync::{oneshot, Mutex, Notify};
use tokio_util::sync::CancellationToken;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Errors from the LSP client.
#[derive(thiserror::Error, Debug)]
pub enum LspError {
    #[error("Server not started")]
    NotStarted,
    #[error("Server process failed to start: {0}")]
    SpawnFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Request timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("RPC error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("Server not initialized")]
    NotInitialized,
    #[error("Server exited")]
    ServerExited,
    #[error("Request cancelled")]
    Cancelled,
    #[error("Not supported by the server: {0}")]
    Unsupported(String),
}

pub type LspResult<T> = Result<T, LspError>;

/// How the server wants document changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    None,
    Full,
    Incremental,
}

/// What the server said it supports, from its `initialize` result.
#[derive(Debug, Clone)]
pub struct ServerCaps {
    /// `utf-8`, `utf-16` or `utf-32`; `utf-16` when the server chose none.
    pub position_encoding: String,
    pub open_close: bool,
    pub sync: SyncKind,
    pub save: bool,
    pub save_include_text: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub document_symbol: bool,
    pub workspace_symbol: bool,
    /// Pull diagnostics (`textDocument/diagnostic`).
    pub diagnostic_pull: bool,
    pub raw: Value,
}

fn provider(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Object(_) => true,
        _ => false,
    }
}

impl ServerCaps {
    pub fn from_initialize(result: &Value) -> Self {
        let c = &result["capabilities"];
        let tds = &c["textDocumentSync"];
        let (open_close, sync, save, save_include_text) = match tds {
            Value::Number(n) => {
                let k = n.as_u64().unwrap_or(0);
                // A bare kind implies open/close notifications.
                (k > 0, sync_kind(k), false, false)
            }
            Value::Object(o) => {
                let save = o.get("save");
                (
                    o.get("openClose").and_then(Value::as_bool).unwrap_or(false),
                    sync_kind(o.get("change").and_then(Value::as_u64).unwrap_or(0)),
                    save.map(provider).unwrap_or(false),
                    save.and_then(|s| s.get("includeText"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
            }
            _ => (false, SyncKind::None, false, false),
        };
        Self {
            position_encoding: c["positionEncoding"]
                .as_str()
                .unwrap_or("utf-16")
                .to_string(),
            open_close,
            sync,
            save,
            save_include_text,
            hover: provider(&c["hoverProvider"]),
            definition: provider(&c["definitionProvider"]),
            references: provider(&c["referencesProvider"]),
            document_symbol: provider(&c["documentSymbolProvider"]),
            workspace_symbol: provider(&c["workspaceSymbolProvider"]),
            diagnostic_pull: provider(&c["diagnosticProvider"]),
            raw: c.clone(),
        }
    }
}

fn sync_kind(k: u64) -> SyncKind {
    match k {
        1 => SyncKind::Full,
        2 => SyncKind::Incremental,
        _ => SyncKind::None,
    }
}

/// Diagnostics last published for one document.
#[derive(Debug, Clone)]
pub struct DiagnosticsEntry {
    /// The document version the server analyzed, when it said so.
    pub version: Option<i64>,
    pub items: Vec<LspDiagnostic>,
    /// Raw LSP diagnostics (ranges in the server's encoding).
    pub raw: Vec<Value>,
    /// Arrival order among all publications of this client.
    pub seq: u64,
}

/// A location from definition / references / symbols, whatever form the
/// server used (`Location`, `Location[]`, `LocationLink[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct RawLocation {
    pub uri: String,
    /// Range of the target (for a `LocationLink`, its selection range).
    pub range: Range,
}

type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// A client connected to a single LSP server process.
pub struct LspClient {
    config: LspServerConfig,
    writer: Arc<Mutex<Option<Writer>>>,
    request_id: AtomicU64,
    pending: Arc<DashMap<u64, oneshot::Sender<Response>>>,
    diagnostics: Arc<DashMap<String, DiagnosticsEntry>>,
    diag_seq: Arc<AtomicU64>,
    diag_notify: Arc<Notify>,
    /// Indexing state: work-done progress tokens in flight, and the
    /// `experimental/serverStatus` quiescence (rust-analyzer).
    busy: Arc<BusyState>,
    is_initialized: AtomicBool,
    alive: Arc<AtomicBool>,
    exited: Arc<Notify>,
    process: Mutex<Option<Child>>,
    pid: OnceLock<u32>,
    root_uri: Mutex<Option<String>>,
    caps: OnceLock<ServerCaps>,
    timeout: Duration,
    /// The end of the server's stderr (to explain a failed start).
    stderr_tail: Arc<parking_mutex::Mutex<String>>,
}

/// A tiny std mutex wrapper that ignores poisoning.
mod parking_mutex {
    #[derive(Default)]
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

const STDERR_TAIL: usize = 2048;

impl LspClient {
    /// Create a new unstarted client.
    pub fn new(config: LspServerConfig) -> Self {
        Self {
            config,
            writer: Arc::new(Mutex::new(None)),
            request_id: AtomicU64::new(1),
            pending: Arc::new(DashMap::new()),
            diagnostics: Arc::new(DashMap::new()),
            diag_seq: Arc::new(AtomicU64::new(0)),
            diag_notify: Arc::new(Notify::new()),
            busy: Arc::new(BusyState::default()),
            is_initialized: AtomicBool::new(false),
            alive: Arc::new(AtomicBool::new(false)),
            exited: Arc::new(Notify::new()),
            process: Mutex::new(None),
            pid: OnceLock::new(),
            root_uri: Mutex::new(None),
            caps: OnceLock::new(),
            timeout: REQUEST_TIMEOUT,
            stderr_tail: Arc::new(parking_mutex::Mutex::new(String::new())),
        }
    }

    /// The last bytes the server wrote on stderr.
    pub fn stderr_tail(&self) -> String {
        self.stderr_tail.lock().clone()
    }

    /// Default timeout of requests sent without an explicit one.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Server name.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn config(&self) -> &LspServerConfig {
        &self.config
    }

    /// Whether the server has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.is_initialized.load(Ordering::Relaxed)
    }

    /// Whether the connection is still open (the server has not exited).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Process id of a spawned server.
    pub fn pid(&self) -> Option<u32> {
        self.pid.get().copied()
    }

    /// The capabilities from `initialize`.
    pub fn capabilities(&self) -> Option<&ServerCaps> {
        self.caps.get()
    }

    /// Start the server process and the I/O pump.
    pub async fn start(&self, working_dir: &Path) -> LspResult<()> {
        let mut cmd = tokio::process::Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .current_dir(working_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| LspError::SpawnFailed(format!("{}: {e}", self.config.command)))?;

        let stdin = child.stdin.take().ok_or(LspError::NotStarted)?;
        let stdout = child.stdout.take().ok_or(LspError::NotStarted)?;
        // Keep only the end of stderr (servers log a lot).
        if let Some(mut err) = child.stderr.take() {
            let tail = Arc::clone(&self.stderr_tail);
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = [0u8; 4096];
                while let Ok(n) = err.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut t = tail.lock();
                    t.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if t.len() > STDERR_TAIL {
                        let mut cut = t.len() - STDERR_TAIL;
                        while !t.is_char_boundary(cut) {
                            cut += 1;
                        }
                        t.drain(..cut);
                    }
                }
            });
        }
        if let Some(pid) = child.id() {
            let _ = self.pid.set(pid);
        }
        *self.process.lock().await = Some(child);
        self.connect(stdout, stdin, working_dir).await;
        tracing::debug!("LSP server '{}' started", self.config.name);
        Ok(())
    }

    /// Attach to an already running server through its streams (used for
    /// in-process servers and tests).
    pub async fn connect<R, W>(&self, reader: R, writer: W, root: &Path)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        *self.writer.lock().await = Some(Box::new(writer));
        *self.root_uri.lock().await = Some(path_to_uri(root));
        self.alive.store(true, Ordering::Relaxed);

        let pending = Arc::clone(&self.pending);
        let diagnostics = Arc::clone(&self.diagnostics);
        let diag_seq = Arc::clone(&self.diag_seq);
        let diag_notify = Arc::clone(&self.diag_notify);
        let busy = Arc::clone(&self.busy);
        let writer = Arc::clone(&self.writer);
        let alive = Arc::clone(&self.alive);
        let exited = Arc::clone(&self.exited);
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            while let Ok(Some(data)) = jsonrpc::read_message(&mut reader).await {
                let Ok(msg) = serde_json::from_slice::<Response>(&data) else {
                    continue;
                };
                match (&msg.id, &msg.method) {
                    // A server request: answer it.
                    (Some(id), Some(method)) => {
                        let reply = answer_server_request(id.clone(), method, msg.params.as_ref());
                        if let Ok(body) = serde_json::to_vec(&reply) {
                            if let Some(w) = writer.lock().await.as_mut() {
                                let _ = jsonrpc::send_message(w, &body).await;
                            }
                        }
                    }
                    // A response to one of our requests.
                    (Some(id), None) => {
                        if let Some((_, tx)) = id.as_u64().and_then(|id| pending.remove(&id)) {
                            let _ = tx.send(msg);
                        }
                    }
                    (None, Some(method)) => {
                        if let Some(p) = &msg.params {
                            busy.observe(method, p);
                        }
                        if method == "textDocument/publishDiagnostics" {
                            if let Some(params) = &msg.params {
                                let seq = diag_seq.fetch_add(1, Ordering::SeqCst) + 1;
                                handle_publish_diagnostics(params, &diagnostics, seq);
                                diag_notify.notify_waiters();
                            }
                        }
                    }
                    (None, None) => {}
                }
            }
            // The server is gone: fail every pending request now.
            alive.store(false, Ordering::Relaxed);
            pending.clear();
            busy.changed.notify_waiters();
            diag_notify.notify_waiters();
            exited.notify_waiters();
        });
    }

    /// Send the LSP `initialize` handshake.
    pub async fn initialize(&self) -> LspResult<Value> {
        let root_uri = self.root_uri.lock().await.clone();
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "workspaceFolders": root_uri.as_ref().map(|uri| {
                vec![json!({ "name": "workspace", "uri": uri })]
            }),
            "capabilities": {
                "general": { "positionEncodings": ["utf-8", "utf-16"] },
                "textDocument": {
                    "synchronization": { "didSave": true, "dynamicRegistration": false },
                    "hover": { "contentFormat": ["plaintext", "markdown"] },
                    "definition": { "linkSupport": true },
                    "references": {},
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "publishDiagnostics": { "versionSupport": true },
                    "diagnostic": { "dynamicRegistration": false }
                },
                "workspace": {
                    "workspaceFolders": true,
                    "configuration": true,
                    "symbol": {}
                },
                "window": { "workDoneProgress": true },
                "experimental": { "serverStatusNotification": true }
            },
            "initializationOptions": self.config.initialization_options,
        });

        let result = self.send_request("initialize", Some(params)).await?;
        let _ = self.caps.set(ServerCaps::from_initialize(&result));
        self.send_notification("initialized", Some(json!({})))
            .await?;
        self.is_initialized.store(true, Ordering::Relaxed);
        Ok(result)
    }

    /// Notify the server about an opened file (version 1, disk content).
    pub async fn open_document(&self, path: &Path) -> LspResult<()> {
        let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let language_id = self.config.language_id(&ext);
        self.did_open(&path_to_uri(path), &language_id, 1, &content)
            .await
    }

    /// The language id of a path.
    pub fn language_id_for(&self, path: &Path) -> String {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        self.config.language_id(&ext)
    }

    /// `textDocument/didOpen`.
    pub async fn did_open(
        &self,
        uri: &str,
        language_id: &str,
        version: i64,
        text: &str,
    ) -> LspResult<()> {
        self.send_notification(
            "textDocument/didOpen",
            Some(json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            })),
        )
        .await
    }

    /// `textDocument/didChange` with the full new content (valid for both
    /// full and incremental synchronization).
    pub async fn did_change_full(&self, uri: &str, version: i64, text: &str) -> LspResult<()> {
        self.send_notification(
            "textDocument/didChange",
            Some(json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [ { "text": text } ],
            })),
        )
        .await
    }

    pub async fn did_save(&self, uri: &str, text: Option<&str>) -> LspResult<()> {
        let mut params = json!({ "textDocument": { "uri": uri } });
        if let Some(t) = text {
            params["text"] = json!(t);
        }
        self.send_notification("textDocument/didSave", Some(params))
            .await
    }

    pub async fn did_close(&self, uri: &str) -> LspResult<()> {
        self.send_notification(
            "textDocument/didClose",
            Some(json!({ "textDocument": { "uri": uri } })),
        )
        .await
    }

    /// Whether the server says it is still working (indexing, loading the
    /// project): its answers may be incomplete meanwhile.
    pub fn is_busy(&self) -> bool {
        self.busy.is_busy()
    }

    /// Whether the server ever reported progress or status (a server that
    /// never does cannot be known to be indexing).
    pub fn reports_activity(&self) -> bool {
        self.busy.seen.load(Ordering::Relaxed)
    }

    /// Wait until the server reports any activity, or `timeout`.
    pub async fn wait_first_report(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.busy.changed.notified();
            if self.reports_activity() || !self.is_alive() {
                return self.reports_activity();
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.reports_activity();
            }
        }
    }

    /// What it is busy with (progress titles), for reports.
    pub fn busy_with(&self) -> Vec<String> {
        self.busy.titles()
    }

    /// Wait until the server is no longer busy, it exits, or `timeout`.
    /// Returns whether it is ready.
    pub async fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.busy.changed.notified();
            if !self.busy.is_busy() {
                return true;
            }
            if !self.is_alive() {
                return false;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return !self.busy.is_busy();
            }
        }
    }

    /// Current diagnostics sequence (publications received so far).
    pub fn diagnostics_seq(&self) -> u64 {
        self.diag_seq.load(Ordering::SeqCst)
    }

    /// The diagnostics last published for `uri`.
    pub fn diagnostics_entry(&self, uri: &str) -> Option<DiagnosticsEntry> {
        self.diagnostics
            .get(&normalize_uri(uri))
            .map(|e| e.value().clone())
    }

    /// Wait until `accept` holds for the diagnostics of `uri`, the server
    /// exits, or `timeout` passes. Returns the last entry seen.
    pub async fn wait_diagnostics<F>(
        &self,
        uri: &str,
        timeout: Duration,
        accept: F,
    ) -> Option<DiagnosticsEntry>
    where
        F: Fn(&DiagnosticsEntry) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.diag_notify.notified();
            let entry = self.diagnostics_entry(uri);
            if entry.as_ref().is_some_and(&accept) || !self.is_alive() {
                return entry;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.diagnostics_entry(uri);
            }
        }
    }

    /// Pull diagnostics (`textDocument/diagnostic`), when supported.
    pub async fn pull_diagnostics(&self, uri: &str, timeout: Duration) -> LspResult<Vec<Value>> {
        if !self.capabilities().is_some_and(|c| c.diagnostic_pull) {
            return Err(LspError::Unsupported("textDocument/diagnostic".into()));
        }
        let r = self
            .request(
                "textDocument/diagnostic",
                json!({ "textDocument": { "uri": uri } }),
                timeout,
                None,
            )
            .await?;
        Ok(r["items"].as_array().cloned().unwrap_or_default())
    }

    /// Get hover information at a position.
    pub async fn hover(&self, path: &Path, line: u32, character: u32) -> LspResult<Option<String>> {
        let result = self
            .send_request(
                "textDocument/hover",
                Some(json!({
                    "textDocument": { "uri": path_to_uri(path) },
                    "position": { "line": line, "character": character },
                })),
            )
            .await?;
        Ok(hover_text(&result))
    }

    /// Go to definition.
    pub async fn definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> LspResult<Vec<String>> {
        let result = self
            .send_request(
                "textDocument/definition",
                Some(json!({
                    "textDocument": { "uri": path_to_uri(path) },
                    "position": { "line": line, "character": character },
                })),
            )
            .await?;

        Ok(format_locations(&parse_locations(&result)))
    }

    /// Find all references.
    pub async fn references(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> LspResult<Vec<String>> {
        let result = self
            .send_request(
                "textDocument/references",
                Some(json!({
                    "textDocument": { "uri": path_to_uri(path) },
                    "position": { "line": line, "character": character },
                    "context": { "includeDeclaration": true },
                })),
            )
            .await?;

        Ok(format_locations(&parse_locations(&result)))
    }

    /// Get document symbols (outline).
    pub async fn document_symbols(&self, path: &Path) -> LspResult<Vec<SymbolInfo>> {
        let result = self
            .send_request(
                "textDocument/documentSymbol",
                Some(json!({
                    "textDocument": { "uri": path_to_uri(path) },
                })),
            )
            .await?;

        let symbols = if let Some(arr) = result.as_array() {
            arr.iter().map(collect_symbol).collect()
        } else {
            vec![]
        };

        Ok(symbols)
    }

    /// Get cached diagnostics for a file.
    pub fn get_diagnostics(&self, path: &Path) -> Vec<LspDiagnostic> {
        self.diagnostics_entry(&path_to_uri(path))
            .map(|e| e.items)
            .unwrap_or_default()
    }

    /// Get all cached diagnostics.
    pub fn all_diagnostics(&self) -> Vec<LspDiagnostic> {
        self.diagnostics
            .iter()
            .flat_map(|entry| entry.value().items.clone())
            .collect()
    }

    /// Gracefully shutdown the server.
    pub async fn shutdown(&self) -> LspResult<()> {
        if self.is_initialized() && self.is_alive() {
            let _ = self
                .request("shutdown", Value::Null, Duration::from_secs(5), None)
                .await;
            let _ = self.send_notification("exit", None).await;
        }

        if let Some(mut child) = self.process.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            let _ = child.kill().await;
        }
        *self.writer.lock().await = None;
        self.is_initialized.store(false, Ordering::Relaxed);
        tracing::debug!("LSP server '{}' shut down", self.config.name);
        Ok(())
    }

    /// Wait until the server exits (or `timeout`).
    pub async fn wait_exit(&self, timeout: Duration) -> bool {
        let notified = self.exited.notified();
        if !self.is_alive() {
            return true;
        }
        tokio::time::timeout(timeout, notified).await.is_ok() || !self.is_alive()
    }

    /// Send a request with an explicit timeout and optional cancellation.
    /// On timeout or cancellation the request is withdrawn and the server
    /// gets `$/cancelRequest`.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> LspResult<Value> {
        if !self.is_alive() {
            return Err(LspError::ServerExited);
        }
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let params = if params.is_null() { None } else { Some(params) };
        let req = Request::new(id, method, params);
        let body = serde_json::to_vec(&req)?;

        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);

        {
            let mut writer_guard = self.writer.lock().await;
            let Some(writer) = writer_guard.as_mut() else {
                self.pending.remove(&id);
                return Err(LspError::NotStarted);
            };
            if let Err(e) = jsonrpc::send_message(writer, &body).await {
                self.pending.remove(&id);
                return Err(e.into());
            }
        }

        let never = CancellationToken::new();
        let cancel = cancel.unwrap_or(&never);
        let outcome = tokio::select! {
            r = tokio::time::timeout(timeout, rx) => match r {
                Ok(Ok(resp)) => Ok(resp),
                Ok(Err(_)) => Err(LspError::ServerExited),
                Err(_) => Err(LspError::Timeout(timeout)),
            },
            _ = cancel.cancelled() => Err(LspError::Cancelled),
        };
        let response = match outcome {
            Ok(r) => r,
            Err(e) => {
                if self.pending.remove(&id).is_some() && self.is_alive() {
                    let _ = self
                        .send_notification("$/cancelRequest", Some(json!({ "id": id })))
                        .await;
                }
                return Err(e);
            }
        };

        if let Some(error) = response.error {
            return Err(LspError::Rpc {
                code: error.code,
                message: error.message,
            });
        }

        Ok(response.result.unwrap_or(Value::Null))
    }

    /// Requests waiting for a response.
    pub fn pending_requests(&self) -> usize {
        self.pending.len()
    }

    // ── Internal ────────────────────────────────────────────────────────

    async fn send_request(&self, method: &str, params: Option<Value>) -> LspResult<Value> {
        self.request(method, params.unwrap_or(Value::Null), self.timeout, None)
            .await
    }

    /// Send a notification.
    pub async fn send_notification(&self, method: &str, params: Option<Value>) -> LspResult<()> {
        let notif = Notification::new(method, params);
        let body = serde_json::to_vec(&notif)?;

        let mut writer_guard = self.writer.lock().await;
        let writer = writer_guard.as_mut().ok_or(LspError::NotStarted)?;
        jsonrpc::send_message(writer, &body).await?;
        Ok(())
    }
}

/// Server activity, from `$/progress` and `experimental/serverStatus`.
#[derive(Default)]
struct BusyState {
    /// Progress token → title, while begun and not ended.
    active: parking_mutex::Mutex<std::collections::HashMap<String, String>>,
    /// `Some(false)` while the server reports it is not quiescent.
    quiescent: parking_mutex::Mutex<Option<bool>>,
    /// Any progress or status was ever received.
    seen: AtomicBool,
    changed: Notify,
}

impl BusyState {
    fn observe(&self, method: &str, p: &Value) {
        match method {
            "$/progress" => {
                let token = match &p["token"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                match p["value"]["kind"].as_str() {
                    Some("begin") => {
                        let title = p["value"]["title"].as_str().unwrap_or(&token).to_string();
                        self.active.lock().insert(token, title);
                    }
                    Some("end") => {
                        self.active.lock().remove(&token);
                    }
                    _ => return,
                }
            }
            "experimental/serverStatus" => {
                *self.quiescent.lock() = p["quiescent"].as_bool();
            }
            _ => return,
        }
        self.seen.store(true, Ordering::Relaxed);
        self.changed.notify_waiters();
    }

    fn is_busy(&self) -> bool {
        *self.quiescent.lock() == Some(false) || !self.active.lock().is_empty()
    }

    fn titles(&self) -> Vec<String> {
        let mut v: Vec<String> = self.active.lock().values().cloned().collect();
        v.sort();
        if v.is_empty() && *self.quiescent.lock() == Some(false) {
            v.push("not quiescent".into());
        }
        v
    }
}

/// Answer a request the server sent us. We advertise no dynamic
/// registration and no edits: acknowledge what is harmless, refuse the rest.
fn answer_server_request(id: Value, method: &str, params: Option<&Value>) -> Reply {
    let ok = |result: Value| Reply {
        jsonrpc: "2.0",
        id: id.clone(),
        result: Some(result),
        error: None,
    };
    match method {
        "workspace/configuration" => {
            let n = params
                .and_then(|p| p["items"].as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            ok(Value::Array(vec![Value::Null; n]))
        }
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "window/showMessageRequest"
        | "workspace/diagnostic/refresh"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/codeLens/refresh" => ok(Value::Null),
        "workspace/applyEdit" => {
            ok(json!({ "applied": false, "failureReason": "read-only client" }))
        }
        "workspace/workspaceFolders" => ok(Value::Null),
        _ => Reply {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code: -32601,
                message: format!("method not supported: {method}"),
            }),
        },
    }
}

fn handle_publish_diagnostics(params: &Value, store: &DashMap<String, DiagnosticsEntry>, seq: u64) {
    let uri = normalize_uri(params["uri"].as_str().unwrap_or_default());
    let file = uri_to_path(&uri);
    let raw: Vec<Value> = params["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let items = raw
        .iter()
        .filter_map(|d| parse_diagnostic(d, &file))
        .collect();
    store.insert(
        uri,
        DiagnosticsEntry {
            version: params["version"].as_i64(),
            items,
            raw,
            seq,
        },
    );
}

/// Parse one LSP diagnostic.
pub fn parse_diagnostic(d: &Value, file: &str) -> Option<LspDiagnostic> {
    let range = &d["range"]["start"];
    let line = range["line"].as_u64()? as u32;
    let col = range["character"].as_u64().unwrap_or(0) as u32;
    let severity = d["severity"]
        .as_u64()
        .map(DiagnosticSeverity::from_lsp)
        .unwrap_or(DiagnosticSeverity::Warning);
    let message = d["message"].as_str()?.to_string();
    let source = d["source"].as_str().map(String::from);
    let code = d["code"]
        .as_str()
        .map(String::from)
        .or_else(|| d["code"].as_u64().map(|n| n.to_string()));

    Some(LspDiagnostic {
        file: file.to_string(),
        line,
        col,
        severity,
        message,
        source,
        code,
    })
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Hover content as text (string, `MarkupContent` or `MarkedString[]`).
pub fn hover_text(result: &Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    let contents = &result["contents"];
    let text = if let Some(s) = contents.as_str() {
        s.to_string()
    } else if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
        value.to_string()
    } else if let Some(arr) = contents.as_array() {
        arr.iter()
            .filter_map(|item| {
                item.as_str()
                    .map(String::from)
                    .or_else(|| item.get("value").and_then(|v| v.as_str()).map(String::from))
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        serde_json::to_string_pretty(contents).unwrap_or_default()
    };
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn parse_range(v: &Value) -> Option<Range> {
    Some(Range {
        start: Position {
            line: v["start"]["line"].as_u64()? as u32,
            character: v["start"]["character"].as_u64()? as u32,
        },
        end: Position {
            line: v["end"]["line"].as_u64()? as u32,
            character: v["end"]["character"].as_u64()? as u32,
        },
    })
}

/// Locations from a definition / references / declaration result:
/// `null`, a `Location`, `Location[]` or `LocationLink[]`.
pub fn parse_locations(value: &Value) -> Vec<RawLocation> {
    let one = |loc: &Value| -> Option<RawLocation> {
        if let Some(uri) = loc["targetUri"].as_str() {
            let range = parse_range(&loc["targetSelectionRange"])
                .or_else(|| parse_range(&loc["targetRange"]))?;
            return Some(RawLocation {
                uri: normalize_uri(uri),
                range,
            });
        }
        Some(RawLocation {
            uri: normalize_uri(loc["uri"].as_str()?),
            range: parse_range(&loc["range"])?,
        })
    };
    match value {
        Value::Array(arr) => arr.iter().filter_map(one).collect(),
        Value::Object(_) => one(value).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn format_locations(locs: &[RawLocation]) -> Vec<String> {
    locs.iter()
        .map(|l| {
            format!(
                "{}:{}:{}",
                uri_to_path(&l.uri),
                l.range.start.line + 1,
                l.range.start.character + 1
            )
        })
        .collect()
}

/// Recursively collect document symbols.
fn collect_symbol(value: &Value) -> SymbolInfo {
    let name = value["name"].as_str().unwrap_or("?").to_string();
    let kind_num = value["kind"].as_u64().unwrap_or(0);
    let kind = symbol_kind_name(kind_num).to_string();

    // `DocumentSymbol` has `range`; `SymbolInformation` has `location.range`.
    let range_val = if value["range"].is_object() {
        &value["range"]
    } else {
        &value["location"]["range"]
    };
    let range = parse_range(range_val).unwrap_or_default();

    let children = value["children"]
        .as_array()
        .map(|arr| arr.iter().map(collect_symbol).collect())
        .unwrap_or_default();

    SymbolInfo {
        name,
        kind,
        range,
        children,
    }
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
}

fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Convert a file path to a `file://` URI (percent-encoded).
pub fn path_to_uri(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };

    #[cfg(windows)]
    {
        let p = abs.display().to_string().replace('\\', "/");
        // Keep the drive colon readable (`file:///C:/...`).
        let (drive, rest) = match p.split_once(':') {
            Some((d, r)) if d.len() == 1 => (format!("{d}:"), r.to_string()),
            _ => (String::new(), p.clone()),
        };
        format!("file:///{drive}{}", percent_encode_path(&rest))
    }
    #[cfg(not(windows))]
    {
        format!("file://{}", percent_encode_path(&abs.display().to_string()))
    }
}

/// Convert a `file://` URI to a path string (percent-decoded).
pub fn uri_to_path(uri: &str) -> String {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    // `file://localhost/...`
    let path = path.strip_prefix("localhost").unwrap_or(path);
    let path = percent_decode(path);

    #[cfg(windows)]
    {
        path.strip_prefix('/').unwrap_or(&path).replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        path
    }
}

/// One canonical spelling of a `file://` URI, so the same file has one key
/// whatever escaping the server used.
pub fn normalize_uri(uri: &str) -> String {
    if uri.starts_with("file:") {
        path_to_uri(Path::new(&uri_to_path(uri)))
    } else {
        uri.to_string()
    }
}

/// Format diagnostics for display.
pub fn format_diagnostics(diagnostics: &[LspDiagnostic]) -> String {
    if diagnostics.is_empty() {
        return "No diagnostics.".to_string();
    }

    let mut errors = 0u32;
    let mut warnings = 0u32;
    let mut lines = Vec::new();

    for d in diagnostics {
        match d.severity {
            DiagnosticSeverity::Error => errors += 1,
            DiagnosticSeverity::Warning => warnings += 1,
            _ => {}
        }
        lines.push(d.to_string());
    }

    let summary = format!("{errors} error(s), {warnings} warning(s)");
    lines.push(format!("\n{summary}"));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_are_escaped_and_normalized() {
        let uri = path_to_uri(Path::new("/tmp/Bureau - Mac/é x.rs"));
        assert_eq!(uri, "file:///tmp/Bureau%20-%20Mac/%C3%A9%20x.rs");
        assert_eq!(uri_to_path(&uri), "/tmp/Bureau - Mac/é x.rs");
        // A server spelling with other escapes maps to the same key.
        assert_eq!(
            normalize_uri("file:///tmp/Bureau%20-%20Mac/%c3%a9%20x.rs"),
            uri
        );
        assert_eq!(normalize_uri("file:///tmp/a%2Db.rs"), "file:///tmp/a-b.rs");
    }

    #[test]
    fn location_shapes() {
        assert!(parse_locations(&Value::Null).is_empty());
        let loc = json!({"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 4}}});
        assert_eq!(parse_locations(&loc).len(), 1);
        assert_eq!(parse_locations(&json!([loc, loc])).len(), 2);
        let link = json!([{
            "targetUri": "file:///b.rs",
            "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 1}},
            "targetSelectionRange": {"start": {"line": 3, "character": 7}, "end": {"line": 3, "character": 10}}
        }]);
        let l = parse_locations(&link);
        assert_eq!(l[0].uri, "file:///b.rs");
        assert_eq!(l[0].range.start.line, 3);
        assert_eq!(l[0].range.start.character, 7);
    }

    #[test]
    fn capabilities_forms() {
        let c = ServerCaps::from_initialize(&json!({"capabilities": {
            "positionEncoding": "utf-8",
            "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": true}},
            "definitionProvider": true,
            "referencesProvider": {"workDoneProgress": false},
            "hoverProvider": false,
            "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false}
        }}));
        assert_eq!(c.position_encoding, "utf-8");
        assert!(c.open_close && c.save && c.save_include_text);
        assert_eq!(c.sync, SyncKind::Incremental);
        assert!(c.definition && c.references && !c.hover && c.diagnostic_pull);
        let c = ServerCaps::from_initialize(&json!({"capabilities": {"textDocumentSync": 1}}));
        assert_eq!(c.position_encoding, "utf-16");
        assert_eq!(c.sync, SyncKind::Full);
        assert!(c.open_close && !c.workspace_symbol);
    }

    #[test]
    fn server_requests_are_answered() {
        let r = answer_server_request(
            json!("abc"),
            "workspace/configuration",
            Some(&json!({"items": [{}, {}]})),
        );
        assert_eq!(r.result, Some(json!([null, null])));
        let r = answer_server_request(json!(7), "unknown/method", None);
        assert_eq!(r.error.unwrap().code, -32601);
    }
}
