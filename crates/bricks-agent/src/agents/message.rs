//! SendMessage tool: a message to a running sub-agent of this session.
//!
//! Native adapter of the runtime: delivery, scope checks and bounds are
//! shared with the desktop controls. Without a runtime nothing is sent.

use super::runtime::RuntimeHandle;
use super::tool::{message_text, viewer};
use async_trait::async_trait;
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::Value;

pub struct SendMessageTool;

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "SendMessage"
    }
    fn description(&self) -> &str {
        "Send a message to one of your running sub-agents by its id: it reads it at its next step."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "to": { "type": "string", "description": "The sub-agent's id (agent_…)" },
                "content": { "type": "string", "description": "Message content" }
            },
            "required": ["to", "content"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            to: String,
            content: String,
        }

        let input: Input = match bricks_tools::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let Some(runtime) = ctx.extensions.get::<RuntimeHandle>() else {
            return ToolResult::error(
                "no agent runtime in this session: there is no sub-agent to reach; nothing was sent.",
            );
        };
        match message_text(
            &input.to,
            runtime
                .0
                .send_message(&input.to, viewer(ctx).as_deref(), input.content),
        ) {
            Ok(text) => ToolResult::success(text),
            Err(text) => ToolResult::error(text),
        }
    }
}
