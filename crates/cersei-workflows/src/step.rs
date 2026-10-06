//! The `Step` trait — the workflow analogue of `cersei_tools::Tool`.
//!
//! A step takes a JSON input, runs async logic against a [`StepContext`], and
//! produces a [`StepOutcome`]. Steps carry both an input and an output JSON
//! schema (Mastra requires both) so the UI builder can validate wiring.

use async_trait::async_trait;
use cersei_tools::Extensions;
use cersei_types::Result;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::events::WorkflowEvent;

/// The result of executing a step.
#[derive(Debug, Clone)]
pub enum StepOutcome {
    /// The step produced output and is complete.
    Done(Value),
    /// The step is awaiting external input; the run suspends here. `resume_schema`
    /// describes the data `Workflow::resume` must supply; `payload` is surfaced to
    /// the caller (e.g. a form spec for a human-in-the-loop pause).
    Suspended {
        resume_schema: Value,
        payload: Value,
    },
}

impl StepOutcome {
    /// Convenience: a completed step from any serializable value.
    pub fn done(value: impl Into<Value>) -> Self {
        StepOutcome::Done(value.into())
    }
}

/// Per-step execution context, passed by reference like `ToolContext`.
#[derive(Clone)]
pub struct StepContext {
    pub run_id: String,
    /// Shared, mutable workflow state (Mastra `setState`/`getState`). Do not hold
    /// the lock across `.await`.
    pub state: Arc<Mutex<Value>>,
    /// Event sink for live UI updates.
    pub events: mpsc::Sender<WorkflowEvent>,
    /// Type-map for runtime injection (reused from `cersei_tools`).
    pub extensions: Extensions,
    /// Present only when this step is being resumed.
    pub resume_data: Option<Value>,
}

impl StepContext {
    /// Read a clone of the current shared state.
    pub fn state(&self) -> Value {
        self.state.lock().clone()
    }

    /// Replace the shared state and emit a `StateUpdated` event.
    pub fn set_state(&self, value: Value) {
        *self.state.lock() = value.clone();
        let _ = self
            .events
            .try_send(WorkflowEvent::StateUpdated { state: value });
    }
}

/// The dynamic step trait. Object-safe so steps live behind `Arc<dyn Step>`.
#[async_trait]
pub trait Step: Send + Sync {
    /// Stable id referenced by `NodeKind::Step { step_id }` in the IR.
    fn id(&self) -> &str;

    fn description(&self) -> &str {
        ""
    }

    /// JSON Schema for the step's input.
    fn input_schema(&self) -> Value {
        Value::Null
    }

    /// JSON Schema for the step's output.
    fn output_schema(&self) -> Value {
        Value::Null
    }

    async fn execute(&self, input: Value, ctx: &StepContext) -> Result<StepOutcome>;
}

/// Typed step trait — the target of a future `#[derive(Step)]` macro, mirroring
/// `cersei_tools::ToolExecute`.
#[async_trait]
pub trait StepRun: Send + Sync {
    type Input: DeserializeOwned + JsonSchema + Send;
    type Output: Serialize + JsonSchema + Send;

    fn id(&self) -> &str;

    async fn run(&self, input: Self::Input, ctx: &StepContext) -> Result<Self::Output>;
}
