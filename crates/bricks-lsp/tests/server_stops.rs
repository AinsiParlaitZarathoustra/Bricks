//! The LSP client against a real process that misbehaves: silent, dead, or
//! answering too late. Every wait here is bounded; the fake servers run
//! with `exec`, so the process the client kills is the one the test named.

#![cfg(unix)]

use bricks_lsp::{LspClient, LspError, LspServerConfig};
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

/// A client whose server is `sh -c <script>`.
fn fake(script: &str) -> LspClient {
    let mut cfg = LspServerConfig::new("fake", "sh", &["*.rs"], &[(".rs", "rust")]);
    cfg.args = vec!["-c".into(), script.into()];
    LspClient::new(cfg)
}

#[tokio::test]
async fn a_request_that_times_out_is_withdrawn() {
    let c = fake("exec sleep 30").with_timeout(Duration::from_millis(100));
    c.start(Path::new("/")).await.unwrap();
    let r = c
        .request(
            "textDocument/hover",
            json!({}),
            Duration::from_millis(100),
            None,
        )
        .await;
    assert!(matches!(r, Err(LspError::Timeout(_))), "{r:?}");
    assert_eq!(c.pending_requests(), 0);
}

#[tokio::test]
async fn a_request_of_a_dead_server_fails_at_once() {
    let c = fake("exit 0");
    c.start(Path::new("/")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    let r = c
        .request(
            "textDocument/hover",
            json!({}),
            Duration::from_secs(30),
            None,
        )
        .await;
    assert!(matches!(r, Err(LspError::ServerExited)), "{r:?}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(!c.is_alive());
}

/// Dropping the request future (an outer timeout, a `select!` losing) must
/// not leave its entry behind: nothing will ever answer it, and the server
/// is not told the request is gone.
#[tokio::test]
async fn a_dropped_request_withdraws_its_pending_entry() {
    let c = fake("exec sleep 30");
    c.start(Path::new("/")).await.unwrap();
    let r = tokio::time::timeout(
        Duration::from_millis(100),
        c.request(
            "textDocument/hover",
            json!({}),
            Duration::from_secs(30),
            None,
        ),
    )
    .await;
    assert!(r.is_err());
    assert_eq!(c.pending_requests(), 0, "the dropped request is withdrawn");
}
