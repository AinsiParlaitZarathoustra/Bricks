//! An MCP server's tool as an agent tool.
//!
//! Named `mcp__<server>__<tool>` (characters outside `[A-Za-z0-9_-]`
//! replaced, at most 64). The result keeps what the server returned: text
//! blocks in order, the structured content (`structuredContent`) as JSON,
//! media described by type and size, resource links, and the tool's own
//! failure flag (`isError`) as the status. Progress notifications of the
//! call are forwarded to the agent's progress events. Permission level comes
//! from the tool's annotations (`readOnlyHint`, `destructiveHint`), which are
//! hints from the server, not guarantees; without them the tool is treated
//! as executing code.

use super::*;
use crate::tool_report::{ToolBody, ToolReport, ToolStatus};
use cersei_mcp::{McpContent, McpError, McpManager, McpToolDef};
use std::fmt::Write as _;
use std::time::Instant;

pub struct McpTool {
    manager: Arc<McpManager>,
    server: String,
    def: McpToolDef,
    name: String,
    description: String,
}

impl McpTool {
    pub fn new(manager: Arc<McpManager>, server: &str, def: McpToolDef) -> Self {
        let name = tool_name(server, &def.name);
        let description = format!(
            "[MCP server '{server}'] {}",
            def.description.as_deref().unwrap_or("(no description)")
        );
        Self {
            manager,
            server: server.to_string(),
            def,
            name,
            description,
        }
    }

    pub fn server(&self) -> &str {
        &self.server
    }
}

/// `mcp__<server>__<tool>`, restricted to `[A-Za-z0-9_-]`, at most 64.
pub fn tool_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let mut n = format!("mcp__{}__{}", clean(server), clean(tool));
    n.truncate(64);
    n
}

/// All tools of a manager.
pub async fn tools_of(manager: &Arc<McpManager>) -> Vec<Box<dyn Tool>> {
    manager
        .tools()
        .await
        .into_iter()
        .map(|(server, def)| Box::new(McpTool::new(manager.clone(), &server, def)) as Box<dyn Tool>)
        .collect()
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn permission_level(&self) -> PermissionLevel {
        let hint = |k: &str| {
            self.def
                .annotations
                .as_ref()
                .and_then(|a| a.get(k))
                .and_then(Value::as_bool)
        };
        match (hint("readOnlyHint"), hint("destructiveHint")) {
            (Some(true), _) => PermissionLevel::ReadOnly,
            (_, Some(true)) => PermissionLevel::Dangerous,
            _ => PermissionLevel::Execute,
        }
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }
    fn input_schema(&self) -> Value {
        self.def.input_schema.clone()
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let start = Instant::now();
        let progress: Option<cersei_mcp::ProgressFn> = ctx
            .extensions
            .get::<crate::shell::ProgressSink>()
            .map(|sink| {
                let name = self.name.clone();
                Arc::new(move |p: f64, total: Option<f64>, msg: Option<String>| {
                    let mut line = match total {
                        Some(t) => format!("{p}/{t}"),
                        None => format!("{p}"),
                    };
                    if let Some(m) = msg {
                        line.push_str(" — ");
                        line.push_str(&m);
                    }
                    (sink.0)(&name, &line);
                }) as cersei_mcp::ProgressFn
            });
        let args = if input.is_null() { None } else { Some(input) };
        let outcome = self
            .manager
            .call(&self.server, &self.def.name, args, progress)
            .await;
        let mut report =
            match outcome {
                Ok(r) => {
                    let mut report = ToolReport::new(
                        if r.is_error {
                            ToolStatus::Failure
                        } else {
                            ToolStatus::Success
                        },
                        ToolBody::Text(render(&r.content, r.structured.as_ref())),
                    );
                    if r.rounds > 0 {
                        report
                            .notes
                            .push(format!("{} input_required round(s) answered", r.rounds));
                    }
                    report.data = Some(serde_json::json!({
                        "server": self.server,
                        "tool": self.def.name,
                        "protocol_version": r.protocol_version,
                        "is_error": r.is_error,
                        "structured_content": r.structured,
                        "content": r.content,
                    }));
                    report
                }
                Err(e) => {
                    let mut report =
                        ToolReport::new(ToolStatus::Failure, ToolBody::Text(e.to_string()));
                    if let McpError::Timeout { .. } = e {
                        report.status = ToolStatus::TimedOut;
                    }
                    report.suggestion = match &e {
                    McpError::ConnectionLost { in_flight: true, .. } => Some(
                        "check whether the action took effect before calling again: the call was \
                         interrupted, not retried"
                            .into(),
                    ),
                    McpError::InputNotSupported(_) => Some(
                        "this tool needs interactive input or sampling, which Bricks does not \
                         provide"
                            .into(),
                    ),
                    _ => None,
                };
                    report.data = Some(serde_json::json!({
                        "server": self.server,
                        "tool": self.def.name,
                        "error": e.to_string(),
                    }));
                    report
                }
            };
        report.duration = Some(start.elapsed());
        ToolResult::from_report(report)
    }
}

fn render(content: &[McpContent], structured: Option<&Value>) -> String {
    let mut out = String::new();
    for c in content {
        match c {
            McpContent::Text { text } => {
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }
            McpContent::Image { mime_type, bytes } => {
                let _ = writeln!(out, "[image {mime_type}, {bytes} bytes — not shown]");
            }
            McpContent::Audio { mime_type, bytes } => {
                let _ = writeln!(out, "[audio {mime_type}, {bytes} bytes — not shown]");
            }
            McpContent::ResourceLink { uri, name, .. } => {
                let _ = writeln!(
                    out,
                    "[resource link {}{uri}]",
                    name.as_deref()
                        .map(|n| format!("{n}: "))
                        .unwrap_or_default()
                );
            }
            McpContent::Resource {
                uri,
                text,
                blob_bytes,
                mime_type,
            } => {
                let _ = writeln!(
                    out,
                    "--- resource {uri} ({}) ---",
                    mime_type.as_deref().unwrap_or("?")
                );
                match (text, blob_bytes) {
                    (Some(t), _) => {
                        out.push_str(t);
                        out.push('\n');
                    }
                    (None, Some(n)) => {
                        let _ = writeln!(out, "[binary, {n} bytes — not shown]");
                    }
                    _ => {}
                }
            }
            McpContent::Other { value } => {
                let _ = writeln!(out, "[unrecognised content block: {value}]");
            }
        }
    }
    if let Some(s) = structured {
        let _ = writeln!(
            out,
            "--- structured content ---\n{}",
            serde_json::to_string_pretty(s).unwrap_or_default()
        );
    }
    if out.is_empty() {
        out.push_str("(the tool returned no content)\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_api_safe() {
        assert_eq!(tool_name("files", "read_file"), "mcp__files__read_file");
        assert_eq!(tool_name("my server", "a.b/c"), "mcp__my_server__a_b_c");
        assert!(tool_name(&"s".repeat(50), &"t".repeat(50)).len() <= 64);
    }

    #[test]
    fn content_keeps_order_structure_and_describes_media() {
        let text = render(
            &[
                McpContent::Text {
                    text: "first".into(),
                },
                McpContent::Image {
                    mime_type: "image/png".into(),
                    bytes: 1234,
                },
                McpContent::Text {
                    text: "second".into(),
                },
            ],
            Some(&serde_json::json!({"n": 1})),
        );
        assert_eq!(
            text,
            "first\n[image image/png, 1234 bytes — not shown]\nsecond\n--- structured content ---\n{\n  \"n\": 1\n}\n"
        );
    }
}
