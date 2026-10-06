//! Interoperability with the official SDK's server (rmcp 3.5), in process
//! over a byte stream (the stdio framing): both protocol eras, structured
//! results, tool errors, progress, total timeout and cancellation.

use cersei_mcp::{Era, McpClient, McpContent, McpError, McpLimits, McpServerConfig, ProtocolMode};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, NumberOrString, ProgressNotificationParam, ProgressToken,
    RequestMetaObject,
};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, Json, Peer, RoleServer, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Deserialize, JsonSchema)]
struct EchoReq {
    text: String,
}

#[derive(Deserialize, JsonSchema)]
struct Calc {
    a: i32,
    b: i32,
}

#[derive(Serialize, JsonSchema)]
struct CalcOut {
    sum: i32,
    product: i32,
}

#[derive(Clone)]
struct Srv {
    tool_router: ToolRouter<Self>,
    cancelled: Arc<AtomicBool>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Srv {}

#[tool_router(router = tool_router)]
impl Srv {
    fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            cancelled,
        }
    }

    #[tool(description = "Echo the text")]
    async fn echo(&self, p: Parameters<EchoReq>) -> String {
        format!("echo: {}", p.0.text)
    }

    #[tool(description = "Sum and product")]
    async fn calculate(&self, p: Parameters<Calc>) -> Result<Json<CalcOut>, String> {
        Ok(Json(CalcOut {
            sum: p.0.a + p.0.b,
            product: p.0.a * p.0.b,
        }))
    }

    #[tool(description = "A tool that reports a failure")]
    async fn fail(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(CallToolResult::error(vec![ContentBlock::text("disk full")]))
    }

    #[tool(description = "Slow, with progress")]
    async fn slow_progress(
        &self,
        meta: RequestMetaObject,
        client: Peer<RoleServer>,
    ) -> Result<String, rmcp::ErrorData> {
        let token = meta
            .get_progress_token()
            .ok_or_else(|| rmcp::ErrorData::invalid_params("no progress token", None))?;
        for step in 1..=4 {
            tokio::time::sleep(Duration::from_millis(70)).await;
            let _ = client
                .notify_progress(
                    ProgressNotificationParam::new(token.clone(), step as f64)
                        .with_total(4.0)
                        .with_message(format!("step {step}")),
                )
                .await;
        }
        Ok("done".into())
    }

    #[tool(description = "Slow, with progress for somebody else")]
    async fn unrelated_progress(&self, client: Peer<RoleServer>) -> String {
        for step in 0..6 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = client
                .notify_progress(ProgressNotificationParam::new(
                    ProgressToken(NumberOrString::Number(999_999)),
                    step as f64,
                ))
                .await;
        }
        "done".into()
    }

    #[tool(description = "Waits until cancelled")]
    async fn wait_cancel(&self, ctx: RequestContext<RoleServer>) -> String {
        tokio::select! {
            _ = ctx.ct.cancelled() => {
                self.cancelled.store(true, Ordering::SeqCst);
                "cancelled".into()
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => "finished".into(),
        }
    }
}

async fn pair(protocol: ProtocolMode, limits: McpLimits) -> (McpClient, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    let srv = Srv::new(cancelled.clone());
    tokio::spawn(async move {
        if let Ok(s) = srv.serve(server_io).await {
            let _ = s.waiting().await;
        }
    });
    let config = McpServerConfig::stdio("sdk", "unused", &[])
        .with_protocol(protocol)
        .with_limits(limits);
    let client = McpClient::connect_with(config, client_io)
        .await
        .expect("connected");
    (client, cancelled)
}

#[tokio::test]
async fn auto_speaks_2026_07_28_with_the_sdk_server() {
    let (c, _) = pair(ProtocolMode::Auto, McpLimits::default()).await;
    assert_eq!(c.info().era, Era::Modern);
    assert_eq!(c.info().protocol_version, "2026-07-28");
    let names: Vec<String> = c.tools().into_iter().map(|t| t.name).collect();
    for t in ["echo", "calculate", "fail", "slow_progress"] {
        assert!(names.contains(&t.to_string()), "{names:?}");
    }
    let r = c
        .call_tool("echo", Some(serde_json::json!({"text": "hi"})), None)
        .await
        .unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "echo: hi".into()
        }]
    );
    assert!(!r.is_error);

    let r = c
        .call_tool("calculate", Some(serde_json::json!({"a": 3, "b": 4})), None)
        .await
        .unwrap();
    assert_eq!(
        r.structured,
        Some(serde_json::json!({"sum": 7, "product": 12}))
    );

    let r = c.call_tool("fail", None, None).await.unwrap();
    assert!(r.is_error, "isError is kept");
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "disk full".into()
        }]
    );
}

#[tokio::test]
async fn legacy_mode_uses_the_initialize_handshake() {
    let (c, _) = pair(ProtocolMode::Legacy, McpLimits::default()).await;
    assert_eq!(c.info().era, Era::Legacy);
    assert_eq!(c.info().protocol_version, "2025-11-25");
    let r = c
        .call_tool("echo", Some(serde_json::json!({"text": "old"})), None)
        .await
        .unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "echo: old".into()
        }]
    );
}

#[tokio::test]
async fn own_progress_keeps_a_call_alive_but_never_past_the_total_limit() {
    let mut limits = McpLimits {
        request_timeout_ms: 150, // shorter than the whole call (~280 ms)
        max_total_timeout_ms: 3000,
        ..Default::default()
    };
    let (c, _) = pair(ProtocolMode::Auto, limits.clone()).await;
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let cb: cersei_mcp::ProgressFn = Arc::new(move |p, total, msg| s2.lock().push((p, total, msg)));
    let r = c.call_tool("slow_progress", None, Some(cb)).await.unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "done".into()
        }]
    );
    let seen = seen.lock().clone();
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert_eq!(seen[3], (4.0, Some(4.0), Some("step 4".into())));

    // Same progress, but a total limit below the call's duration.
    limits.max_total_timeout_ms = 200;
    let (c, _) = pair(ProtocolMode::Auto, limits).await;
    match c.call_tool("slow_progress", None, None).await {
        Err(McpError::Timeout { total: true, .. }) => {}
        other => panic!("expected the total limit: {other:?}"),
    }
}

#[tokio::test]
async fn someone_elses_progress_extends_nothing() {
    let limits = McpLimits {
        request_timeout_ms: 100,
        ..Default::default()
    };
    let (c, _) = pair(ProtocolMode::Auto, limits).await;
    match c.call_tool("unrelated_progress", None, None).await {
        Err(McpError::Timeout { total: false, .. }) => {}
        other => panic!("expected the idle timeout: {other:?}"),
    }
}

#[tokio::test]
async fn dropping_a_call_cancels_it_on_the_server() {
    let (c, cancelled) = pair(ProtocolMode::Auto, McpLimits::default()).await;
    let r = tokio::time::timeout(
        Duration::from_millis(150),
        c.call_tool("wait_cancel", None, None),
    )
    .await;
    assert!(r.is_err());
    for _ in 0..50 {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        cancelled.load(Ordering::SeqCst),
        "notifications/cancelled reached the server"
    );
    // The connection stays usable.
    let r = c
        .call_tool("echo", Some(serde_json::json!({"text": "after"})), None)
        .await
        .unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "echo: after".into()
        }]
    );
}
