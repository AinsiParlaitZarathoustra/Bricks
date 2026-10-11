//! `WorkflowStep` — runs a nested workflow as a single step (workflows-as-steps).

use crate::Workflow;
use crate::result::RunStatus;
use crate::step::{Step, StepContext, StepOutcome};
use async_trait::async_trait;
use cersei_types::{CerseiError, Result};
use serde_json::Value;
use std::sync::Arc;

/// A step that delegates to a nested compiled workflow.
pub struct WorkflowStep {
    id: String,
    inner: Arc<Workflow>,
}

impl WorkflowStep {
    pub fn new(id: impl Into<String>, inner: Arc<Workflow>) -> Self {
        Self {
            id: id.into(),
            inner,
        }
    }
}

#[async_trait]
impl Step for WorkflowStep {
    fn id(&self) -> &str {
        &self.id
    }

    fn description(&self) -> &str {
        "Runs a nested workflow"
    }

    async fn execute(&self, input: Value, _ctx: &StepContext) -> Result<StepOutcome> {
        let res = self.inner.start(input).await?;
        match res.status {
            RunStatus::Success => Ok(StepOutcome::Done(res.result.unwrap_or(Value::Null))),
            RunStatus::Suspended => {
                // Propagate the first suspend point upward.
                let sp = res.suspended.into_iter().next();
                let (resume_schema, payload) = sp
                    .map(|p| (p.resume_schema, p.payload))
                    .unwrap_or((Value::Null, Value::Null));
                Ok(StepOutcome::Suspended {
                    resume_schema,
                    payload,
                })
            }
            _ => Err(CerseiError::Tool(
                res.error.unwrap_or_else(|| "nested workflow failed".into()),
            )),
        }
    }
}
