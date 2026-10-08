//! The production [`AgentRlRunner`]: drives real `cersei-agent` agents with a
//! real `Provider`. This is the SDK control layer that turns the AgentRL loop
//! into actual coding work.
//!
//! - `run_general` runs a coding agent (with registry search + any matching
//!   `DynamicTool`s pre-loaded), traces it via [`GraphReporter`], and judges
//!   success with a [`Verifier`].
//! - `plan` asks a planner agent for directed JSON proposals.
//! - `run_proposal` runs a sub-agent in an isolated working dir and verifies it.
//! - `promote` copies the winning dir back into the canonical working dir.

use crate::graph::{ExecutionGraph, FailureTrace, NodeDetail, NodeKind, NodeStatus};
use crate::graph_reporter::GraphReporter;
use crate::orchestrator::{AgentRlRunner, GeneralResult, Proposal, ProposalOutcome};
use crate::planner::{proposal_context, proposals_from_trace};
use crate::registry::dynamic_tool::{DynamicTool, RegistrySearchTool, SolutionReplayer};
use crate::registry::{RegistryEntry, SolutionSpec, ToolRegistry};
use crate::verify::Verifier;
use async_trait::async_trait;
use cersei_agent::Agent;
use cersei_provider::Provider;
use cersei_tools::permissions::AllowAll;
use cersei_tools::{Tool, ToolContext, ToolResult};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Builds a fresh boxed provider per agent (sub-agents get their own client).
pub type ProviderFactory = Arc<dyn Fn() -> Box<dyn Provider> + Send + Sync>;

const GENERAL_SYS: &str = "You are a capable coding agent. Solve the task by reading and writing files \
and running shell commands in the working directory. Before building something new, you may call \
`registry_search` to find a previously-built tool that already solves a similar problem. Finish only \
when the task is fully done.";

const PLANNER_SYS: &str = "You are a planning agent. A previous coding attempt failed. Given the failure \
trace, propose a few DISTINCT recovery strategies that fix the specific failures. Respond with ONLY a \
JSON array (no prose, no code fences) of objects: [{\"angle\": \"...\", \"goal\": \"...\"}].";

const PROPOSAL_SYS: &str =
    "You are a focused coding agent fixing a specific failure. Work only in the \
given working directory. Make the task pass.";

/// Builds a fresh toolset for the GeneralAgent (lets callers restrict it — e.g.
/// a cheap read-only first pass that escalates to richer sandboxed proposals).
pub type ToolsFactory = Arc<dyn Fn() -> Vec<Box<dyn Tool>> + Send + Sync>;

pub struct CerseiRunner {
    provider_factory: ProviderFactory,
    model: Option<String>,
    workdir: PathBuf,
    registry: Arc<ToolRegistry>,
    verifier: Arc<dyn Verifier>,
    proposal_verifier: Option<Arc<dyn Verifier>>,
    general_tools_factory: Option<ToolsFactory>,
    general_system_prompt: Option<String>,
    /// Explicit level; when `None`, the `bricks.toml` level, else `off`.
    compression: Option<cersei_compression::CompressionLevel>,
    /// `bricks.toml` and user rules, applied to every agent this runner builds.
    bricks: Option<cersei_agent::BricksConfig>,
}

impl CerseiRunner {
    pub fn new(
        provider_factory: ProviderFactory,
        workdir: impl Into<PathBuf>,
        registry: Arc<ToolRegistry>,
        verifier: Arc<dyn Verifier>,
    ) -> Self {
        Self {
            provider_factory,
            model: None,
            workdir: workdir.into(),
            registry,
            verifier,
            proposal_verifier: None,
            general_tools_factory: None,
            general_system_prompt: None,
            compression: None,
            bricks: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Restrict the GeneralAgent's base toolset (registry tools are always added
    /// on top). Useful to force escalation into the RL recovery loop.
    pub fn with_general_tools(mut self, f: ToolsFactory) -> Self {
        self.general_tools_factory = Some(f);
        self
    }

    /// Override the GeneralAgent's system prompt (e.g. a domain-specialized one).
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.general_system_prompt = Some(prompt.into());
        self
    }

    /// Code-aware compression of tool outputs (reads/bash) before they reach the
    /// model — a token-efficiency lever inspired by existing agents (vix/codex).
    /// Applies to all agents this runner builds (general + proposals).
    pub fn with_compression(mut self, level: cersei_compression::CompressionLevel) -> Self {
        self.compression = Some(level);
        self
    }

    /// Use a STRICTER verifier for proposal selection than for general
    /// acceptance. A proposal must only win if it genuinely passes a real check —
    /// so this should default to FAIL when no signal exists, whereas the general
    /// verifier may default to PASS (accept the single attempt). Without this,
    /// recovery can promote/register unverifiable proposals.
    pub fn with_proposal_verifier(mut self, v: Arc<dyn Verifier>) -> Self {
        self.proposal_verifier = Some(v);
        self
    }

    /// Context and compression settings plus output rules (normally
    /// `BricksConfig::load(workdir)`), for every agent this runner builds.
    /// An explicit [`Self::with_compression`] wins over its `level`.
    pub fn with_bricks_config(mut self, config: cersei_agent::BricksConfig) -> Self {
        self.bricks = Some(config);
        self
    }

    pub fn bricks_config(&self) -> Option<&cersei_agent::BricksConfig> {
        self.bricks.as_ref()
    }

    fn replayer(&self) -> Arc<dyn SolutionReplayer> {
        Arc::new(SubAgentReplayer {
            provider_factory: self.provider_factory.clone(),
            model: self.model.clone(),
            bricks: self.bricks.clone(),
        })
    }

    fn general_tools(&self, available: &[RegistryEntry]) -> Vec<Box<dyn Tool>> {
        let mut tools = match &self.general_tools_factory {
            Some(f) => f(),
            None => cersei_tools::coding(),
        };
        tools.push(Box::new(RegistrySearchTool::new(self.registry.clone())));
        let replayer = self.replayer();
        for entry in available {
            tools.push(Box::new(DynamicTool::new(entry.clone(), replayer.clone())));
        }
        tools
    }

    fn build_agent(
        &self,
        tools: Vec<Box<dyn Tool>>,
        workdir: &Path,
        system_prompt: &str,
        reporter: Option<GraphReporter>,
    ) -> cersei_types::Result<Agent> {
        let mut b = Agent::builder()
            .provider_boxed((self.provider_factory)())
            .tools(tools)
            .working_dir(workdir.to_path_buf())
            .permission_policy(AllowAll)
            .system_prompt(system_prompt);
        if let Some(cfg) = &self.bricks {
            b = b.bricks_config(cfg.clone());
        }
        if let Some(level) = self.compression {
            b = b.compression_level(level);
        }
        if let Some(m) = &self.model {
            b = b.model(m);
        }
        if let Some(r) = reporter {
            b = b.reporter(r);
        }
        b.build()
    }
}

#[async_trait]
impl AgentRlRunner for CerseiRunner {
    async fn run_general(&self, task: &str, available: &[RegistryEntry]) -> GeneralResult {
        let gr = GraphReporter::new();
        let tools = self.general_tools(available);
        let sys = self.general_system_prompt.as_deref().unwrap_or(GENERAL_SYS);
        let agent = match self.build_agent(tools, &self.workdir, sys, Some(gr.clone())) {
            Ok(a) => a,
            Err(e) => {
                let mut graph = gr.graph();
                let root = graph.root;
                graph.set_status(root, NodeStatus::Failed);
                return GeneralResult {
                    success: false,
                    graph,
                    answer: format!("failed to build agent: {e}"),
                    used_tool: None,
                };
            }
        };

        let result = agent.run(task).await;
        let mut graph = gr.graph();

        match result {
            Ok(out) => {
                let vr = self.verifier.verify(&self.workdir).await;
                if !vr.passed {
                    inject_verifier_failure(&mut graph, &vr.detail);
                }
                let used_tool = detect_used_tool(&graph, available);
                GeneralResult {
                    success: vr.passed,
                    graph,
                    answer: out.text().to_string(),
                    used_tool,
                }
            }
            Err(e) => {
                let root = graph.root;
                graph.set_status(root, NodeStatus::Failed);
                GeneralResult {
                    success: false,
                    graph,
                    answer: e.to_string(),
                    used_tool: None,
                }
            }
        }
    }

    async fn plan(&self, trace: &FailureTrace, n: usize) -> Vec<Proposal> {
        let agent = match self.build_agent(vec![], &self.workdir, PLANNER_SYS, None) {
            Ok(a) => a,
            Err(_) => return proposals_from_trace(trace, n),
        };
        let prompt = format!(
            "{}\n\nReturn ONLY a JSON array of up to {} proposals.",
            trace.directionality(),
            n
        );
        match agent.run(&prompt).await {
            Ok(out) => {
                let parsed = parse_proposals(out.text(), trace, n);
                if parsed.is_empty() {
                    proposals_from_trace(trace, n)
                } else {
                    parsed
                }
            }
            Err(_) => proposals_from_trace(trace, n),
        }
    }

    async fn run_proposal(&self, proposal: &Proposal) -> ProposalOutcome {
        // Isolated working dir = a copy of the current workdir (so the proposal
        // builds on the same starting state without clobbering siblings).
        let dir = std::env::temp_dir().join(format!("agentrl-{}", uuid::Uuid::new_v4()));
        if let Err(e) = copy_dir(&self.workdir, &dir) {
            return ProposalOutcome {
                proposal_id: proposal.id.clone(),
                passed: false,
                solution: None,
                summary: format!("sandbox setup failed: {e}"),
                artifact_dir: None,
            };
        }

        let agent = match self.build_agent(cersei_tools::coding(), &dir, PROPOSAL_SYS, None) {
            Ok(a) => a,
            Err(e) => {
                return ProposalOutcome {
                    proposal_id: proposal.id.clone(),
                    passed: false,
                    solution: None,
                    summary: format!("agent build failed: {e}"),
                    artifact_dir: Some(dir),
                }
            }
        };

        let prompt = format!(
            "{}\n\nComplete the task in the current working directory.",
            proposal.context
        );
        let summary = match agent.run(&prompt).await {
            Ok(out) => out.text().to_string(),
            Err(e) => e.to_string(),
        };

        let pv = self.proposal_verifier.as_ref().unwrap_or(&self.verifier);
        let vr = pv.verify(&dir).await;
        ProposalOutcome {
            proposal_id: proposal.id.clone(),
            passed: vr.passed,
            solution: vr.passed.then(|| SolutionSpec {
                system_prompt: PROPOSAL_SYS.to_string(),
                goal_template: proposal.goal.clone(),
                allowed_tools: vec![
                    "Read".into(),
                    "Write".into(),
                    "Edit".into(),
                    "Bash".into(),
                    "Glob".into(),
                    "Grep".into(),
                ],
                snapshot_id: None,
            }),
            summary: if vr.passed {
                format!("PASSED: {summary}")
            } else {
                format!("FAILED ({}): {summary}", vr.detail)
            },
            artifact_dir: Some(dir),
        }
    }

    async fn promote(&self, winner: &ProposalOutcome) {
        if let Some(dir) = &winner.artifact_dir {
            let _ = copy_dir(dir, &self.workdir);
        }
    }
}

/// Replays a registered solution by spawning a fresh coding sub-agent seeded
/// from its `SolutionSpec`.
struct SubAgentReplayer {
    provider_factory: ProviderFactory,
    model: Option<String>,
    bricks: Option<cersei_agent::BricksConfig>,
}

#[async_trait]
impl SolutionReplayer for SubAgentReplayer {
    async fn replay(&self, entry: &RegistryEntry, goal: &str, ctx: &ToolContext) -> ToolResult {
        let mut b = Agent::builder()
            .provider_boxed((self.provider_factory)())
            .tools(cersei_tools::coding())
            .working_dir(ctx.working_dir.clone())
            .permission_policy(AllowAll)
            .system_prompt(&entry.solution.system_prompt);
        if let Some(cfg) = &self.bricks {
            b = b.bricks_config(cfg.clone());
        }
        if let Some(m) = &self.model {
            b = b.model(m);
        }
        let agent = match b.build() {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("replay build failed: {e}")),
        };
        match agent.run(goal).await {
            Ok(out) => ToolResult::success(out.text().to_string()),
            Err(e) => ToolResult::error(format!("replay failed: {e}")),
        }
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// If a tool node in the graph matches one of the available registered tools,
/// return that tool's id (the agent solved it via a cached tool).
fn detect_used_tool(graph: &ExecutionGraph, available: &[RegistryEntry]) -> Option<String> {
    for node in &graph.nodes {
        if node.kind == NodeKind::ToolCall {
            if let Some(entry) = available.iter().find(|e| e.name == node.label) {
                return Some(entry.tool_id.clone());
            }
        }
    }
    None
}

/// Inject a synthetic failed node so a verifier-rejected (but tool-clean) run
/// still produces a directional failure trace.
fn inject_verifier_failure(graph: &mut ExecutionGraph, detail: &str) {
    let root = graph.root;
    let node = graph.add_node(
        NodeKind::ToolCall,
        "verifier",
        0,
        NodeDetail::Tool {
            input: "verify".into(),
            result: detail.to_string(),
            is_error: true,
        },
    );
    graph.add_edge(root, node, crate::graph::EdgeRel::FailedAfter);
    graph.finish_tool(node, detail.to_string(), true);
    graph.set_status(root, NodeStatus::Failed);
}

/// Best-effort recursive directory copy.
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Parse a JSON array of `{angle, goal}` proposals from (possibly fenced) LLM text.
fn parse_proposals(text: &str, trace: &FailureTrace, n: usize) -> Vec<Proposal> {
    let json = extract_json_array(text);
    let Some(json) = json else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&json) else {
        return Vec::new();
    };
    items
        .into_iter()
        .take(n)
        .enumerate()
        .map(|(i, v)| {
            let angle = v
                .get("angle")
                .and_then(|a| a.as_str())
                .unwrap_or("fix the root cause")
                .to_string();
            let goal = v
                .get("goal")
                .and_then(|g| g.as_str())
                .unwrap_or(&trace.problem_statement)
                .to_string();
            Proposal {
                id: format!("proposal-{i}"),
                goal,
                context: proposal_context(trace, &angle),
            }
        })
        .collect()
}

/// Extract the first top-level JSON array substring from text (handles fences).
fn extract_json_array(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end > start {
        Some(text[start..=end].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod bricks_config_tests {
    use super::*;
    use cersei_provider::{CompletionRequest, CompletionStream};
    use cersei_types::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// First call: one `Bash` tool call; then plain text.
    struct OneToolCall(Arc<AtomicUsize>);

    #[async_trait]
    impl Provider for OneToolCall {
        fn name(&self) -> &str {
            "scripted"
        }
        fn context_window(&self, _: &str) -> u64 {
            128_000
        }
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionStream> {
            let first = self.0.fetch_add(1, Ordering::SeqCst) == 0;
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                let _ = tx
                    .send(StreamEvent::MessageStart {
                        id: "m".into(),
                        model: "x".into(),
                        usage: None,
                    })
                    .await;
                let stop = if first {
                    let _ = tx
                        .send(StreamEvent::ContentBlockStart {
                            index: 0,
                            block_type: "tool_use".into(),
                            id: Some("t1".into()),
                            name: Some("Bash".into()),
                        })
                        .await;
                    let _ = tx
                        .send(StreamEvent::InputJsonDelta {
                            index: 0,
                            partial_json: r#"{"command":"mytool run"}"#.into(),
                        })
                        .await;
                    StopReason::ToolUse
                } else {
                    let _ = tx
                        .send(StreamEvent::ContentBlockStart {
                            index: 0,
                            block_type: "text".into(),
                            id: None,
                            name: None,
                        })
                        .await;
                    let _ = tx
                        .send(StreamEvent::TextDelta {
                            index: 0,
                            text: "done".into(),
                        })
                        .await;
                    StopReason::EndTurn
                };
                let _ = tx.send(StreamEvent::ContentBlockStop { index: 0 }).await;
                let _ = tx
                    .send(StreamEvent::MessageDelta {
                        stop_reason: Some(stop),
                        usage: None,
                    })
                    .await;
                let _ = tx.send(StreamEvent::MessageStop).await;
            });
            Ok(CompletionStream::new(rx))
        }
    }

    /// `n` distinct tool calls (one per request), then a final answer.
    struct ManyCalls(Arc<AtomicUsize>, usize);

    #[async_trait]
    impl Provider for ManyCalls {
        fn name(&self) -> &str {
            "many"
        }
        fn context_window(&self, _: &str) -> u64 {
            128_000
        }
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionStream> {
            let i = self.0.fetch_add(1, Ordering::SeqCst);
            let call = i < self.1;
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                let _ = tx
                    .send(StreamEvent::MessageStart {
                        id: "m".into(),
                        model: "x".into(),
                        usage: None,
                    })
                    .await;
                let stop = if call {
                    let _ = tx
                        .send(StreamEvent::ContentBlockStart {
                            index: 0,
                            block_type: "tool_use".into(),
                            id: Some(format!("t{i}")),
                            name: Some("Bash".into()),
                        })
                        .await;
                    let _ = tx
                        .send(StreamEvent::InputJsonDelta {
                            index: 0,
                            partial_json: format!(r#"{{"command":"step {i}"}}"#),
                        })
                        .await;
                    StopReason::ToolUse
                } else {
                    let _ = tx
                        .send(StreamEvent::ContentBlockStart {
                            index: 0,
                            block_type: "text".into(),
                            id: None,
                            name: None,
                        })
                        .await;
                    let _ = tx
                        .send(StreamEvent::TextDelta {
                            index: 0,
                            text: "done".into(),
                        })
                        .await;
                    StopReason::EndTurn
                };
                let _ = tx.send(StreamEvent::ContentBlockStop { index: 0 }).await;
                let _ = tx
                    .send(StreamEvent::MessageDelta {
                        stop_reason: Some(stop),
                        usage: None,
                    })
                    .await;
                let _ = tx.send(StreamEvent::MessageStop).await;
            });
            Ok(CompletionStream::new(rx))
        }
    }

    /// The runner's agents (general, proposals, replays share
    /// `build_agent` / the replayer) have no turn budget: past the former
    /// default of 20, then the answer.
    #[tokio::test]
    async fn the_runners_agents_have_no_turn_limit() {
        let work = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let factory: ProviderFactory = Arc::new(move || Box::new(ManyCalls(c.clone(), 25)));
        let runner = CerseiRunner::new(
            factory,
            work.path(),
            ToolRegistry::in_memory(),
            Arc::new(crate::verify::AcceptVerifier),
        );
        let agent = runner
            .build_agent(vec![Box::new(NoisyTool)], work.path(), "test", None)
            .unwrap();
        let out = tokio::time::timeout(std::time::Duration::from_secs(60), agent.run("go"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.turns, 26);
        assert_eq!(calls.load(Ordering::SeqCst), 26);
        assert!(out.is_complete());
    }

    struct NoisyTool;

    #[async_trait]
    impl Tool for NoisyTool {
        fn name(&self) -> &str {
            "Bash"
        }
        fn description(&self) -> &str {
            "fake shell"
        }
        fn permission_level(&self) -> cersei_tools::PermissionLevel {
            cersei_tools::PermissionLevel::None
        }
        fn category(&self) -> cersei_tools::ToolCategory {
            cersei_tools::ToolCategory::Shell
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        async fn execute(&self, _: serde_json::Value, _: &ToolContext) -> ToolResult {
            let mut out: String = (0..300).map(|i| format!("noise line {i}\n")).collect();
            out.push_str("result: keep me\n");
            ToolResult::success(out)
        }
    }

    async fn tool_result(runner: &CerseiRunner, work: &Path) -> String {
        let agent = runner
            .build_agent(vec![Box::new(NoisyTool)], work, "test", None)
            .unwrap();
        agent.run("go").await.unwrap();
        agent
            .messages()
            .iter()
            .flat_map(|m| m.content_blocks())
            .find_map(|b| match b {
                ContentBlock::ToolResult {
                    content: ToolResultContent::Text(t),
                    ..
                } => Some(t),
                _ => None,
            })
            .unwrap()
    }

    fn runner(work: &Path) -> CerseiRunner {
        let calls = Arc::new(AtomicUsize::new(0));
        let factory: ProviderFactory = Arc::new(move || Box::new(OneToolCall(calls.clone())));
        CerseiRunner::new(
            factory,
            work,
            ToolRegistry::in_memory(),
            Arc::new(crate::verify::AcceptVerifier),
        )
    }

    #[tokio::test]
    async fn a_custom_rule_from_bricks_toml_is_applied() {
        let work = tempfile::tempdir().unwrap();
        std::fs::write(
            work.path().join("bricks.toml"),
            "[compression]\nlevel = \"minimal\"\n\n[compression.filters.mytool]\n\
             match = [{ program = \"mytool\" }]\nstrip_lines_matching = ['^noise line ']\n",
        )
        .unwrap();
        let config = cersei_agent::BricksConfig::from_sources(&cersei_compression::RuleSources {
            user_rules_dir: None,
            bricks_toml: Some(work.path().join("bricks.toml")),
        });
        assert!(config.diagnostics.is_empty(), "{:?}", config.diagnostics);
        let r = runner(work.path()).with_bricks_config(config);
        let out = tool_result(&r, work.path()).await;
        assert!(out.contains("rule `mytool`"), "{out}");
        assert!(out.contains("result: keep me"));
        assert!(!out.contains("noise line 150"));
    }

    #[tokio::test]
    async fn without_configuration_the_builtin_behaviour_is_kept() {
        let work = tempfile::tempdir().unwrap();
        let r = runner(work.path());
        assert!(r.bricks_config().is_none());
        // Level `off` and no rule: the output is passed through.
        let out = tool_result(&r, work.path()).await;
        assert!(out.contains("noise line 150"));
        assert!(!out.starts_with("[bricks:"));
    }
}
