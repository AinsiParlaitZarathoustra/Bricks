//! Interoperability over Streamable HTTP with the official SDK's server
//! (rmcp 3.5 `StreamableHttpService` on hyper, local port): stateless
//! 2026-07-28 with SSE responses and progress, and a legacy session-mode
//! server that answers only `initialize`.

use cersei_mcp::{Era, McpClient, McpContent, McpServerConfig};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    DiscoverResult, ErrorCode, ErrorData, ProgressNotificationParam, RequestMetaObject,
};
use rmcp::service::{MaybeSendFuture, RequestContext};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, Peer, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Deserialize, JsonSchema)]
struct EchoReq {
    text: String,
}

#[derive(Clone)]
struct Srv {
    tool_router: ToolRouter<Self>,
    legacy_only: bool,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Srv {
    fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<DiscoverResult, ErrorData>> + MaybeSendFuture + '_
    {
        let legacy_only = self.legacy_only;
        async move {
            if legacy_only {
                return Err(ErrorData::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    "Method not found",
                    None,
                ));
            }
            let _ = context;
            Ok(DiscoverResult::new(
                vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                self.get_info().capabilities,
            ))
        }
    }
}

#[tool_router(router = tool_router)]
impl Srv {
    #[tool(description = "Echo the text")]
    async fn echo(&self, p: Parameters<EchoReq>) -> String {
        format!("echo: {}", p.0.text)
    }

    #[tool(description = "Two progress steps")]
    async fn steps(&self, meta: RequestMetaObject, client: Peer<RoleServer>) -> String {
        if let Some(token) = meta.get_progress_token() {
            for i in 1..=2 {
                tokio::time::sleep(Duration::from_millis(30)).await;
                let _ = client
                    .notify_progress(
                        ProgressNotificationParam::new(token.clone(), i as f64).with_total(2.0),
                    )
                    .await;
            }
        }
        "stepped".into()
    }
}

async fn serve(legacy_only: bool) -> String {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(legacy_only)
        .with_json_response(false);
    let service: StreamableHttpService<Srv, LocalSessionManager> = StreamableHttpService::new(
        move || {
            Ok(Srv {
                tool_router: Srv::tool_router(),
                legacy_only,
            })
        },
        Default::default(),
        config,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let svc = hyper_util::service::TowerToHyperService::new(service.clone());
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn sdk_http_server_modern_with_sse_progress() {
    let url = serve(false).await;
    let c = McpClient::connect(McpServerConfig::http("sdk-http", url))
        .await
        .unwrap();
    assert_eq!(c.info().era, Era::Modern);
    assert_eq!(c.info().protocol_version, "2026-07-28");
    let r = c
        .call_tool("echo", Some(serde_json::json!({"text": "web"})), None)
        .await
        .unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "echo: web".into()
        }]
    );
    let seen = Arc::new(parking_lot::Mutex::new(0));
    let s2 = seen.clone();
    let r = c
        .call_tool(
            "steps",
            None,
            Some(Arc::new(move |_, _, _| *s2.lock() += 1)),
        )
        .await
        .unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "stepped".into()
        }]
    );
    assert_eq!(
        *seen.lock(),
        2,
        "progress travelled on the request's SSE stream"
    );
}

#[tokio::test]
async fn sdk_http_legacy_session_server() {
    let url = serve(true).await;
    let c = McpClient::connect(McpServerConfig::http("sdk-legacy", url))
        .await
        .unwrap();
    assert_eq!(c.info().era, Era::Legacy);
    let r = c
        .call_tool("echo", Some(serde_json::json!({"text": "old web"})), None)
        .await
        .unwrap();
    assert_eq!(
        r.content,
        vec![McpContent::Text {
            text: "echo: old web".into()
        }]
    );
    c.close().await;
}
