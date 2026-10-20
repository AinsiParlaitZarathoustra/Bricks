//! Tools that expose the registry to an agent:
//! - [`DynamicTool`] wraps a registered solution so it is callable like any
//!   built-in tool (replay delegated to a [`SolutionReplayer`]).
//! - [`RegistrySearchTool`] lets an agent look up prior solutions explicitly.

use super::{RegistryEntry, ToolRegistry};
use async_trait::async_trait;
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;

/// Strategy for re-applying a registered solution. The default (production) impl
/// spawns a fresh sub-agent from the `SolutionSpec`; tests supply a mock.
#[async_trait]
pub trait SolutionReplayer: Send + Sync {
    async fn replay(&self, entry: &RegistryEntry, goal: &str, ctx: &ToolContext) -> ToolResult;
}

/// A registered solution surfaced as a callable [`Tool`].
pub struct DynamicTool {
    entry: RegistryEntry,
    replayer: Arc<dyn SolutionReplayer>,
}

impl DynamicTool {
    pub fn new(entry: RegistryEntry, replayer: Arc<dyn SolutionReplayer>) -> Self {
        Self { entry, replayer }
    }

    pub fn entry(&self) -> &RegistryEntry {
        &self.entry
    }
}

#[async_trait]
impl Tool for DynamicTool {
    fn name(&self) -> &str {
        &self.entry.name
    }

    fn description(&self) -> &str {
        &self.entry.description
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string", "description": "The concrete task to apply this solution to" }
            },
            "required": ["goal"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let goal = input
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.entry.solution.goal_template);
        self.replayer.replay(&self.entry, goal, ctx).await
    }
}

/// Lets an agent search the registry for prior solutions.
pub struct RegistrySearchTool {
    registry: Arc<ToolRegistry>,
}

impl RegistrySearchTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for RegistrySearchTool {
    fn name(&self) -> &str {
        "registry_search"
    }

    fn description(&self) -> &str {
        "Search the local tool registry for previously-built tools that solve a similar problem. \
         Returns matching tool ids, names, and descriptions. Call this BEFORE building a new tool."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Natural-language description of the problem" },
                "k": { "type": "integer", "description": "Max results (default 5)" }
            },
            "required": ["query"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return ToolResult::error("missing required field: query"),
        };
        let k = input.get("k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
        let hits: Vec<Value> = self
            .registry
            .search(query, k)
            .into_iter()
            .map(|e| {
                json!({
                    "tool_id": e.tool_id,
                    "name": e.name,
                    "description": e.description,
                    "success_count": e.success_count,
                })
            })
            .collect();
        ToolResult::success(format!("{} match(es)", hits.len()))
            .with_metadata(json!({ "matches": hits }))
    }
}
