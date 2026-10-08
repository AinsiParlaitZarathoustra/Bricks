//! `DelegateTool` — a batch/parallel variant of `AgentTool`.
//!
//! Where `AgentTool` spawns one sub-agent, `DelegateTool` spawns N in parallel
//! with the isolation / blocklist / depth rules from `crate::delegate`.
//!
//! The tool input accepts either a single `goal` or a `tasks` array. When both
//! are present, `goal` is ignored in favor of `tasks` (matches the hermes-agent
//! contract). Every goal must be non-empty and every workspace inside the
//! parent's working directory; the whole batch is checked before any task
//! starts.

use crate::delegate::{
    run_batch, DelegateConfig, DelegateTask, ProviderFactory, ToolsetFactory,
    DEFAULT_MAX_CONCURRENT,
};
use crate::subagent;
use async_trait::async_trait;
use cersei_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct DelegateTool {
    provider_factory: ProviderFactory,
    toolset_factory: ToolsetFactory,
    model: Option<String>,
    max_concurrent: usize,
}

impl DelegateTool {
    pub fn new(provider_factory: ProviderFactory, toolset_factory: ToolsetFactory) -> Self {
        Self {
            provider_factory,
            toolset_factory,
            model: None,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
        }
    }

    pub fn with_model(mut self, m: impl Into<String>) -> Self {
        self.model = Some(m.into());
        self
    }

    pub fn with_max_concurrent(mut self, n: usize) -> Self {
        self.max_concurrent = n.max(1);
        self
    }
}

#[derive(Debug, Deserialize)]
struct TaskInput {
    goal: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Input {
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    tasks: Option<Vec<TaskInput>>,
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        "Delegate one or more focused sub-tasks to isolated sub-agents running in \
         parallel. Each child starts with a fresh conversation, a restricted toolset, \
         and cannot spawn further sub-agents. Use `tasks` for a batch; otherwise pass \
         a single `goal`. Returns a combined summary with one block per task."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Single-task mode: what the sub-agent should accomplish."
                },
                "context": {
                    "type": "string",
                    "description": "Optional background context shared with the sub-agent(s)."
                },
                "tasks": {
                    "type": "array",
                    "description": "Batch mode: multiple tasks to run in parallel.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "goal":      { "type": "string", "minLength": 1 },
                            "context":   { "type": "string" },
                            "workspace": { "type": "string" }
                        },
                        "required": ["goal"]
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolResult::error(format!("Invalid input: {e}")),
        };
        let tasks = match tasks_of(parsed, &ctx.working_dir) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(format!("{e} Nothing was started.")),
        };

        // The session's limits apply to this path too (one slot for the
        // batch; its own `max_concurrent` inside).
        let _slot = match subagent::legacy_admission(&ctx.extensions, tasks.len() as u32).await {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e),
        };
        let cfg = DelegateConfig {
            tasks,
            provider_factory: self.provider_factory.clone(),
            toolset_factory: self.toolset_factory.clone(),
            model: self.model.clone(),
            max_concurrent: self.max_concurrent,
            depth: subagent::depth_of(&ctx.extensions) + 1,
            extra_blocked: Vec::new(),
            permissions: Some(Arc::clone(&ctx.permissions)),
            cancel: subagent::run_token(&ctx.extensions),
        };

        match run_batch(cfg).await {
            Ok(results) => {
                let total = results.len();
                let failures = results.iter().filter(|r| !r.is_ok()).count();
                let mut out = String::new();
                for (i, r) in results.iter().enumerate() {
                    out.push_str(&format!(
                        "── Task {}/{} [{}]: {}\n",
                        i + 1,
                        total,
                        r.status.label(),
                        truncate(&r.goal, 120)
                    ));
                    if let Some(err) = &r.error {
                        out.push_str(&format!("   ERROR: {err}\n"));
                    }
                    if subagent::is_blank(&r.summary) {
                        out.push('\n');
                    } else {
                        out.push_str(&format!("{}\n\n", r.summary.trim()));
                    }
                }
                let statuses: Vec<&str> = results.iter().map(|r| r.status.label()).collect();
                let meta = json!({
                    "tasks": total,
                    "failures": failures,
                    "statuses": statuses,
                });
                // Each block carries its child's status; the call as a
                // whole failed only when no child completed.
                if failures == total {
                    ToolResult::error(out).with_metadata(meta)
                } else {
                    ToolResult::success(out).with_metadata(meta)
                }
            }
            Err(e) => ToolResult::error(format!("delegate refused: {e}")),
        }
    }
}

/// The tasks of an input, all checked: non-empty goals, workspaces inside
/// `working_dir` (relative ones resolved against it).
fn tasks_of(parsed: Input, working_dir: &Path) -> Result<Vec<DelegateTask>, String> {
    let raw: Vec<(String, Option<String>, Option<String>)> = if let Some(batch) = parsed.tasks {
        if batch.is_empty() {
            return Err("tasks array is empty.".into());
        }
        batch
            .into_iter()
            .map(|t| (t.goal, t.context, t.workspace))
            .collect()
    } else if let Some(goal) = parsed.goal {
        vec![(goal, parsed.context, None)]
    } else {
        return Err("must provide either `goal` or `tasks`.".into());
    };
    let blank: Vec<String> = raw
        .iter()
        .enumerate()
        .filter(|(_, (g, _, _))| subagent::is_blank(g))
        .map(|(i, _)| (i + 1).to_string())
        .collect();
    if !blank.is_empty() {
        return Err(format!(
            "every task needs a precise goal; task(s) {} of {} have an empty goal.",
            blank.join(", "),
            raw.len()
        ));
    }
    raw.into_iter()
        .map(|(goal, context, workspace)| {
            let dir = match workspace.filter(|w| !subagent::is_blank(w)) {
                Some(w) => workspace_inside(working_dir, &w)?,
                None => working_dir.to_path_buf(),
            };
            let mut task = DelegateTask::new(goal).with_workspace(dir);
            if let Some(c) = context {
                task = task.with_context(c);
            }
            Ok(task)
        })
        .collect()
}

/// `w` as an existing directory inside `working_dir`.
fn workspace_inside(working_dir: &Path, w: &str) -> Result<PathBuf, String> {
    let joined = if Path::new(w).is_absolute() {
        PathBuf::from(w)
    } else {
        working_dir.join(w)
    };
    let canon = std::fs::canonicalize(&joined)
        .map_err(|e| format!("workspace `{w}` is not usable: {e}."))?;
    let root = std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
    if !canon.starts_with(&root) || !canon.is_dir() {
        return Err(format!(
            "workspace `{w}` must be a directory inside {}.",
            root.display()
        ));
    }
    Ok(canon)
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut end = n;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delegate::{ProviderFactory, ToolsetFactory};
    use cersei_provider::{CompletionRequest, CompletionStream, Provider};
    use cersei_tools::permissions::AllowAll;
    use cersei_tools::{CostTracker, Extensions};
    use cersei_types::*;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    struct EchoProvider;

    #[async_trait]
    impl Provider for EchoProvider {
        fn name(&self) -> &str {
            "echo"
        }
        fn context_window(&self, _: &str) -> u64 {
            4096
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
                        text: format!("done: {prompt}"),
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

    fn ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            session_id: "t".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    fn factories() -> (ProviderFactory, ToolsetFactory) {
        let pf: ProviderFactory = Arc::new(|| Box::new(EchoProvider));
        let tf: ToolsetFactory = Arc::new(Vec::new);
        (pf, tf)
    }

    #[tokio::test]
    async fn single_goal_runs_one_child() {
        let (pf, tf) = factories();
        let tool = DelegateTool::new(pf, tf);
        let r = tool.execute(json!({ "goal": "ping" }), &ctx()).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("Task 1/1"));
        assert!(r.content.contains("done:"));
    }

    #[tokio::test]
    async fn batch_mode_runs_all_tasks() {
        let (pf, tf) = factories();
        let tool = DelegateTool::new(pf, tf).with_max_concurrent(2);
        let r = tool
            .execute(
                json!({ "tasks": [{"goal": "a"}, {"goal": "b"}, {"goal": "c"}] }),
                &ctx(),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("Task 1/3"));
        assert!(r.content.contains("Task 3/3"));
    }

    #[tokio::test]
    async fn rejects_missing_goal_and_tasks() {
        let (pf, tf) = factories();
        let tool = DelegateTool::new(pf, tf);
        let r = tool.execute(json!({}), &ctx()).await;
        assert!(r.is_error);
    }

    #[tokio::test]
    async fn rejects_empty_tasks_array() {
        let (pf, tf) = factories();
        let tool = DelegateTool::new(pf, tf);
        let r = tool.execute(json!({ "tasks": [] }), &ctx()).await;
        assert!(r.is_error);
    }
}
