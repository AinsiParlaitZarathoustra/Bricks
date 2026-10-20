//! The RL orchestration loop: run → (success?) → trace → plan → fan-out into
//! sandboxes → select winner → promote → register → recall via memory.
//!
//! The loop is written against the [`AgentRlRunner`] trait so it can be driven
//! by the production runner (bricks-agent + bricks-vms sandboxes) or by a mock
//! in tests. This keeps the *policy* (the loop) separate from the *mechanism*
//! (how a general agent / proposal is actually executed).

use crate::graph::{ExecutionGraph, FailureTrace};
use crate::memory_bridge;
use crate::planner::{build_entry, tool_name_for};
use crate::registry::{RegistryEntry, SolutionSpec, ToolRegistry};
use async_trait::async_trait;
use bricks_memory::Memory;
use bricks_types::Result;
use std::sync::Arc;

/// Result of running the GeneralAgent on a task.
pub struct GeneralResult {
    pub success: bool,
    pub graph: ExecutionGraph,
    pub answer: String,
    /// If a registered `DynamicTool` was used to solve it, its tool_id.
    pub used_tool: Option<String>,
}

/// A directed recovery proposal to run in a sandbox.
pub struct Proposal {
    pub id: String,
    pub goal: String,
    pub context: String,
}

/// Outcome of running a single proposal sub-agent in its sandbox.
pub struct ProposalOutcome {
    pub proposal_id: String,
    pub passed: bool,
    /// If it passed, how to replay it as a registered tool.
    pub solution: Option<SolutionSpec>,
    pub summary: String,
    /// Runner-specific handle to the proposal's artifacts (e.g. its sandbox /
    /// working directory) so the winner can be promoted. Opaque to the loop.
    pub artifact_dir: Option<std::path::PathBuf>,
}

/// The mechanism behind the loop: how to run the general agent, generate
/// proposals, and execute a proposal in a sandbox.
#[async_trait]
pub trait AgentRlRunner: Send + Sync {
    /// Run the GeneralAgent on `task`, pre-loading any `available` registry tools.
    async fn run_general(&self, task: &str, available: &[RegistryEntry]) -> GeneralResult;
    /// Ask the PlannerAgent for up to `n` proposals given the failure trace.
    async fn plan(&self, trace: &FailureTrace, n: usize) -> Vec<Proposal>;
    /// Execute one proposal in an isolated sandbox; report whether it passed.
    async fn run_proposal(&self, proposal: &Proposal) -> ProposalOutcome;

    /// Promote the winning proposal's artifacts into the canonical working dir.
    /// Default: no-op (mock/in-memory runners need no promotion).
    async fn promote(&self, _winner: &ProposalOutcome) {}
}

#[derive(Clone)]
pub struct OrchestratorConfig {
    pub max_rl_rounds: u32,
    pub num_proposals: usize,
    pub registry_search_k: usize,
    pub session_id: String,
    /// Best-of-N for the GeneralAgent phase: retry the general attempt up to this
    /// many times, accepting the first that the verifier passes. The biggest raw
    /// pass-rate lever when no recovery loop is needed. Default 1 (single attempt).
    pub num_samples: u32,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_rl_rounds: 1,
            num_proposals: 2,
            registry_search_k: 5,
            session_id: "agentrl".to_string(),
            num_samples: 1,
        }
    }
}

/// How a task was ultimately solved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Solved {
    /// Solved on the first pass with no registered tool.
    Directly,
    /// Solved by a previously-registered tool (a cache hit).
    ByCachedTool(String),
    /// Solved by a newly-built-and-registered tool.
    ByNewTool(String),
}

pub struct SolveOutcome {
    pub solved: bool,
    pub how: Option<Solved>,
    pub last_graph: ExecutionGraph,
    pub answer: String,
}

pub struct Orchestrator {
    runner: Arc<dyn AgentRlRunner>,
    registry: Arc<ToolRegistry>,
    memory: Option<Arc<dyn Memory>>,
    config: OrchestratorConfig,
    /// Monotonic counter for deterministic tool ids (avoids needing randomness
    /// in tests; real ids can be uuids via `with_id_fn`).
    id_fn: Arc<dyn Fn() -> String + Send + Sync>,
    now_fn: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Orchestrator {
    pub fn new(runner: Arc<dyn AgentRlRunner>, registry: Arc<ToolRegistry>) -> Self {
        Self {
            runner,
            registry,
            memory: None,
            config: OrchestratorConfig::default(),
            id_fn: Arc::new(|| format!("tool-{}", uuid::Uuid::new_v4())),
            now_fn: Arc::new(|| chrono::Utc::now().timestamp()),
        }
    }

    pub fn with_memory(mut self, memory: Arc<dyn Memory>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_config(mut self, config: OrchestratorConfig) -> Self {
        self.config = config;
        self
    }

    /// Override id/time sources (tests use deterministic ones).
    pub fn with_id_fn(mut self, f: Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        self.id_fn = f;
        self
    }

    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    /// Run the full loop for a task.
    pub async fn solve(&self, task: &str) -> Result<SolveOutcome> {
        // 1) lookup-before-build: surface matching registered tools.
        let available = self.registry.search(task, self.config.registry_search_k);
        let had_match = !available.is_empty();

        // 2) run the GeneralAgent (best-of-N): retry up to `num_samples` times,
        //    accepting the first attempt the verifier passes.
        let samples = self.config.num_samples.max(1);
        let mut gr = self.runner.run_general(task, &available).await;
        let mut sample = 1;
        while !gr.success && sample < samples {
            sample += 1;
            gr = self.runner.run_general(task, &available).await;
        }
        if gr.success {
            let how = match &gr.used_tool {
                Some(tid) if had_match => {
                    self.registry.record_success(tid);
                    Solved::ByCachedTool(tid.clone())
                }
                _ => Solved::Directly,
            };
            return Ok(SolveOutcome {
                solved: true,
                how: Some(how),
                last_graph: gr.graph,
                answer: gr.answer,
            });
        }

        // 3) failed → extract directionality from the execution graph.
        let trace = gr.graph.failure_trace(task);
        let last_graph = gr.graph;

        // 4) plan + fan-out proposals into sandboxes, bounded by max_rl_rounds.
        for _round in 0..self.config.max_rl_rounds {
            let proposals = self.runner.plan(&trace, self.config.num_proposals).await;
            if proposals.is_empty() {
                break;
            }

            // Run all proposals concurrently; winner = first to pass (the real
            // runner uses KvStore CAS for an authoritative first-writer).
            let outcomes =
                futures::future::join_all(proposals.iter().map(|p| self.runner.run_proposal(p)))
                    .await;

            if let Some(win) = outcomes.into_iter().find(|o| o.passed) {
                // Promote the winner's artifacts before registering the tool.
                self.runner.promote(&win).await;
                if let Some(solution) = win.solution {
                    let tool_id = (self.id_fn)();
                    let entry =
                        build_entry(tool_id.clone(), task, &trace, solution, (self.now_fn)());
                    let name = entry.name.clone();
                    self.registry.register(entry)?;
                    if let Some(mem) = &self.memory {
                        let _ = memory_bridge::record_solution(
                            mem,
                            &self.config.session_id,
                            task,
                            &name,
                            &tool_id,
                        )
                        .await;
                    }
                    return Ok(SolveOutcome {
                        solved: true,
                        how: Some(Solved::ByNewTool(tool_id)),
                        last_graph,
                        answer: win.summary,
                    });
                }
            }
        }

        // 5) exhausted rounds without a passing proposal.
        let _ = tool_name_for(task); // (keeps the helper exercised; no-op otherwise)
        Ok(SolveOutcome {
            solved: false,
            how: None,
            last_graph,
            answer: String::new(),
        })
    }
}
