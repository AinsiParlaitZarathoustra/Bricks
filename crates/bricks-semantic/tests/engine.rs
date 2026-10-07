//! The engine end to end, with a scripted language server (no real server
//! is involved here: see `rust_analyzer.rs` for that).

mod common;

use bricks_semantic::*;
use cersei_lsp::mock::MockConfig;
use common::*;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn req(root: &std::path::Path) -> Requester {
    Requester::new("test", root)
}

fn mock_engine(utf16: bool) -> (tempfile::TempDir, Arc<SemanticEngine>, Arc<MockLauncher>) {
    let d = workspace();
    let root = canonical(&d);
    let l = MockLauncher::new(MockConfig::new(caps(utf16), handler(root.clone(), utf16)));
    let e = engine_with(&root, Arc::clone(&l), SemanticConfig::default());
    (d, e, l)
}

fn pos(path: &str, line: u32, column: u32) -> Target {
    Target::Position {
        path: path.into(),
        line,
        column,
    }
}

#[tokio::test]
async fn text_search_is_lexical_exact_and_complete() {
    let (d, e, l) = mock_engine(true);
    let r = e
        .query(
            CodeQuery::text("println!").intent(Intent::TextSearch),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Complete);
    assert_eq!(r.items.len(), 1);
    let it = &r.items[0];
    assert_eq!(it.path, "src/main.rs");
    assert_eq!(it.relation, Relation::TextMention);
    assert_eq!(it.certainty, Certainty::Textual);
    assert_eq!((it.range.start.line, it.range.start.col), (6, 4));
    // No server for a text search.
    assert_eq!(l.launches.load(Ordering::SeqCst), 0);
    // Nothing found, complete: a real absence.
    let r = e
        .query(
            CodeQuery::text("no_such_text_zz").intent(Intent::TextSearch),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Complete);
    assert!(r.items.is_empty());
    // Limits: partial, said so.
    let mut q = CodeQuery::text("run").intent(Intent::TextSearch);
    q.limits.max_matches = Some(2);
    let r = e.query(q, &req(d.path())).await;
    assert_eq!(r.status, ResultStatus::Partial);
    assert!(r
        .omissions
        .iter()
        .any(|o| matches!(o, Omission::LimitReached { limit, .. } if limit == "max_matches")));
    // Exclusions and extensions.
    let mut q = CodeQuery::text("run").intent(Intent::TextSearch);
    q.scope.extensions = vec!["md".into()];
    let r = e.query(q, &req(d.path())).await;
    assert!(r.items.iter().all(|i| i.path == "README.md"));
}

#[tokio::test]
async fn homonyms_are_never_chosen_silently() {
    let (d, e, l) = mock_engine(true);
    // Symbol search: both, with the ambiguity stated.
    let r = e.query(CodeQuery::text("run"), &req(d.path())).await;
    assert_eq!(r.plan.intent, "find_symbol");
    let paths: Vec<&str> = r.items.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert!(paths.contains(&"src/alpha.rs") && paths.contains(&"src/beta.rs"));
    assert_eq!(r.ambiguity.as_ref().unwrap().candidates, 2);
    assert!(r.items.iter().all(|i| i.certainty == Certainty::Syntactic));
    // Definition by name only: ambiguous, no server started for nothing.
    let r = e
        .query(
            CodeQuery::text("run").intent(Intent::Definition),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Ambiguous);
    assert_eq!(l.launches.load(Ordering::SeqCst), 0);
    // By position: the server resolves `beta::run` to beta.rs.
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::Definition)
                .target(pos("src/main.rs", 6, 19)),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Complete, "{:?}", r.plan);
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.items[0].path, "src/beta.rs");
    assert_eq!(r.items[0].certainty, Certainty::Confirmed);
    assert_eq!(r.items[0].relation, Relation::Definition);
    let sn = r.items[0].snippet.as_ref().unwrap();
    assert!(sn.parts[0].text.contains("pub fn run() -> u32"));
    // A returned id is a valid target.
    let id = r.items[0].id.clone();
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::References)
                .target(Target::Item { id }),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Complete);
    assert!(r.items.iter().all(|i| i.relation == Relation::Reference));
}

#[tokio::test]
async fn references_from_position_are_confirmed() {
    let (d, e, _l) = mock_engine(true);
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::References)
                .target(pos("src/main.rs", 5, 20)),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Complete);
    assert_eq!(r.items.len(), 1, "only the alpha::run use");
    assert_eq!(r.items[0].line_text.trim(), "let a = alpha::run();");
    assert_eq!(r.items[0].signature.as_deref(), Some("fn main()"));
}

#[tokio::test]
async fn without_a_server_fallbacks_say_what_is_missing() {
    let d = workspace();
    let root = canonical(&d);
    let e = SemanticEngine::new(&root, no_lsp());
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::Definition)
                .target(pos("src/main.rs", 6, 19)),
            &req(&root),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Ambiguous);
    assert!(r.plan.fallbacks[0].contains("not established"));
    assert!(r.items.iter().all(|i| i.certainty == Certainty::Syntactic));
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::References)
                .target(pos("src/main.rs", 6, 19)),
            &req(&root),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Partial);
    assert!(r.items.iter().all(|i| i.relation == Relation::TextMention));
    assert!(r.plan.fallbacks[0].contains("not confirmed references"));
    // Not installed: unavailable, and the read path still works.
    let l = MockLauncher::new(MockConfig::new(caps(true), handler(root.clone(), true)));
    let l = Arc::new(MockLauncher {
        installed: false,
        config: parking_lot::Mutex::new(l.config.lock().clone()),
        launches: Default::default(),
        handles: Default::default(),
        launch_delay: Duration::ZERO,
    });
    let e = engine_with(&root, l, SemanticConfig::default());
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::Diagnostics)
                .target(Target::File {
                    path: "src/main.rs".into(),
                }),
            &req(&root),
        )
        .await;
    match r.diagnostics.unwrap().state {
        DiagnosticsState::Unavailable { reason } => assert!(reason.contains("not installed")),
        s => panic!("{s:?}"),
    }
}

#[tokio::test]
async fn utf8_and_utf16_servers_map_to_the_same_bytes() {
    for utf16 in [true, false] {
        let (d, e, _l) = mock_engine(utf16);
        // Line 3 of uni.rs: `fn caller() { ünï(); }`, column of `ünï`.
        let r = e
            .query(
                CodeQuery::text("")
                    .intent(Intent::Definition)
                    .target(pos("src/uni.rs", 3, 16)),
                &req(d.path()),
            )
            .await;
        assert_eq!(
            r.status,
            ResultStatus::Complete,
            "utf16={utf16} {:?}",
            r.plan
        );
        let it = &r.items[0];
        let text = std::fs::read_to_string(d.path().join("src/uni.rs")).unwrap();
        assert_eq!(
            &text[it.range.start_byte..it.range.end_byte],
            "ünï",
            "utf16={utf16}"
        );
        let _ = e.stats().lsp_instances[0].position_encoding.clone();
    }
}

#[tokio::test]
async fn versioned_sync_after_an_edit_and_cache_invalidation() {
    let (d, e, l) = mock_engine(true);
    let q = || {
        CodeQuery::text("")
            .intent(Intent::Definition)
            .target(pos("src/main.rs", 5, 20))
    };
    let r1 = e.query(q(), &req(d.path())).await;
    assert_eq!(r1.items[0].path, "src/alpha.rs");
    let r2 = e.query(q(), &req(d.path())).await;
    assert!(r2.metrics.result_cache_hit, "same generation: cached");
    // Edit main.rs: `alpha::run` becomes `beta::run` on that line.
    let main = d.path().join("src/main.rs");
    let t = std::fs::read_to_string(&main).unwrap();
    std::fs::write(
        &main,
        t.replace("let a = alpha::run();", "let a = beta::run(); "),
    )
    .unwrap();
    e.notify_changed(std::slice::from_ref(&main));
    let r3 = e.query(q(), &req(d.path())).await;
    assert!(!r3.metrics.result_cache_hit);
    assert_eq!(r3.items[0].path, "src/beta.rs");
    let srv = l.last();
    let uri = cersei_lsp::path_to_uri(&std::fs::canonicalize(&main).unwrap());
    let (version, text) = srv.doc(&uri).unwrap();
    assert_eq!(version, 2, "didChange with the next version");
    assert!(text.contains("beta::run(); "));
    // A dependency changed (beta.rs), main.rs did not: the cached answer
    // must not survive the notification.
    let beta = d.path().join("src/beta.rs");
    std::fs::write(&beta, "\n\n/// moved\npub fn run() -> u32 {\n    2\n}\n").unwrap();
    e.notify_changed(&[beta]);
    let r4 = e.query(q(), &req(d.path())).await;
    assert!(!r4.metrics.result_cache_hit);
    assert_eq!(r4.items[0].range.start.line, 3);
}

#[tokio::test]
async fn a_file_changed_during_the_query_is_flagged_stale() {
    let d = workspace();
    let root = canonical(&d);
    let mut cfg = MockConfig::new(caps(true), handler(root.clone(), true));
    cfg.delays
        .insert("textDocument/references".into(), Duration::from_millis(300));
    let l = MockLauncher::new(cfg);
    let e = engine_with(&root, l, SemanticConfig::default());
    let e2 = Arc::clone(&e);
    let r = tokio::spawn(async move {
        e2.query(
            CodeQuery::text("")
                .intent(Intent::References)
                .target(pos("src/main.rs", 5, 20)),
            &Requester::new("t", &root),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let main = d.path().join("src/main.rs");
    let t = std::fs::read_to_string(&main).unwrap();
    std::fs::write(&main, format!("// shifted\n{t}")).unwrap();
    let r = r.await.unwrap();
    assert_eq!(r.items.len(), 1);
    assert!(matches!(r.items[0].freshness, Freshness::Stale { .. }));
    // The offsets still refer to the version read: the line is the old one.
    assert_eq!(r.items[0].line_text.trim(), "let a = alpha::run();");
}

#[tokio::test]
async fn buffers_and_previews_stay_private() {
    let (d, e, l) = mock_engine(true);
    let root = canonical(&d);
    let view = e.open_view();
    view.set_buffer(
        root.join("src/alpha.rs"),
        "pub fn run_buffered() -> u32 { 1 }\n",
        7,
    )
    .unwrap();
    let mine = req(&root).with_view(view.clone());
    let r = e
        .query(
            CodeQuery::text("run_buffered").intent(Intent::TextSearch),
            &mine,
        )
        .await;
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.items[0].revision.buffer_version, Some(7));
    // Another client (shared view) does not see the buffer.
    let r = e
        .query(
            CodeQuery::text("run_buffered").intent(Intent::TextSearch),
            &req(&root),
        )
        .await;
    assert!(r.items.is_empty());
    // The server never receives private content.
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::Definition)
                .target(pos("src/alpha.rs", 1, 8)),
            &mine,
        )
        .await;
    assert!(
        r.plan.fallbacks[0].contains("shared view only"),
        "{:?}",
        r.plan
    );
    assert_eq!(l.launches.load(Ordering::SeqCst), 0);
    // A preview of a pending diff is visible only through its handle.
    let preview = view.preview(vec![(
        root.join("src/beta.rs"),
        Some("fn pending_change() {}\n".into()),
    )]);
    let r = e
        .query(
            CodeQuery::text("pending_change").intent(Intent::TextSearch),
            &req(&root).with_view(preview),
        )
        .await;
    assert_eq!(r.items.len(), 1);
    for v in [None, Some(view)] {
        let mut rq = req(&root);
        rq.view = v;
        let r = e
            .query(
                CodeQuery::text("pending_change").intent(Intent::TextSearch),
                &rq,
            )
            .await;
        assert!(r.items.is_empty());
    }
}

#[tokio::test]
async fn one_server_for_concurrent_clients_and_shared_work() {
    let d = workspace();
    let root = canonical(&d);
    let mut cfg = MockConfig::new(caps(true), handler(root.clone(), true));
    cfg.delays
        .insert("textDocument/definition".into(), Duration::from_millis(150));
    let l = MockLauncher::new(cfg);
    let l = Arc::new(MockLauncher {
        launch_delay: Duration::from_millis(100),
        config: parking_lot::Mutex::new(l.config.lock().clone()),
        launches: Default::default(),
        handles: Default::default(),
        installed: true,
    });
    let e = engine_with(&root, Arc::clone(&l), SemanticConfig::default());
    let mut tasks = Vec::new();
    for i in 0..8 {
        let e = Arc::clone(&e);
        let root = root.clone();
        tasks.push(tokio::spawn(async move {
            // Two distinct queries, four clients each.
            let line = if i % 2 == 0 { 5 } else { 6 };
            e.query(
                CodeQuery::text("")
                    .intent(Intent::Definition)
                    .target(pos("src/main.rs", line, 19)),
                &Requester::new(format!("client{i}"), &root),
            )
            .await
        }));
    }
    let mut shared = 0;
    for t in tasks {
        let r = t.await.unwrap();
        assert_eq!(r.status, ResultStatus::Complete);
        if r.metrics.shared_inflight {
            shared += 1;
        }
    }
    assert_eq!(l.launches.load(Ordering::SeqCst), 1, "one start for all");
    assert_eq!(shared, 6, "identical queries wait for the running one");
    assert_eq!(l.last().count("textDocument/definition"), 2);
    assert_eq!(e.stats().lsp_instances.len(), 1);
}

#[tokio::test]
async fn scope_limits_results_and_cache_does_not_leak() {
    let (d, e, _l) = mock_engine(true);
    let root = canonical(&d);
    // References of `run` from alpha's definition, asked by a client
    // limited to src/alpha.rs's folder... restricted to one file.
    let mut narrow = Requester::new("narrow", root.join("src/alpha.rs"));
    narrow.scope = vec![root.join("src/alpha.rs")];
    let r = e
        .query(
            CodeQuery::text("alpha::run").intent(Intent::TextSearch),
            &narrow,
        )
        .await;
    assert!(r.items.is_empty(), "main.rs is outside this scope");
    let r = e
        .query(
            CodeQuery::text("alpha::run").intent(Intent::TextSearch),
            &req(&root),
        )
        .await;
    assert_eq!(r.items.len(), 1);
    // A query path outside the requester's scope is refused.
    let mut q = CodeQuery::text("run").intent(Intent::TextSearch);
    q.scope.paths = vec!["src/main.rs".into()];
    let r = e.query(q, &narrow).await;
    assert!(matches!(r.status, ResultStatus::Error { .. }));
    // Semantic results outside the scope are dropped and counted.
    let mut only_main = Requester::new("main-only", root.join("src/main.rs"));
    only_main.scope = vec![root.join("src/main.rs")];
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::Definition)
                .target(pos("src/main.rs", 5, 20)),
            &only_main,
        )
        .await;
    assert!(r.items.is_empty());
    assert!(r
        .omissions
        .iter()
        .any(|o| matches!(o, Omission::ItemsOmitted { reason, .. } if reason.contains("scope"))));
    // The same query from the wider client is not served from that answer.
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::Definition)
                .target(pos("src/main.rs", 5, 20)),
            &req(&root),
        )
        .await;
    assert_eq!(r.items.len(), 1);
    // A scope outside the workspace is an error.
    let r = e
        .query(CodeQuery::text("x"), &Requester::new("out", "/"))
        .await;
    assert!(matches!(r.status, ResultStatus::Error { .. }));
}

#[tokio::test]
async fn diagnostics_freshness_and_baseline() {
    let d = workspace();
    let root = canonical(&d);
    let mut cfg = MockConfig::new(caps(true), handler(root.clone(), true));
    cfg.publisher = Some(Arc::new(|_uri: &str, version: i64, text: &str| {
        let items: Vec<Value> = text
            .match_indices("BAD")
            .map(|(i, _)| {
                json!({"range": {"start": {"line": 0, "character": i}, "end": {"line": 0, "character": i + 3}},
                       "severity": 1, "code": "E1", "message": "BAD here"})
            })
            .collect();
        Some((Some(version), items))
    }));
    let l = MockLauncher::new(cfg);
    let e = engine_with(&root, Arc::clone(&l), SemanticConfig::default());
    let f = root.join("src/diag.rs");
    std::fs::write(&f, "BAD\n").unwrap();
    let q = || {
        CodeQuery::text("")
            .intent(Intent::Diagnostics)
            .target(Target::File {
                path: "src/diag.rs".into(),
            })
    };
    let r = e.query(q(), &req(&root)).await;
    let rep = r.diagnostics.clone().unwrap();
    assert_eq!(rep.state, DiagnosticsState::Analyzed { version: 1 });
    assert_eq!(rep.errors, 1);
    assert_eq!(r.items[0].relation, Relation::Diagnostic);
    assert!(rep.introduced.is_none(), "no baseline yet");
    // Edit: one more BAD. The old one is not "introduced".
    std::fs::write(&f, "BAD BAD\n").unwrap();
    e.notify_changed(std::slice::from_ref(&f));
    let r = e.query(q(), &req(&root)).await;
    let rep = r.diagnostics.unwrap();
    assert_eq!(rep.state, DiagnosticsState::Analyzed { version: 2 });
    assert_eq!(
        (rep.introduced, rep.preexisting, rep.resolved),
        (Some(1), Some(1), Some(0))
    );

    // A server that never publishes: pending, not "no error".
    let silent = MockLauncher::new(MockConfig::new(caps(true), handler(root.clone(), true)));
    let mut c = SemanticConfig::default();
    c.lsp.diagnostics_wait_ms = 100;
    let e = engine_with(&root, silent, c);
    let r = e.query(q(), &req(&root)).await;
    assert_eq!(r.diagnostics.unwrap().state, DiagnosticsState::Pending);
    assert_eq!(r.status, ResultStatus::Partial);
}

#[tokio::test]
async fn timeout_cancellation_and_crash_restart() {
    let d = workspace();
    let root = canonical(&d);
    let mut cfg = MockConfig::new(caps(true), handler(root.clone(), true));
    cfg.delays
        .insert("textDocument/definition".into(), Duration::from_secs(30));
    let l = MockLauncher::new(cfg);
    let mut c = SemanticConfig::default();
    c.lsp.request_timeout_ms = 100;
    c.lsp.max_restarts = 1;
    let e = engine_with(&root, Arc::clone(&l), c);
    let q = || {
        CodeQuery::text("")
            .intent(Intent::Definition)
            .target(pos("src/main.rs", 5, 20))
    };
    // Timeout → the syntactic fallback, said so.
    let r = e.query(q(), &req(&root)).await;
    assert!(r
        .plan
        .steps
        .iter()
        .any(|s| s.backend == Backend::Lsp && s.outcome == StepOutcome::Failed));
    assert!(
        r.plan.fallbacks[0].contains("timed out"),
        "{:?}",
        r.plan.fallbacks
    );
    assert!(!r.items.is_empty());
    // Cancellation by the requester.
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        c2.cancel();
    });
    let mut cfg2 = SemanticConfig::default();
    cfg2.lsp.max_restarts = 1;
    let e2 = engine_with(&root, Arc::clone(&l), cfg2);
    let t = std::time::Instant::now();
    let r = e2.query(q(), &req(&root).with_cancel(cancel)).await;
    assert_eq!(r.status, ResultStatus::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(5));
    // Crash → one restart, then no more.
    let launches = l.launches.load(Ordering::SeqCst);
    l.last().crash().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    l.config.lock().delays.clear();
    let r = e2.query(q(), &req(&root)).await;
    assert_eq!(r.status, ResultStatus::Complete, "{:?}", r.plan);
    assert_eq!(l.launches.load(Ordering::SeqCst), launches + 1);
    assert_eq!(e2.stats().lsp_restarts, 1);
    l.last().crash().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let r = e2.query(q(), &req(&root)).await;
    assert!(
        r.plan.fallbacks[0].contains("crashed"),
        "{:?}",
        r.plan.fallbacks
    );
    assert_eq!(l.launches.load(Ordering::SeqCst), launches + 1);
}

#[tokio::test]
async fn understand_combines_scope_hover_and_definition_within_budget() {
    let (d, e, _l) = mock_engine(true);
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::Understand)
                .target(pos("src/main.rs", 6, 19)),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Complete, "{:?}", r.plan);
    assert_eq!(r.items[0].relation, Relation::Context);
    assert_eq!(r.items[0].documentation.as_deref(), Some("fn run()"));
    assert!(r
        .items
        .iter()
        .any(|i| i.relation == Relation::Definition && i.path == "src/beta.rs"));
    let text = render::render_response(&r);
    assert!(text.contains("src/beta.rs:2:8"), "{text}");
    // A tiny budget: fewer items, said so, the estimate under the limit.
    let mut q = CodeQuery::text("")
        .intent(Intent::Understand)
        .target(pos("src/main.rs", 6, 19));
    q.limits.budget_tokens = Some(150);
    let r = e.query(q, &req(d.path())).await;
    assert!(r.budget.truncated);
    assert!(r.budget.used_upper_tokens <= 150);
}

#[tokio::test]
async fn stale_item_ids_are_refused() {
    let (d, e, _l) = mock_engine(true);
    let r = e
        .query(
            CodeQuery::text("run").intent(Intent::FindSymbol),
            &req(d.path()),
        )
        .await;
    let id = r.items[0].id.clone();
    let f = d.path().join(&r.items[0].path);
    std::fs::write(&f, "// changed\npub fn run() -> u32 { 0 }\n").unwrap();
    let r = e
        .query(
            CodeQuery::text("")
                .intent(Intent::Definition)
                .target(Target::Item { id }),
            &req(d.path()),
        )
        .await;
    match r.status {
        ResultStatus::Error { message } => assert!(message.contains("search again")),
        s => panic!("{s:?}"),
    }
}

#[tokio::test]
async fn workspace_and_document_symbols() {
    let (d, e, l) = mock_engine(true);
    // No server running: symbol search stays syntactic, starts nothing.
    let r = e
        .query(
            CodeQuery::text("run").intent(Intent::FindSymbol),
            &req(d.path()),
        )
        .await;
    assert!(r.items.iter().all(|i| i.certainty == Certainty::Syntactic));
    assert_eq!(l.launches.load(Ordering::SeqCst), 0);
    // An outline starts the server (understand may), document symbols.
    let r = e
        .query(
            CodeQuery::text("").target(Target::File {
                path: "src/main.rs".into(),
            }),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.plan.strategy, "lsp outline", "{:?}", r.plan);
    let names: Vec<&str> = r
        .items
        .iter()
        .filter_map(|i| i.symbol.as_ref().map(|s| s.name.as_str()))
        .collect();
    assert_eq!(names, vec!["main"]);
    assert!(r.items[0].snippet.is_none(), "an outline shows signatures");
    assert_eq!(r.items[0].signature.as_deref(), Some("fn main() {"));
    // Now a server runs: symbol search is confirmed by workspace/symbol.
    let r = e
        .query(
            CodeQuery::text("run").intent(Intent::FindSymbol),
            &req(d.path()),
        )
        .await;
    assert_eq!(r.items.len(), 2, "homonyms still both listed");
    assert!(r.items.iter().all(|i| i.certainty == Certainty::Confirmed));
    assert!(r.items[0]
        .provenance
        .iter()
        .any(|p| p.method == "workspace/symbol"));
    assert_eq!(r.status, ResultStatus::Complete);
    assert!(r.ambiguity.is_some());
    // Without a server: the syntax outline, said so.
    let root = canonical(&d);
    let e = SemanticEngine::new(&root, no_lsp());
    let r = e
        .query(
            CodeQuery::text("").target(Target::File {
                path: "src/alpha.rs".into(),
            }),
            &req(&root),
        )
        .await;
    assert_eq!(r.status, ResultStatus::Partial);
    assert_eq!(r.items[0].symbol.as_ref().unwrap().name, "run");
    assert!(r.plan.fallbacks[0].contains("syntax outline"));
}

#[tokio::test]
async fn answers_while_the_server_indexes_are_partial() {
    let (d, e, l) = mock_engine(true);
    let q = || {
        CodeQuery::text("")
            .intent(Intent::References)
            .target(pos("src/main.rs", 5, 20))
    };
    // Start the server, then make it report indexing.
    let r = e.query(q(), &req(d.path())).await;
    assert_eq!(r.status, ResultStatus::Complete);
    let srv = l.last();
    srv.notify(
        "$/progress",
        json!({"token": "rustAnalyzer/Indexing", "value": {"kind": "begin", "title": "Indexing"}}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut cfg_q = q();
    cfg_q.limits.timeout_ms = Some(300);
    e.notify_any_change();
    let r = e.query(cfg_q, &req(d.path())).await;
    assert_eq!(r.status, ResultStatus::Partial, "{:?}", r.plan);
    assert!(r
        .plan
        .fallbacks
        .iter()
        .any(|f| f.contains("still indexing (Indexing)")));
    // Indexing ends while a query waits: it proceeds, complete.
    let e2 = Arc::clone(&e);
    let root = canonical(&d);
    let waiting = tokio::spawn(async move { e2.query(q(), &Requester::new("t", &root)).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    srv.notify(
        "$/progress",
        json!({"token": "rustAnalyzer/Indexing", "value": {"kind": "end"}}),
    )
    .await;
    let r = waiting.await.unwrap();
    assert_eq!(r.status, ResultStatus::Complete, "{:?}", r.plan);
    assert!(r
        .plan
        .steps
        .iter()
        .any(|s| s.action == "wait for indexing" && s.outcome == StepOutcome::Ok));
    // rust-analyzer's own status notification counts too.
    srv.notify(
        "experimental/serverStatus",
        json!({"health": "ok", "quiescent": false}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut short =
        CodeQuery::text("")
            .intent(Intent::References)
            .target(pos("src/main.rs", 6, 19));
    short.limits.timeout_ms = Some(200);
    let r = e.query(short, &req(d.path())).await;
    assert_eq!(r.status, ResultStatus::Partial);
}
