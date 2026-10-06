//! Compilation: `WorkflowDef` + `StepRegistry` -> executable `Workflow`.
//!
//! Validation resolves every `Step` node against the registry and checks the
//! graph's structural sanity before any execution begins, so the executor never
//! re-hits the registry or trips over a malformed graph mid-run.

use crate::ir::{EdgeKind, NodeKind, WorkflowDef, WorkflowEdge};
use crate::registry::StepRegistry;
use crate::step::Step;
use crate::store::RunStore;
use cersei_types::{CerseiError, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// A validated, executable workflow: the IR plus resolved step handles and
/// pre-built adjacency indexes.
pub struct Workflow {
    pub(crate) def: WorkflowDef,
    /// step_id -> resolved implementation.
    pub(crate) steps: HashMap<String, Arc<dyn Step>>,
    /// node_id -> outgoing edges.
    pub(crate) outgoing: HashMap<String, Vec<WorkflowEdge>>,
    /// Snapshot store backing suspend/resume.
    pub(crate) store: RunStore,
}

impl Workflow {
    /// Validate and resolve a workflow definition against a step registry.
    pub fn compile(def: WorkflowDef, registry: &StepRegistry) -> Result<Arc<Workflow>> {
        if def.nodes.is_empty() {
            return Err(CerseiError::Config("workflow has no nodes".into()));
        }
        if def.node(&def.entry).is_none() {
            return Err(CerseiError::Config(format!(
                "workflow entry node '{}' does not exist",
                def.entry
            )));
        }

        // Resolve every Step node's implementation.
        let mut steps: HashMap<String, Arc<dyn Step>> = HashMap::new();
        for node in &def.nodes {
            if let NodeKind::Step { step_id, .. } = &node.kind {
                if !steps.contains_key(step_id) {
                    let step = registry
                        .get(step_id)
                        .ok_or_else(|| CerseiError::Tool(format!("unknown step: '{}'", step_id)))?;
                    steps.insert(step_id.clone(), step);
                }
            }
        }

        // Build adjacency and validate edge endpoints.
        let mut outgoing: HashMap<String, Vec<WorkflowEdge>> = HashMap::new();
        for edge in &def.edges {
            if def.node(&edge.from).is_none() {
                return Err(CerseiError::Config(format!(
                    "edge from unknown node '{}'",
                    edge.from
                )));
            }
            if def.node(&edge.to).is_none() {
                return Err(CerseiError::Config(format!(
                    "edge to unknown node '{}'",
                    edge.to
                )));
            }
            outgoing
                .entry(edge.from.clone())
                .or_default()
                .push(edge.clone());
        }

        // Structural checks per node kind.
        for node in &def.nodes {
            match &node.kind {
                NodeKind::Branch => {
                    let outs = outgoing.get(&node.id).map(|v| v.as_slice()).unwrap_or(&[]);
                    if !outs.iter().any(|e| matches!(e.kind, EdgeKind::When { .. })) {
                        return Err(CerseiError::Config(format!(
                            "branch node '{}' has no `When` arms",
                            node.id
                        )));
                    }
                }
                NodeKind::Loop { body, .. } if def.node(body).is_none() => {
                    return Err(CerseiError::Config(format!(
                        "loop node '{}' references missing body '{}'",
                        node.id, body
                    )));
                }
                _ => {}
            }
        }

        Ok(Arc::new(Workflow {
            def,
            steps,
            outgoing,
            store: RunStore::in_memory(),
        }))
    }

    /// The underlying definition.
    pub fn def(&self) -> &WorkflowDef {
        &self.def
    }
}
