//! Workflow events and the live event stream.
//!
//! Mirrors `bricks_agent::events::{AgentEvent, AgentStream}`, with one crucial
//! difference: `WorkflowEvent` is `Serialize`, so the React/xyflow UI can consume
//! it over SSE/WebSocket and light up node status live.

use crate::ir::NodeId;
use crate::result::WorkflowResult;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;

/// A live event emitted during workflow execution.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEvent {
    WorkflowStarted {
        run_id: String,
    },
    StepStarted {
        node_id: NodeId,
        step_id: String,
        input: Value,
    },
    StepCompleted {
        node_id: NodeId,
        output: Value,
        duration_ms: u64,
    },
    StepFailed {
        node_id: NodeId,
        error: String,
    },
    StepSuspended {
        node_id: NodeId,
        resume_schema: Value,
    },
    BranchTaken {
        node_id: NodeId,
        edge_to: NodeId,
    },
    StateUpdated {
        state: Value,
    },
    LoopIteration {
        node_id: NodeId,
        iteration: u32,
    },
    /// Activity from a nested `AgentStep`/`WorkflowStep`, bridged up to this stream.
    Inner {
        node_id: NodeId,
        inner: Box<Value>,
    },
    WorkflowCompleted {
        result: Box<WorkflowResult>,
    },
    Error(String),
}

/// Returned by `Workflow::stream`. Async iteration over events plus a control
/// channel for resume/cancel — mirrors `AgentStream`.
pub struct WorkflowStream {
    rx: mpsc::Receiver<WorkflowEvent>,
    control_tx: mpsc::Sender<WorkflowControl>,
}

impl WorkflowStream {
    pub(crate) fn new(
        rx: mpsc::Receiver<WorkflowEvent>,
        control_tx: mpsc::Sender<WorkflowControl>,
    ) -> Self {
        Self { rx, control_tx }
    }

    /// Receive the next event.
    pub async fn next(&mut self) -> Option<WorkflowEvent> {
        self.rx.recv().await
    }

    /// Cancel the run.
    pub fn cancel(&self) {
        let _ = self.control_tx.try_send(WorkflowControl::Cancel);
    }

    /// Provide resume data for a suspended step.
    pub fn resume(&self, node_id: NodeId, data: Value) {
        let _ = self
            .control_tx
            .try_send(WorkflowControl::Resume { node_id, data });
    }

    /// Drain the stream and return the terminal result.
    pub async fn collect(mut self) -> Option<WorkflowResult> {
        let mut last = None;
        while let Some(event) = self.rx.recv().await {
            if let WorkflowEvent::WorkflowCompleted { result } = event {
                last = Some(*result);
            }
        }
        last
    }
}

/// Control messages sent back into a running workflow.
#[derive(Debug)]
pub enum WorkflowControl {
    Cancel,
    Resume { node_id: NodeId, data: Value },
}
