//! longmem-bench — head-to-head LongMemEval runner for the Cersei memory stack.
//!
//! Usage:
//!   longmem-bench --dataset s --config all --limit 10
//!
//! Datasets:   s | m | oracle       (must be pre-downloaded into ./data/)
//! Configs:    all | baseline | embed | graph | hybrid | structured-vector | structured-hybrid
//!
//! Models (answerer, judge, extractor) are selected as `provider_id/model_id` from
//! the providers configuration (`--providers`, default `~/.bricks/providers.toml`);
//! their keys are the `api_key_env` variables that configuration names.
//!
//! Embeddings are a separate service and keep their own key variable:
//!   OPENAI_API_KEY   — with `--embeddings openai`
//!   GOOGLE_API_KEY   — with `--embeddings gemini` (or GEMINI_API_KEY)
//!
//! Output: JSON + summary JSON in ./results/<config>-<dataset>.json

mod configs;
mod dataset;
mod judge;
mod mastra_prompts;
mod omega_prompts;
mod query_expand;
mod report;
mod runner;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use cersei_embeddings::{EmbeddingError, EmbeddingProvider, GeminiEmbeddings, OpenAiEmbeddings};
use cersei_provider::{
    CompletionRequest, CompletionStream, ConfiguredProvider, Provider, ProviderRegistry,
};
use clap::{Parser, ValueEnum};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Dispatching enum so we can swap the underlying embedding backend at runtime
/// while keeping the generic `EmbedConfig<P>` / `HybridConfig<P, E>` bounds
/// happy. Both variants delegate to their inner concrete provider.
pub enum AnyEmbeddings {
    Openai(OpenAiEmbeddings),
    Gemini(GeminiEmbeddings),
}

#[async_trait]
impl EmbeddingProvider for AnyEmbeddings {
    fn name(&self) -> &str {
        match self {
            AnyEmbeddings::Openai(p) => p.name(),
            AnyEmbeddings::Gemini(p) => p.name(),
        }
    }
    fn dimensions(&self) -> usize {
        match self {
            AnyEmbeddings::Openai(p) => p.dimensions(),
            AnyEmbeddings::Gemini(p) => p.dimensions(),
        }
    }
    fn model_id(&self) -> String {
        match self {
            AnyEmbeddings::Openai(p) => p.model_id(),
            AnyEmbeddings::Gemini(p) => p.model_id(),
        }
    }
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        match self {
            AnyEmbeddings::Openai(p) => p.embed_batch(texts).await,
            AnyEmbeddings::Gemini(p) => p.embed_batch(texts).await,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "longmem-bench",
    about = "LongMemEval runner for the Cersei memory stack"
)]
struct Cli {
    /// Dataset variant. Files expected at `./data/longmemeval_<name>.json`.
    #[arg(long, default_value = "oracle")]
    dataset: DatasetArg,

    /// Which config to run.
    #[arg(long, default_value = "all")]
    config: ConfigArg,

    /// Cap the number of questions (useful for smoke runs).
    #[arg(long)]
    limit: Option<usize>,

    /// Where to write per-config JSON results.
    #[arg(long, default_value = "./results")]
    results_dir: PathBuf,

    /// Providers configuration file (`.toml` or `.json`). Default:
    /// `~/.bricks/providers.toml`. An explicit path replaces the default.
    #[arg(long)]
    providers: Option<PathBuf>,

    /// Embedding service for the retrieval configs (a separate service from the
    /// chat models; it reads its own key variable, see above).
    #[arg(long, default_value = "gemini")]
    embeddings: ProviderArg,

    /// Answerer model, as `provider_id/model_id`.
    #[arg(long)]
    answerer_model: String,

    /// Judge model, as `provider_id/model_id`. Mastra's published numbers use
    /// gpt-4o-mini; pick the equivalent from your configuration when you want
    /// Mastra-comparable scoring.
    #[arg(long)]
    judge_model: String,

    /// Observer / fact-extractor model (only used by the hybrid config), as
    /// `provider_id/model_id`. Defaults to the answerer model.
    #[arg(long)]
    extractor_model: Option<String>,

    /// Top-k for retrieval-based configs. Matches Mastra's RAG config.
    #[arg(long, default_value = "20")]
    top_k: usize,

    /// Parallel in-flight questions per config. Higher = faster, but bounded
    /// by provider rate limits. Gemini 2.5 flash tolerates higher concurrency.
    #[arg(long, default_value = "8")]
    concurrency: usize,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
enum ProviderArg {
    Gemini,
    Openai,
}

#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
enum DatasetArg {
    S,
    M,
    Oracle,
}

impl DatasetArg {
    fn file_name(&self) -> &'static str {
        match self {
            Self::S => "longmemeval_s.json",
            Self::M => "longmemeval_m.json",
            Self::Oracle => "longmemeval_oracle.json",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::S => "longmemeval_s",
            Self::M => "longmemeval_m",
            Self::Oracle => "longmemeval_oracle",
        }
    }
}

#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
enum ConfigArg {
    All,
    Baseline,
    Embed,
    Graph,
    Hybrid,
    #[value(name = "structured-vector")]
    StructuredVector,
    #[value(name = "structured-hybrid")]
    StructuredHybrid,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,longmem_bench=info")),
        )
        .with_target(false)
        .without_time()
        .init();

    let mut cli = Cli::parse();
    let extractor = cli
        .extractor_model
        .clone()
        .unwrap_or_else(|| cli.answerer_model.clone());

    // Load dataset
    let dataset_path = PathBuf::from("./data").join(cli.dataset.file_name());
    if !dataset_path.exists() {
        return Err(anyhow!(
            "Dataset {} not found. Run ./setup.sh or fetch it manually.",
            dataset_path.display()
        ));
    }
    let mut questions = dataset::load_dataset(&dataset_path)
        .with_context(|| format!("loading {}", dataset_path.display()))?;
    eprintln!(
        "Loaded {} questions from {}",
        questions.len(),
        cli.dataset.label()
    );
    if let Some(n) = cli.limit {
        questions.truncate(n);
        eprintln!("Limit applied → running {} questions", questions.len());
    }

    // Chat models: one configured provider per distinct selection, behind a
    // router that dispatches on the request's model string.
    let registry = ProviderRegistry::load(cli.providers.as_deref()).map_err(|e| anyhow!("{e}"))?;
    let mut by_selection: HashMap<String, ConfiguredProvider> = HashMap::new();
    for selection in [&cli.answerer_model, &cli.judge_model, &extractor] {
        if !by_selection.contains_key(selection.as_str()) {
            let provider = registry
                .resolve(selection)
                .map_err(|e| anyhow!("{e}"))?
                .build_provider()
                .map_err(|e| anyhow!("{e}"))?;
            by_selection.insert(selection.clone(), provider);
        }
    }
    cli.extractor_model = Some(extractor);
    let provider: Arc<dyn Provider + Send + Sync> = Arc::new(ModelRouter { by_selection });

    // Embeddings: an independent service with its own key variable.
    let embed_factory: EmbedFactoryArc = match cli.embeddings {
        ProviderArg::Gemini => {
            let api_key = std::env::var("GOOGLE_API_KEY")
                .or_else(|_| std::env::var("GEMINI_API_KEY"))
                .map_err(|_| {
                    anyhow!(
                        "GOOGLE_API_KEY (or GEMINI_API_KEY) is required for --embeddings gemini"
                    )
                })?;
            Arc::new(move || AnyEmbeddings::Gemini(GeminiEmbeddings::new(api_key.clone())))
        }
        ProviderArg::Openai => {
            let api_key = std::env::var("OPENAI_API_KEY")
                .map_err(|_| anyhow!("OPENAI_API_KEY is required for --embeddings openai"))?;
            Arc::new(move || AnyEmbeddings::Openai(OpenAiEmbeddings::new(api_key.clone())))
        }
    };

    std::fs::create_dir_all(&cli.results_dir)?;

    let selected: Vec<ConfigArg> = match &cli.config {
        ConfigArg::All => vec![
            ConfigArg::Baseline,
            ConfigArg::Embed,
            ConfigArg::Graph,
            ConfigArg::Hybrid,
        ],
        other => vec![other.clone()],
    };

    let mut summaries: Vec<report::BenchmarkMetrics> = Vec::new();

    for cfg_choice in selected {
        let out_path = cli.results_dir.join(format!(
            "{}-{}.json",
            config_slug(&cfg_choice),
            cli.dataset.label()
        ));

        let metrics = run_one_config(
            cfg_choice.clone(),
            &questions,
            provider.clone(),
            &embed_factory,
            &cli,
            cli.dataset.label(),
        )
        .await?;

        std::fs::write(&out_path, serde_json::to_vec_pretty(&metrics)?)
            .with_context(|| format!("writing {}", out_path.display()))?;
        eprintln!(
            "✓ {} → {:.3} overall ({} correct / {} total), {:.3} abstention, wrote {}",
            metrics.config,
            metrics.overall_accuracy,
            metrics.correct_answers,
            metrics.total_questions,
            metrics.abstention_accuracy,
            out_path.display()
        );
        summaries.push(metrics);
    }

    // Write combined summary
    let summary_path = cli
        .results_dir
        .join(format!("summary-{}.json", cli.dataset.label()));
    std::fs::write(&summary_path, serde_json::to_vec_pretty(&summaries)?)?;
    eprintln!(
        "→ {} written with {} configs",
        summary_path.display(),
        summaries.len()
    );

    Ok(())
}

fn config_slug(c: &ConfigArg) -> &'static str {
    match c {
        ConfigArg::Baseline => "a-baseline-jsonl",
        ConfigArg::Embed => "b-embed-only",
        ConfigArg::Graph => "c-graph-substring",
        ConfigArg::Hybrid => "d-hybrid-embed-graph",
        ConfigArg::StructuredVector => "e-structured-vector",
        ConfigArg::StructuredHybrid => "f-structured-hybrid",
        ConfigArg::All => "all",
    }
}

/// Routes a request to the configured provider named by its `model` field
/// (a `provider_id/model_id` selection), so the answerer, the judge and the
/// extractor can each be a different configured model.
struct ModelRouter {
    by_selection: HashMap<String, ConfiguredProvider>,
}

impl ModelRouter {
    fn lookup(&self, model: &str) -> cersei_types::Result<&ConfiguredProvider> {
        self.by_selection.get(model).ok_or_else(|| {
            cersei_types::CerseiError::Config(format!(
                "model `{model}` was not configured for this run"
            ))
        })
    }
}

#[async_trait]
impl Provider for ModelRouter {
    fn name(&self) -> &str {
        "router"
    }

    fn context_window(&self, model: &str) -> u64 {
        self.lookup(model)
            .map(|p| p.context_window(model))
            .unwrap_or(0)
    }

    async fn complete(&self, request: CompletionRequest) -> cersei_types::Result<CompletionStream> {
        self.lookup(&request.model)?.complete(request).await
    }
}

type EmbedFactoryArc = Arc<dyn Fn() -> AnyEmbeddings + Send + Sync + 'static>;

async fn run_one_config(
    choice: ConfigArg,
    questions: &[dataset::Question],
    provider: Arc<dyn Provider + Send + Sync>,
    embed_factory: &EmbedFactoryArc,
    cli: &Cli,
    dataset_label: &str,
) -> Result<report::BenchmarkMetrics> {
    eprintln!(
        "─── running config: {:?}  (concurrency={})  ───",
        choice, cli.concurrency
    );

    use tokio::sync::Semaphore;
    let sem = Arc::new(Semaphore::new(cli.concurrency));

    // Kick off all questions; use JoinSet so completions arrive in order of
    // finish, not submission — good for progress reporting.
    let mut set = tokio::task::JoinSet::new();
    let total = questions.len();
    for (i, q) in questions.iter().cloned().enumerate() {
        let permit = sem.clone().acquire_owned().await.unwrap();
        let provider = provider.clone();
        let choice = choice.clone();
        let factory = embed_factory.clone();
        let answerer_model = cli.answerer_model.clone();
        let judge_model = cli.judge_model.clone();
        let extractor_model = cli.extractor_model.clone().expect("set in main");
        let top_k = cli.top_k;
        set.spawn(async move {
            let _permit = permit; // released on drop
            let factory_for_task = factory.clone();
            // Wrap the Arc'd factory in a fresh `Fn` closure so the concrete
            // EmbedConfig / HybridConfig can consume it via their `impl Fn`
            // constructors.
            let closure = move || factory_for_task();
            let res = run_one(
                choice,
                &q,
                provider,
                closure,
                Models {
                    answerer: &answerer_model,
                    judge: &judge_model,
                    extractor: &extractor_model,
                },
                top_k,
            )
            .await;
            (i, q, res)
        });
    }

    let mut rows: Vec<report::PerQuestion> = Vec::with_capacity(total);
    let mut completed = 0usize;
    while let Some(joined) = set.join_next().await {
        let (_i, q, res) = joined.context("task join failed")?;
        completed += 1;
        if completed.is_multiple_of(10) || completed == total {
            eprintln!("  [{}/{}] done (last={})", completed, total, q.question_id);
        }
        match res {
            Ok(r) => rows.push(r),
            Err(e) => {
                eprintln!("  ✗ {}: {e:#}", q.question_id);
                rows.push(report::PerQuestion {
                    question_id: q.question_id.clone(),
                    question_type: q.question_type,
                    is_abstention: q.is_abstention(),
                    question: q.question.clone(),
                    expected_answer: q.answer.clone(),
                    hypothesis: format!("<error: {e:#}>"),
                    is_correct: false,
                    input_tokens: 0,
                    output_tokens: 0,
                    judge_tokens: 0,
                    elapsed_ms: 0,
                });
            }
        }
    }

    // Also dump per-question rows alongside the summary so we can inspect
    // failures after the fact.
    let rows_path = cli.results_dir.join(format!(
        "{}-rows-{}.json",
        config_slug(&choice),
        dataset_label
    ));
    std::fs::write(&rows_path, serde_json::to_vec_pretty(&rows)?)
        .with_context(|| format!("writing {}", rows_path.display()))?;

    Ok(report::summarize(
        config_slug(&choice),
        dataset_label,
        &cli.judge_model,
        &rows,
    ))
}

/// The model selections a question is run with.
struct Models<'a> {
    answerer: &'a str,
    judge: &'a str,
    extractor: &'a str,
}

async fn run_one<F>(
    choice: ConfigArg,
    q: &dataset::Question,
    provider: Arc<dyn Provider + Send + Sync>,
    embed_factory: F,
    models: Models<'_>,
    top_k: usize,
) -> Result<report::PerQuestion>
where
    F: Fn() -> AnyEmbeddings + Send + Sync + 'static + Clone,
{
    let Models {
        answerer: answerer_model,
        judge: judge_model,
        extractor: extractor_model,
    } = models;
    match choice {
        ConfigArg::Baseline => {
            let mut c = configs::baseline::BaselineConfig::new();
            runner::run_question(&mut c, provider, answerer_model, judge_model, q).await
        }
        ConfigArg::Embed => {
            let mut c = configs::embed::EmbedConfig::new(embed_factory).with_top_k(top_k);
            runner::run_question(&mut c, provider, answerer_model, judge_model, q).await
        }
        ConfigArg::Graph => {
            let mut c = configs::graph::GraphConfig::new().with_top_k(top_k);
            runner::run_question(&mut c, provider, answerer_model, judge_model, q).await
        }
        ConfigArg::Hybrid => {
            let mut c =
                configs::hybrid::HybridConfig::<AnyEmbeddings, dyn Provider + Send + Sync>::new(
                    embed_factory,
                    provider.clone(),
                    extractor_model.to_string(),
                )
                .with_top_k(top_k);
            runner::run_question(&mut c, provider, answerer_model, judge_model, q).await
        }
        ConfigArg::StructuredVector => {
            let f = embed_factory.clone();
            let mut c = configs::structured::StructuredConfig::vector(move || {
                Arc::new(f()) as Arc<dyn EmbeddingProvider>
            })
            .with_top_k(top_k);
            runner::run_question(&mut c, provider, answerer_model, judge_model, q).await
        }
        ConfigArg::StructuredHybrid => {
            let f = embed_factory.clone();
            let mut c = configs::structured::StructuredConfig::hybrid(
                move || Arc::new(f()) as Arc<dyn EmbeddingProvider>,
                provider.clone(),
                extractor_model.to_string(),
            )
            .with_top_k(top_k);
            runner::run_question(&mut c, provider, answerer_model, judge_model, q).await
        }
        ConfigArg::All => unreachable!("expanded above"),
    }
}
