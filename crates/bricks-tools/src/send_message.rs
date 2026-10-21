//! SendMessage tool: a message to a running sub-agent of this session.
//!
//! The tool only adapts: delivery, scope checks and bounds belong to the
//! agent runtime ([`MessageRouter`], installed by the runtime in the agent's
//! extensions). Without a router nothing is sent, and the tool says so.

use super::*;
use serde::Deserialize;

/// Delivers a message to an agent by its id, checking that the sender may
/// reach it. `Ok` is what really happened (queued, with its id); `Err` why
/// nothing was sent.
pub trait MessageRouter: Send + Sync {
    fn deliver(&self, sender: &Extensions, to: &str, content: String) -> std::result::Result<String, String>;
}

/// The router of the session, in the agent's extensions.
#[derive(Clone)]
pub struct MessageRouterHandle(pub Arc<dyn MessageRouter>);

pub struct SendMessageTool;

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "SendMessage"
    }
    fn description(&self) -> &str {
        "Send a message to one of your running sub-agents by its id (the same as \
         `AgentControl message`): it reads it at its next step."
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

        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let Some(router) = ctx.extensions.get::<MessageRouterHandle>() else {
            return ToolResult::error(
                "no agent runtime in this session: there is no sub-agent to reach; nothing was sent.",
            );
        };
        match router.0.deliver(&ctx.extensions, &input.to, input.content) {
            Ok(text) => ToolResult::success(text),
            Err(text) => ToolResult::error(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use parking_lot::Mutex;

    fn test_ctx(ext: Extensions) -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            session_id: "agent-a".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: ext,
        }
    }

    struct Recorder(Mutex<Vec<(String, String)>>);
    impl MessageRouter for Recorder {
        fn deliver(&self, _: &Extensions, to: &str, content: String) -> std::result::Result<String, String> {
            if to == "agent_gone" {
                return Err("agent_gone has ended; nothing was sent.".into());
            }
            self.0.lock().push((to.into(), content));
            Ok(format!("message queued for {to}"))
        }
    }

    #[tokio::test]
    async fn without_a_router_nothing_is_claimed_sent() {
        let r = SendMessageTool
            .execute(
                serde_json::json!({"to": "agent-b", "content": "Hello B!"}),
                &test_ctx(Extensions::default()),
            )
            .await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("nothing was sent"));
    }

    #[tokio::test]
    async fn the_router_decides_and_its_answer_is_returned() {
        let rec = Arc::new(Recorder(Mutex::new(Vec::new())));
        let ext = Extensions::default();
        ext.insert(MessageRouterHandle(rec.clone()));
        let ok = SendMessageTool
            .execute(
                serde_json::json!({"to": "agent_b", "content": "Hello"}),
                &test_ctx(ext.clone()),
            )
            .await;
        assert!(!ok.is_error && ok.content.contains("queued"));
        assert_eq!(rec.0.lock().as_slice(), &[("agent_b".to_string(), "Hello".to_string())]);
        let gone = SendMessageTool
            .execute(
                serde_json::json!({"to": "agent_gone", "content": "Hello"}),
                &test_ctx(ext),
            )
            .await;
        assert!(gone.is_error && gone.content.contains("nothing was sent"));
    }
}
