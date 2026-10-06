//! End-to-end test of the AgentRL loop with a mock runner: a first task fails,
//! the planner proposes fixes, a sandboxed proposal passes, a tool is
//! registered — and a second similar task is solved by that cached tool WITHOUT
//! invoking the planner (the self-improvement claim).

use async_trait::async_trait;
use cersei_agentrl::graph::{EdgeRel, ExecutionGraph, NodeDetail, NodeKind, NodeStatus};
use cersei_agentrl::orchestrator::{
    AgentRlRunner, GeneralResult, Orchestrator, OrchestratorConfig, Proposal, ProposalOutcome,
};
use cersei_agentrl::planner::proposals_from_trace;
use cersei_agentrl::{FailureTrace, SolutionSpec, Solved, ToolRegistry};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn failing_graph() -> ExecutionGraph {
    let mut g = ExecutionGraph::new();
    let root = g.root;
    let turn = g.add_node(NodeKind::Turn, "turn 1", 1, NodeDetail::Empty);
    g.add_edge(root, turn, EdgeRel::Contains);
    let tool = g.add_node(
        NodeKind::ToolCall,
        "Bash",
        1,
        NodeDetail::Tool {
            input: "cargo test".into(),
            result: String::new(),
            is_error: false,
        },
    );
    g.add_edge(turn, tool, EdgeRel::Contains);
    g.finish_tool(
        tool,
        "error[E0433]: compile error; exit code 101".into(),
        true,
    );
    g.set_status(root, NodeStatus::Failed);
    g
}

#[derive(Default)]
struct MockRunner {
    plan_calls: AtomicUsize,
    proposal_runs: AtomicUsize,
}

#[async_trait]
impl AgentRlRunner for MockRunner {
    async fn run_general(
        &self,
        task: &str,
        available: &[cersei_agentrl::RegistryEntry],
    ) -> GeneralResult {
        if let Some(tool) = available.first() {
            // A matching cached tool exists → solve immediately via it.
            GeneralResult {
                success: true,
                graph: ExecutionGraph::new(),
                answer: format!("solved '{task}' using {}", tool.name),
                used_tool: Some(tool.tool_id.clone()),
            }
        } else {
            // No tool yet → the general agent fails.
            GeneralResult {
                success: false,
                graph: failing_graph(),
                answer: String::new(),
                used_tool: None,
            }
        }
    }

    async fn plan(&self, trace: &FailureTrace, n: usize) -> Vec<Proposal> {
        self.plan_calls.fetch_add(1, Ordering::SeqCst);
        proposals_from_trace(trace, n)
    }

    async fn run_proposal(&self, proposal: &Proposal) -> ProposalOutcome {
        self.proposal_runs.fetch_add(1, Ordering::SeqCst);
        // The first proposal passes and yields a replayable solution.
        let passes = proposal.id.ends_with('0');
        ProposalOutcome {
            proposal_id: proposal.id.clone(),
            passed: passes,
            solution: passes.then(|| SolutionSpec {
                system_prompt: "You fix compile errors.".into(),
                goal_template: proposal.goal.clone(),
                allowed_tools: vec!["Bash".into(), "Edit".into()],
                snapshot_id: None,
            }),
            summary: if passes {
                "fixed the compile error".into()
            } else {
                "did not pass".into()
            },
            artifact_dir: None,
        }
    }
}

#[tokio::test]
async fn full_loop_registers_then_reuses_tool() {
    let runner = Arc::new(MockRunner::default());
    let registry = ToolRegistry::in_memory();
    // deterministic tool id for assertions
    let orch = Orchestrator::new(runner.clone(), registry.clone())
        .with_config(OrchestratorConfig {
            max_rl_rounds: 1,
            num_proposals: 2,
            registry_search_k: 5,
            session_id: "test".into(),
            num_samples: 1,
        })
        .with_id_fn(Arc::new(|| "tool-fixed".to_string()));

    // ── Round 1: novel task, must fail → plan → propose → register ──
    let task = "fix the failing parser build";
    let out = orch.solve(task).await.unwrap();
    assert!(out.solved, "first task should be solved via a new tool");
    assert_eq!(out.how, Some(Solved::ByNewTool("tool-fixed".into())));
    assert_eq!(registry.len(), 1, "a tool should be registered");
    assert_eq!(
        runner.plan_calls.load(Ordering::SeqCst),
        1,
        "planner ran once"
    );
    assert!(
        runner.proposal_runs.load(Ordering::SeqCst) >= 1,
        "proposals ran"
    );

    // ── Round 2: similar task → cache hit, NO planner invocation ──
    let task2 = "fix the parser build error";
    let out2 = orch.solve(task2).await.unwrap();
    assert!(out2.solved, "second task should be solved");
    match out2.how {
        Some(Solved::ByCachedTool(id)) => assert_eq!(id, "tool-fixed"),
        other => panic!("expected cache hit, got {other:?}"),
    }
    // The planner must NOT have run again — this is the self-improvement claim.
    assert_eq!(
        runner.plan_calls.load(Ordering::SeqCst),
        1,
        "planner must not run on a cache hit"
    );
    // success count bumped on the cached tool.
    assert_eq!(registry.get("tool-fixed").unwrap().success_count, 1);
}

#[test]
fn failure_trace_is_scrubbed_and_ordered() {
    let g = failing_graph();
    let trace = g.failure_trace("fix the build");
    assert_eq!(trace.failing_nodes.len(), 1);
    assert_eq!(trace.failing_nodes[0].tool, "Bash");
    assert!(trace.failing_nodes[0].error_excerpt.contains("E0433"));
    // directionality text is usable as a planner prompt
    let d = trace.directionality();
    assert!(d.contains("Original task: fix the build"));
    assert!(d.contains("Bash"));
}
