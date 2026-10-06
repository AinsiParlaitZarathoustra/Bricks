//! MCP server configuration.
//!
//! ```toml
//! # stdio: Bricks launches the server.
//! { name = "files", type = "stdio", command = "mcp-files", args = ["--root", "."] }
//! # Streamable HTTP: one endpoint, POST per message.
//! { name = "docs", type = "http", url = "https://mcp.example.com/mcp",
//!   headers = { Authorization = "Bearer ${DOCS_TOKEN}" } }
//! ```
//!
//! `protocol` selects the protocol era: `auto` (default) probes with
//! `server/discover` (2026-07-28) and falls back to the `initialize`
//! handshake of 2025-11-25 for older servers; `modern` and `legacy` force
//! one. The old two-endpoint HTTP+SSE transport (2024-11-05, deprecated) is
//! not supported: a `type = "sse"` server is refused with an explanation.
//!
//! `${VAR}` / `${VAR:-default}` are expanded in command, arguments, env,
//! URL and header values; values are never logged.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Which protocol revisions to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolMode {
    /// `server/discover` first; `initialize` (2025-11-25) when the server is
    /// legacy.
    #[default]
    Auto,
    /// 2026-07-28 only (stateless, per-request metadata).
    Modern,
    /// The `initialize` handshake (2025-11-25) only.
    Legacy,
}

/// Bounds of one server connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpLimits {
    /// Startup: spawn or first request, discovery or handshake, tool list.
    pub connect_timeout_ms: u64,
    /// Silence allowed during a request (reset by its own progress
    /// notifications when `reset_on_progress`).
    pub request_timeout_ms: u64,
    /// Hard limit of one request, progress or not.
    pub max_total_timeout_ms: u64,
    pub reset_on_progress: bool,
    /// Largest message accepted from the server (stdio line, HTTP JSON body,
    /// SSE event).
    pub max_message_bytes: usize,
    /// Requests in flight on this connection.
    pub max_in_flight: usize,
    /// `input_required` rounds of one call before giving up.
    pub mrtr_max_rounds: usize,
}

impl Default for McpLimits {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 20_000,
            request_timeout_ms: 60_000,
            max_total_timeout_ms: 300_000,
            reset_on_progress: true,
            max_message_bytes: 16 * 1024 * 1024,
            max_in_flight: 16,
            mrtr_max_rounds: 4,
        }
    }
}

impl McpLimits {
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }
    pub fn max_total_timeout(&self) -> Duration {
        Duration::from_millis(self.max_total_timeout_ms)
    }

    pub fn validate(&self) -> Result<(), String> {
        let check = |name: &str, v: u64, max: u64| {
            if v == 0 || v > max {
                Err(format!("{name} must be between 1 and {max} (got {v})"))
            } else {
                Ok(())
            }
        };
        check("connect_timeout_ms", self.connect_timeout_ms, 600_000)?;
        check("request_timeout_ms", self.request_timeout_ms, 3_600_000)?;
        check(
            "max_total_timeout_ms",
            self.max_total_timeout_ms,
            86_400_000,
        )?;
        if self.request_timeout_ms > self.max_total_timeout_ms {
            return Err("request_timeout_ms must not exceed max_total_timeout_ms".into());
        }
        check("max_message_bytes", self.max_message_bytes as u64, 1 << 30)?;
        check("max_in_flight", self.max_in_flight as u64, 1024)?;
        check("mrtr_max_rounds", self.mrtr_max_rounds as u64, 32)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    /// `stdio` (default) or `http` (Streamable HTTP; `streamable-http` is
    /// accepted). `sse` names the deprecated HTTP+SSE transport and is refused.
    #[serde(rename = "type", default = "default_type")]
    pub server_type: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory of a stdio server.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    pub url: Option<String>,
    /// HTTP headers sent with every request (e.g. `Authorization`).
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub protocol: ProtocolMode,
    #[serde(default)]
    pub limits: McpLimits,
}

fn default_type() -> String {
    "stdio".to_string()
}

/// The transport a configuration resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportKind {
    Stdio,
    StreamableHttp,
}

impl McpServerConfig {
    pub fn stdio(name: impl Into<String>, command: impl Into<String>, args: &[&str]) -> Self {
        Self {
            name: name.into(),
            server_type: "stdio".into(),
            command: Some(command.into()),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
            cwd: None,
            url: None,
            headers: HashMap::new(),
            protocol: ProtocolMode::Auto,
            limits: McpLimits::default(),
        }
    }

    /// A Streamable HTTP server.
    pub fn http(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            server_type: "http".into(),
            command: None,
            url: Some(url.into()),
            ..Self::stdio(name, "", &[])
        }
    }

    pub fn with_protocol(mut self, protocol: ProtocolMode) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn with_limits(mut self, limits: McpLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn transport(&self) -> Result<TransportKind, String> {
        match self.server_type.as_str() {
            "stdio" => Ok(TransportKind::Stdio),
            "http" | "streamable-http" | "streamable_http" => Ok(TransportKind::StreamableHttp),
            "sse" => Err(format!(
                "MCP server '{}': the HTTP+SSE transport of protocol 2024-11-05 (separate SSE \
                 and POST endpoints) is deprecated and not supported; use the server's \
                 Streamable HTTP endpoint with type = \"http\"",
                self.name
            )),
            other => Err(format!(
                "MCP server '{}': unknown transport type `{other}` (stdio or http)",
                self.name
            )),
        }
    }

    /// Check the configuration (after `${VAR}` expansion).
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("an MCP server needs a name".into());
        }
        self.limits
            .validate()
            .map_err(|e| format!("MCP server '{}': limits.{e}", self.name))?;
        match self.transport()? {
            TransportKind::Stdio => {
                if self.command.as_deref().is_none_or(|c| c.trim().is_empty()) {
                    return Err(format!("MCP server '{}': stdio needs `command`", self.name));
                }
            }
            TransportKind::StreamableHttp => {
                let url = self
                    .url
                    .as_deref()
                    .ok_or_else(|| format!("MCP server '{}': http needs `url`", self.name))?;
                let u = url::Url::parse(url)
                    .map_err(|e| format!("MCP server '{}': invalid url: {e}", self.name))?;
                if !matches!(u.scheme(), "http" | "https") {
                    return Err(format!(
                        "MCP server '{}': url must be http or https",
                        self.name
                    ));
                }
                for k in self.headers.keys() {
                    http::HeaderName::from_bytes(k.as_bytes()).map_err(|_| {
                        format!("MCP server '{}': invalid header name `{k}`", self.name)
                    })?;
                }
            }
        }
        Ok(())
    }
}

// ─── Env var expansion ───────────────────────────────────────────────────────

/// Expand `${VAR}` and `${VAR:-default}` patterns.
pub fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    let mut search_from = 0;
    loop {
        match result[search_from..].find("${") {
            None => break,
            Some(rel_start) => {
                let start = search_from + rel_start;
                match result[start..].find('}') {
                    None => break,
                    Some(rel_end) => {
                        let end = start + rel_end;
                        let inner = &result[start + 2..end];
                        let (var_name, default_value) = if let Some(pos) = inner.find(":-") {
                            (&inner[..pos], Some(&inner[pos + 2..]))
                        } else {
                            (inner, None)
                        };

                        let replacement = match std::env::var(var_name) {
                            Ok(val) => val,
                            Err(_) => match default_value {
                                Some(def) => def.to_string(),
                                None => {
                                    search_from = end + 1;
                                    continue;
                                }
                            },
                        };

                        result =
                            format!("{}{}{}", &result[..start], replacement, &result[end + 1..]);
                        search_from = start + replacement.len();
                    }
                }
            }
        }
    }
    result
}

/// Expand env vars in all string fields of a server config.
pub fn expand_server_config(config: &McpServerConfig) -> McpServerConfig {
    McpServerConfig {
        command: config.command.as_deref().map(expand_env_vars),
        args: config.args.iter().map(|a| expand_env_vars(a)).collect(),
        env: config
            .env
            .iter()
            .map(|(k, v)| (k.clone(), expand_env_vars(v)))
            .collect(),
        url: config.url.as_deref().map(expand_env_vars),
        headers: config
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), expand_env_vars(v)))
            .collect(),
        ..config.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env_vars_simple() {
        std::env::set_var("CERSEI_TEST_VAR", "hello");
        assert_eq!(expand_env_vars("${CERSEI_TEST_VAR}"), "hello");
        std::env::remove_var("CERSEI_TEST_VAR");
    }

    #[test]
    fn test_expand_env_vars_default() {
        assert_eq!(expand_env_vars("${NONEXISTENT_VAR:-fallback}"), "fallback");
    }

    #[test]
    fn test_expand_env_vars_missing_no_default() {
        let result = expand_env_vars("${CERSEI_MISSING_XYZ}");
        assert_eq!(result, "${CERSEI_MISSING_XYZ}"); // left as-is
    }

    #[test]
    fn test_expand_env_vars_multiple() {
        std::env::set_var("CERSEI_A", "one");
        std::env::set_var("CERSEI_B", "two");
        assert_eq!(expand_env_vars("${CERSEI_A}-${CERSEI_B}"), "one-two");
        std::env::remove_var("CERSEI_A");
        std::env::remove_var("CERSEI_B");
    }

    #[test]
    fn transports_and_the_legacy_sse_refusal() {
        let s = McpServerConfig::stdio("test", "node", &["server.js"]);
        assert_eq!(s.transport(), Ok(TransportKind::Stdio));
        assert!(s.validate().is_ok());
        let h = McpServerConfig::http("remote", "https://mcp.example.com/mcp");
        assert_eq!(h.transport(), Ok(TransportKind::StreamableHttp));
        assert!(h.validate().is_ok());
        let mut sse = h.clone();
        sse.server_type = "sse".into();
        let e = sse.validate().unwrap_err();
        assert!(
            e.contains("deprecated") && e.contains("type = \"http\""),
            "{e}"
        );
        let toml_like: McpServerConfig = serde_json::from_value(serde_json::json!({
            "name": "x", "command": "srv", "protocol": "legacy",
            "limits": {"request_timeout_ms": 1000}
        }))
        .unwrap();
        assert_eq!(toml_like.protocol, ProtocolMode::Legacy);
        assert_eq!(toml_like.limits.request_timeout_ms, 1000);
        assert_eq!(toml_like.limits.max_in_flight, 16);
    }

    #[test]
    fn invalid_limits_are_refused() {
        let mut c = McpServerConfig::stdio("t", "srv", &[]);
        c.limits.request_timeout_ms = 10_000_000;
        assert!(c.validate().is_err());
        c.limits = McpLimits::default();
        c.limits.max_in_flight = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_expand_server_config() {
        std::env::set_var("CERSEI_MCP_CMD", "/usr/bin/node");
        let mut config =
            McpServerConfig::stdio("test", "${CERSEI_MCP_CMD}", &["${CERSEI_MCP_CMD}"]);
        config.env = HashMap::from([("KEY".into(), "${CERSEI_MCP_CMD}".into())]);
        config.headers = HashMap::from([("X".into(), "${CERSEI_MCP_CMD}".into())]);
        let expanded = expand_server_config(&config);
        assert_eq!(expanded.command.as_deref(), Some("/usr/bin/node"));
        assert_eq!(expanded.args[0], "/usr/bin/node");
        assert_eq!(expanded.env["KEY"], "/usr/bin/node");
        assert_eq!(expanded.headers["X"], "/usr/bin/node");
        std::env::remove_var("CERSEI_MCP_CMD");
    }
}
