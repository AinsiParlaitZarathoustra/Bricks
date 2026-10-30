//! An image an MCP tool returns reaches the model the way a ViewImage image
//! does (see tool_images.rs): inside the tool result for a protocol that
//! carries media there (Anthropic messages, `ToolResult`), and as a note that
//! it was not shown for a model that reads no image (`None`). A real MCP
//! server over HTTP answers the call; the scripted model records its requests.

use async_trait::async_trait;
use base64::Engine as _;
use bricks_agent::Agent;
use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
use bricks_mcp::McpServerConfig;
use bricks_provider::{CompletionRequest, CompletionStream, ImageTransport, ModelInfo, Provider};
use bricks_testkit::{Reply as HttpReply, TestServer};
use bricks_types::{ContentBlock, Message, MessageContent, Result, ToolResultContent};
use serde_json::{Value, json};
use std::sync::Arc;

/// A 1×1 PNG: 70 bytes once decoded.
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/// The scripted model, with the image transport of a chosen protocol.
struct WithTransport {
    inner: ScriptedProvider,
    transport: ImageTransport,
}

#[async_trait]
impl Provider for WithTransport {
    fn name(&self) -> &str {
        "with-transport"
    }
    fn context_window(&self, _model: &str) -> u64 {
        100_000
    }
    fn model_info(&self) -> Option<ModelInfo> {
        Provider::model_info(&self.inner)
    }
    fn image_transport(&self) -> ImageTransport {
        self.transport
    }
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream> {
        Provider::complete(&self.inner, request).await
    }
}

/// An MCP server (Streamable HTTP, 2026 protocol) with one tool, `snap`, whose
/// result is a PNG followed by a caption.
async fn mcp_server() -> TestServer {
    TestServer::start(|req| async move {
        let m: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let id = m["id"].clone();
        let ok = |result: Value| {
            HttpReply::json(
                200,
                &json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
            )
        };
        match m["method"].as_str().unwrap_or("") {
            "server/discover" => ok(json!({"resultType": "complete", "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}}, "ttlMs": 0, "cacheScope": "private"})),
            "tools/list" => ok(json!({"resultType": "complete", "ttlMs": 0, "cacheScope": "private", "tools": [
                {"name": "snap", "description": "Take a picture", "inputSchema": {"type": "object", "properties": {}},
                 "annotations": {"readOnlyHint": true}}]})),
            "tools/call" => ok(json!({"resultType": "complete", "content": [
                {"type": "image", "data": PNG_B64, "mimeType": "image/png"},
                {"type": "text", "text": "the snapshot"}]})),
            _ => HttpReply::new(202),
        }
    })
    .await
}

fn agent(
    dir: &std::path::Path,
    transport: ImageTransport,
    script: &Arc<Script>,
    server: &TestServer,
) -> Agent {
    Agent::builder()
        .provider(WithTransport {
            inner: ScriptedProvider::new("a", script.clone()),
            transport,
        })
        .model("a")
        .working_dir(dir)
        .mcp_server(McpServerConfig::http("pic", server.url("/mcp")))
        .build()
        .expect("build agent")
}

/// The user turn that carries the results of the round (the second request's last one).
fn round_turn(messages: &[Message]) -> &[ContentBlock] {
    let turn = messages
        .iter()
        .rev()
        .find(|m| {
            matches!(&m.content, MessageContent::Blocks(b)
                if b.iter().any(|x| matches!(x, ContentBlock::ToolResult { .. })))
        })
        .expect("the round's results are in the request");
    match &turn.content {
        MessageContent::Blocks(b) => b,
        MessageContent::Text(_) => panic!("the round is a block turn"),
    }
}

fn result_of<'a>(blocks: &'a [ContentBlock], id: &str) -> (&'a ToolResultContent, Option<bool>) {
    blocks
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } if tool_use_id == id => Some((content, *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no result for {id} in {blocks:?}"))
}

fn text_of(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text(t) => t.clone(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn image_data(block: &ContentBlock) -> Option<(String, String)> {
    match block {
        ContentBlock::Image { source } => Some((
            source.media_type.clone().unwrap_or_default(),
            source.data.clone().unwrap_or_default(),
        )),
        _ => None,
    }
}

#[tokio::test]
async fn an_mcp_image_sits_inside_its_result_for_a_protocol_that_carries_media_there() {
    let server = mcp_server().await;
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(vec![
        Reply::tool("s1", "mcp__pic__snap", json!({})),
        Reply::text("I see it"),
    ]);
    let a = agent(dir.path(), ImageTransport::ToolResult, &script, &server);
    a.run("take a picture").await.unwrap();

    let reqs = script.requests();
    assert!(
        reqs[0].1.tools.iter().any(|t| t.name == "mcp__pic__snap"),
        "the model is offered the MCP tool"
    );
    let turn = round_turn(&reqs[1].1.messages);
    assert_eq!(
        turn.len(),
        1,
        "the image is inside the result, nothing follows it"
    );
    let (content, is_error) = result_of(turn, "s1");
    assert_eq!(is_error, Some(false));
    let ToolResultContent::Blocks(inner) = content else {
        panic!("an image result is a block content: {content:?}");
    };
    assert_eq!(inner.len(), 2, "{inner:?}");
    let text = text_of(content);
    assert!(text.contains("[image image/png, 70 bytes]"), "{text}");
    assert!(text.contains("the snapshot"), "{text}");
    let (media, data) = image_data(&inner[1]).expect("the image follows the text");
    assert_eq!(media, "image/png");
    assert_eq!(data, PNG_B64, "the model receives the server's image");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&data)
            .unwrap()
            .len(),
        70
    );
    a.close().await;
}

#[tokio::test]
async fn an_mcp_image_is_described_as_not_shown_for_a_model_that_reads_no_image() {
    let server = mcp_server().await;
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(vec![
        Reply::tool("s1", "mcp__pic__snap", json!({})),
        Reply::text("ok"),
    ]);
    let a = agent(dir.path(), ImageTransport::None, &script, &server);
    a.run("take a picture").await.unwrap();

    let reqs = script.requests();
    let turn = round_turn(&reqs[1].1.messages);
    assert_eq!(
        turn.len(),
        1,
        "no image block reaches a model that reads none: {turn:?}"
    );
    let (content, is_error) = result_of(turn, "s1");
    assert_eq!(is_error, Some(false));
    assert!(matches!(content, ToolResultContent::Text(_)), "{content:?}");
    let text = text_of(content);
    assert!(text.contains("[image image/png, 70 bytes]"), "{text}");
    assert!(text.contains("the snapshot"), "{text}");
    assert!(
        text.ends_with("[the image was not shown: the current model does not accept images]"),
        "{text}"
    );
    a.close().await;
}
