//! The LSP client against the in-process scripted server.

use cersei_lsp::mock::{self, MockConfig};
use cersei_lsp::{LspClient, LspError, LspServerConfig};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn config() -> LspServerConfig {
    LspServerConfig::new("mock", "mock-ls", &["*.rs"], &[(".rs", "rust")])
}

async fn connect(cfg: MockConfig) -> (Arc<LspClient>, mock::MockHandle) {
    let (handle, reader, writer) = mock::spawn(cfg);
    let client = Arc::new(LspClient::new(config()));
    client.connect(reader, writer, Path::new("/ws")).await;
    client.initialize().await.unwrap();
    (client, handle)
}

fn echo() -> mock::Handler {
    Arc::new(
        |method: &str, params: &Value, _docs: &mock::MockDocs| match method {
            "textDocument/definition" => Ok(json!([{
                "targetUri": params["textDocument"]["uri"],
                "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 2, "character": 1}},
                "targetSelectionRange": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}}
            }])),
            "textDocument/references" => Ok(Value::Null),
            _ => Err((-32601, "nope".into())),
        },
    )
}

#[tokio::test]
async fn capabilities_and_encoding_are_kept() {
    let (client, mock) = connect(MockConfig::new(
        json!({"positionEncoding": "utf-8", "textDocumentSync": 2, "definitionProvider": true}),
        echo(),
    ))
    .await;
    let caps = client.capabilities().unwrap();
    assert_eq!(caps.position_encoding, "utf-8");
    assert!(caps.definition && !caps.references);
    // We offered UTF-8 and UTF-16.
    let init = &mock.received("initialize")[0];
    assert_eq!(
        init["capabilities"]["general"]["positionEncodings"],
        json!(["utf-8", "utf-16"])
    );
}

#[tokio::test]
async fn versioned_sync_and_response_shapes() {
    let (client, mock) = connect(MockConfig::new(json!({"textDocumentSync": 2}), echo())).await;
    let uri = "file:///ws/a%20b.rs";
    client
        .did_open(uri, "rust", 1, "fn main() {}\n")
        .await
        .unwrap();
    client
        .did_change_full(uri, 2, "fn main() { x }\n")
        .await
        .unwrap();
    let r = client
        .request(
            "textDocument/definition",
            json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 4}}),
            Duration::from_secs(2),
            None,
        )
        .await
        .unwrap();
    assert_eq!(mock.doc(uri), Some((2, "fn main() { x }\n".into())));
    let locs = cersei_lsp::parse_locations(&r);
    assert_eq!(locs[0].range.start.character, 3);
    // `null` references → no location (not an error).
    let r = client
        .request(
            "textDocument/references",
            json!({}),
            Duration::from_secs(2),
            None,
        )
        .await
        .unwrap();
    assert!(cersei_lsp::parse_locations(&r).is_empty());
    // An RPC error stays an error.
    let e = client
        .request(
            "textDocument/hover",
            json!({}),
            Duration::from_secs(2),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(e, LspError::Rpc { code: -32601, .. }));
}

#[tokio::test]
async fn timeout_and_cancel_withdraw_the_request() {
    let mut cfg = MockConfig::new(json!({}), echo());
    cfg.delays
        .insert("textDocument/definition".into(), Duration::from_secs(5));
    let (client, mock) = connect(cfg).await;
    let e = client
        .request(
            "textDocument/definition",
            json!({}),
            Duration::from_millis(50),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(e, LspError::Timeout(_)));
    assert_eq!(client.pending_requests(), 0);

    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        c2.cancel();
    });
    let e = client
        .request(
            "textDocument/definition",
            json!({}),
            Duration::from_secs(10),
            Some(&cancel),
        )
        .await
        .unwrap_err();
    assert!(matches!(e, LspError::Cancelled));
    assert_eq!(client.pending_requests(), 0);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        mock.count("$/cancelRequest"),
        2,
        "both withdrawn on the server"
    );
}

#[tokio::test]
async fn crash_fails_pending_requests_at_once() {
    let mut cfg = MockConfig::new(json!({}), echo());
    cfg.delays
        .insert("textDocument/definition".into(), Duration::from_secs(30));
    let (client, mock) = connect(cfg).await;
    let c = Arc::clone(&client);
    let pending = tokio::spawn(async move {
        c.request(
            "textDocument/definition",
            json!({}),
            Duration::from_secs(30),
            None,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    let t = std::time::Instant::now();
    mock.crash().await;
    let r = pending.await.unwrap();
    assert!(matches!(r, Err(LspError::ServerExited)), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(!client.is_alive());
    assert!(matches!(
        client
            .request("x", json!({}), Duration::from_secs(1), None)
            .await,
        Err(LspError::ServerExited)
    ));
}

#[tokio::test]
async fn diagnostics_wait_for_the_version_sent() {
    let mut cfg = MockConfig::new(json!({"textDocumentSync": 1}), echo());
    // The server publishes for each version, one error per `bad`.
    cfg.publisher = Some(Arc::new(|_uri: &str, version: i64, text: &str| {
        let items: Vec<Value> = text
            .match_indices("bad")
            .map(|_| json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}, "severity": 1, "message": "bad"}))
            .collect();
        Some((Some(version), items))
    }));
    let (client, mock) = connect(cfg).await;
    let uri = "file:///ws/d.rs";
    client.did_open(uri, "rust", 1, "bad\n").await.unwrap();
    let e = client
        .wait_diagnostics(uri, Duration::from_secs(2), |e| e.version == Some(1))
        .await
        .unwrap();
    assert_eq!(e.items.len(), 1);
    client.did_change_full(uri, 2, "good\n").await.unwrap();
    let e = client
        .wait_diagnostics(uri, Duration::from_secs(2), |e| e.version == Some(2))
        .await
        .unwrap();
    assert!(e.items.is_empty(), "analyzed version 2: no error");

    // A version the server never reports on: the wait ends at the timeout
    // with the last (older) entry, which the caller can tell apart.
    let e = client
        .wait_diagnostics(uri, Duration::from_millis(80), |e| e.version == Some(9))
        .await
        .unwrap();
    assert_eq!(e.version, Some(2));
    // Publications are ordered.
    mock.publish(uri, None, vec![]).await;
    let seq_before = e.seq;
    let e = client
        .wait_diagnostics(uri, Duration::from_secs(2), |e| e.seq > seq_before)
        .await
        .unwrap();
    assert_eq!(e.version, None);
}
