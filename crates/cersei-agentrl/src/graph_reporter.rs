//! A [`Reporter`] that folds an agent's `AgentEvent` stream into an
//! [`ExecutionGraph`]. This is the passive-observation seam: Hooks alter control
//! flow, Reporters record what happened.

use crate::graph::{
    summarize_input, EdgeRel, ExecutionGraph, NodeDetail, NodeId, NodeKind, NodeStatus,
};
use async_trait::async_trait;
use cersei_agent::events::AgentEvent;
use cersei_agent::{AgentOutput, Reporter};
use cersei_types::{CerseiError, StopReason};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
struct State {
    current_turn_node: NodeId,
    current_turn: u32,
    tool_nodes: HashMap<String, NodeId>,
}

/// Builds an [`ExecutionGraph`] from agent events. Cheap to clone (shared state).
#[derive(Clone)]
pub struct GraphReporter {
    graph: Arc<Mutex<ExecutionGraph>>,
    state: Arc<Mutex<State>>,
}

impl GraphReporter {
    pub fn new() -> Self {
        Self {
            graph: Arc::new(Mutex::new(ExecutionGraph::new())),
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Snapshot the graph built so far.
    pub fn graph(&self) -> ExecutionGraph {
        self.graph.lock().clone()
    }
}

impl Default for GraphReporter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Reporter for GraphReporter {
    async fn on_event(&self, event: &AgentEvent) {
        let mut g = self.graph.lock();
        let mut st = self.state.lock();
        match event {
            AgentEvent::TurnStart { turn } => {
                let root = g.root;
                let node = g.add_node(
                    NodeKind::Turn,
                    format!("turn {turn}"),
                    *turn,
                    NodeDetail::Empty,
                );
                g.add_edge(root, node, EdgeRel::Contains);
                st.current_turn_node = node;
                st.current_turn = *turn;
            }
            AgentEvent::ToolStart { name, id, input } => {
                let turn = st.current_turn;
                let parent = st.current_turn_node;
                let node = g.add_node(
                    NodeKind::ToolCall,
                    name.clone(),
                    turn,
                    NodeDetail::Tool {
                        input: summarize_input(input),
                        result: String::new(),
                        is_error: false,
                    },
                );
                g.add_edge(parent, node, EdgeRel::Contains);
                st.tool_nodes.insert(id.clone(), node);
            }
            AgentEvent::ToolEnd {
                id,
                result,
                is_error,
                ..
            } => {
                if let Some(node) = st.tool_nodes.get(id).copied() {
                    g.finish_tool(node, result.clone(), *is_error);
                    if *is_error {
                        let parent = st.current_turn_node;
                        g.add_edge(parent, node, EdgeRel::FailedAfter);
                    }
                }
            }
            AgentEvent::SubAgentSpawned { agent_id, prompt } => {
                let parent = st.current_turn_node;
                let turn = st.current_turn;
                let node = g.add_node(
                    NodeKind::AgentRun,
                    format!("sub:{agent_id}"),
                    turn,
                    NodeDetail::Agent {
                        summary: summarize_input(&serde_json::Value::String(prompt.clone())),
                    },
                );
                g.add_edge(parent, node, EdgeRel::Spawned);
            }
            AgentEvent::Error(_) => {
                let turn_node = st.current_turn_node;
                g.set_status(turn_node, NodeStatus::Failed);
                let root = g.root;
                g.set_status(root, NodeStatus::Failed);
            }
            _ => {}
        }
    }

    async fn on_complete(&self, output: &AgentOutput) {
        let mut g = self.graph.lock();
        let root = g.root;
        // Only mark the root OK if it ended cleanly and nothing failed underneath.
        if matches!(output.stop_reason, StopReason::EndTurn) && !g.has_failure() {
            g.set_status(root, NodeStatus::Ok);
        } else if g.has_failure() {
            g.set_status(root, NodeStatus::Failed);
        }
    }

    async fn on_error(&self, _error: &CerseiError) {
        let mut g = self.graph.lock();
        let root = g.root;
        g.set_status(root, NodeStatus::Failed);
    }
}
