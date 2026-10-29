//! CodeStats at run time, on a single-threaded runtime: cancellation before
//! and between files, one analysis for identical calls of a session, and no
//! finished result kept. Progress is signalled by probes on the analysis
//! thread, never by timing alone; scenarios run in order because they read
//! process-wide counters.

use bricks_semantic::CancellationToken;
use bricks_tools::code_scout::RunCancel;
use bricks_tools::code_stats::{
    CodeStatsProbe, CodeStatsTool, analyses_running, waiting_in_session,
};
use bricks_tools::permissions::AllowAll;
use bricks_tools::tool_report::ToolStatus;
use bricks_tools::{CostTracker, Extensions, Tool, ToolContext, ToolResult};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const RUST: &str = "// comment\nfn main() {\n\n    println!(\"hi\");\n}\n";

fn ctx(dir: &Path, session: &str) -> ToolContext {
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: session.into(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

fn tree(files: usize) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for i in 0..files {
        std::fs::write(d.path().join(format!("f{i:03}.rs")), RUST).unwrap();
    }
    d
}

fn json_of(r: &ToolResult) -> Value {
    assert!(!r.is_error, "{}", r.content);
    serde_json::from_str(&r.content).unwrap()
}

/// Blocks the analysis thread until `cond` holds (bounded, so a broken
/// contract fails instead of hanging).
fn wait_for(cond: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !cond() {
        assert!(Instant::now() < until, "condition never met");
        std::thread::sleep(Duration::from_millis(1));
    }
}

async fn settle(cond: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !cond() {
        assert!(Instant::now() < until, "condition never met");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn counting_probe(
    count: &Arc<AtomicUsize>,
    then: impl Fn() + Send + Sync + 'static,
) -> CodeStatsProbe {
    let count = Arc::clone(count);
    CodeStatsProbe {
        counted: Some(Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            then();
        })),
        ..Default::default()
    }
}

#[tokio::test]
async fn cancellation_sharing_and_freshness() {
    cancelled_before_start().await;
    cancelled_between_files().await;
    identical_calls_share_one_analysis().await;
    different_calls_and_sessions_do_not_share().await;
    one_caller_leaving_does_not_stop_the_others().await;
    errors_free_the_entry_and_a_retry_runs().await;
    a_new_call_after_a_change_sees_the_change().await;
    settle(|| analyses_running() == 0).await;
}

async fn cancelled_before_start() {
    let d = tree(5);
    let session = "cancel-before";
    let count = Arc::new(AtomicUsize::new(0));
    let walked = Arc::new(AtomicBool::new(false));
    let c = ctx(d.path(), session);
    let token = CancellationToken::new();
    token.cancel();
    c.extensions.insert(RunCancel(token));
    let w = Arc::clone(&walked);
    let mut probe = counting_probe(&count, || {});
    probe.walked = Some(Arc::new(move || w.store(true, Ordering::SeqCst)));
    c.extensions.insert(probe);

    let r = CodeStatsTool.execute(json!({}), &c).await;
    assert!(r.is_error);
    assert_eq!(r.to_report().status, ToolStatus::Cancelled);
    assert!(!r.content.contains("totals"), "no counters after a stop");
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(!walked.load(Ordering::SeqCst), "nothing walked");
    assert_eq!(waiting_in_session(session), 0);
}

async fn cancelled_between_files() {
    const FILES: usize = 400;
    let d = tree(FILES);
    let session = "cancel-between";
    let count = Arc::new(AtomicUsize::new(0));
    let token = CancellationToken::new();
    let c = ctx(d.path(), session);
    c.extensions.insert(RunCancel(token.clone()));
    // The first counted file cancels the run; every file thread then waits
    // until the caller has gone (so no file starts in between).
    let t = token.clone();
    c.extensions.insert(counting_probe(&count, move || {
        t.cancel();
        wait_for(|| waiting_in_session("cancel-between") == 0);
    }));

    let r = CodeStatsTool.execute(json!({"detail": "files"}), &c).await;
    assert_eq!(r.to_report().status, ToolStatus::Cancelled, "{}", r.content);
    assert_eq!(waiting_in_session(session), 0);
    settle(|| analyses_running() == 0).await;
    // Each pool thread may finish the file it holds (and one it picked up
    // while Tokei waited inside the first); none goes further.
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .max(rayon::current_num_threads());
    let counted = count.load(Ordering::SeqCst);
    assert!(
        counted <= 2 * threads && counted < FILES,
        "files in progress may finish, no new one starts: {counted} of {FILES}"
    );
}

async fn identical_calls_share_one_analysis() {
    let d = tree(1);
    let session = "share";
    let count = Arc::new(AtomicUsize::new(0));
    let c = ctx(d.path(), session);
    // Held until the second caller has joined.
    c.extensions.insert(counting_probe(&count, || {
        wait_for(|| waiting_in_session("share") >= 2)
    }));
    let (a, b) = tokio::join!(
        CodeStatsTool.execute(json!({"detail": "files"}), &c),
        CodeStatsTool.execute(json!({"detail": "files", "top_n": 20}), &c),
    );
    assert_eq!(count.load(Ordering::SeqCst), 1, "one analysis for both");
    assert_eq!(json_of(&a), json_of(&b));
    assert_eq!(waiting_in_session(session), 0, "released on success");
}

async fn different_calls_and_sessions_do_not_share() {
    let d = tree(1);
    let count = Arc::new(AtomicUsize::new(0));
    let one = ctx(d.path(), "distinct-1");
    let two = ctx(d.path(), "distinct-2");
    // Same session and target, seen from another directory: paths are
    // shown from there, so it is another analysis.
    let above = ctx(d.path().parent().unwrap(), "distinct-1");
    let name = d.path().file_name().unwrap().to_str().unwrap();
    for c in [&one, &two, &above] {
        c.extensions.insert(counting_probe(&count, || {}));
    }
    let (a, b, c, e) = tokio::join!(
        CodeStatsTool.execute(json!({"detail": "summary"}), &one),
        CodeStatsTool.execute(json!({"detail": "languages"}), &one),
        CodeStatsTool.execute(json!({"detail": "summary"}), &two),
        CodeStatsTool.execute(json!({"detail": "summary", "path": name}), &above),
    );
    for r in [&a, &b, &c, &e] {
        json_of(r);
    }
    assert_eq!(count.load(Ordering::SeqCst), 4);
    assert_eq!(json_of(&a)["path"], ".");
    assert_eq!(json_of(&e)["path"], name);
}

async fn one_caller_leaving_does_not_stop_the_others() {
    let d = tree(1);
    let session = "abandon";
    let count = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(AtomicBool::new(false));
    let c = ctx(d.path(), session);
    let r2 = Arc::clone(&release);
    c.extensions.insert(counting_probe(&count, move || {
        wait_for(|| r2.load(Ordering::SeqCst))
    }));
    let (ca, cb) = (c.clone(), c.clone());
    let a = tokio::spawn(async move { CodeStatsTool.execute(json!({}), &ca).await });
    let b = tokio::spawn(async move { CodeStatsTool.execute(json!({}), &cb).await });
    settle(|| waiting_in_session(session) == 2).await;
    a.abort();
    assert!(a.await.unwrap_err().is_cancelled());
    assert_eq!(
        waiting_in_session(session),
        1,
        "the other caller still waits"
    );
    release.store(true, Ordering::SeqCst);
    let v = json_of(&b.await.unwrap());
    assert_eq!(v["totals"]["files"], 1);
    assert_eq!(v["partial"], false);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(waiting_in_session(session), 0);
}

async fn errors_free_the_entry_and_a_retry_runs() {
    let d = tree(2);
    let session = "retry";
    let c = ctx(d.path(), session);
    let r = CodeStatsTool.execute(json!({"exclude": ["a[b"]}), &c).await;
    assert!(r.is_error);
    let r = CodeStatsTool.execute(json!({"path": "nope"}), &c).await;
    assert!(r.is_error);
    assert_eq!(waiting_in_session(session), 0);
    let v = json_of(&CodeStatsTool.execute(json!({}), &c).await);
    assert_eq!(v["totals"]["files"], 2);
}

async fn a_new_call_after_a_change_sees_the_change() {
    let d = tree(1);
    let c = ctx(d.path(), "fresh");
    let before = json_of(&CodeStatsTool.execute(json!({}), &c).await);
    std::fs::write(d.path().join("g.rs"), "fn g() {}\n").unwrap();
    let after = json_of(&CodeStatsTool.execute(json!({}), &c).await);
    assert_eq!(before["totals"]["files"], 1);
    assert_eq!(after["totals"]["files"], 2, "no finished result kept");
    assert_eq!(
        after["totals"]["code"].as_u64().unwrap(),
        before["totals"]["code"].as_u64().unwrap() + 1
    );
}
