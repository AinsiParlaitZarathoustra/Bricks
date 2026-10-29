//! Images a tool shows the model, through the runner's round: the image travels
//! as the model's protocol allows. With `ToolResult` it sits inside the result;
//! with `UserTurn` the results come first, then the labelled images, in the same
//! user turn; with `None` the result says the image was not shown. The scripted
//! model records each request, so the assertions read what it received.

use async_trait::async_trait;
use base64::Engine as _;
use bricks_agent::Agent;
use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
use bricks_provider::{CompletionRequest, CompletionStream, ImageTransport, ModelInfo, Provider};
use bricks_types::{ContentBlock, Message, MessageContent, Result, ToolResultContent};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// A PNG of `w`×`h` pixels. `padding` extra bytes make each file's size distinct.
fn png(w: u32, h: u32, padding: usize) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0]);
    b.extend(std::iter::repeat_n(0u8, padding));
    b
}

fn write_png(dir: &Path, name: &str, w: u32, h: u32, padding: usize) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, png(w, h, padding)).unwrap();
    path
}

/// An agent whose model is scripted with `replies` and whose image transport is `transport`.
fn agent(dir: &Path, transport: ImageTransport, script: &Arc<Script>) -> Agent {
    Agent::builder()
        .provider(WithTransport {
            inner: ScriptedProvider::new("a", script.clone()),
            transport,
        })
        .model("a")
        .working_dir(dir)
        .tools(bricks_tools::filesystem())
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
async fn with_tool_result_transport_the_image_sits_inside_its_result() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_png(dir.path(), "shot.png", 640, 480, 0);
    let bytes = std::fs::read(&file).unwrap();
    let script = Script::new(vec![
        Reply::tool("v1", "ViewImage", json!({ "file_path": file.display().to_string() })),
        Reply::text("I see it"),
    ]);
    let a = agent(dir.path(), ImageTransport::ToolResult, &script);
    let out = a.run("look").await.unwrap();

    let reqs = script.requests();
    let turn = round_turn(&reqs[1].1.messages);
    assert_eq!(turn.len(), 1, "the image is inside the result, nothing follows it");
    let (content, is_error) = result_of(turn, "v1");
    assert_eq!(is_error, Some(false));
    let ToolResultContent::Blocks(inner) = content else {
        panic!("an image result is a block content: {content:?}");
    };
    assert_eq!(inner.len(), 2);
    assert!(text_of(content).contains("image/png, 640×480"), "{}", text_of(content));
    let (media, data) = image_data(&inner[1]).expect("the image follows the text");
    assert_eq!(media, "image/png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(&data).unwrap(),
        bytes,
        "the model receives the file's bytes"
    );

    // The record of the call names the image, never its data.
    let call = out.tool_calls.iter().find(|c| c.id == "v1").unwrap();
    assert!(!call.result.contains(&data));
    // The raw history keeps the same image inside the result.
    let raw = a.raw_history();
    let raw_turn = round_turn(&raw);
    assert_eq!(raw_turn.len(), 1);
    let (raw_content, _) = result_of(raw_turn, "v1");
    assert!(matches!(raw_content, ToolResultContent::Blocks(b) if b.len() == 2));
}

#[tokio::test]
async fn with_user_turn_transport_the_result_is_text_and_the_labelled_image_follows_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_png(dir.path(), "plan.png", 320, 200, 5);
    let bytes_len = std::fs::read(&file).unwrap().len();
    let script = Script::new(vec![
        Reply::tool("v1", "ViewImage", json!({ "file_path": file.display().to_string() })),
        Reply::text("I see it"),
    ]);
    let a = agent(dir.path(), ImageTransport::UserTurn, &script);
    a.run("look").await.unwrap();

    let reqs = script.requests();
    let turn = round_turn(&reqs[1].1.messages);
    assert_eq!(turn.len(), 3, "result, label, image: {turn:?}");
    let (content, _) = result_of(turn, "v1");
    assert!(matches!(content, ToolResultContent::Text(_)), "{content:?}");
    let ContentBlock::Text { text: label } = &turn[1] else {
        panic!("the label follows the result: {:?}", turn[1]);
    };
    let expected_path = file.display().to_string();
    assert_eq!(
        label,
        &format!("[image from tool call v1] {expected_path}, image/png, {bytes_len} bytes")
    );
    assert!(image_data(&turn[2]).is_some(), "the image follows its label");

    // The raw history carries the same turn.
    let raw = a.raw_history();
    let raw_turn = round_turn(&raw);
    assert_eq!(raw_turn.len(), 3);
    assert!(image_data(&raw_turn[2]).is_some());
}

#[tokio::test]
async fn with_no_transport_the_result_says_the_image_was_not_shown() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_png(dir.path(), "hidden.png", 16, 16, 0);
    let script = Script::new(vec![
        Reply::tool("v1", "ViewImage", json!({ "file_path": file.display().to_string() })),
        Reply::text("ok"),
    ]);
    agent(dir.path(), ImageTransport::None, &script)
        .run("look")
        .await
        .unwrap();

    let reqs = script.requests();
    let turn = round_turn(&reqs[1].1.messages);
    assert_eq!(turn.len(), 1, "no image block reaches a model that does not accept one");
    let (content, _) = result_of(turn, "v1");
    let text = text_of(content);
    assert!(
        text.ends_with("[the image was not shown: the current model does not accept images]"),
        "{text}"
    );
    assert!(text.contains("image/png, 16×16"), "the image's own text stays: {text}");
}

#[tokio::test]
async fn several_images_follow_every_result_of_the_round_in_call_order() {
    let dir = tempfile::tempdir().unwrap();
    let first = write_png(dir.path(), "one.png", 10, 10, 0);
    let second = write_png(dir.path(), "two.png", 20, 20, 9);
    std::fs::write(dir.path().join("notes.txt"), "un texte").unwrap();
    let script = Script::new(vec![
        Reply::tools(vec![
            ("v1", "ViewImage", json!({ "file_path": first.display().to_string() })),
            ("r1", "Read", json!({ "file_path": "notes.txt" })),
            ("v2", "ViewImage", json!({ "file_path": second.display().to_string() })),
        ]),
        Reply::text("ok"),
    ]);
    agent(dir.path(), ImageTransport::UserTurn, &script)
        .run("look")
        .await
        .unwrap();

    let reqs = script.requests();
    let turn = round_turn(&reqs[1].1.messages);
    // Three results in call order, then label+image for each image in call order.
    assert_eq!(turn.len(), 7, "{turn:?}");
    let ids: Vec<&str> = turn[..3]
        .iter()
        .map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.as_str(),
            other => panic!("a result expected first: {other:?}"),
        })
        .collect();
    assert_eq!(ids, ["v1", "r1", "v2"]);
    let (text_read, _) = result_of(turn, "r1");
    assert!(matches!(text_read, ToolResultContent::Text(t) if t.contains("un texte")));
    let labels: Vec<String> = turn[3..]
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(labels.len(), 2);
    assert!(labels[0].starts_with("[image from tool call v1]"), "{labels:?}");
    assert!(labels[1].starts_with("[image from tool call v2]"), "{labels:?}");
    let sizes: Vec<(String, String)> = turn[3..]
        .iter()
        .filter_map(image_data)
        .collect();
    assert_eq!(sizes.len(), 2);
    assert_ne!(sizes[0].1, sizes[1].1, "each label is followed by its own image");
}

#[tokio::test]
async fn a_view_that_fails_carries_no_image_on_any_transport() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fake.png"), "not an image").unwrap();
    for transport in [
        ImageTransport::ToolResult,
        ImageTransport::UserTurn,
        ImageTransport::None,
    ] {
        let script = Script::new(vec![
            Reply::tool("v1", "ViewImage", json!({ "file_path": "fake.png" })),
            Reply::text("ok"),
        ]);
        agent(dir.path(), transport, &script).run("look").await.unwrap();
        let reqs = script.requests();
        let turn = round_turn(&reqs[1].1.messages);
        assert_eq!(turn.len(), 1, "{transport:?}: {turn:?}");
        let (content, is_error) = result_of(turn, "v1");
        assert_eq!(is_error, Some(true), "{transport:?}");
        assert!(matches!(content, ToolResultContent::Text(_)), "{transport:?}");
    }
}
