//! The serializable workflow IR (`WorkflowDef`).
//!
//! This is the single source of truth for a workflow's structure. It is emitted
//! identically by the visual UI builder (React + xyflow) and the programmatic
//! [`crate::builder::WorkflowBuilder`], and consumed by [`crate::Workflow::compile`].
//!
//! Modeled on the shape of `bricks_agentrl::graph::ExecutionGraph` — a flat list
//! of nodes plus a flat list of edges — but where `ExecutionGraph` is
//! *observational* (records what an agent did), `WorkflowDef` is *prescriptive*
//! (defines what to execute). The two are deliberately distinct types.

use crate::condition::Condition;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// String node ids so the xyflow UI can own and round-trip them.
pub type NodeId = String;

/// A complete, serializable workflow definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub id: String,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default)]
    pub output_schema: Value,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    /// The root node where execution begins (mirrors `ExecutionGraph.root`).
    pub entry: NodeId,
}

impl WorkflowDef {
    /// Look up a node by id.
    pub fn node(&self, id: &str) -> Option<&WorkflowNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// A node in the workflow graph: an id, a kind, and optional UI-only hints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: NodeId,
    pub kind: NodeKind,
    /// UI-only positioning/labeling. Ignored at execution time, so the IR stays
    /// lossless across the React Flow boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiHints>,
}

impl WorkflowNode {
    pub fn new(id: impl Into<NodeId>, kind: NodeKind) -> Self {
        Self {
            id: id.into(),
            kind,
            ui: None,
        }
    }
}

/// The kind of work a node performs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// Executes a registered [`crate::step::Step`] by id, merging `config` into
    /// the step's input.
    Step {
        step_id: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        config: Value,
    },
    /// A pure JSON reshape between steps (Mastra `.map`). No registry lookup.
    Map { mapping: MapSpec },
    /// Fan-out marker: every outgoing `Fork` edge runs concurrently.
    Parallel,
    /// Join marker: waits for all incoming parallel branches before continuing.
    Join {
        #[serde(default)]
        strategy: JoinStrategy,
    },
    /// Branch point: outgoing `When` edges are evaluated in order; first match
    /// wins (Mastra `.branch`).
    Branch,
    /// A loop region. `body` is the entry node of the looped sub-region; the
    /// region ends with a `LoopBack` edge to this node.
    Loop {
        mode: LoopMode,
        body: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        condition: Option<Condition>,
    },
}

/// How a [`NodeKind::Loop`] repeats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    /// Run the body, then repeat while the condition is true.
    DoWhile,
    /// Run the body, then repeat until the condition is true.
    DoUntil,
    /// Map the body over an input array with bounded concurrency.
    ForEach { concurrency: usize },
}

/// How a [`NodeKind::Join`] handles branch failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinStrategy {
    /// Abort the whole join if any branch errors.
    #[default]
    AllOrFail,
    /// Collect every branch result; failed branches contribute `null`.
    AllSettled,
}

/// An edge connecting two nodes, carrying control-flow semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
}

impl WorkflowEdge {
    pub fn new(from: impl Into<NodeId>, to: impl Into<NodeId>, kind: EdgeKind) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
        }
    }
}

/// The control-flow meaning of an edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Sequential continuation (`.then`).
    Then,
    /// A `Parallel` node fanning out to a branch.
    Fork,
    /// A branch merging back into a `Join` node.
    Merge,
    /// A `Branch` arm, taken when `condition` evaluates true.
    When { condition: Condition },
    /// A loop body's tail edge back to its `Loop` node (the only legal cycle).
    LoopBack,
}

/// A declarative field mapping for a [`NodeKind::Map`] node.
///
/// Each output field is filled from a JSON-pointer path resolved against the
/// run scope `{ input, state, steps: { <node_id>: <output> } }`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MapSpec {
    /// `output_field` -> JSON-pointer path into the scope.
    pub fields: HashMap<String, String>,
}

/// UI-only hints for the visual builder. Never read during execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiHints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}
