//! The step registry — host-side map of step-id -> executable step.
//!
//! Mirrors `bricks_agentrl::registry::ToolRegistry`, but holds live
//! `Arc<dyn Step>` handles rather than serialized specs. UI-emitted workflow
//! JSON carries only `step_id` references; the host supplies implementations
//! here, and [`crate::Workflow::compile`] resolves them.

use crate::step::Step;
use dashmap::DashMap;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

/// A concurrent registry of executable steps, keyed by [`Step::id`].
#[derive(Default)]
pub struct StepRegistry {
    steps: DashMap<String, Arc<dyn Step>>,
}

impl StepRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a step under its own id. Replaces any existing step with that id.
    pub fn register(&self, step: Arc<dyn Step>) {
        self.steps.insert(step.id().to_string(), step);
    }

    /// Builder-style registration for chaining.
    pub fn with(self: Arc<Self>, step: Arc<dyn Step>) -> Arc<Self> {
        self.register(step);
        self
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Step>> {
        self.steps.get(id).map(|e| Arc::clone(e.value()))
    }

    pub fn ids(&self) -> Vec<String> {
        self.steps.iter().map(|e| e.key().clone()).collect()
    }

    /// A UI-facing catalog of registered steps, for the builder palette.
    pub fn catalog(&self) -> Vec<StepInfo> {
        self.steps
            .iter()
            .map(|e| {
                let s = e.value();
                StepInfo {
                    id: s.id().to_string(),
                    description: s.description().to_string(),
                    input_schema: s.input_schema(),
                    output_schema: s.output_schema(),
                }
            })
            .collect()
    }
}

/// Catalog entry describing a registered step to the UI builder.
#[derive(Debug, Clone, Serialize)]
pub struct StepInfo {
    pub id: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}
