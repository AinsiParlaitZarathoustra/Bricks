//! Corpus tests: every fixture goes through the real dispatcher with the
//! built-in rules, and must keep the information a reader needs.
//!
//! `fixtures/captured/` holds real outputs recorded on a development machine
//! (paths anonymised); `fixtures/reconstructed/` reproduces the documented
//! formats of tools that were not installed there (see its README).

#[path = "support/cases.rs"]
mod cases;
use cases::{fixture, CASES};
use cersei_compression::{
    CompressionConfig, CompressionLevel, Compressor, RawStore, RuleSet, ToolOutput,
};
use serde_json::json;
use std::sync::Arc;

#[test]
fn every_fixture_keeps_what_matters_and_names_its_original() {
    let dir = tempfile::tempdir().unwrap();
    let rules = Arc::new(RuleSet::builtin());
    let c = Compressor::new(
        CompressionConfig::default(),
        rules.clone(),
        Some(RawStore::new(dir.path())),
    );
    let mut failures = Vec::new();
    for case in CASES {
        let content = fixture(case.fixture);
        // Rule choice, from the command actually run.
        let analysis = cersei_compression::command::analyze(case.command);
        let inv = analysis.primary().unwrap().producer().unwrap();
        let chosen = rules.find(inv).map(|r| r.id.as_str()).unwrap_or("-");
        if chosen != case.rule {
            failures.push(format!(
                "{}: rule {chosen}, expected {}",
                case.fixture, case.rule
            ));
        }
        let input = json!({ "command": case.command });
        let p = c.process(
            &ToolOutput {
                tool: "Bash",
                input: &input,
                content: &content,
                is_error: false,
                call_id: "c",
                exit_code: None,
            },
            CompressionLevel::Minimal,
        );
        for k in case.keep {
            if !p.text.contains(k) {
                failures.push(format!("{}: lost {k:?}", case.fixture));
            }
        }
        // Small outputs that would shrink by less than a tenth are returned
        // unchanged (no header, no partial view); noise is checked otherwise.
        for d in case.drop.iter().filter(|_| p.transformed) {
            if p.text.contains(d) {
                failures.push(format!("{}: kept noise {d:?}", case.fixture));
            }
        }
        if p.transformed {
            match &p.raw {
                Some(r) => assert_eq!(std::fs::read_to_string(&r.path).unwrap(), content),
                None => failures.push(format!(
                    "{}: transformed without a saved original",
                    case.fixture
                )),
            }
            if !p.text.starts_with("[bricks: ") {
                failures.push(format!("{}: transformed without a header", case.fixture));
            }
        }
        if content.lines().count() >= 100 && !p.transformed {
            failures.push(format!("{}: a long log was not reduced", case.fixture));
        }
        assert!(
            p.stats.after_bytes <= p.stats.before_bytes,
            "{}",
            case.fixture
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn off_level_is_exact_passthrough_for_ordinary_outputs() {
    for case in CASES {
        let content = fixture(case.fixture);
        let out = cersei_compression::compress_tool_output(
            "Bash",
            &json!({ "command": case.command }),
            &content,
            CompressionLevel::Off,
        );
        assert_eq!(out, content, "{}", case.fixture);
    }
}

#[test]
fn small_outputs_that_barely_shrink_are_left_alone() {
    let log = "commit 4c8b9d1\nAuthor: Alice <alice@example.com>\nDate:   Mon Apr 14 2026\n\n    fix: off-by-one\n";
    let out = cersei_compression::compress_tool_output(
        "Bash",
        &json!({ "command": "git log -1" }),
        log,
        CompressionLevel::Aggressive,
    );
    // Authors and dates are data: `git log` is never stripped of them.
    assert_eq!(out, log);
}
