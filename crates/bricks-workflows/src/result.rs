//! Workflow run results, shaped to match Mastra's JSON output so the Atlas UI
//! can render them directly.

use crate::ir::NodeId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// The discriminating status of a run (or an individual step).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Success,
    Failed,
    Suspended,
    Paused,
}

/// The result of a single step within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub status: RunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

/// A node awaiting external resume input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspendPoint {
    pub node_id: NodeId,
    pub resume_schema: Value,
    pub payload: Value,
}

/// The terminal (or current) result of a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub run_id: String,
    pub status: RunStatus,
    pub input: Value,
    #[serde(default)]
    pub steps: HashMap<NodeId, StepResult>,
    /// Final output on `Success`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Terminal shared state.
    #[serde(default)]
    pub state: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Nodes awaiting resume (when `status == Suspended`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suspended: Vec<SuspendPoint>,
}
