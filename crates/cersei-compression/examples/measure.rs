//! Measure the reductions on the fixture corpus and on real files.
//!
//! ```text
//! cargo run --release -p cersei-compression --example measure -- \
//!     [--read <source file>]... [--json <json file>]...
//! ```
//!
//! For each input: size before/after, local token estimate before/after
//! (method: `cersei_types::tokens::ESTIMATION_METHOD`), processing time
//! (median of 5 runs) and whether the information listed for the fixture
//! was preserved. Prints a Markdown table.

#[path = "../tests/support/cases.rs"]
mod corpus;

use cersei_compression::{
    CompressionConfig, CompressionLevel, Compressor, RawStore, RuleSet, ToolOutput,
};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Row {
    name: String,
    strategy: String,
    lines: (usize, usize),
    bytes: (usize, usize),
    tokens: (u64, u64),
    time: Duration,
    preserved: String,
}

fn timed(
    c: &Compressor,
    out: &ToolOutput,
    level: CompressionLevel,
) -> (cersei_compression::Processed, Duration) {
    let mut times = Vec::new();
    let mut last = None;
    for _ in 0..5 {
        let t = Instant::now();
        let p = c.process(out, level);
        times.push(t.elapsed());
        last = Some(p);
    }
    times.sort();
    (last.unwrap(), times[2])
}

fn main() {
    let dir = std::env::temp_dir().join("bricks-measure");
    let c = Compressor::new(
        CompressionConfig::default(),
        Arc::new(RuleSet::builtin()),
        Some(RawStore::new(&dir)),
    );
    let mut rows = Vec::new();

    for case in corpus::CASES {
        let content = corpus::fixture(case.fixture);
        let input = json!({ "command": case.command });
        let out = ToolOutput {
            tool: "Bash",
            input: &input,
            content: &content,
            is_error: false,
            call_id: "m",
            exit_code: None,
        };
        let (p, time) = timed(&c, &out, CompressionLevel::Minimal);
        let kept = case.keep.iter().filter(|k| p.text.contains(*k)).count();
        let noise_left = case
            .drop
            .iter()
            .filter(|d| p.transformed && p.text.contains(*d))
            .count();
        assert_eq!(noise_left, 0, "{}: noise kept", case.fixture);
        if p.transformed {
            assert_eq!(p.detail, case.rule, "{}: unexpected rule", case.fixture);
        }
        rows.push(Row {
            name: case.fixture.to_string(),
            strategy: if p.transformed {
                format!("{} `{}`", p.strategy, p.detail)
            } else {
                "unchanged".into()
            },
            lines: (p.stats.before_lines, p.stats.after_lines),
            bytes: (p.stats.before_bytes, p.stats.after_bytes),
            tokens: (
                p.stats.before_tokens_estimate,
                p.stats.after_tokens_estimate,
            ),
            time,
            preserved: format!("{kept}/{}", case.keep.len()),
        });
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    for pair in args.chunks(2) {
        let (flag, path) = (&pair[0], &pair[1]);
        let text = std::fs::read_to_string(path).expect("readable file");
        let numbered: String = text
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>6} | {l}\n", i + 1))
            .collect();
        let input = json!({ "file_path": path });
        let level = if flag == "--read" {
            CompressionLevel::Aggressive
        } else {
            CompressionLevel::Minimal
        };
        let out = ToolOutput {
            tool: "Read",
            input: &input,
            content: &numbered,
            is_error: false,
            call_id: "m",
            exit_code: None,
        };
        let (p, time) = timed(&c, &out, level);
        rows.push(Row {
            name: format!("{path} ({level})"),
            strategy: if p.transformed {
                p.strategy.to_string()
            } else {
                "unchanged".into()
            },
            lines: (p.stats.before_lines, p.stats.after_lines),
            bytes: (p.stats.before_bytes, p.stats.after_bytes),
            tokens: (
                p.stats.before_tokens_estimate,
                p.stats.after_tokens_estimate,
            ),
            time,
            preserved: "n/a".into(),
        });
    }

    println!(
        "Token estimate: {}\n",
        cersei_types::tokens::ESTIMATION_METHOD
    );
    println!("| input | result | lines | bytes | est. tokens | saved | time | kept |");
    println!("|---|---|---|---|---|---|---|---|");
    let (mut tb, mut ta) = (0u64, 0u64);
    for r in &rows {
        tb += r.tokens.0;
        ta += r.tokens.1;
        let saved = 100.0 * (r.bytes.0 as f64 - r.bytes.1 as f64) / r.bytes.0.max(1) as f64;
        println!(
            "| {} | {} | {} → {} | {} → {} | {} → {} | {:.0}% | {:.2} ms | {} |",
            r.name,
            r.strategy,
            r.lines.0,
            r.lines.1,
            r.bytes.0,
            r.bytes.1,
            r.tokens.0,
            r.tokens.1,
            saved,
            r.time.as_secs_f64() * 1000.0,
            r.preserved
        );
    }
    println!(
        "\nTotal estimated tokens: {tb} → {ta} ({:.0}% less)",
        100.0 * (tb as f64 - ta as f64) / tb.max(1) as f64
    );
    let _ = std::fs::remove_dir_all(dir);
}
