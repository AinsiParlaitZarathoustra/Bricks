//! Against a real rust-analyzer (not a mock). Ignored by default: it needs
//! the server installed (`rustup component add rust-analyzer`) and takes
//! seconds to index. Run with:
//!
//! ```sh
//! cargo test -p bricks-semantic --test rust_analyzer -- --ignored --nocapture
//! ```
//!
//! When the server cannot start, the test fails and says so: it is never
//! reported as passed without the server.

use bricks_semantic::*;
use std::sync::Arc;
use std::time::Duration;

fn workspace() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let w = |p: &str, t: &str| {
        let f = d.path().join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    };
    w(
        "Cargo.toml",
        "[package]\nname = \"ra_demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    );
    w("src/alpha.rs", "pub fn run() -> u32 {\n    1\n}\n");
    w("src/beta.rs", "pub fn run() -> u32 {\n    2\n}\n");
    w(
        "src/main.rs",
        "mod alpha;\nmod beta;\n\nfn main() {\n    let a = alpha::run();\n    let b = beta::run();\n    println!(\"{}\", a + b);\n}\n",
    );
    d
}

async fn until_complete(
    e: &Arc<SemanticEngine>,
    q: impl Fn() -> CodeQuery,
    root: &std::path::Path,
) -> CodeResponse {
    // rust-analyzer answers before indexing ends (empty results): retry
    // for a bounded time.
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        let r = e.query(q(), &Requester::new("ra-test", root)).await;
        let ok = r.status == ResultStatus::Complete && !r.items.is_empty();
        let cannot_start = r.plan.steps.iter().any(|s| {
            s.outcome == StepOutcome::Unavailable
                && s.note
                    .as_deref()
                    .is_some_and(|n| n.contains("failed to start") || n.contains("not installed"))
        });
        if ok || cannot_start || std::time::Instant::now() > deadline {
            return r;
        }
        e.notify_any_change();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
#[ignore = "needs rust-analyzer installed; run with --ignored"]
async fn definitions_references_and_edits_with_rust_analyzer() {
    let d = workspace();
    let root = std::fs::canonicalize(d.path()).unwrap();
    let mut cfg = SemanticConfig::default();
    cfg.lsp.request_timeout_ms = 60_000;
    let e = SemanticEngine::new(&root, cfg);
    let def = |line: u32, column: u32| {
        move || {
            CodeQuery::text("")
                .intent(Intent::Definition)
                .target(Target::Position {
                    path: "src/main.rs".into(),
                    line,
                    column,
                })
        }
    };
    let r = until_complete(&e, def(6, 19), &root).await;
    let started = e.stats().lsp_instances;
    assert!(
        !started.is_empty(),
        "rust-analyzer did not start — test NOT executed against a real server: {:?} {:?}",
        r.plan.steps,
        r.plan.fallbacks
    );
    assert_eq!(r.items.len(), 1, "{:#?}", r.plan);
    assert_eq!(
        r.items[0].path, "src/beta.rs",
        "homonym resolved by the server"
    );
    assert_eq!(r.items[0].certainty, Certainty::Confirmed);
    let r = until_complete(&e, def(5, 20), &root).await;
    assert_eq!(r.items[0].path, "src/alpha.rs");

    let refs = || {
        CodeQuery::text("")
            .intent(Intent::References)
            .target(Target::Position {
                path: "src/alpha.rs".into(),
                line: 1,
                column: 8,
            })
    };
    let r = until_complete(&e, refs, &root).await;
    assert_eq!(r.items.len(), 1, "{:#?}", r.items);
    assert_eq!(r.items[0].path, "src/main.rs");
    assert_eq!(r.items[0].range.start.line, 4);

    // Edit main.rs: line 5 now calls beta::run; ask again.
    let main = root.join("src/main.rs");
    let t = std::fs::read_to_string(&main).unwrap();
    std::fs::write(
        &main,
        t.replace("let a = alpha::run();", "let a = beta::run();"),
    )
    .unwrap();
    e.notify_changed(std::slice::from_ref(&main));
    let r = until_complete(&e, def(5, 19), &root).await;
    assert_eq!(r.items[0].path, "src/beta.rs", "after the edit");
    let stats = e.stats();
    println!(
        "rust-analyzer: pid {:?}, encoding {}, {} requests",
        stats.lsp_instances[0].pid,
        stats.lsp_instances[0].position_encoding,
        stats.lsp_instances[0].requests
    );
    e.shutdown().await;
}
