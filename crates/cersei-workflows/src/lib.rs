//! # cersei-workflows
//!
//! A first-party workflow engine for the Cersei SDK, designed so the entire
//! workflow can round-trip to and from a visual builder (React + xyflow). The
//! [`WorkflowDef`] IR is the single source of truth: the UI draws nodes/edges
//! and emits it as JSON, and the programmatic [`WorkflowBuilder`] emits the very
//! same structure. [`Workflow::compile`] validates a def against a
//! [`StepRegistry`] and produces an executable workflow.
//!
//! ## Quick start
//!
//! ```rust,ignore
//! use cersei_workflows::prelude::*;
//! use serde_json::json;
//!
//! // 1. Register executable steps by id.
//! let registry = StepRegistry::new();
//! registry.register(std::sync::Arc::new(FnStep::new("upper", |input, _ctx| async move {
//!     let s = input.get("message").and_then(|v| v.as_str()).unwrap_or("");
//!     Ok(json!({ "message": s.to_uppercase() }))
//! })));
//!
//! // 2. Author a workflow (here via the builder; the UI emits the same IR).
//! let def = WorkflowBuilder::new("demo").then("upper").commit();
//!
//! // 3. Compile and run.
//! let wf = Workflow::compile(def, &registry)?;
//! let result = wf.start(json!({ "message": "hi" })).await?;
//! assert_eq!(result.status, RunStatus::Success);
//! ```

pub mod builder;
mod compile;
pub mod condition;
pub mod events;
mod executor;
pub mod ir;
pub mod registry;
pub mod result;
pub mod step;
pub mod steps;
pub mod store;

pub use builder::WorkflowBuilder;
pub use compile::Workflow;
pub use condition::Condition;
pub use events::{WorkflowControl, WorkflowEvent, WorkflowStream};
pub use ir::{
    EdgeKind, JoinStrategy, LoopMode, MapSpec, NodeId, NodeKind, UiHints, WorkflowDef,
    WorkflowEdge, WorkflowNode,
};
pub use registry::{StepInfo, StepRegistry};
pub use result::{RunStatus, StepResult, SuspendPoint, WorkflowResult};
pub use step::{Step, StepContext, StepOutcome, StepRun};
pub use steps::{AgentStep, FnStep, ToolStep, WorkflowStep};
pub use store::{RunSnapshot, RunStore};

/// The prelude — import this for the most common workflow types.
pub mod prelude {
    pub use crate::builder::WorkflowBuilder;
    pub use crate::condition::Condition;
    pub use crate::events::{WorkflowEvent, WorkflowStream};
    pub use crate::ir::{JoinStrategy, MapSpec, NodeKind, WorkflowDef};
    pub use crate::registry::StepRegistry;
    pub use crate::result::{RunStatus, WorkflowResult};
    pub use crate::step::{Step, StepContext, StepOutcome};
    pub use crate::steps::{AgentStep, FnStep, ToolStep, WorkflowStep};
    pub use crate::Workflow;
}
