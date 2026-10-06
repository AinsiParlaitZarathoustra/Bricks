//! cersei-mcp: Model Context Protocol client.
//!
//! Built on the official Rust SDK (`rmcp` 3.5) behind Bricks' types:
//! protocol revision **2026-07-28** (stateless requests with per-request
//! metadata, `server/discover`, `resultType`, multi round-trip requests)
//! and, for older servers, the `initialize`-based **2025-11-25** — detected
//! per connection (see [`client`]). Transports: **stdio** and **Streamable
//! HTTP**. The deprecated HTTP+SSE transport (2024-11-05) is not supported.
//!
//! [`McpManager`] holds the connections of an agent, exposes their tools and
//! routes calls; [`McpClient`] is one connection.

pub mod client;
pub mod config;
pub mod http;
pub mod stdio;

pub use client::{Era, McpCallResult, McpClient, McpContent, McpToolDef, ProgressFn, ServerInfo};
pub use config::{expand_env_vars, expand_server_config, McpLimits, McpServerConfig, ProtocolMode};

use cersei_types::ToolDefinition;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Errors of MCP connections and calls.
#[derive(Debug, Clone, thiserror::Error)]
pub enum McpError {
    #[error("MCP configuration: {0}")]
    Config(String),
    #[error("MCP server '{server}' could not be reached: {reason}")]
    Connect { server: String, reason: String },
    #[error("the MCP request {} after {after:?}", if *total { "hit its total time limit" } else { "got no answer" })]
    Timeout { after: Duration, total: bool },
    #[error("the connection to MCP server '{server}' was lost{}{}{}", reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default(), if *in_flight { " during the call; it was not retried (the tool may have run)" } else { "" }, if stderr_tail.is_empty() { String::new() } else { format!(" — server log: {}", stderr_tail.join(" | ")) })]
    ConnectionLost {
        server: String,
        in_flight: bool,
        reason: Option<String>,
        stderr_tail: Vec<String>,
    },
    #[error("MCP server error {code}: {message}")]
    Server { code: i32, message: String },
    #[error("the MCP request was cancelled{}", .0.as_deref().map(|r| format!(": {r}")).unwrap_or_default())]
    Cancelled(Option<String>),
    #[error(
        "the server needs `{0}` input, which Bricks does not provide (capability not declared)"
    )]
    InputNotSupported(String),
    #[error("the server still needed more input after {0} round(s) (input_required)")]
    RoundsExceeded(usize),
    #[error("malformed or unexpected MCP response: {0}")]
    Protocol(String),
    #[error("no MCP server '{server}' has a tool '{tool}'")]
    UnknownTool { server: String, tool: String },
}

impl From<McpError> for cersei_types::CerseiError {
    fn from(e: McpError) -> Self {
        cersei_types::CerseiError::Mcp(e.to_string())
    }
}

/// Connection state of a configured server.
#[derive(Debug, Clone, PartialEq)]
pub enum McpServerStatus {
    Connected,
    Error(String),
    Disconnected,
}

/// The MCP servers of an agent.
pub struct McpManager {
    configs: Vec<McpServerConfig>,
    clients: tokio::sync::RwLock<HashMap<String, Arc<McpClient>>>,
    errors: parking_lot::Mutex<HashMap<String, String>>,
}

impl McpManager {
    /// Connect to every server, concurrently, each within its own connect
    /// timeout. A server that fails is reported in [`Self::statuses`]; the
    /// others are usable.
    pub async fn connect(configs: &[McpServerConfig]) -> Self {
        let results =
            futures::future::join_all(configs.iter().map(|c| McpClient::connect(c.clone()))).await;
        let mut clients = HashMap::new();
        let mut errors = HashMap::new();
        for (c, r) in configs.iter().zip(results) {
            match r {
                Ok(client) => {
                    clients.insert(c.name.clone(), Arc::new(client));
                }
                Err(e) => {
                    tracing::warn!(server = %c.name, error = %e, "MCP server not connected");
                    errors.insert(c.name.clone(), e.to_string());
                }
            }
        }
        Self {
            configs: configs.to_vec(),
            clients: tokio::sync::RwLock::new(clients),
            errors: parking_lot::Mutex::new(errors),
        }
    }

    /// A manager over already connected clients (custom transports, tests).
    pub fn from_clients(clients: Vec<McpClient>) -> Self {
        Self {
            configs: clients.iter().map(|c| c.config.clone()).collect(),
            clients: tokio::sync::RwLock::new(
                clients
                    .into_iter()
                    .map(|c| (c.config.name.clone(), Arc::new(c)))
                    .collect(),
            ),
            errors: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// `(server, tool)` for every tool of every connected server, in a
    /// stable order.
    pub async fn tools(&self) -> Vec<(String, McpToolDef)> {
        let clients = self.clients.read().await;
        let mut names: Vec<&String> = clients.keys().collect();
        names.sort();
        names
            .into_iter()
            .flat_map(|n| clients[n].tools().into_iter().map(move |t| (n.clone(), t)))
            .collect()
    }

    pub async fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools()
            .await
            .iter()
            .map(|(_, t)| ToolDefinition::from(t))
            .collect()
    }

    pub async fn client(&self, server: &str) -> Option<Arc<McpClient>> {
        self.clients.read().await.get(server).cloned()
    }

    /// Call `tool` on `server`. A connection found closed is re-opened once
    /// before the call; a call interrupted by a lost connection is never
    /// replayed.
    pub async fn call(
        &self,
        server: &str,
        tool: &str,
        arguments: Option<serde_json::Value>,
        progress: Option<ProgressFn>,
    ) -> Result<McpCallResult, McpError> {
        let client = self.live_client(server).await?;
        if client.tools_changed() {
            let _ = client.refresh_tools().await;
        }
        if !client.tools().iter().any(|t| t.name == tool) {
            return Err(McpError::UnknownTool {
                server: server.into(),
                tool: tool.into(),
            });
        }
        client.call_tool(tool, arguments, progress).await
    }

    async fn live_client(&self, server: &str) -> Result<Arc<McpClient>, McpError> {
        let existing = self.clients.read().await.get(server).cloned();
        match existing {
            Some(c) if !c.is_closed() => Ok(c),
            _ => {
                let config = self
                    .configs
                    .iter()
                    .find(|c| c.name == server)
                    .cloned()
                    .ok_or_else(|| McpError::Config(format!("no MCP server named '{server}'")))?;
                let fresh = Arc::new(McpClient::connect(config).await.inspect_err(|e| {
                    self.errors.lock().insert(server.to_string(), e.to_string());
                })?);
                self.errors.lock().remove(server);
                self.clients
                    .write()
                    .await
                    .insert(server.to_string(), fresh.clone());
                Ok(fresh)
            }
        }
    }

    /// Call a tool by its bare name, on the first server that has it (the
    /// former API; prefer [`Self::call`]).
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<McpCallResult, McpError> {
        let server = self
            .tools()
            .await
            .into_iter()
            .find(|(_, t)| t.name == tool_name)
            .map(|(s, _)| s)
            .ok_or_else(|| McpError::UnknownTool {
                server: "*".into(),
                tool: tool_name.into(),
            })?;
        self.call(&server, tool_name, arguments, None).await
    }

    pub async fn statuses(&self) -> HashMap<String, McpServerStatus> {
        let clients = self.clients.read().await;
        let errors = self.errors.lock();
        self.configs
            .iter()
            .map(|c| {
                let status = match (clients.get(&c.name), errors.get(&c.name)) {
                    (_, Some(e)) => McpServerStatus::Error(e.clone()),
                    (Some(cl), None) if cl.is_closed() => McpServerStatus::Disconnected,
                    (Some(_), None) => McpServerStatus::Connected,
                    (None, None) => McpServerStatus::Disconnected,
                };
                (c.name.clone(), status)
            })
            .collect()
    }

    pub async fn infos(&self) -> Vec<ServerInfo> {
        let clients = self.clients.read().await;
        let mut v: Vec<ServerInfo> = clients.values().map(|c| c.info().clone()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn configs(&self) -> &[McpServerConfig] {
        &self.configs
    }

    /// Close every connection.
    pub async fn close(&self) {
        let clients: Vec<Arc<McpClient>> =
            self.clients.write().await.drain().map(|(_, c)| c).collect();
        for c in clients {
            c.close().await;
        }
    }
}
