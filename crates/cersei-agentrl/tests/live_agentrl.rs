//! Live end-to-end AgentRL tests against a model from your provider configuration.
//!
//! Ignored by default (makes paid network calls). Name the model and, if needed,
//! the configuration file; the key is whatever `api_key_env` names (never put a
//! key on the command line):
//!
//!   BRICKS_LIVE_MODEL=provider_id/model_id \
//!   BRICKS_LIVE_CONFIG=/path/to/providers.toml   # optional; default ~/.bricks/providers.toml
//!   cargo test -p cersei-agentrl --test live_agentrl -- --ignored --nocapture
//!
//! It hands AgentRL a real coding task with an INDEPENDENT verifier (the agent
//! cannot cheat the check), runs the full orchestrator, asserts the produced
//! artifact actually works, and prints which solve path AgentRL took.

use cersei_agentrl::orchestrator::{Orchestrator, OrchestratorConfig};
use cersei_agentrl::verify::Verifier;
use cersei_agentrl::{CerseiRunner, CommandVerifier, Solved, ToolRegistry};
use cersei_provider::{Provider, ProviderRegistry};
use std::sync::Arc;

/// The configured model under test, or `None` when the test is not set up.
fn live_model() -> Option<cersei_provider::ResolvedModel> {
    let selection = std::env::var("BRICKS_LIVE_MODEL")
        .ok()
        .filter(|m| !m.is_empty())?;
    let config = std::env::var_os("BRICKS_LIVE_CONFIG").map(std::path::PathBuf::from);
    let registry = ProviderRegistry::load(config.as_deref()).expect("providers configuration");
    Some(
        registry
            .resolve(&selection)
            .expect("BRICKS_LIVE_MODEL resolves"),
    )
}

#[tokio::test]
#[ignore = "live network test; run with --ignored after sourcing .env"]
async fn agentrl_solves_a_real_coding_task() {
    let Some(model) = live_model() else {
        eprintln!("SKIP: BRICKS_LIVE_MODEL is not set");
        return;
    };
    let selection = model.selection();

    // Isolated working dir + persisted registry dir.
    let work = tempfile::tempdir().unwrap();
    let reg_dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::open(reg_dir.path()).unwrap();

    // The task: a real little CLI program with edge cases.
    let task = "Create a Python script named `gcd.py` in the working directory. It takes two \
        integer command-line arguments and prints (only) their greatest common divisor computed \
        with the Euclidean algorithm. Example: `python3 gcd.py 48 36` prints `12`. Handle the \
        case where one argument is 0 (gcd(0, n) == n).";

    // INDEPENDENT verifier — our own checks, not the agent's. Exercises edge cases.
    let verifier: Arc<dyn Verifier> = Arc::new(CommandVerifier::new(
        "python3 gcd.py 48 36 | grep -qx 12 && \
         python3 gcd.py 1071 462 | grep -qx 21 && \
         python3 gcd.py 0 7 | grep -qx 7 && \
         python3 gcd.py 17 5 | grep -qx 1",
    ));

    // Provider factory: the key is resolved from the environment reference each
    // time and held in memory only.
    let provider_factory: cersei_agentrl::ProviderFactory = Arc::new(move || {
        model
            .build_provider()
            .expect("provider builds")
            .into_boxed() as Box<dyn Provider>
    });

    let runner = Arc::new(
        CerseiRunner::new(
            provider_factory,
            work.path(),
            registry.clone(),
            verifier.clone(),
        )
        .with_model(&selection)
        .with_max_turns(16),
    );

    let orch = Orchestrator::new(runner, registry.clone()).with_config(OrchestratorConfig {
        max_rl_rounds: 1,
        num_proposals: 2,
        registry_search_k: 5,
        session_id: "live".into(),
        num_samples: 1,
    });

    let outcome = orch.solve(task).await.expect("orchestrator ran");

    // ── Report what AgentRL actually did ──
    eprintln!("\n──────── AgentRL run report ────────");
    eprintln!("solved: {}", outcome.solved);
    eprintln!("path:   {:?}", outcome.how);
    eprintln!(
        "graph:  {} nodes, {} failed",
        outcome.last_graph.nodes.len(),
        outcome
            .last_graph
            .nodes
            .iter()
            .filter(|n| n.status == cersei_agentrl::NodeStatus::Failed)
            .count()
    );
    eprintln!("registry entries: {}", registry.len());
    match &outcome.how {
        Some(Solved::Directly) => eprintln!("→ GeneralAgent solved it on the first pass."),
        Some(Solved::ByNewTool(id)) => {
            eprintln!("→ RL loop fired: GeneralAgent failed, a sandboxed proposal passed and was registered as {id}.")
        }
        Some(Solved::ByCachedTool(id)) => eprintln!("→ Solved via a cached tool {id}."),
        None => eprintln!("→ NOT solved."),
    }
    eprintln!("────────────────────────────────────\n");

    // ── Assert the artifact genuinely works (independent of `solved`) ──
    assert!(outcome.solved, "AgentRL did not solve the task");
    let final_check = verifier.verify(work.path()).await;
    assert!(
        final_check.passed,
        "the produced gcd.py failed the independent verifier: {}",
        final_check.detail
    );
}

/// Forces the RL RECOVERY loop with a live LLM: the GeneralAgent is restricted to
/// a read-only toolset, so it CANNOT create the file and fails the verifier. The
/// orchestrator then traces the failure, the PlannerAgent proposes fixes, the
/// proposals run with full coding tools in isolated sandboxes, a winner is
/// promoted, and the solution is registered as a reusable tool.
#[tokio::test]
#[ignore = "live network test; run with --ignored after sourcing .env"]
async fn agentrl_recovery_loop_registers_a_tool() {
    let Some(model) = live_model() else {
        eprintln!("SKIP: BRICKS_LIVE_MODEL is not set");
        return;
    };
    let selection = model.selection();
    let work = tempfile::tempdir().unwrap();
    let reg_dir = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::open(reg_dir.path()).unwrap();

    let task = "Create a Python script named `greet.py` in the working directory that prints \
        exactly `hello, world` when run as `python3 greet.py`.";
    let verifier: Arc<dyn Verifier> = Arc::new(CommandVerifier::new(
        "python3 greet.py | grep -qx 'hello, world'",
    ));

    let provider_factory: cersei_agentrl::ProviderFactory = Arc::new(move || {
        model
            .build_provider()
            .expect("provider builds")
            .into_boxed() as Box<dyn Provider>
    });

    // GeneralAgent gets a READ-ONLY toolset → guaranteed first-pass failure →
    // forces escalation into the planner/proposal/register loop.
    let readonly: cersei_agentrl::ToolsFactory = Arc::new(|| {
        vec![Box::new(cersei_tools::file_read::FileReadTool) as Box<dyn cersei_tools::Tool>]
    });

    let runner = Arc::new(
        CerseiRunner::new(
            provider_factory,
            work.path(),
            registry.clone(),
            verifier.clone(),
        )
        .with_model(&selection)
        .with_max_turns(12)
        .with_general_tools(readonly),
    );

    let orch = Orchestrator::new(runner, registry.clone()).with_config(OrchestratorConfig {
        max_rl_rounds: 1,
        num_proposals: 2,
        registry_search_k: 5,
        session_id: "recovery".into(),
        num_samples: 1,
    });

    let outcome = orch.solve(task).await.expect("orchestrator ran");

    eprintln!("\n──────── AgentRL recovery report ────────");
    eprintln!("solved: {}  path: {:?}", outcome.solved, outcome.how);
    eprintln!("registry entries after run: {}", registry.len());
    for e in registry.all() {
        eprintln!("  registered tool: {} ({})", e.name, e.tool_id);
    }
    eprintln!("─────────────────────────────────────────\n");

    assert!(outcome.solved, "recovery loop failed to solve the task");
    assert!(
        matches!(outcome.how, Some(Solved::ByNewTool(_))),
        "expected the RL loop to register a NEW tool, got {:?}",
        outcome.how
    );
    assert_eq!(registry.len(), 1, "a reusable tool should be registered");
    // The promoted winner must actually satisfy the independent verifier.
    assert!(
        verifier.verify(work.path()).await.passed,
        "promoted artifact does not work"
    );
}
