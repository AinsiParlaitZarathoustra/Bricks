//! One connection to an MCP server, on the official SDK (`rmcp`).
//!
//! **Lifecycle.** `auto` sends `server/discover` (2026-07-28) and, when the
//! server answers with an error that is not a recognised modern error or
//! does not answer, falls back to the `initialize` handshake of 2025-11-25 —
//! once, at connection; the era is a property of the connection and the two
//! contracts are never mixed on it. `modern` / `legacy` force one era.
//!
//! **Capabilities.** Bricks declares none: no sampling, no elicitation, no
//! roots (sampling and roots are deprecated in 2026-07-28 and were never
//! implemented here). A server that asks for such input anyway — as a
//! legacy server-to-client request or as an `input_required` request — gets
//! a clear refusal; a capability is never announced without a handler.
//!
//! **Calls.** Each request has an idle timeout (reset by progress
//! notifications carrying *its* token, when enabled) and a total timeout no
//! progress extends. `input_required` results (MRTR) are answered and the
//! call retried with a new id and the server's `requestState` echoed, at
//! most `mrtr_max_rounds` times. A call whose future is dropped sends
//! `notifications/cancelled` (stdio) / closes its stream (HTTP). A call cut
//! by a lost connection is reported as such and never replayed: the tool may
//! have run.

use crate::config::{expand_server_config, McpServerConfig, ProtocolMode, TransportKind};
use crate::http::BoundedHttpClient;
use crate::stdio::{CloseReason, StderrLog, StdioProcess};
use crate::McpError;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientConfig, ClientRequest,
    Implementation, InputRequest, InputRequiredResult, InputResponses, ProgressNotificationParam,
    ProgressToken, ProtocolVersion, ReadResourceRequestParams, Request, ServerResult,
};
use rmcp::service::{
    ClientLifecycleMode, ClientServiceExt, NotificationContext, PeerRequestOptions, RunningService,
};
use rmcp::{ClientHandler, RoleClient, ServiceError};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Called with `(progress, total, message)` for the call's own progress.
pub type ProgressFn = Arc<dyn Fn(f64, Option<f64>, Option<String>) + Send + Sync>;

/// The protocol era of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Era {
    /// 2026-07-28 and later: stateless, per-request metadata.
    Modern,
    /// `initialize`-based revisions (2025-11-25 and earlier).
    Legacy,
}

/// What the connection negotiated.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub protocol_version: String,
    pub era: Era,
    pub transport: &'static str,
    pub capabilities: Value,
    pub instructions: Option<String>,
}

/// A tool of a server.
#[derive(Debug, Clone, Serialize)]
pub struct McpToolDef {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    /// `readOnlyHint`, `destructiveHint`… as announced (hints, not
    /// guarantees).
    pub annotations: Option<Value>,
}

impl From<&McpToolDef> for cersei_types::ToolDefinition {
    fn from(t: &McpToolDef) -> Self {
        cersei_types::ToolDefinition {
            name: t.name.clone(),
            description: t.description.clone().unwrap_or_default(),
            input_schema: t.input_schema.clone(),
        }
    }
}

/// One block of a tool result.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContent {
    Text {
        text: String,
    },
    /// Media are described, not inlined: type and decoded size.
    Image {
        mime_type: String,
        bytes: usize,
    },
    Audio {
        mime_type: String,
        bytes: usize,
    },
    ResourceLink {
        uri: String,
        name: Option<String>,
        mime_type: Option<String>,
    },
    Resource {
        uri: String,
        mime_type: Option<String>,
        text: Option<String>,
        blob_bytes: Option<usize>,
    },
    /// A block type this client does not know, kept as JSON.
    Other {
        value: Value,
    },
}

/// A tool result, as the server gave it.
#[derive(Debug, Clone, Serialize)]
pub struct McpCallResult {
    pub content: Vec<McpContent>,
    /// `structuredContent`.
    pub structured: Option<Value>,
    /// The tool reported a failure (`isError`).
    pub is_error: bool,
    /// `input_required` rounds that were answered.
    pub rounds: usize,
    pub protocol_version: String,
}

// ─── Handler ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Handler {
    info: ClientConfig,
    progress: Arc<parking_lot::Mutex<HashMap<ProgressToken, ProgressFn>>>,
    tools_changed: Arc<AtomicBool>,
}

impl ClientHandler for Handler {
    fn get_info(&self) -> ClientConfig {
        self.info.clone()
    }

    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        // Only the call that owns the token hears it; an unknown token is
        // dropped (and extends nothing).
        let cb = self.progress.lock().get(&params.progress_token).cloned();
        if let Some(cb) = cb {
            cb(params.progress, params.total, params.message);
        }
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.tools_changed.store(true, Ordering::SeqCst);
    }
}

fn client_config(cfg: &McpServerConfig) -> ClientConfig {
    let mut info = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("bricks", env!("CARGO_PKG_VERSION")),
    );
    if cfg.protocol == ProtocolMode::Legacy {
        info.protocol_version = ProtocolVersion::V_2025_11_25;
    }
    info
}

// ─── Client ──────────────────────────────────────────────────────────────────

enum Owner {
    Process(tokio::sync::Mutex<StdioProcess>),
    None,
}

pub struct McpClient {
    pub config: McpServerConfig,
    info: ServerInfo,
    tools: parking_lot::RwLock<Vec<McpToolDef>>,
    service: RunningService<RoleClient, Handler>,
    handler: Handler,
    slots: Arc<Semaphore>,
    owner: Owner,
    stderr: Option<StderrLog>,
    close_reason: CloseReason,
}

impl McpClient {
    /// Connect to the configured server (stdio or Streamable HTTP), within
    /// `limits.connect_timeout_ms`, and list its tools.
    pub async fn connect(config: McpServerConfig) -> Result<Self, McpError> {
        let cfg = expand_server_config(&config);
        cfg.validate().map_err(McpError::Config)?;
        let timeout = cfg.limits.connect_timeout();
        let fut = async {
            match cfg.transport().map_err(McpError::Config)? {
                TransportKind::Stdio => {
                    let (process, pipes) =
                        crate::stdio::spawn(&cfg).map_err(|e| McpError::Connect {
                            server: cfg.name.clone(),
                            reason: format!(
                                "cannot start `{}`: {e}",
                                cfg.command.as_deref().unwrap_or_default()
                            ),
                        })?;
                    let stderr = process.stderr.clone();
                    let reason = process.close_reason.clone();
                    let owner = Owner::Process(tokio::sync::Mutex::new(process));
                    Self::start(cfg.clone(), pipes, "stdio", owner)
                        .await
                        .map(|mut c| {
                            c.stderr = Some(stderr);
                            c.close_reason = reason;
                            c
                        })
                }
                TransportKind::StreamableHttp => {
                    let client = BoundedHttpClient::new(cfg.limits.max_message_bytes)
                        .map_err(McpError::Config)?;
                    let mut headers = HashMap::new();
                    for (k, v) in &cfg.headers {
                        let name = http::HeaderName::from_bytes(k.as_bytes())
                            .map_err(|_| McpError::Config(format!("invalid header `{k}`")))?;
                        let value = http::HeaderValue::from_str(v).map_err(|_| {
                            McpError::Config(format!("invalid value for header `{k}`"))
                        })?;
                        headers.insert(name, value);
                    }
                    let mut tcfg = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                        cfg.url.clone().unwrap_or_default(),
                    );
                    tcfg.custom_headers = headers;
                    tcfg.max_concurrent_requests = cfg.limits.max_in_flight;
                    tcfg.max_sse_event_size = cfg.limits.max_message_bytes;
                    // Never re-send a request after a session loss: the tool
                    // may already have run.
                    tcfg.reinit_on_expired_session = false;
                    tcfg.retry_config =
                        Arc::new(rmcp::transport::common::client_side_sse::NeverRetry::default());
                    let reason = client.close_reason();
                    let transport =
                        rmcp::transport::StreamableHttpClientTransport::with_client(client, tcfg);
                    Self::start(cfg.clone(), transport, "streamable_http", Owner::None)
                        .await
                        .map(|mut c| {
                            c.close_reason = reason;
                            c
                        })
                }
            }
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(McpError::Connect {
                server: config.name.clone(),
                reason: format!("no answer within {} ms", timeout.as_millis()),
            }),
        }
    }

    /// Connect over any transport (custom transports, tests).
    pub async fn connect_with<T, E, A>(
        config: McpServerConfig,
        transport: T,
    ) -> Result<Self, McpError>
    where
        T: rmcp::transport::IntoTransport<RoleClient, E, A> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let cfg = expand_server_config(&config);
        cfg.limits.validate().map_err(McpError::Config)?;
        let timeout = cfg.limits.connect_timeout();
        match tokio::time::timeout(timeout, Self::start(cfg, transport, "custom", Owner::None))
            .await
        {
            Ok(r) => r,
            Err(_) => Err(McpError::Connect {
                server: config.name.clone(),
                reason: format!("no answer within {} ms", timeout.as_millis()),
            }),
        }
    }

    async fn start<T, E, A>(
        cfg: McpServerConfig,
        transport: T,
        kind: &'static str,
        owner: Owner,
    ) -> Result<Self, McpError>
    where
        T: rmcp::transport::IntoTransport<RoleClient, E, A> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let handler = Handler {
            info: client_config(&cfg),
            progress: Arc::default(),
            tools_changed: Arc::default(),
        };
        let lifecycle = match cfg.protocol {
            ProtocolMode::Auto => ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
            ProtocolMode::Modern => ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
            ProtocolMode::Legacy => ClientLifecycleMode::Initialize,
        };
        let service = handler
            .clone()
            .serve_with_lifecycle(transport, lifecycle)
            .await
            .map_err(|e| McpError::Connect {
                server: cfg.name.clone(),
                reason: e.to_string(),
            })?;
        let peer = service.peer_info().ok_or_else(|| McpError::Connect {
            server: cfg.name.clone(),
            reason: "the server did not identify its protocol version".into(),
        })?;
        let version = peer.protocol_version.to_string();
        let era = if peer.protocol_version >= ProtocolVersion::V_2026_07_28 {
            Era::Modern
        } else {
            Era::Legacy
        };
        let info = ServerInfo {
            name: cfg.name.clone(),
            server_name: peer.server_info.as_ref().map(|s| s.name.clone()),
            server_version: peer.server_info.as_ref().map(|s| s.version.clone()),
            protocol_version: version,
            era,
            transport: kind,
            capabilities: serde_json::to_value(&peer.capabilities).unwrap_or(Value::Null),
            instructions: peer.instructions.clone(),
        };
        let client = Self {
            slots: Arc::new(Semaphore::new(cfg.limits.max_in_flight)),
            config: cfg,
            info,
            tools: parking_lot::RwLock::new(Vec::new()),
            service,
            handler,
            owner,
            stderr: None,
            close_reason: CloseReason::default(),
        };
        client.refresh_tools().await?;
        tracing::info!(
            server = %client.info.name,
            protocol = %client.info.protocol_version,
            tools = client.tools.read().len(),
            "MCP server connected"
        );
        Ok(client)
    }

    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    pub fn tools(&self) -> Vec<McpToolDef> {
        self.tools.read().clone()
    }

    /// The server's last stderr lines (stdio).
    pub fn stderr(&self) -> Vec<String> {
        self.stderr.as_ref().map(|s| s.lines()).unwrap_or_default()
    }

    /// The connection is gone (process ended, stream closed).
    pub fn is_closed(&self) -> bool {
        self.service.is_closed() || self.service.is_transport_closed()
    }

    /// Re-read the tool list (after `notifications/tools/list_changed`).
    pub async fn refresh_tools(&self) -> Result<(), McpError> {
        if self.info.capabilities.get("tools").is_none() && !self.info.capabilities.is_null() {
            return Ok(());
        }
        let list = self
            .service
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| self.service_error(e, false))?;
        let defs = list
            .into_iter()
            .map(|t| McpToolDef {
                name: t.name.to_string(),
                title: t.title.clone(),
                description: t.description.as_ref().map(|d| d.to_string()),
                input_schema: Value::Object((*t.input_schema).clone()),
                output_schema: t
                    .output_schema
                    .as_ref()
                    .map(|s| Value::Object((**s).clone())),
                annotations: t
                    .annotations
                    .as_ref()
                    .and_then(|a| serde_json::to_value(a).ok()),
            })
            .collect();
        *self.tools.write() = defs;
        self.handler.tools_changed.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn tools_changed(&self) -> bool {
        self.handler.tools_changed.load(Ordering::SeqCst)
    }

    fn options(&self) -> PeerRequestOptions {
        let l = &self.config.limits;
        let mut o = PeerRequestOptions::with_timeout(l.request_timeout());
        o.reset_timeout_on_progress = l.reset_on_progress;
        o.max_total_timeout = Some(l.max_total_timeout());
        o
    }

    fn lost(&self, in_flight: bool) -> McpError {
        McpError::ConnectionLost {
            server: self.info.name.clone(),
            in_flight,
            reason: self.close_reason.get(),
            stderr_tail: self.stderr().into_iter().rev().take(5).rev().collect(),
        }
    }

    fn service_error(&self, e: ServiceError, in_flight: bool) -> McpError {
        match e {
            ServiceError::Timeout { timeout } => McpError::Timeout {
                after: timeout,
                total: timeout == self.config.limits.max_total_timeout(),
            },
            ServiceError::TransportClosed | ServiceError::TransportSend(_) => self.lost(in_flight),
            ServiceError::McpError(err) => McpError::Server {
                code: err.code.0,
                message: err.message.to_string(),
            },
            ServiceError::Cancelled { reason } => McpError::Cancelled(reason),
            ServiceError::InputRequiredRoundsExceeded { max_rounds } => {
                McpError::RoundsExceeded(max_rounds)
            }
            other => McpError::Protocol(other.to_string()),
        }
    }

    /// Call `tool` with `arguments`. `progress` hears this call's progress
    /// notifications.
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: Option<Value>,
        progress: Option<ProgressFn>,
    ) -> Result<McpCallResult, McpError> {
        if self.is_closed() {
            return Err(self.lost(false));
        }
        let args = match arguments {
            None | Some(Value::Null) => None,
            Some(Value::Object(m)) => Some(m),
            Some(other) => {
                return Err(McpError::Config(format!(
                    "tool arguments must be an object, got {other}"
                )))
            }
        };
        let total = self.config.limits.max_total_timeout();
        let _slot = tokio::time::timeout(total, self.slots.clone().acquire_owned())
            .await
            .map_err(|_| McpError::Timeout {
                after: total,
                total: true,
            })?
            .expect("semaphore open");
        let mut params = CallToolRequestParams::new(tool.to_string());
        params.arguments = args;
        let max_rounds = self.config.limits.mrtr_max_rounds;
        let mut rounds = 0;
        loop {
            let request = ClientRequest::CallToolRequest(Request::new(params.clone()));
            let result = self.send(request, progress.clone()).await?;
            let response = match result {
                ServerResult::CallToolResult(r) => CallToolResponse::Complete(r),
                ServerResult::InputRequiredResult(r) => CallToolResponse::InputRequired(r),
                other => {
                    return Err(McpError::Protocol(format!(
                        "unexpected answer to tools/call: {}",
                        serde_json::to_string(&other)
                            .unwrap_or_default()
                            .chars()
                            .take(200)
                            .collect::<String>()
                    )))
                }
            };
            match response {
                CallToolResponse::Complete(r) => {
                    return Ok(McpCallResult {
                        content: r.content.iter().map(convert_content).collect(),
                        structured: r.structured_content.clone(),
                        is_error: r.is_error.unwrap_or(false),
                        rounds,
                        protocol_version: self.info.protocol_version.clone(),
                    })
                }
                CallToolResponse::InputRequired(ir) => {
                    rounds += 1;
                    if rounds > max_rounds {
                        return Err(McpError::RoundsExceeded(max_rounds));
                    }
                    let (responses, state) = self.answer(ir)?;
                    params.input_responses = responses;
                    params.request_state = state;
                }
                _ => {
                    return Err(McpError::Protocol(
                        "the server returned a task handle; the tasks extension is not declared"
                            .into(),
                    ))
                }
            }
        }
    }

    /// The retry of an `input_required` result. Bricks declares no input
    /// capability, so only a state-only result (no `inputRequests`) can be
    /// retried; any input request is refused by name.
    fn answer(
        &self,
        ir: InputRequiredResult,
    ) -> Result<(Option<InputResponses>, Option<String>), McpError> {
        if let Some((_, req)) = ir.input_requests.as_ref().and_then(|m| m.iter().next()) {
            let method = match req {
                InputRequest::ListRoots(_) => "roots/list",
                InputRequest::CreateMessage(_) => "sampling/createMessage",
                InputRequest::Elicitation(_) => "elicitation/create",
                _ => "an unknown input request",
            };
            return Err(McpError::InputNotSupported(method.into()));
        }
        Ok((None, ir.request_state))
    }

    async fn send(
        &self,
        request: ClientRequest,
        progress: Option<ProgressFn>,
    ) -> Result<ServerResult, McpError> {
        let handle = self
            .service
            .peer()
            .send_request_with_option(request, self.options())
            .await
            .map_err(|e| self.service_error(e, false))?;
        let token = handle.progress_token.clone();
        if let Some(cb) = progress {
            self.handler.progress.lock().insert(token.clone(), cb);
        }
        // Dropped before the answer (the agent's turn was cancelled): tell
        // the server the result will not be used.
        let guard = CancelOnDrop {
            peer: Some(handle.peer.clone()),
            id: handle.id.clone(),
        };
        let r = handle.await_response().await;
        guard.disarm();
        self.handler.progress.lock().remove(&token);
        r.map_err(|e| self.service_error(e, true))
    }

    /// Read a resource (text contents joined).
    pub async fn read_resource(&self, uri: &str) -> Result<Vec<McpContent>, McpError> {
        let request = ClientRequest::ReadResourceRequest(Request::new(
            ReadResourceRequestParams::new(uri.to_string()),
        ));
        match self.send(request, None).await? {
            ServerResult::ReadResourceResult(r) => Ok(r
                .contents
                .iter()
                .map(|c| {
                    let v = serde_json::to_value(c).unwrap_or(Value::Null);
                    McpContent::Resource {
                        uri: v
                            .get("uri")
                            .and_then(Value::as_str)
                            .unwrap_or(uri)
                            .to_string(),
                        mime_type: v
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        text: v.get("text").and_then(Value::as_str).map(str::to_string),
                        blob_bytes: v
                            .get("blob")
                            .and_then(Value::as_str)
                            .map(|b| b.len() * 3 / 4),
                    }
                })
                .collect()),
            ServerResult::InputRequiredResult(_) => Err(McpError::InputNotSupported(
                "input_required on resources/read".into(),
            )),
            _ => Err(McpError::Protocol(
                "unexpected answer to resources/read".into(),
            )),
        }
    }

    /// Close the connection: transport closed, then the process (stdio)
    /// given time to exit before being terminated.
    pub async fn close(&self) {
        // Stops the service loop, which closes the transport (stdin).
        self.service.cancellation_token().cancel();
        if let Owner::Process(p) = &self.owner {
            p.lock().await.shutdown(Duration::from_secs(2)).await;
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Owner::Process(p) = &self.owner {
            if let Ok(mut p) = p.try_lock() {
                p.kill_now();
            }
        }
    }
}

struct CancelOnDrop {
    peer: Option<rmcp::Peer<RoleClient>>,
    id: rmcp::model::RequestId,
}

impl CancelOnDrop {
    fn disarm(mut self) {
        self.peer = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.take() {
            let id = self.id.clone();
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    let _ = peer
                        .notify_cancelled(rmcp::model::CancelledNotificationParam::new(
                            Some(id),
                            Some("the caller stopped waiting".into()),
                        ))
                        .await;
                });
            }
        }
    }
}

fn convert_content(c: &rmcp::model::ContentBlock) -> McpContent {
    let v = serde_json::to_value(c).unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let b64 = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(|d| d.len() / 4 * 3)
            .unwrap_or(0)
    };
    match v.get("type").and_then(Value::as_str) {
        Some("text") => McpContent::Text {
            text: s("text").unwrap_or_default(),
        },
        Some("image") => McpContent::Image {
            mime_type: s("mimeType").unwrap_or_default(),
            bytes: b64("data"),
        },
        Some("audio") => McpContent::Audio {
            mime_type: s("mimeType").unwrap_or_default(),
            bytes: b64("data"),
        },
        Some("resource_link") => McpContent::ResourceLink {
            uri: s("uri").unwrap_or_default(),
            name: s("name"),
            mime_type: s("mimeType"),
        },
        Some("resource") => {
            let r = v.get("resource").cloned().unwrap_or(Value::Null);
            let rs = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
            McpContent::Resource {
                uri: rs("uri").unwrap_or_default(),
                mime_type: rs("mimeType"),
                text: rs("text"),
                blob_bytes: r
                    .get("blob")
                    .and_then(Value::as_str)
                    .map(|d| d.len() / 4 * 3),
            }
        }
        _ => McpContent::Other { value: v },
    }
}
