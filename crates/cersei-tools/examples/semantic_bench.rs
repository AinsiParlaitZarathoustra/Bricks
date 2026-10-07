//! Deterministic benchmark: the existing path (the real `Grep` tool, then
//! `Read` of the first matching files) against one `CodeScout` query, on a
//! corpus of code questions with known answers (`bench/semantic/corpus.json`).
//!
//! No model and no network: what is measured is what the tools return.
//!
//! Columns: "cold" is a fresh engine without language servers; "warm" is
//! the long-lived engine (servers too with `--lsp`, started and indexed
//! once beforehand, measured apart). Cached answers are dropped before each
//! warm query.
//!
//! ```sh
//! cargo run --release -p cersei-tools --example semantic_bench -- \
//!     --root . --runs 7 --out target/semantic-bench.json [--lsp]
//! ```
//!
//! Token counts are estimates (`cersei_types::tokens`, the Context
//! Manager's heuristic), the same for both sides. "Precision" and "recall"
//! compare returned locations with the corpus's annotated ones; they are
//! not "useful tokens", which would need a reading protocol.

use bricks_semantic::{
    render, CodeQuery, Intent, Requester, SemanticConfig, SemanticEngine, Target,
};
use cersei_tools::{
    file_read::FileReadTool, grep_tool::GrepTool, permissions::AllowAll, CostTracker, Extensions,
    Tool, ToolContext,
};
use cersei_types::tokens::estimate_text;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

#[derive(Deserialize, Clone)]
struct Case {
    id: String,
    category: String,
    query: CodeQuery,
    baseline: Baseline,
    expected: Vec<Expected>,
}

#[derive(Deserialize, Clone)]
struct Baseline {
    pattern: String,
    reads: usize,
}

#[derive(Deserialize, Clone)]
struct Expected {
    path: String,
    contains: String,
}

#[derive(Default, Clone, serde::Serialize)]
struct Side {
    calls: usize,
    tokens: u64,
    locations: usize,
    relevant: usize,
    found_expected: usize,
    expected: usize,
    cold_ms: Vec<f64>,
    warm_ms: Vec<f64>,
}

impl Side {
    fn precision(&self) -> f64 {
        if self.locations == 0 {
            0.0
        } else {
            self.relevant as f64 / self.locations as f64
        }
    }
    fn recall(&self) -> f64 {
        if self.expected == 0 {
            1.0
        } else {
            self.found_expected as f64 / self.expected as f64
        }
    }
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((s.len() - 1) as f64 * p).round() as usize;
    s[i]
}

fn rss_kb(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn matches(exp: &[Expected], path: &str, line: &str) -> Option<usize> {
    exp.iter()
        .position(|e| path.ends_with(&e.path) && line.contains(&e.contains))
}

fn ctx(root: &Path) -> ToolContext {
    ToolContext {
        working_dir: root.to_path_buf(),
        session_id: "bench".into(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

/// The existing path: Grep, then Read the first `reads` distinct files.
async fn baseline(root: &Path, case: &Case) -> (Side, f64) {
    let c = ctx(root);
    let t = Instant::now();
    let g = GrepTool
        .execute(json!({"pattern": case.baseline.pattern, "path": root}), &c)
        .await;
    let mut side = Side {
        calls: 1,
        tokens: estimate_text(&g.content).tokens,
        expected: case.expected.len(),
        ..Default::default()
    };
    let mut found = vec![false; case.expected.len()];
    let mut files: Vec<String> = Vec::new();
    for l in g.content.lines() {
        // `path:line:text`
        let mut parts = l.splitn(3, ':');
        let (Some(p), Some(_), Some(text)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let rel = p.strip_prefix(&format!("{}/", root.display())).unwrap_or(p);
        side.locations += 1;
        if let Some(i) = matches(&case.expected, rel, text) {
            side.relevant += 1;
            found[i] = true;
        }
        if !files.iter().any(|f| f == p) {
            files.push(p.to_string());
        }
    }
    for f in files.iter().take(case.baseline.reads) {
        let r = FileReadTool.execute(json!({"file_path": f}), &c).await;
        side.calls += 1;
        side.tokens += estimate_text(&r.content).tokens;
    }
    side.found_expected = found.iter().filter(|f| **f).count();
    (side, t.elapsed().as_secs_f64() * 1000.0)
}

async fn scout(engine: &Arc<SemanticEngine>, root: &Path, case: &Case) -> (Side, f64, String) {
    let t = Instant::now();
    let r = engine
        .query(case.query.clone(), &Requester::new("bench", root))
        .await;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let text = render::render_response(&r);
    let mut side = Side {
        calls: 1,
        tokens: estimate_text(&text).tokens,
        locations: r.items.len(),
        expected: case.expected.len(),
        ..Default::default()
    };
    let mut found = vec![false; case.expected.len()];
    for it in &r.items {
        if let Some(i) = matches(&case.expected, &it.path, &it.line_text) {
            side.relevant += 1;
            found[i] = true;
        }
    }
    side.found_expected = found.iter().filter(|f| **f).count();
    (side, ms, r.status.label().to_string())
}

/// A file edited between two queries: the second must see the new place.
async fn modified_file_case(lsp: bool) -> serde_json::Value {
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    std::fs::write(root.join("lib.rs"), "pub fn moving() -> u32 { 1 }\n").unwrap();
    let mut cfg = SemanticConfig::default();
    cfg.lsp.enabled = lsp;
    let e = SemanticEngine::new(&root, cfg);
    let q = || CodeQuery::text("moving").intent(Intent::FindSymbol);
    let r1 = e.query(q(), &Requester::new("bench", &root)).await;
    std::fs::write(
        root.join("lib.rs"),
        "// a\n// b\n// c\npub fn moving() -> u32 { 2 }\n",
    )
    .unwrap();
    e.notify_changed(&[root.join("lib.rs")]);
    let t = Instant::now();
    let r2 = e.query(q(), &Requester::new("bench", &root)).await;
    json!({
        "before_line": r1.items.first().map(|i| i.range.start.line + 1),
        "after_line": r2.items.first().map(|i| i.range.start.line + 1),
        "correct": r2.items.first().map(|i| i.range.start.line + 1) == Some(4),
        "requery_ms": t.elapsed().as_secs_f64() * 1000.0,
        "tree_incremental_reparses": e.stats().tree_incremental,
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let root = std::fs::canonicalize(arg("--root").unwrap_or_else(|| ".".into())).unwrap();
    let runs: usize = arg("--runs").and_then(|r| r.parse().ok()).unwrap_or(7);
    let lsp = args.iter().any(|a| a == "--lsp");
    let corpus_path = arg("--corpus")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("bench/semantic/corpus.json"));
    let corpus: Corpus =
        serde_json::from_str(&std::fs::read_to_string(&corpus_path).unwrap()).unwrap();
    let mut cfg = SemanticConfig::default();
    cfg.lsp.enabled = lsp;
    // Targets in the corpus are names: no target, nothing to do here.
    let _ = Target::File {
        path: String::new(),
    };

    let pid = std::process::id();
    let rss_start = rss_kb(pid);
    let mut rows = Vec::new();
    let warm_engine = SemanticEngine::new(&root, cfg.clone());
    // With servers: start and index once, measured apart, then only warm
    // queries (one server per cold engine would re-index every time).
    let mut indexing = serde_json::Value::Null;
    if lsp {
        let t = Instant::now();
        let probe = corpus
            .cases
            .iter()
            .find(|c| c.category == "references")
            .cloned();
        if let Some(c) = probe {
            let _ = scout(&warm_engine, &root, &c).await;
        }
        let started = t.elapsed().as_secs_f64() * 1000.0;
        let limit = Instant::now() + std::time::Duration::from_secs(600);
        while warm_engine.stats().lsp_instances.iter().any(|i| i.indexing) && Instant::now() < limit
        {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let ready = !warm_engine.stats().lsp_instances.iter().any(|i| i.indexing);
        warm_engine.notify_any_change();
        indexing = json!({
            "first_query_ms": started,
            "until_indexed_ms": t.elapsed().as_secs_f64() * 1000.0,
            "indexed": ready,
            "server_rss_kb": warm_engine.stats().lsp_instances.first().and_then(|i| i.pid).and_then(rss_kb),
        });
        println!("language server start + indexing: {indexing}");
    }
    for case in &corpus.cases {
        let mut base = Side::default();
        let mut new = Side::default();
        let mut status = String::new();
        for run in 0..runs {
            let (b, bms) = baseline(&root, case).await;
            // Cold: a fresh engine (empty caches); without servers in
            // `--lsp` mode (see above).
            let mut cold_cfg = cfg.clone();
            cold_cfg.lsp.enabled = false;
            let cold_engine = SemanticEngine::new(&root, cold_cfg);
            let (n_cold, cold_ms, st_cold) = scout(&cold_engine, &root, case).await;
            // Warm: the long-lived engine. Its cached answers are dropped
            // first, so warm means warm caches of trees and servers only.
            warm_engine.notify_any_change();
            let (n_warm, warm_ms, st_warm) = scout(&warm_engine, &root, case).await;
            if run == 0 {
                base = b.clone();
                let (n, st) = if lsp {
                    (n_warm, st_warm)
                } else {
                    (n_cold, st_cold)
                };
                new = n;
                status = st;
            }
            base.cold_ms.push(bms);
            new.cold_ms.push(cold_ms);
            new.warm_ms.push(warm_ms);
        }
        rows.push((case.clone(), base, new, status));
    }
    let stats = warm_engine.stats();
    let rss_end = rss_kb(pid);
    let modified = modified_file_case(lsp).await;

    println!(
        "| case | category | status | baseline calls | baseline tokens | scout tokens | Δ tokens | baseline P/R | scout P/R | baseline p50/p95 ms | scout cold p50/p95 ms | scout warm p50/p95 ms |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mut json_rows = Vec::new();
    let (mut tb, mut tn) = (0u64, 0u64);
    for (case, b, n, status) in &rows {
        tb += b.tokens;
        tn += n.tokens;
        let delta = if b.tokens > 0 {
            (n.tokens as f64 - b.tokens as f64) / b.tokens as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "| {} | {} | {} | {} | {} | {} | {:+.0}% | {:.2}/{:.2} | {:.2}/{:.2} | {:.1}/{:.1} | {:.1}/{:.1} | {:.1}/{:.1} |",
            case.id,
            case.category,
            status,
            b.calls,
            b.tokens,
            n.tokens,
            delta,
            b.precision(),
            b.recall(),
            n.precision(),
            n.recall(),
            pct(&b.cold_ms, 0.5),
            pct(&b.cold_ms, 0.95),
            pct(&n.cold_ms, 0.5),
            pct(&n.cold_ms, 0.95),
            pct(&n.warm_ms, 0.5),
            pct(&n.warm_ms, 0.95),
        );
        json_rows.push(json!({
            "id": case.id, "category": case.category, "status": status,
            "baseline": {"calls": b.calls, "tokens_raw": b.tokens, "locations": b.locations,
                         "precision": b.precision(), "recall": b.recall(),
                         "p50_ms": pct(&b.cold_ms, 0.5), "p95_ms": pct(&b.cold_ms, 0.95)},
            "scout": {"calls": n.calls, "tokens": n.tokens, "locations": n.locations,
                      "precision": n.precision(), "recall": n.recall(),
                      "cold_p50_ms": pct(&n.cold_ms, 0.5), "cold_p95_ms": pct(&n.cold_ms, 0.95),
                      "warm_p50_ms": pct(&n.warm_ms, 0.5), "warm_p95_ms": pct(&n.warm_ms, 0.95)},
        }));
    }
    println!(
        "\ntotal estimated tokens: baseline {tb}, scout {tn} ({:+.0}%)",
        (tn as f64 - tb as f64) / tb.max(1) as f64 * 100.0
    );
    let tree_rate = stats.tree_hits as f64 / (stats.tree_hits + stats.tree_misses).max(1) as f64;
    println!(
        "warm engine: tree cache hit rate {:.0}% ({} hits, {} misses), {} trees, {} KiB of source held; result cache hits {}",
        tree_rate * 100.0,
        stats.tree_hits,
        stats.tree_misses,
        stats.tree_cache_entries,
        stats.tree_cache_source_bytes / 1024,
        stats.result_cache_hits
    );
    println!(
        "process RSS: {:?} KiB at start, {:?} KiB at end (whole bench process)",
        rss_start, rss_end
    );
    let lsp_rss: Vec<_> = stats
        .lsp_instances
        .iter()
        .map(|i| json!({"server": i.server, "pid": i.pid, "rss_kb": i.pid.and_then(rss_kb)}))
        .collect();
    println!(
        "language servers: {}",
        if lsp_rss.is_empty() {
            "none started".to_string()
        } else {
            serde_json::to_string(&lsp_rss).unwrap()
        }
    );
    println!("modified file: {modified}");
    if let Some(out) = arg("--out") {
        let doc = json!({
            "root": root, "runs": runs, "lsp": lsp,
            "token_method": cersei_types::tokens::ESTIMATION_METHOD,
            "rows": json_rows,
            "totals": {"baseline_tokens": tb, "scout_tokens": tn},
            "engine": stats,
            "process_rss_kb": {"start": rss_start, "end": rss_end},
            "lsp_processes": lsp_rss,
            "modified_file": modified,
            "lsp_indexing": indexing,
        });
        std::fs::write(&out, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        println!("written: {out}");
    }
    warm_engine.shutdown().await;
}
