//! Latency of the structured memory on synthetic corpora.
//!
//! ```text
//! cargo run --release -p cersei-memory --features structured --example measure_memory
//! cargo run --release -p cersei-memory --features structured --example measure_memory -- 500 2000
//! ```
//!
//! Corpora of N episodes ("<person> works at <company>", "<company> is based
//! in <city>", filler chat). A deterministic rule-based extractor turns the
//! templated sentences into facts (no model call); embeddings come from the
//! local hashing embedder (no network). Reported: ingestion time per
//! episode, reopen time (projection rebuild), and recall p50/p95 per stage
//! over 200 queries. These numbers measure the memory's own work on this
//! machine; with a hosted embedder, the embedding call dominates.

use async_trait::async_trait;
use cersei_embeddings::HashingEmbeddings;
use cersei_memory::structured::extract::{ExtractError, ExtractionRequest, Extractor};
use cersei_memory::structured::*;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

struct Rules;

#[async_trait]
impl Extractor for Rules {
    fn id(&self) -> String {
        "rules/measure".into()
    }
    async fn extract(
        &self,
        r: &ExtractionRequest,
        _: &CancellationToken,
    ) -> Result<String, ExtractError> {
        let c = r.content.trim_end_matches('.');
        let out = if let Some((a, b)) = c.split_once(" works at ") {
            serde_json::json!({"entities": [{"ref":"e1","name":a,"type":"person"},{"ref":"e2","name":b,"type":"organization"}],
                "facts": [{"subject":"e1","predicate":"works_at","value":b,"object":"e2","quote":c}]})
        } else if let Some((a, b)) = c.split_once(" is based in ") {
            serde_json::json!({"entities": [{"ref":"e1","name":a,"type":"organization"},{"ref":"e2","name":b,"type":"place"}],
                "facts": [{"subject":"e1","predicate":"located_in","value":b,"object":"e2","quote":c}]})
        } else {
            serde_json::json!({"entities": [], "facts": []})
        };
        Ok(out.to_string())
    }
}

fn corpus(n: usize) -> Vec<EpisodeInput> {
    let filler = [
        "We talked about the weather and the weekend plans",
        "Can you suggest a recipe with lentils and spinach",
        "I watched a documentary about deep sea creatures",
        "The build failed again because of a missing feature flag",
        "Let us review the pull request about caching tomorrow",
    ];
    (0..n)
        .map(|i| {
            let content = match i % 4 {
                0 => format!("Person{} works at Company{}.", i, i % 97),
                1 => format!("Company{} is based in City{}.", i % 97, i % 31),
                _ => format!("{} number {i}.", filler[i % filler.len()]),
            };
            EpisodeInput {
                space: "project:measure".into(),
                session_id: Some(format!("s{}", i / 20)),
                role: "user".into(),
                author: None,
                content,
                occurred_at: Some(1_700_000_000_000 + i as i64 * 60_000),
                source_ref: None,
            }
        })
        .collect()
}

fn pct(v: &mut [u64], p: f64) -> u64 {
    v.sort_unstable();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[tokio::main]
async fn main() {
    println!(
        "build: {}, {} {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("| episodes | facts | ingest ms/episode | reopen ms | stage | p50 µs | p95 µs |");
    println!("|---|---|---|---|---|---|---|");
    let mut sizes: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    if sizes.is_empty() {
        sizes = vec![500, 2_000, 10_000];
    }
    for n in sizes {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.grafeo");
        let embedder = Arc::new(HashingEmbeddings::new(384));
        let cfg = MemoryConfig::default().with_space("project:measure");
        let m = StructuredMemory::builder(embedder.clone())
            .path(&path)
            .config(cfg.clone())
            .extractor(Arc::new(Rules))
            .open()
            .unwrap();
        let t = Instant::now();
        m.ingest(corpus(n), &CancellationToken::new())
            .await
            .unwrap();
        let ingest = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let facts = m.stats().facts;
        m.close().unwrap();
        drop(m);
        let t = Instant::now();
        let m = StructuredMemory::builder(embedder)
            .path(&path)
            .config(cfg)
            .open()
            .unwrap();
        let reopen = t.elapsed().as_secs_f64() * 1000.0;
        let (mut items, mut stopped) = (0usize, 0usize);
        let mut stages: Vec<(&str, Vec<u64>)> = [
            "embed",
            "vector",
            "lexical",
            "expansion",
            "fusion",
            "render",
            "total",
        ]
        .iter()
        .map(|s| (*s, Vec::new()))
        .collect();
        for i in 0..200 {
            let q = if i % 2 == 0 {
                format!("Where does Person{} work?", (i * 4) % n)
            } else {
                format!("Which city is Company{} based in?", i % 97)
            };
            let r = m.recall(&RecallQuery::new(q)).await.unwrap();
            items += r.items.len();
            stopped += usize::from(r.expansion.stopped.is_some());
            let t = &r.timings;
            for (name, v) in stages.iter_mut() {
                v.push(match *name {
                    "embed" => t.embed_us,
                    "vector" => t.vector_us,
                    "lexical" => t.lexical_us,
                    "expansion" => t.expansion_us,
                    "fusion" => t.fusion_us,
                    "render" => t.render_us,
                    _ => t.total_us,
                });
            }
        }
        for (i, (name, v)) in stages.iter_mut().enumerate() {
            let (p50, p95) = (pct(v, 0.5), pct(v, 0.95));
            if i == 0 {
                println!("| {n} | {facts} | {ingest:.2} | {reopen:.0} | {name} | {p50} | {p95} |");
                println!("| | | | | (items per recall: {:.1}; expansions stopped early: {stopped}) | | |", items as f64 / 200.0);
            } else {
                println!("| | | | | {name} | {p50} | {p95} |");
            }
        }
    }
}
