//! `ToolStep` — wraps any `bricks_tools::Tool` as a workflow step.
//!
//! Reuses the tool's own `input_schema` and dispatches the node input straight
//! into `Tool::execute`, then maps `ToolResult` into a JSON step output.

use crate::step::{Step, StepContext, StepOutcome};
use async_trait::async_trait;
use bricks_tools::permissions::AllowAll;
use bricks_tools::{CostTracker, Tool, ToolContext};
use bricks_types::{BricksError, Result};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// A step that runs a Bricks tool. Output JSON: `{ content, is_error, metadata }`.
pub struct ToolStep {
    id: String,
    tool: Arc<dyn Tool>,
    working_dir: PathBuf,
}

impl ToolStep {
    pub fn new(id: impl Into<String>, tool: Arc<dyn Tool>) -> Self {
        Self {
            id: id.into(),
            tool,
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    pub fn working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = dir.into();
        self
    }

    fn tool_context(&self, ctx: &StepContext) -> ToolContext {
        ToolContext {
            working_dir: self.working_dir.clone(),
            session_id: ctx.run_id.clone(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            // Share the workflow's extension type-map so injected handles flow through.
            extensions: ctx.extensions.clone(),
        }
    }
}

#[async_trait]
impl Step for ToolStep {
    fn id(&self) -> &str {
        &self.id
    }

    fn description(&self) -> &str {
        self.tool.description()
    }

    fn input_schema(&self) -> Value {
        self.tool.input_schema()
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string" },
                "is_error": { "type": "boolean" },
                "metadata": {}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &StepContext) -> Result<StepOutcome> {
        let tool_ctx = self.tool_context(ctx);
        let res = self.tool.execute(input, &tool_ctx).await;
        if res.is_error {
            return Err(BricksError::Tool(res.content));
        }
        Ok(StepOutcome::Done(json!({
            "content": res.content,
            "is_error": res.is_error,
            "metadata": res.metadata,
        })))
    }
}
