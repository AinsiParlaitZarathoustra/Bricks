//! The programmatic builder — the second front-end that emits the same
//! [`WorkflowDef`] IR the visual UI produces. One IR, two front-ends.

use crate::condition::Condition;
use crate::ir::{
    EdgeKind, JoinStrategy, MapSpec, NodeId, NodeKind, WorkflowDef, WorkflowEdge, WorkflowNode,
};
use serde_json::Value;

/// Fluent builder for a [`WorkflowDef`]. Auto-generates node ids and synthesizes
/// `Parallel`/`Join`/`Branch` marker nodes so its output is structurally
/// identical to what the xyflow UI draws.
pub struct WorkflowBuilder {
    id: String,
    input_schema: Value,
    output_schema: Value,
    nodes: Vec<WorkflowNode>,
    edges: Vec<WorkflowEdge>,
    entry: Option<NodeId>,
    /// The node a subsequent `.then`/etc. should attach to.
    cursor: Option<NodeId>,
    counter: usize,
}

impl WorkflowBuilder {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            input_schema: Value::Null,
            output_schema: Value::Null,
            nodes: Vec::new(),
            edges: Vec::new(),
            entry: None,
            cursor: None,
            counter: 0,
        }
    }

    pub fn input_schema(mut self, s: Value) -> Self {
        self.input_schema = s;
        self
    }

    pub fn output_schema(mut self, s: Value) -> Self {
        self.output_schema = s;
        self
    }

    fn next_id(&mut self, hint: &str) -> NodeId {
        self.counter += 1;
        format!("{}_{}", hint, self.counter)
    }

    fn push_node(&mut self, id: NodeId, kind: NodeKind) {
        self.nodes.push(WorkflowNode::new(id.clone(), kind));
        if self.entry.is_none() {
            self.entry = Some(id);
        }
    }

    /// Attach a node after the cursor with a `Then` edge, then advance the cursor.
    fn append(&mut self, id: NodeId, kind: NodeKind) {
        let from = self.cursor.clone();
        self.push_node(id.clone(), kind);
        if let Some(from) = from {
            self.edges
                .push(WorkflowEdge::new(from, id.clone(), EdgeKind::Then));
        }
        self.cursor = Some(id);
    }

    /// Append a step node (Mastra `.then`).
    pub fn then(mut self, step_id: impl Into<String>) -> Self {
        let step_id = step_id.into();
        let id = self.next_id(&step_id);
        self.append(
            id,
            NodeKind::Step {
                step_id,
                config: Value::Null,
            },
        );
        self
    }

    /// Append a step node carrying static config.
    pub fn then_with(mut self, step_id: impl Into<String>, config: Value) -> Self {
        let step_id = step_id.into();
        let id = self.next_id(&step_id);
        self.append(id, NodeKind::Step { step_id, config });
        self
    }

    /// Append a `Map` reshape node (Mastra `.map`).
    pub fn map(mut self, mapping: MapSpec) -> Self {
        let id = self.next_id("map");
        self.append(id, NodeKind::Map { mapping });
        self
    }

    /// Fan out to several steps concurrently, then join (Mastra `.parallel`).
    pub fn parallel(mut self, step_ids: &[&str], strategy: JoinStrategy) -> Self {
        let par_id = self.next_id("parallel");
        self.append(par_id.clone(), NodeKind::Parallel);

        let join_id = self.next_id("join");
        // Create the join node up front (no Then edge from parallel).
        self.push_node(join_id.clone(), NodeKind::Join { strategy });

        for step_id in step_ids {
            let step_node = self.next_id(step_id);
            self.push_node(
                step_node.clone(),
                NodeKind::Step {
                    step_id: step_id.to_string(),
                    config: Value::Null,
                },
            );
            self.edges.push(WorkflowEdge::new(
                par_id.clone(),
                step_node.clone(),
                EdgeKind::Fork,
            ));
            self.edges.push(WorkflowEdge::new(
                step_node,
                join_id.clone(),
                EdgeKind::Merge,
            ));
        }
        self.cursor = Some(join_id);
        self
    }

    /// Branch on conditions; first matching arm runs (Mastra `.branch`). Each arm
    /// is a single step. Arms converge so a later `.then` continues the workflow.
    pub fn branch(mut self, arms: Vec<(Condition, &str)>) -> Self {
        let branch_id = self.next_id("branch");
        self.append(branch_id.clone(), NodeKind::Branch);

        let join_id = self.next_id("branch_join");
        self.push_node(
            join_id.clone(),
            NodeKind::Join {
                strategy: JoinStrategy::AllSettled,
            },
        );

        for (cond, step_id) in arms {
            let step_node = self.next_id(step_id);
            self.push_node(
                step_node.clone(),
                NodeKind::Step {
                    step_id: step_id.to_string(),
                    config: Value::Null,
                },
            );
            self.edges.push(WorkflowEdge::new(
                branch_id.clone(),
                step_node.clone(),
                EdgeKind::When { condition: cond },
            ));
            // Each arm continues to the convergence node.
            self.edges.push(WorkflowEdge::new(
                step_node,
                join_id.clone(),
                EdgeKind::Then,
            ));
        }
        self.cursor = Some(join_id);
        self
    }

    /// Repeat a single-step body while/until a condition holds.
    fn loop_step(mut self, step_id: &str, mode: crate::ir::LoopMode, condition: Condition) -> Self {
        let body_id = self.next_id(step_id);
        self.push_node(
            body_id.clone(),
            NodeKind::Step {
                step_id: step_id.to_string(),
                config: Value::Null,
            },
        );
        let loop_id = self.next_id("loop");
        self.append(
            loop_id.clone(),
            NodeKind::Loop {
                mode,
                body: body_id.clone(),
                condition: Some(condition),
            },
        );
        // The body's tail loops back to the loop node.
        self.edges.push(WorkflowEdge::new(
            body_id,
            loop_id.clone(),
            EdgeKind::LoopBack,
        ));
        self.cursor = Some(loop_id);
        self
    }

    pub fn dowhile(self, step_id: &str, condition: Condition) -> Self {
        self.loop_step(step_id, crate::ir::LoopMode::DoWhile, condition)
    }

    pub fn dountil(self, step_id: &str, condition: Condition) -> Self {
        self.loop_step(step_id, crate::ir::LoopMode::DoUntil, condition)
    }

    /// Finalize into the IR.
    pub fn commit(self) -> WorkflowDef {
        WorkflowDef {
            id: self.id,
            input_schema: self.input_schema,
            output_schema: self.output_schema,
            nodes: self.nodes,
            edges: self.edges,
            entry: self.entry.unwrap_or_default(),
        }
    }
}
