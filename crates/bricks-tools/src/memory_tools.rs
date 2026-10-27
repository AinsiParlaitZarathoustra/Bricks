//! `memory_search` and `memory_write`: the long-term memory of the run, by
//! scope (`project` by default, or `global`, shared by every project).
//!
//! Both use the memory the run started with ([`RunMemory`], set by the
//! session at the start of each run): a change of setting applies from the
//! next run. When the run started with the memory off, they refuse with
//! `memory_disabled`. Their permissions are the session's: a search reads, a
//! note writes.

use super::*;
use bricks_memory::{LongTermMemory, MemoryScope};
use serde::Deserialize;

/// Names of the memory tools.
pub const MEMORY_TOOLS: [&str; 2] = ["memory_search", "memory_write"];

/// The memory of the current run (`None`: off for this run).
#[derive(Clone, Default)]
pub struct RunMemory(pub Option<Arc<dyn LongTermMemory>>);

/// The run's memory from the context, or the `memory_disabled` refusal.
fn run_memory(ctx: &ToolContext) -> std::result::Result<Arc<dyn LongTermMemory>, ToolResult> {
    ctx.extensions
        .get::<RunMemory>()
        .and_then(|m| m.0.clone())
        .ok_or_else(|| ToolResult::error("memory_disabled: the memory is off for this run."))
}

fn scope_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["project", "global"],
        "description": "project (default): this project's memory. global: shared by every project."
    })
}

fn default_limit() -> usize {
    5
}

pub struct MemorySearchTool;

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }
    fn description(&self) -> &str {
        "Search the long-term memory: notes, remembered exchanges and facts of earlier \
         sessions, with their sources."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "What to look for" },
                "scope": scope_schema(),
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": 20,
                    "description": "Results at most (default 5)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            query: String,
            #[serde(default)]
            scope: Option<MemoryScope>,
            #[serde(default = "default_limit")]
            limit: usize,
        }
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        if input.query.trim().is_empty() {
            return ToolResult::error("`query` is empty.");
        }
        if !(1..=20).contains(&input.limit) {
            return ToolResult::error("`limit` must be between 1 and 20.");
        }
        let memory = match run_memory(ctx) {
            Ok(m) => m,
            Err(e) => return e,
        };
        let scope = input.scope.unwrap_or(MemoryScope::Project);
        match memory.search(scope, &input.query, input.limit).await {
            Ok(Some(found)) => ToolResult::success(found.text),
            Ok(None) => {
                ToolResult::success(format!("Nothing in the {} memory matches.", scope.as_str()))
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

pub struct MemoryWriteTool;

#[async_trait]
impl Tool for MemoryWriteTool {
    fn name(&self) -> &str {
        "memory_write"
    }
    fn description(&self) -> &str {
        "Save a durable note in the long-term memory, found by later searches at once. \
         To correct a note, write a new one."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "The note, self-contained" },
                "scope": scope_schema()
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            text: String,
            #[serde(default)]
            scope: Option<MemoryScope>,
        }
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        if input.text.trim().is_empty() {
            return ToolResult::error("`text` is empty.");
        }
        let memory = match run_memory(ctx) {
            Ok(m) => m,
            Err(e) => return e,
        };
        let scope = input.scope.unwrap_or(MemoryScope::Project);
        let session = (!ctx.session_id.is_empty()).then_some(ctx.session_id.as_str());
        match memory.write_note(scope, session, &input.text).await {
            Ok(id) => {
                ToolResult::success(format!("Saved in the {} memory as {id}.", scope.as_str()))
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

/// The two memory tools.
pub fn memory_tools() -> Vec<Box<dyn Tool>> {
    vec![Box::new(MemorySearchTool), Box::new(MemoryWriteTool)]
}
