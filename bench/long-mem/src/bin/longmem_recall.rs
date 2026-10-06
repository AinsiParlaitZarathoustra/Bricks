//! longmem-recall — evidence recall on LongMemEval, without answering.
//!
//! For each question, the haystack is ingested into a fresh memory (one
//! episode per turn: role, content, session id, session date — never the
//! answer, the evidence session ids or the `has_answer` turn annotations),
//! the question is recalled, and the ranked results are mapped to their
//! sessions. Reported, per question type:
//!
//! * `recall_any@K` — at least one evidence session among the first K
//!   distinct sessions retrieved;
//! * `recall_all@K` — every evidence session among them;
//!
//! following LongMemEval's session-level retrieval metrics. Abstention
//! questions (`_abs`) have no evidence and are left out of recall.
//!
//! Modes, compared on the same questions and embeddings:
//! * `legacy` — the former `GraphMemory::recall_top_k` (substring), the
//!   starting point;
//! * `vector` — structured memory, vector lane only;
//! * `vector-lexical` — vector and lexical lanes (no extraction);
//! * `hybrid` — vector, lexical and relational lanes over facts extracted
//!   by `--extractor-model` (paid calls; needs `--providers`).
//!
//! Embeddings: `hashing` (local, deterministic, lexical — free, but not a
//! semantic model), `openai` or `gemini` (paid, key from the environment).
//!
//! ```text
//! cargo run --release -p longmem-bench --bin longmem-recall -- \
//!     --dataset oracle --modes legacy,vector,vector-lexical --embeddings hashing --sample 60
//! ```

#[path = "../dataset.rs"]
mod dataset;

use anyhow::{anyhow, Context, Result};
use cersei_embeddings::{EmbeddingProvider, GeminiEmbeddings, HashingEmbeddings, OpenAiEmbeddings};
use cersei_memory::structured::clock::parse_time;
use cersei_memory::structured::{
    EpisodeInput, LlmExtractor, MemoryConfig, RecallQuery, StructuredMemory,
};
use clap::Parser;
use dataset::{load_dataset, Question};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(
    name = "longmem-recall",
    about = "Evidence recall on LongMemEval (no answering)"
)]
struct Cli {
    /// `s`, `m` or `oracle` (file `./data/longmemeval_<name>.json`).
    #[arg(long, default_value = "oracle")]
    dataset: String,
    /// Explicit dataset file (overrides --dataset).
    #[arg(long)]
    data_file: Option<PathBuf>,
    /// Comma-separated: legacy, vector, vector-lexical, hybrid.
    #[arg(long, default_value = "legacy,vector,vector-lexical")]
    modes: String,
    /// hashing | openai | gemini
    #[arg(long, default_value = "hashing")]
    embeddings: String,
    /// Dimensions of the hashing embedder.
    #[arg(long, default_value = "512")]
    hashing_dims: usize,
    /// Fixed validation sample: the first N questions of each type, by
    /// question id (0 = all questions).
    #[arg(long, default_value = "0")]
    sample: usize,
    /// Comma-separated K values.
    #[arg(long, default_value = "5,10")]
    k: String,
    /// Extraction model (`provider/model`) for `hybrid`.
    #[arg(long)]
    extractor_model: Option<String>,
    #[arg(long)]
    providers: Option<PathBuf>,
    /// Vector seeds for the relational expansion.
    #[arg(long, default_value = "10")]
    seeds: usize,
    #[arg(long, default_value = "./results-recall")]
    out: PathBuf,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct Row {
    question_id: String,
    question_type: String,
    evidence_sessions: usize,
    retrieved_sessions: Vec<String>,
    any: BTreeMap<usize, bool>,
    all: BTreeMap<usize, bool>,
    ingest_ms: f64,
    recall_us: u64,
    embed_us: u64,
    vector_us: u64,
    lexical_us: u64,
    expansion_us: u64,
    fusion_us: u64,
    render_us: u64,
    episodes: usize,
    facts: usize,
}

fn sample(mut qs: Vec<Question>, n: usize) -> Vec<Question> {
    if n == 0 {
        return qs;
    }
    qs.sort_by(|a, b| a.question_id.cmp(&b.question_id));
    let mut per_type: HashMap<&'static str, usize> = HashMap::new();
    qs.into_iter()
        .filter(|q| {
            let c = per_type.entry(q.question_type.as_str()).or_insert(0);
            *c += 1;
            *c <= n
        })
        .collect()
}

fn embedder(cli: &Cli) -> Result<Arc<dyn EmbeddingProvider>> {
    Ok(match cli.embeddings.as_str() {
        "hashing" => Arc::new(HashingEmbeddings::new(cli.hashing_dims)),
        "openai" => Arc::new(OpenAiEmbeddings::from_env().map_err(|e| anyhow!("{e}"))?),
        "gemini" => Arc::new(GeminiEmbeddings::from_env().map_err(|e| anyhow!("{e}"))?),
        other => return Err(anyhow!("unknown embeddings `{other}`")),
    })
}

/// Sessions of ranked results, distinct, in order.
fn distinct(sessions: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    sessions
        .into_iter()
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

fn score(row: &mut Row, retrieved: Vec<String>, evidence: &[String], ks: &[usize]) {
    let ev: HashSet<&String> = evidence.iter().collect();
    for &k in ks {
        let top: HashSet<&String> = retrieved.iter().take(k).collect();
        row.any.insert(k, ev.iter().any(|e| top.contains(e)));
        row.all
            .insert(k, !ev.is_empty() && ev.iter().all(|e| top.contains(e)));
    }
    row.retrieved_sessions = retrieved
        .into_iter()
        .take(*ks.iter().max().unwrap_or(&10))
        .collect();
}

async fn run_structured(
    q: &Question,
    mode: &str,
    cli: &Cli,
    ks: &[usize],
    extractor: Option<Arc<dyn cersei_memory::structured::Extractor>>,
) -> Result<Row> {
    let mut cfg = MemoryConfig::default().with_space("user:longmemeval");
    let kmax = *ks.iter().max().unwrap_or(&10);
    cfg.recall.max_results = 100;
    cfg.recall.max_tokens = 50_000;
    cfg.recall.candidates = (kmax * 5).max(30);
    cfg.recall.seeds = cli.seeds;
    match mode {
        "vector" => {
            cfg.recall.weight_lexical = 0.0;
            cfg.recall.weight_relational = 0.0;
            cfg.recall.hops = 0;
        }
        "vector-lexical" => {
            cfg.recall.weight_relational = 0.0;
            cfg.recall.hops = 0;
        }
        _ => {}
    }
    let mut b = StructuredMemory::builder(embedder(cli)?).config(cfg);
    if let Some(x) = extractor {
        b = b.extractor(x);
    }
    let m = b.open().map_err(|e| anyhow!("{e}"))?;
    let t0 = Instant::now();
    let mut inputs = Vec::new();
    for (i, session) in q.haystack_sessions.iter().enumerate() {
        let sid = q
            .haystack_session_ids
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("s{i}"));
        let at = q.haystack_dates.get(i).and_then(|d| parse_time(d));
        for (n, turn) in session.iter().enumerate() {
            // Role and content only: no `has_answer`, no answer, no evidence ids.
            inputs.push(EpisodeInput {
                space: "user:longmemeval".into(),
                session_id: Some(sid.clone()),
                role: turn.role.clone(),
                author: None,
                content: turn.content.clone(),
                occurred_at: at.map(|t| t + n as i64),
                source_ref: Some(format!("{sid}#{n}")),
            });
        }
    }
    m.ingest(inputs, &CancellationToken::new())
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let ingest_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mut query = RecallQuery::new(q.question.clone());
    query.valid_at = parse_time(&q.question_date);
    let r = m.recall(&query).await.map_err(|e| anyhow!("{e}"))?;
    let sessions = r.items.iter().flat_map(|i| {
        let mut v = Vec::new();
        if let Some(e) = &i.episode {
            v.extend(e.session_id.clone());
        }
        for ev in &i.evidence {
            v.extend(ev.session_id.clone());
        }
        v
    });
    let stats = m.stats();
    let mut row = Row {
        question_id: q.question_id.clone(),
        question_type: q.question_type.as_str().into(),
        evidence_sessions: q.answer_session_ids.len(),
        ingest_ms,
        recall_us: r.timings.total_us,
        embed_us: r.timings.embed_us,
        vector_us: r.timings.vector_us,
        lexical_us: r.timings.lexical_us,
        expansion_us: r.timings.expansion_us,
        fusion_us: r.timings.fusion_us,
        render_us: r.timings.render_us,
        episodes: stats.episodes,
        facts: stats.facts,
        ..Default::default()
    };
    score(&mut row, distinct(sessions), &q.answer_session_ids, ks);
    Ok(row)
}

fn run_legacy(q: &Question, ks: &[usize]) -> Result<Row> {
    let g = cersei_memory::graph::GraphMemory::open_in_memory().map_err(|e| anyhow!("{e}"))?;
    let t0 = Instant::now();
    let mut by_content: HashMap<String, String> = HashMap::new();
    for (i, session) in q.haystack_sessions.iter().enumerate() {
        let sid = q.haystack_session_ids.get(i).cloned().unwrap_or_default();
        let date = q.haystack_dates.get(i).map(String::as_str).unwrap_or("");
        for turn in session {
            // Same formatting as the former `graph` configuration.
            let content = format!("[{date}] {}: {}", turn.role, turn.content);
            let _ = g.store_memory(&content, cersei_memory::memdir::MemoryType::Project, 0.9);
            by_content.entry(content).or_insert(sid.clone());
        }
    }
    let ingest_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let t1 = Instant::now();
    let kmax = *ks.iter().max().unwrap_or(&10);
    let hits = g.recall_top_k(&q.question, kmax * 4);
    let recall_us = t1.elapsed().as_micros() as u64;
    let sessions = hits
        .into_iter()
        .filter_map(|(c, _)| by_content.get(&c).cloned());
    let mut row = Row {
        question_id: q.question_id.clone(),
        question_type: q.question_type.as_str().into(),
        evidence_sessions: q.answer_session_ids.len(),
        ingest_ms,
        recall_us,
        ..Default::default()
    };
    score(&mut row, distinct(sessions), &q.answer_session_ids, ks);
    Ok(row)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i]
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = cli.data_file.clone().unwrap_or_else(|| {
        PathBuf::from("./data").join(format!("longmemeval_{}.json", cli.dataset))
    });
    let questions = load_dataset(&path).with_context(|| format!("loading {}", path.display()))?;
    let questions: Vec<Question> = sample(questions, cli.sample)
        .into_iter()
        .filter(|q| !q.is_abstention())
        .collect();
    let ks: Vec<usize> = cli
        .k
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let modes: Vec<String> = cli.modes.split(',').map(|s| s.trim().to_string()).collect();
    let extractor: Option<Arc<dyn cersei_memory::structured::Extractor>> =
        match &cli.extractor_model {
            Some(sel) => {
                let registry = cersei_provider::ProviderRegistry::load(cli.providers.as_deref())
                    .map_err(|e| anyhow!("{e}"))?;
                let provider = registry
                    .resolve(sel)
                    .map_err(|e| anyhow!("{e}"))?
                    .build_provider()
                    .map_err(|e| anyhow!("{e}"))?;
                Some(Arc::new(LlmExtractor::new(
                    Arc::new(provider),
                    sel.clone(),
                    Default::default(),
                )))
            }
            None => None,
        };
    if modes.iter().any(|m| m == "hybrid") && extractor.is_none() {
        return Err(anyhow!(
            "mode `hybrid` needs --extractor-model (and --providers)"
        ));
    }
    std::fs::create_dir_all(&cli.out)?;
    eprintln!(
        "{} questions (abstention excluded), embeddings {}, K = {:?}",
        questions.len(),
        cli.embeddings,
        ks
    );
    let mut summary = serde_json::Map::new();
    println!(
        "| mode | type | n | {} | ingest p50 ms | recall p50 µs | recall p95 µs |",
        ks.iter()
            .map(|k| format!("any@{k} | all@{k}"))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!("|{}", "---|".repeat(5 + ks.len() * 2));
    for mode in &modes {
        let mut rows = Vec::new();
        for (i, q) in questions.iter().enumerate() {
            let row = match mode.as_str() {
                "legacy" => run_legacy(q, &ks)?,
                "vector" | "vector-lexical" => run_structured(q, mode, &cli, &ks, None).await?,
                "hybrid" => run_structured(q, mode, &cli, &ks, extractor.clone()).await?,
                other => return Err(anyhow!("unknown mode `{other}`")),
            };
            if (i + 1) % 25 == 0 {
                eprintln!("  {mode}: {}/{}", i + 1, questions.len());
            }
            rows.push(row);
        }
        let mut groups: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
        for r in &rows {
            groups.entry(r.question_type.clone()).or_default().push(r);
            groups.entry("ALL".into()).or_default().push(r);
        }
        let mut mode_summary = serde_json::Map::new();
        for (ty, rs) in &groups {
            let n = rs.len() as f64;
            let mut cells = Vec::new();
            let mut ty_summary = serde_json::Map::new();
            for k in &ks {
                let any = rs.iter().filter(|r| r.any[k]).count() as f64 / n;
                let all = rs.iter().filter(|r| r.all[k]).count() as f64 / n;
                cells.push(format!("{:.3} | {:.3}", any, all));
                ty_summary.insert(format!("recall_any@{k}"), any.into());
                ty_summary.insert(format!("recall_all@{k}"), all.into());
            }
            let mut ingest: Vec<f64> = rs.iter().map(|r| r.ingest_ms).collect();
            let mut recall: Vec<f64> = rs.iter().map(|r| r.recall_us as f64).collect();
            let (i50, r50, r95) = (
                pct(&mut ingest, 0.5),
                pct(&mut recall, 0.5),
                pct(&mut recall, 0.95),
            );
            ty_summary.insert("n".into(), (rs.len() as u64).into());
            ty_summary.insert("ingest_ms_p50".into(), i50.into());
            ty_summary.insert("recall_us_p50".into(), r50.into());
            ty_summary.insert("recall_us_p95".into(), r95.into());
            if ty == "ALL" {
                for (name, f) in [
                    ("embed_us", (|r: &Row| r.embed_us) as fn(&Row) -> u64),
                    ("vector_us", |r: &Row| r.vector_us),
                    ("lexical_us", |r: &Row| r.lexical_us),
                    ("expansion_us", |r: &Row| r.expansion_us),
                    ("fusion_us", |r: &Row| r.fusion_us),
                    ("render_us", |r: &Row| r.render_us),
                ] {
                    let mut v: Vec<f64> = rs.iter().map(|r| f(r) as f64).collect();
                    ty_summary.insert(format!("{name}_p50"), pct(&mut v, 0.5).into());
                    ty_summary.insert(format!("{name}_p95"), pct(&mut v, 0.95).into());
                }
            }
            println!(
                "| {mode} | {ty} | {} | {} | {:.1} | {:.0} | {:.0} |",
                rs.len(),
                cells.join(" | "),
                i50,
                r50,
                r95
            );
            mode_summary.insert(ty.clone(), ty_summary.into());
        }
        summary.insert(mode.clone(), mode_summary.into());
        let file = cli.out.join(format!(
            "recall-{mode}-{}-{}.json",
            cli.dataset, cli.embeddings
        ));
        std::fs::write(&file, serde_json::to_vec_pretty(&rows)?)?;
    }
    let meta = serde_json::json!({
        "dataset": cli.dataset,
        "questions": questions.len(),
        "sample_per_type": cli.sample,
        "embeddings": cli.embeddings,
        "hashing_dims": cli.hashing_dims,
        "seeds": cli.seeds,
        "extractor_model": cli.extractor_model,
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "summary": summary,
    });
    let file = cli.out.join(format!(
        "recall-summary-{}-{}.json",
        cli.dataset, cli.embeddings
    ));
    std::fs::write(&file, serde_json::to_vec_pretty(&meta)?)?;
    eprintln!("→ {}", file.display());
    Ok(())
}
