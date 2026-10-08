//! tbench-agent — a purpose-built Terminal-Bench coding agent.
//!
//! A thin, fast binary over the Cersei SDK + AgentRL: a TB-specialized coding
//! agent (Gemini 3.1 Pro by default) with the full IO/coding toolset, a high
//! turn budget, and a verifier that trusts a real in-container test script when
//! one exists (enabling best-of-N / recovery), and otherwise runs a single
//! strong attempt. File changes land in the working directory for grading.

mod prompt;

use cersei_agent::BricksConfig;
use cersei_agentrl::{
    CerseiRunner, ChainVerifier, Orchestrator, OrchestratorConfig, ProviderFactory, Solved,
    TestScriptVerifier, ToolRegistry, Verifier,
};
use cersei_provider::Provider;
use clap::Parser;
use std::io::Read;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "tbench-agent",
    version,
    about = "Purpose-built Terminal-Bench coding agent (Cersei SDK + AgentRL)"
)]
struct Cli {
    /// Task instruction (positional). Alternatively use -p, or pipe via stdin.
    task: Option<String>,

    /// Task instruction.
    #[arg(short = 'p', long)]
    prompt: Option<String>,

    /// Model to use, as `provider_id/model_id` from the providers configuration.
    #[arg(long)]
    model: String,

    /// Providers configuration file (`.toml` or `.json`). Default:
    /// `~/.bricks/providers.toml`. An explicit path replaces the default; the
    /// two are never merged.
    #[arg(long)]
    providers: Option<String>,

    /// Tool-output compression for token efficiency: off | minimal | aggressive.
    /// Default: `[compression] level` of `bricks.toml`, else minimal.
    #[arg(long)]
    compress: Option<String>,

    /// Working directory (default: current dir).
    #[arg(short = 'C', long)]
    dir: Option<String>,

    /// Best-of-N: retry the attempt up to N times, keeping the first that passes
    /// the verifier. Only adds value when a real test script is present.
    #[arg(long, default_value_t = 1)]
    samples: u32,

    /// Recovery rounds: on verified failure, plan + run sandboxed proposals.
    /// Only meaningful with a real in-container test script (default off).
    #[arg(long, default_value_t = 0)]
    rounds: u32,

    /// Proposals per recovery round.
    #[arg(long, default_value_t = 2)]
    proposals: usize,

    /// Removed in 0.4.8 (agents have no turn limit): refused with a
    /// migration message, never applied.
    #[arg(long, hide = true)]
    max_turns: Option<String>,

    /// Emit a machine-readable JSON result line on stdout.
    #[arg(long)]
    json: bool,

    /// Tool-registry directory (default: ephemeral temp dir).
    #[arg(long)]
    registry: Option<String>,
}

fn resolve_task(cli: &Cli) -> anyhow::Result<String> {
    if let Some(t) = cli.prompt.clone().or_else(|| cli.task.clone()) {
        if !t.trim().is_empty() {
            return Ok(t);
        }
    }
    // fall back to stdin
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    if buf.trim().is_empty() {
        anyhow::bail!("no task provided (positional arg, -p, or stdin)");
    }
    Ok(buf)
}

/// Load the Bricks configuration and report its problems on stderr (the
/// agents also report them as `Status` events at their first run).
fn load_bricks_config(sources: &cersei_compression::RuleSources) -> BricksConfig {
    let config = BricksConfig::from_sources(sources);
    for d in &config.diagnostics {
        eprintln!("[tbench-agent] configuration: {d}");
    }
    config
}

/// Apply the configuration; `--compress` wins over `[compression] level`,
/// and `minimal` stays the default when neither is set.
fn configure_runner(
    runner: CerseiRunner,
    config: BricksConfig,
    compress: Option<&str>,
) -> CerseiRunner {
    let level = compress
        .map(|c| {
            c.parse::<cersei_compression::CompressionLevel>()
                .unwrap_or_default()
        })
        .or(config.compression_level)
        .unwrap_or(cersei_compression::CompressionLevel::Minimal);
    runner.with_bricks_config(config).with_compression(level)
}

/// Options that no longer exist are refused before anything starts, with
/// what to do instead.
fn refuse_removed_options(cli: &Cli) -> anyhow::Result<()> {
    if cli.max_turns.is_some() {
        anyhow::bail!(
            "--max-turns was removed in 0.4.8: agents have no turn limit; run again without it"
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    refuse_removed_options(&cli)?;
    let instruction = resolve_task(&cli)?;

    // Load the configuration and validate the selection up front, including
    // the secret reference, so a bad setup fails before any work starts.
    let registry =
        cersei_provider::ProviderRegistry::load(cli.providers.as_deref().map(std::path::Path::new))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    let resolved = registry
        .resolve(&cli.model)
        .map_err(|e| anyhow::anyhow!("cannot resolve model '{}': {e}", cli.model))?;
    resolved
        .build_provider()
        .map_err(|e| anyhow::anyhow!("cannot build provider for '{}': {e}", cli.model))?;

    // The model label used in events and logs: the explicit selection.
    let resolved_model = cli.model.clone();
    let provider_factory: ProviderFactory = Arc::new(move || {
        resolved
            .build_provider()
            .expect("provider resolution already validated")
            .into_boxed()
    });

    // Connectivity probe: surface provider/network errors directly (a static
    // musl binary can fail at DNS/TLS before the agent loop ever runs a tool).
    {
        let p = (provider_factory)();
        let mut req = cersei_provider::CompletionRequest::new(&resolved_model);
        req.messages = vec![cersei_types::Message::user("ping")];
        req.max_tokens = 8;
        match p.complete_blocking(req).await {
            Ok(r) => eprintln!(
                "[tbench-agent] probe OK: {:?}",
                r.message
                    .get_text()
                    .map(|t| t.chars().take(40).collect::<String>())
            ),
            Err(e) => eprintln!("[tbench-agent] probe ERR: {e}"),
        }
    }

    let workdir = cli
        .dir
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);

    // Trust only a real in-container test script; otherwise accept the single
    // attempt (default-pass) — never speculatively "verify" with accept-anything.
    let verifier: Arc<dyn Verifier> = Arc::new(
        ChainVerifier::new(vec![Arc::new(TestScriptVerifier::default_candidates())])
            .with_default(true),
    );
    // Proposals must NEVER win by default — only on a genuine test-script pass.
    // (A recovery proposal that can't be verified must not be promoted/registered.)
    let proposal_verifier: Arc<dyn Verifier> = Arc::new(
        ChainVerifier::new(vec![Arc::new(TestScriptVerifier::default_candidates())])
            .with_default(false),
    );

    let registry_dir = cli
        .registry
        .clone()
        .or_else(|| std::env::var("TBENCH_REGISTRY").ok())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("cersei-tbench-registry"));
    let registry = ToolRegistry::open(&registry_dir)
        .map_err(|e| anyhow::anyhow!("registry open failed: {e}"))?;

    // Optional extra hints (failure patterns) injected by the harness.
    let hints = std::env::var("TBENCH_HINTS")
        .ok()
        .or_else(|| std::env::var("ABSTRACT_FAILURE_PATTERNS").ok());
    let task = prompt::build_task(&instruction, hints.as_deref());

    // `<workdir>/bricks.toml` and `~/.bricks/rules/*.toml`, as for every agent.
    let bricks = load_bricks_config(&cersei_compression::RuleSources::standard(&workdir));
    let runner = Arc::new(configure_runner(
        CerseiRunner::new(
            provider_factory,
            workdir.clone(),
            registry.clone(),
            verifier,
        )
        .with_proposal_verifier(proposal_verifier)
        .with_model(&resolved_model)
        .with_system_prompt(prompt::TBENCH_SYSTEM_PROMPT),
        bricks,
        cli.compress.as_deref(),
    ));

    let cfg = OrchestratorConfig {
        max_rl_rounds: cli.rounds,
        num_proposals: cli.proposals,
        registry_search_k: 5,
        session_id: "tbench".to_string(),
        num_samples: cli.samples,
    };

    let orchestrator = Orchestrator::new(runner, registry.clone()).with_config(cfg);

    let outcome = orchestrator.solve(&task).await;

    match outcome {
        Ok(o) => {
            let how = match &o.how {
                Some(Solved::Directly) => "directly",
                Some(Solved::ByCachedTool(_)) => "cached_tool",
                Some(Solved::ByNewTool(_)) => "new_tool",
                None => "unsolved",
            };
            // Diagnostics: how much work actually happened. A near-empty graph
            // means the agent never really ran (e.g. provider/network failure);
            // a rich graph means it worked and the task was just hard.
            let nodes = o.last_graph.nodes.len();
            let tool_calls = o
                .last_graph
                .nodes
                .iter()
                .filter(|n| matches!(n.kind, cersei_agentrl::NodeKind::ToolCall))
                .count();
            eprintln!(
                "[tbench-agent] solved={} via={} graph_nodes={} tool_calls={} answer={:?} model={} dir={}",
                o.solved,
                how,
                nodes,
                tool_calls,
                o.answer.chars().take(200).collect::<String>(),
                resolved_model,
                workdir.display()
            );
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "type": "tbench_result",
                        "solved": o.solved,
                        "how": how,
                    })
                );
            }
        }
        Err(e) => {
            eprintln!("[tbench-agent] error: {e}");
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({ "type": "tbench_result", "solved": false, "error": e.to_string() })
                );
            }
        }
    }

    // Always exit 0 — the grader scores the filesystem, not our exit code.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cersei_agentrl::AcceptVerifier;

    fn runner(dir: &std::path::Path) -> CerseiRunner {
        let factory: ProviderFactory = Arc::new(|| panic!("no request is made by these tests"));
        CerseiRunner::new(
            factory,
            dir,
            ToolRegistry::in_memory(),
            Arc::new(AcceptVerifier),
        )
    }

    #[test]
    fn user_and_project_rules_reach_the_runner() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("rules");
        std::fs::create_dir(&rules).unwrap();
        std::fs::write(
            rules.join("mine.toml"),
            "schema_version = 1\n[filters.mytool]\nmatch = [{ program = \"mytool\" }]\nmax_lines = 5\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("bricks.toml"),
            "[compression.filters.mytool]\nmatch = [{ program = \"mytool\" }]\nmax_lines = 9\n",
        )
        .unwrap();
        let config = load_bricks_config(&cersei_compression::RuleSources {
            user_rules_dir: Some(rules),
            bricks_toml: Some(dir.path().join("bricks.toml")),
        });
        let r = configure_runner(runner(dir.path()), config, None);
        let applied = r.bricks_config().unwrap();
        assert!(applied.diagnostics.is_empty());
        // bricks.toml wins over the user rule file, as everywhere else.
        assert_eq!(applied.rules.get("mytool").unwrap().max_lines, Some(9));
    }

    #[test]
    fn without_user_configuration_the_builtin_rules_apply() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_bricks_config(&cersei_compression::RuleSources {
            user_rules_dir: Some(dir.path().join("absent")),
            bricks_toml: Some(dir.path().join("bricks.toml")),
        });
        let r = configure_runner(runner(dir.path()), config, None);
        let applied = r.bricks_config().unwrap();
        assert!(applied.diagnostics.is_empty());
        assert!(applied.rules.get("cargo-test").is_some());
        assert!(applied.rules.get("mytool").is_none());
    }
}

#[cfg(test)]
mod removed_options_tests {
    use super::*;

    #[test]
    fn the_former_turn_option_is_refused_and_hidden() {
        let cli = Cli::try_parse_from(["tbench-agent", "--model", "p/m", "-p", "x"]).unwrap();
        assert!(refuse_removed_options(&cli).is_ok());
        let cli = Cli::try_parse_from([
            "tbench-agent",
            "--model",
            "p/m",
            "-p",
            "x",
            "--max-turns",
            "80",
        ])
        .unwrap();
        let e = refuse_removed_options(&cli).unwrap_err().to_string();
        assert!(e.contains("was removed"), "{e}");
        let mut help = Vec::new();
        <Cli as clap::CommandFactory>::command()
            .write_long_help(&mut help)
            .unwrap();
        assert!(!String::from_utf8(help).unwrap().contains("max-turns"));
    }
}
