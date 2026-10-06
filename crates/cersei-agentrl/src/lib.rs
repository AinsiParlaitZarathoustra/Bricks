//! cersei-agentrl: AgentRL — a self-evolving orchestration layer on top of the
//! Cersei agent SDK. Governs the run → fail → trace → plan → sandbox → promote
//! → register loop via an [`ExecutionGraph`] and a dynamic [`ToolRegistry`].
//!
//! - [`graph`] / [`graph_reporter`]: build a failure-tracing DAG from agent events.
//! - [`registry`]: a persisted, searchable database of agent-built tools.
//! - [`orchestrator`]: the RL loop, written against the [`AgentRlRunner`] trait.
//! - [`scrub`]: the hard rule — no secrets in persisted artifacts.

pub mod graph;
pub mod graph_reporter;
pub mod memory_bridge;
pub mod orchestrator;
pub mod planner;
pub mod registry;
pub mod runner;
pub mod scrub;
pub mod verify;

pub use graph::{ExecutionGraph, FailurePoint, FailureTrace, NodeKind, NodeStatus};
pub use graph_reporter::GraphReporter;
pub use orchestrator::{
    AgentRlRunner, GeneralResult, Orchestrator, OrchestratorConfig, Proposal, ProposalOutcome,
    SolveOutcome, Solved,
};
pub use registry::dynamic_tool::{DynamicTool, RegistrySearchTool, SolutionReplayer};
pub use registry::{RegistryEntry, SolutionSpec, ToolRegistry};
pub use runner::{CerseiRunner, ProviderFactory, ToolsFactory};
pub use verify::{
    AcceptVerifier, ChainVerifier, CommandVerifier, TestScriptVerifier, Verifier, VerifyResult,
};
