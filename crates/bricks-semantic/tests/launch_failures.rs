//! Starting a real language server process that fails: the last stderr
//! line explains the failure even when it arrives late, a silent server
//! times out within a bound, and nothing waits forever.

#![cfg(unix)]

use bricks_lsp::LspServerConfig;
use bricks_semantic::lsp::{Launcher, ProcessLauncher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn server(script: &str) -> LspServerConfig {
    let mut cfg = LspServerConfig::new("fake", "sh", &["*.rs"], &[(".rs", "rust")]);
    cfg.args = vec!["-c".into(), script.into()];
    cfg
}

#[tokio::test]
async fn a_late_last_stderr_line_still_explains_the_failure() {
    let started = Instant::now();
    let err = ProcessLauncher
        .launch(
            server("sleep 0.3; echo 'late: no workspace found' >&2; exit 3"),
            PathBuf::from("/"),
            Duration::from_secs(10),
        )
        .await
        .err()
        .expect("the launch fails");
    let msg = err.to_string();
    assert!(msg.contains("late: no workspace found"), "{msg}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_silent_server_times_out_within_a_bound() {
    // Never answers `initialize`, never writes, never exits on its own.
    let started = Instant::now();
    let err = ProcessLauncher
        .launch(
            server("exec sleep 30"),
            PathBuf::from("/"),
            Duration::from_millis(300),
        )
        .await
        .err()
        .expect("the launch fails");
    let took = started.elapsed();
    assert_eq!(err.to_string(), "Request timed out after 300ms", "{err}");
    // The shutdown of a server that was never initialised waits up to 5 s
    // for its exit, then kills it: bounded, never forever.
    assert!(took < Duration::from_secs(12), "{took:?}");
}

#[tokio::test]
async fn a_server_missing_its_stdout_or_stderr_pipe_is_not_waited_for() {
    // A server that closes its streams at once and exits: no line, no wait.
    let started = Instant::now();
    let err = ProcessLauncher
        .launch(
            server("exec 1>&- 2>&-; exit 4"),
            PathBuf::from("/"),
            Duration::from_secs(10),
        )
        .await
        .err()
        .expect("the launch fails");
    assert!(!err.to_string().is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "{:?}",
        started.elapsed()
    );
}
