//! AgentTool: spawn a sub-agent to handle one sub-task.
//!
//! Each sub-agent runs its own agentic loop with independent message
//! history. The invariants it shares with `delegate` are in
//! [`crate::subagent`]: an empty task is refused before anything is built,
//! the child has its parent's permissions and at most its parent's tools
//! (never a delegation tool), it is cancelled with the parent's run, and a
//! child stopped at a limit is reported as incomplete.

use crate::delegate::{ToolsetFactory, MAX_DEPTH};
use crate::subagent;
use crate::Agent;
use async_trait::async_trait;
use cersei_provider::Provider;
use cersei_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Default turn limit of a sub-agent.
pub const DEFAULT_SUBAGENT_TURNS: u32 = 10;
/// Default largest turn limit a call may ask for.
pub const DEFAULT_SUBAGENT_TURNS_CAP: u32 = 50;

/// The AgentTool — spawns independent sub-agents.
pub struct AgentTool {
    provider_factory: Arc<dyn Fn() -> Box<dyn Provider> + Send + Sync>,
    toolset_factory: ToolsetFactory,
    /// Tools named at construction that cannot be rebuilt for a child:
    /// refused at execution rather than silently dropped.
    unavailable: Vec<String>,
    max_turns: u32,
    max_turns_cap: u32,
}

impl AgentTool {
    /// A tool whose sub-agents get the tools named by `tools` (minus the
    /// delegation tools), rebuilt from the standard registry. A name the
    /// registry cannot rebuild makes every call fail with that name; an
    /// empty list gives the sub-agent no tools. Use
    /// [`AgentTool::with_toolset`] for tools outside the registry.
    pub fn new(
        provider_factory: impl Fn() -> Box<dyn Provider> + Send + Sync + 'static,
        tools: Vec<Box<dyn Tool>>,
    ) -> Self {
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.name().to_string())
            .filter(|n| !subagent::DELEGATION_TOOLS.contains(&n.as_str()))
            .collect();
        let registry: Vec<String> = cersei_tools::all()
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        let unavailable = names
            .iter()
            .filter(|n| !registry.contains(n))
            .cloned()
            .collect();
        let factory: ToolsetFactory = Arc::new(move || {
            cersei_tools::all()
                .into_iter()
                .filter(|t| names.iter().any(|n| n == t.name()))
                .collect()
        });
        Self {
            provider_factory: Arc::new(provider_factory),
            toolset_factory: factory,
            unavailable,
            max_turns: DEFAULT_SUBAGENT_TURNS,
            max_turns_cap: DEFAULT_SUBAGENT_TURNS_CAP,
        }
    }

    /// A tool whose sub-agents get a fresh set from `toolset_factory`
    /// (minus the delegation tools).
    pub fn with_toolset(
        provider_factory: impl Fn() -> Box<dyn Provider> + Send + Sync + 'static,
        toolset_factory: ToolsetFactory,
    ) -> Self {
        Self {
            provider_factory: Arc::new(provider_factory),
            toolset_factory,
            unavailable: Vec::new(),
            max_turns: DEFAULT_SUBAGENT_TURNS,
            max_turns_cap: DEFAULT_SUBAGENT_TURNS_CAP,
        }
    }

    /// Turn limit when the call gives none.
    pub fn with_max_turns(mut self, n: u32) -> Self {
        self.max_turns = n.max(1);
        self
    }

    /// Largest turn limit a call may ask for.
    pub fn with_max_turns_cap(mut self, n: u32) -> Self {
        self.max_turns_cap = n.max(1);
        self
    }
}

#[derive(Debug, Deserialize)]
struct AgentInput {
    description: String,
    prompt: String,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    max_turns: Option<u32>,
    #[serde(default)]
    model: Option<String>,
}

const CHILD_SYSTEM_PROMPT: &str =
    "You are a sub-agent working on one task given by another agent. \
Do that task, within its scope, then reply with your result: what you did, what you found, \
and anything left undone. Stop as soon as the task is done; do not widen it.";

impl AgentTool {
    /// Everything that can be refused without building anything.
    fn check(&self, input: &AgentInput, ctx: &ToolContext) -> Result<u32, String> {
        if subagent::is_blank(&input.prompt) {
            return Err(
                "`prompt` is empty: give the sub-agent a precise task. Nothing was started.".into(),
            );
        }
        if input
            .model
            .as_deref()
            .is_some_and(|m| !subagent::is_blank(m))
        {
            return Err("`model` cannot be chosen here: a sub-agent uses its parent's model. Nothing was started.".into());
        }
        let turns = input.max_turns.unwrap_or(self.max_turns);
        if turns == 0 || turns > self.max_turns_cap {
            return Err(format!(
                "`max_turns` must be between 1 and {}; got {turns}. Nothing was started.",
                self.max_turns_cap
            ));
        }
        let depth = subagent::depth_of(&ctx.extensions);
        if depth + 1 >= MAX_DEPTH {
            return Err("a sub-agent cannot start another sub-agent. Nothing was started.".into());
        }
        if subagent::run_token(&ctx.extensions).is_some_and(|t| t.is_cancelled()) {
            return Err("the run was cancelled; no sub-agent was started.".into());
        }
        if !self.unavailable.is_empty() {
            return Err(format!(
                "these tools cannot be given to a sub-agent: {}. Nothing was started.",
                self.unavailable.join(", ")
            ));
        }
        Ok(turns)
    }
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        "Agent"
    }

    fn description(&self) -> &str {
        "Launch a sub-agent for one well-defined, multi-step sub-task. It runs its own loop \
         with your permissions and at most your tools, and returns its result. Give it a \
         precise, self-contained task; do not use it for work you can do in a few tool calls."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short description of the agent's task (3-5 words)"
                },
                "prompt": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The complete, precise task for the agent to perform"
                },
                "system_prompt": {
                    "type": "string",
                    "description": "Optional system prompt override for the sub-agent"
                },
                "max_turns": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": self.max_turns_cap,
                    "description": format!("Max turns for the sub-agent (default {})", self.max_turns)
                }
            },
            "required": ["description", "prompt"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let input: AgentInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };
        let max_turns = match self.check(&input, ctx) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e),
        };

        tracing::info!(description = %input.description, "Spawning sub-agent");

        let tools = subagent::child_tools((self.toolset_factory)(), &[]);
        let depth = subagent::depth_of(&ctx.extensions);
        let mut builder = Agent::builder()
            .provider_boxed((self.provider_factory)())
            .tools(tools)
            .max_turns(max_turns)
            .permission_policy_arc(Arc::clone(&ctx.permissions))
            .working_dir(&ctx.working_dir)
            .extensions(subagent::child_extensions(depth))
            .system_prompt(
                input
                    .system_prompt
                    .filter(|s| !subagent::is_blank(s))
                    .unwrap_or_else(|| CHILD_SYSTEM_PROMPT.to_string()),
            );
        if let Some(token) = subagent::run_token(&ctx.extensions) {
            builder = builder.cancel_token(token.child_token());
        }

        let agent = match builder.build() {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Failed to build sub-agent: {}", e)),
        };

        let result = agent.run(&input.prompt).await;
        let partial = subagent::partial_text(&agent);
        agent.close().await;
        subagent::tool_result(result, partial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cersei_provider::{CompletionRequest, CompletionStream};
    use cersei_tools::permissions::AllowAll;
    use cersei_tools::{CostTracker, Extensions};
    use cersei_types::*;
    use tokio::sync::mpsc;

    /// Mock provider that returns EndTurn immediately with a text response.
    struct EchoProvider;

    #[async_trait]
    impl Provider for EchoProvider {
        fn name(&self) -> &str {
            "echo"
        }
        // Large enough for every built-in tool definition: the runner now
        // refuses to send a request that manifestly exceeds the window.
        fn context_window(&self, _: &str) -> u64 {
            128_000
        }
        async fn complete(&self, req: CompletionRequest) -> cersei_types::Result<CompletionStream> {
            let prompt = req
                .messages
                .last()
                .and_then(|m| m.get_text())
                .unwrap_or("")
                .to_string();
            let (tx, rx) = mpsc::channel(16);
            tokio::spawn(async move {
                let _ = tx
                    .send(StreamEvent::MessageStart {
                        id: "1".into(),
                        model: "echo".into(),
                        usage: None,
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index: 0,
                        block_type: "text".into(),
                        id: None,
                        name: None,
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::TextDelta {
                        index: 0,
                        text: format!("Echo: {}", prompt),
                    })
                    .await;
                let _ = tx.send(StreamEvent::ContentBlockStop { index: 0 }).await;
                let _ = tx
                    .send(StreamEvent::MessageDelta {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Some(Usage {
                            input_tokens: 10,
                            output_tokens: 5,
                            ..Default::default()
                        }),
                    })
                    .await;
                let _ = tx.send(StreamEvent::MessageStop).await;
            });
            Ok(CompletionStream::new(rx))
        }
    }

    #[tokio::test]
    async fn test_agent_tool_spawns_sub_agent() {
        let agent_tool = AgentTool::new(|| Box::new(EchoProvider), cersei_tools::filesystem());

        let ctx = ToolContext {
            working_dir: std::env::temp_dir(),
            session_id: "parent".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        };

        let result = agent_tool
            .execute(
                json!({
                    "description": "test sub-agent",
                    "prompt": "Hello from parent"
                }),
                &ctx,
            )
            .await;

        assert!(
            !result.is_error,
            "Sub-agent should succeed: {}",
            result.content
        );
        assert!(
            result.content.contains("Echo:"),
            "Should contain echo response"
        );
        assert!(result.metadata.is_some(), "Should have metadata");
    }

    #[tokio::test]
    async fn test_agent_tool_filters_self() {
        // Verify Agent tool is not available to sub-agents (no recursion)
        let agent_tool = AgentTool::new(|| Box::new(EchoProvider), cersei_tools::all());

        let ctx = ToolContext {
            working_dir: std::env::temp_dir(),
            session_id: "parent".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        };

        // This should work — sub-agent gets tools minus "Agent"
        let result = agent_tool
            .execute(
                json!({
                    "description": "test no recursion",
                    "prompt": "Do something"
                }),
                &ctx,
            )
            .await;

        assert!(!result.is_error);
    }
}
