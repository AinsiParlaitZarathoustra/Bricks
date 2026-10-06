//! `anthropic_messages` adapter: `POST <base>/messages`.
//!
//! Request: `system` as cacheable text blocks, `messages` whose content is a
//! list of typed blocks, tools as `{name, description, input_schema}`.
//! Response and stream: `message_start`, `content_block_*`, `message_delta`,
//! `message_stop`.
//!
//! Extended thinking is *not* decided here. What a model sends to enable,
//! size or disable thinking (`thinking`, `output_config`, …) comes from the
//! model's `parameters` and the selected reasoning profile. This adapter only
//! carries what the protocol requires to keep a conversation valid: thinking
//! blocks with their signatures and redacted-thinking data are echoed back.

use super::*;
use crate::pricing::CACHE_WRITE_BY_VARIANT;
use crate::CompletionRequest;
use base64::Engine;
use cersei_types::{DocumentSource, ImageSource, Result, StopReason, StreamEvent, Usage};
use serde_json::{json, Map, Value};

// ─── Request ─────────────────────────────────────────────────────────────────

pub(crate) fn build_body(ctx: &BuildCtx, req: &CompletionRequest) -> Result<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for msg in req.messages.iter().filter(|m| m.role != Role::System) {
        let content = convert_content(msg)?;
        if content.is_empty() {
            continue; // the API rejects empty content
        }
        messages.push(json!({ "role": role_str(msg.role), "content": content }));
    }

    let mut body = json!({
        "model": ctx.api_model,
        "max_tokens": ctx.max_tokens(req.max_tokens),
        "messages": messages,
        "stream": ctx.stream,
    });

    if let Some(system) = &req.system {
        let blocks = system_blocks(system, ctx.compat.prompt_cache_markers);
        if !blocks.is_empty() {
            body["system"] = Value::Array(blocks);
        }
    }

    if !req.tools.is_empty() {
        let mut tools =
            crate::adapt::adapt_tools(&req.tools, crate::adapt::SchemaDialect::AnthropicNative);
        if ctx.compat.prompt_cache_markers {
            if let Some(last) = tools.last_mut() {
                last["cache_control"] = json!({ "type": "ephemeral" });
            }
        }
        body["tools"] = Value::Array(tools);
        if req.options.get::<String>("tool_choice").as_deref() == Some("required") {
            body["tool_choice"] = json!({ "type": "any" });
        }
    }
    if !req.stop_sequences.is_empty() {
        body["stop_sequences"] = json!(req.stop_sequences);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    Ok(body)
}

/// System prompt as text blocks. With the engine's stable/dynamic boundary the
/// cache breakpoint goes on the stable half only, so per-turn tail changes do
/// not invalidate the cached prefix. The marker never reaches the wire.
fn system_blocks(system: &str, cache: bool) -> Vec<Value> {
    let mark = |mut v: Value| {
        if cache {
            v["cache_control"] = json!({ "type": "ephemeral" });
        }
        v
    };
    match system.split_once(SYSTEM_PROMPT_DYNAMIC_BOUNDARY) {
        Some((stable, dynamic)) => {
            let dynamic = dynamic.replace(SYSTEM_PROMPT_DYNAMIC_BOUNDARY, "");
            let mut blocks = Vec::new();
            if !stable.trim().is_empty() {
                blocks.push(mark(json!({ "type": "text", "text": stable })));
            }
            if !dynamic.trim().is_empty() {
                let block = json!({ "type": "text", "text": dynamic });
                blocks.push(if blocks.is_empty() {
                    mark(block)
                } else {
                    block
                });
            }
            blocks
        }
        None if system.trim().is_empty() => Vec::new(),
        None => vec![mark(json!({ "type": "text", "text": system }))],
    }
}

fn convert_content(msg: &Message) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for block in blocks_of(msg) {
        if let Some(v) = convert_block(&block)? {
            out.push(v);
        }
    }
    Ok(out)
}

fn convert_block(block: &ContentBlock) -> Result<Option<Value>> {
    Ok(Some(match block {
        // The API rejects empty text blocks.
        ContentBlock::Text { text } if text.is_empty() => return Ok(None),
        ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
        ContentBlock::Image { source } => {
            json!({ "type": "image", "source": image_source(source)? })
        }
        ContentBlock::Document {
            source,
            title,
            context,
            citations,
        } => {
            let mut v = json!({ "type": "document", "source": document_source(source)? });
            if let Some(t) = title {
                v["title"] = json!(t);
            }
            if let Some(c) = context {
                v["context"] = json!(c);
            }
            if let Some(c) = citations {
                v["citations"] = json!({ "enabled": c.enabled });
            }
            v
        }
        ContentBlock::ToolUse { id, name, input } => {
            json!({ "type": "tool_use", "id": id, "name": name, "input": input })
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let content = match content {
                cersei_types::ToolResultContent::Text(t) => json!(t),
                cersei_types::ToolResultContent::Blocks(blocks) => {
                    let mut inner = Vec::new();
                    for b in blocks {
                        if let Some(v) = convert_block(b)? {
                            inner.push(v);
                        }
                    }
                    Value::Array(inner)
                }
            };
            let mut v =
                json!({ "type": "tool_result", "tool_use_id": tool_use_id, "content": content });
            if let Some(e) = is_error {
                v["is_error"] = json!(e);
            }
            v
        }
        // A signature is what makes a thinking block valid history; one that
        // never had a signature (from another protocol) cannot be echoed.
        ContentBlock::Thinking {
            thinking,
            signature,
        } if !signature.is_empty() => {
            json!({ "type": "thinking", "thinking": thinking, "signature": signature })
        }
        ContentBlock::Thinking { .. } => return Ok(None),
        ContentBlock::RedactedThinking { data } => {
            json!({ "type": "redacted_thinking", "data": data })
        }
        ContentBlock::Audio { .. } => {
            return Err(unsupported("anthropic_messages cannot carry audio input"))
        }
        ContentBlock::Video { .. } => {
            return Err(unsupported("anthropic_messages cannot carry video input"))
        }
        ContentBlock::ProtocolItem { .. } | ContentBlock::Opaque => return Ok(None),
    }))
}

fn image_source(s: &ImageSource) -> Result<Value> {
    if let Some(id) = &s.file_id {
        return Ok(json!({ "type": "file", "file_id": id }));
    }
    if let Some(data) = &s.data {
        return Ok(json!({
            "type": "base64",
            "media_type": s.media_type.as_deref().unwrap_or("image/png"),
            "data": data,
        }));
    }
    if let Some(url) = &s.url {
        return Ok(json!({ "type": "url", "url": url }));
    }
    Err(unsupported("an image block has no data, url or file_id"))
}

fn document_source(s: &DocumentSource) -> Result<Value> {
    if let Some(id) = &s.file_id {
        return Ok(json!({ "type": "file", "file_id": id }));
    }
    if let Some(url) = &s.url {
        return Ok(json!({ "type": "url", "url": url }));
    }
    let Some(data) = &s.data else {
        return Err(unsupported("a document block has no data, url or file_id"));
    };
    match s.media_type.as_deref().unwrap_or("application/pdf") {
        "application/pdf" => Ok(json!({ "type": "base64", "media_type": "application/pdf", "data": data })),
        // Plain text documents travel as text, not base64.
        "text/plain" => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| unsupported("a text/plain document is not valid base64"))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| unsupported("a text/plain document is not valid UTF-8"))?;
            Ok(json!({ "type": "text", "media_type": "text/plain", "data": text }))
        }
        other => Err(unsupported(format!(
            "anthropic_messages carries inline documents as application/pdf or text/plain; `{other}` \
             is neither (send it by URL or file id if the server supports it)"
        ))),
    }
}

// ─── Usage ───────────────────────────────────────────────────────────────────

/// Normalize an Anthropic `usage` object. `input_tokens` already *excludes*
/// cache reads and writes, and `output_tokens` includes thinking tokens.
/// Cache writes by retention (`cache_creation.ephemeral_5m_input_tokens`, …)
/// are kept so each can be priced with its own price.
pub(crate) fn parse_usage(u: &Value) -> Usage {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let mut usage = Usage {
        input_tokens: n(&u["input_tokens"]),
        output_tokens: n(&u["output_tokens"]),
        cache_creation_input_tokens: n(&u["cache_creation_input_tokens"]),
        cache_read_input_tokens: n(&u["cache_read_input_tokens"]),
        ..Default::default()
    };
    if let Some(by_ttl) = u["cache_creation"].as_object() {
        let mut variants = Map::new();
        for (key, val) in by_ttl {
            // `ephemeral_5m_input_tokens` -> `5m`
            if let Some(label) = key
                .strip_prefix("ephemeral_")
                .and_then(|k| k.strip_suffix("_input_tokens"))
            {
                if let Some(tokens) = val.as_u64().filter(|t| *t > 0) {
                    variants.insert(label.to_string(), json!(tokens));
                }
            }
        }
        if !variants.is_empty() {
            usage.provider_usage = json!({ CACHE_WRITE_BY_VARIANT: Value::Object(variants) });
        }
    }
    usage
}

fn stop_reason(s: &str) -> Option<StopReason> {
    match s {
        "end_turn" => Some(StopReason::EndTurn),
        "max_tokens" => Some(StopReason::MaxTokens),
        "tool_use" => Some(StopReason::ToolUse),
        "stop_sequence" => Some(StopReason::StopSequence),
        "refusal" => Some(StopReason::ContentFilter),
        _ => None,
    }
}

// ─── Stream ──────────────────────────────────────────────────────────────────

/// Translate one SSE event (`event:` name + JSON `data`) into a stream event.
pub(crate) fn parse_sse_event(event: Option<&str>, data: &str) -> Option<StreamEvent> {
    let json: Value = serde_json::from_str(data).ok()?;
    // The `event:` line and the payload's `type` always agree; prefer the
    // payload so servers that omit `event:` still work.
    let kind = json["type"].as_str().or(event).unwrap_or("");
    let index = json["index"].as_u64().unwrap_or(0) as usize;
    match kind {
        "message_start" => {
            let msg = &json["message"];
            Some(StreamEvent::MessageStart {
                id: msg["id"].as_str().unwrap_or("").to_string(),
                model: msg["model"].as_str().unwrap_or("").to_string(),
                usage: msg["usage"].is_object().then(|| parse_usage(&msg["usage"])),
            })
        }
        "content_block_start" => {
            let block = &json["content_block"];
            let block_type = block["type"].as_str().unwrap_or("text").to_string();
            // A redacted thinking block arrives complete, in the start event.
            if block_type == "redacted_thinking" {
                return Some(StreamEvent::ContentBlockStart {
                    index,
                    block_type,
                    id: block["data"].as_str().map(String::from),
                    name: None,
                });
            }
            Some(StreamEvent::ContentBlockStart {
                index,
                block_type,
                id: block["id"].as_str().map(String::from),
                name: block["name"].as_str().map(String::from),
            })
        }
        "content_block_delta" => {
            let d = &json["delta"];
            match d["type"].as_str().unwrap_or("") {
                "text_delta" => Some(StreamEvent::TextDelta {
                    index,
                    text: d["text"].as_str().unwrap_or("").to_string(),
                }),
                "input_json_delta" => Some(StreamEvent::InputJsonDelta {
                    index,
                    partial_json: d["partial_json"].as_str().unwrap_or("").to_string(),
                }),
                "thinking_delta" => Some(StreamEvent::ThinkingDelta {
                    index,
                    thinking: d["thinking"].as_str().unwrap_or("").to_string(),
                }),
                // Must be captured and echoed back, or adaptive models reject
                // the next turn.
                "signature_delta" => Some(StreamEvent::SignatureDelta {
                    index,
                    signature: d["signature"].as_str().unwrap_or("").to_string(),
                }),
                _ => None,
            }
        }
        "content_block_stop" => Some(StreamEvent::ContentBlockStop { index }),
        "message_delta" => Some(StreamEvent::MessageDelta {
            stop_reason: json["delta"]["stop_reason"].as_str().and_then(stop_reason),
            usage: json["usage"]
                .is_object()
                .then(|| parse_usage(&json["usage"])),
        }),
        "message_stop" => Some(StreamEvent::MessageStop),
        "ping" => Some(StreamEvent::Ping),
        "error" => Some(StreamEvent::Error {
            message: json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string(),
        }),
        _ => None,
    }
}

/// Convert a complete (non-streamed) message into stream events.
pub(crate) fn response_to_events(json: &Value) -> Result<Vec<StreamEvent>> {
    if json["type"].as_str() == Some("error") {
        return Err(CerseiError::Provider(
            json["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
                .to_string(),
        ));
    }
    let mut out = vec![StreamEvent::MessageStart {
        id: json["id"].as_str().unwrap_or("").to_string(),
        model: json["model"].as_str().unwrap_or("").to_string(),
        usage: None,
    }];
    for (i, block) in json["content"].as_array().into_iter().flatten().enumerate() {
        let kind = block["type"].as_str().unwrap_or("text");
        match kind {
            "text" => {
                out.push(StreamEvent::ContentBlockStart {
                    index: i,
                    block_type: "text".into(),
                    id: None,
                    name: None,
                });
                out.push(StreamEvent::TextDelta {
                    index: i,
                    text: block["text"].as_str().unwrap_or("").to_string(),
                });
            }
            "tool_use" => {
                out.push(StreamEvent::ContentBlockStart {
                    index: i,
                    block_type: "tool_use".into(),
                    id: block["id"].as_str().map(String::from),
                    name: block["name"].as_str().map(String::from),
                });
                out.push(StreamEvent::InputJsonDelta {
                    index: i,
                    partial_json: block["input"].to_string(),
                });
            }
            "thinking" => {
                out.push(StreamEvent::ContentBlockStart {
                    index: i,
                    block_type: "thinking".into(),
                    id: None,
                    name: None,
                });
                out.push(StreamEvent::ThinkingDelta {
                    index: i,
                    thinking: block["thinking"].as_str().unwrap_or("").to_string(),
                });
                if let Some(sig) = block["signature"].as_str() {
                    out.push(StreamEvent::SignatureDelta {
                        index: i,
                        signature: sig.to_string(),
                    });
                }
            }
            "redacted_thinking" => {
                out.push(StreamEvent::ContentBlockStart {
                    index: i,
                    block_type: "redacted_thinking".into(),
                    id: block["data"].as_str().map(String::from),
                    name: None,
                });
            }
            _ => continue,
        }
        out.push(StreamEvent::ContentBlockStop { index: i });
    }
    out.push(StreamEvent::MessageDelta {
        stop_reason: json["stop_reason"].as_str().and_then(stop_reason),
        usage: json["usage"]
            .is_object()
            .then(|| parse_usage(&json["usage"])),
    });
    out.push(StreamEvent::MessageStop);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Compat;
    use crate::StreamAccumulator;
    use cersei_types::{ToolDefinition, ToolResultContent};

    fn body_with(r: &CompletionRequest, compat: Compat) -> Result<Value> {
        let c = Compat::resolve(&Compat::default(), &compat);
        build_body(
            &BuildCtx {
                api_model: "vendor-claude",
                compat: &c,
                max_output_tokens: 4096,
                stream: true,
            },
            r,
        )
    }

    fn body(r: &CompletionRequest) -> Result<Value> {
        body_with(r, Compat::default())
    }

    fn sse(pairs: &[(&str, Value)]) -> cersei_types::Result<crate::CompletionResponse> {
        let mut acc = StreamAccumulator::new();
        for (ev, data) in pairs {
            if let Some(e) = parse_sse_event(Some(ev), &data.to_string()) {
                acc.process_event(e);
            }
        }
        acc.into_response()
    }

    #[test]
    fn request_basics_and_no_automatic_thinking() {
        let mut r = CompletionRequest::new("m");
        r.system = Some("sys".into());
        r.messages.push(Message::user("hello"));
        r.max_tokens = 99_999;
        r.temperature = Some(0.4);
        let b = body(&r).unwrap();
        assert_eq!(b["model"], "vendor-claude");
        assert_eq!(b["max_tokens"], 4096);
        assert_eq!(
            b["system"][0],
            json!({"type":"text","text":"sys","cache_control":{"type":"ephemeral"}})
        );
        assert_eq!(
            b["messages"][0],
            json!({"role":"user","content":[{"type":"text","text":"hello"}]})
        );
        assert!(
            b.get("thinking").is_none() && b.get("output_config").is_none(),
            "profiles decide, not the adapter"
        );
        assert!((b["temperature"].as_f64().unwrap() - 0.4).abs() < 1e-6);
    }

    #[test]
    fn cache_markers_follow_the_boundary_and_the_compat_switch() {
        let mut r = CompletionRequest::new("m");
        r.system = Some(format!("stable{}tail", SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
        r.tools.push(ToolDefinition {
            name: "a".into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
        });
        r.tools.push(ToolDefinition {
            name: "b".into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
        });
        let b = body(&r).unwrap();
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(b["system"][1].get("cache_control").is_none());
        assert!(!b.to_string().contains(SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
        assert!(b["tools"][0].get("cache_control").is_none());
        assert_eq!(b["tools"][1]["cache_control"]["type"], "ephemeral");
        let off = body_with(
            &r,
            Compat {
                prompt_cache_markers: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!off.to_string().contains("cache_control"));
    }

    #[test]
    fn tool_exchange_keeps_thinking_signature_and_redacted_data() {
        let mut r = CompletionRequest::new("m");
        r.messages.push(Message::user("go"));
        r.messages.push(Message::assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "plan".into(),
                signature: "SIG".into(),
            },
            ContentBlock::RedactedThinking {
                data: "OPAQUE".into(),
            },
            ContentBlock::Thinking {
                thinking: "unsigned".into(),
                signature: String::new(),
            },
            ContentBlock::Text {
                text: String::new(),
            },
            ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "read".into(),
                input: json!({"p": 1}),
            },
        ]));
        r.messages
            .push(Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tu_1".into(),
                content: ToolResultContent::Text("out".into()),
                is_error: Some(true),
            }]));
        let m = body(&r).unwrap()["messages"].clone();
        assert_eq!(
            m[1]["content"],
            json!([
                {"type":"thinking","thinking":"plan","signature":"SIG"},
                {"type":"redacted_thinking","data":"OPAQUE"},
                {"type":"tool_use","id":"tu_1","name":"read","input":{"p":1}},
            ]),
            "unsigned thinking and empty text are not sent"
        );
        assert_eq!(
            m[2]["content"][0],
            json!({"type":"tool_result","tool_use_id":"tu_1","content":"out","is_error":true})
        );
    }

    #[test]
    fn media_and_tool_result_media() {
        let mut r = CompletionRequest::new("m");
        r.messages.push(Message::user_blocks(vec![
            ContentBlock::image_base64("image/jpeg", "QUJD"),
            ContentBlock::image_url("https://x/y.png"),
            ContentBlock::image_file_id("file-1"),
            ContentBlock::document_base64("application/pdf", "JVBE"),
            ContentBlock::document_bytes("text/plain", b"hello text"),
            ContentBlock::document_url("https://x/d.pdf"),
            ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: ToolResultContent::Blocks(vec![
                    ContentBlock::Text { text: "see".into() },
                    ContentBlock::image_base64("image/png", "AA=="),
                ]),
                is_error: None,
            },
        ]));
        let c = body(&r).unwrap()["messages"][0]["content"].clone();
        assert_eq!(
            c[0]["source"],
            json!({"type":"base64","media_type":"image/jpeg","data":"QUJD"})
        );
        assert_eq!(
            c[1]["source"],
            json!({"type":"url","url":"https://x/y.png"})
        );
        assert_eq!(c[2]["source"], json!({"type":"file","file_id":"file-1"}));
        assert_eq!(c[3]["source"]["media_type"], "application/pdf");
        assert_eq!(
            c[4]["source"],
            json!({"type":"text","media_type":"text/plain","data":"hello text"})
        );
        assert_eq!(
            c[5]["source"],
            json!({"type":"url","url":"https://x/d.pdf"})
        );
        assert_eq!(c[6]["content"][1]["type"], "image");
    }

    #[test]
    fn unsupported_media_errors() {
        for bad in [
            ContentBlock::audio_base64("audio/wav", "AA=="),
            ContentBlock::video_url("https://x"),
            ContentBlock::document_bytes("application/zip", b"PK"),
        ] {
            let mut r = CompletionRequest::new("m");
            r.messages.push(Message::user_blocks(vec![bad]));
            assert!(matches!(body(&r), Err(CerseiError::Unsupported(_))));
        }
    }

    #[test]
    fn forced_tool_choice_and_stop_sequences() {
        let mut r = CompletionRequest::new("m");
        r.tools.push(ToolDefinition {
            name: "a".into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
        });
        r.options.set("tool_choice", "required");
        r.stop_sequences = vec!["X".into()];
        let b = body(&r).unwrap();
        assert_eq!(b["tool_choice"], json!({"type":"any"}));
        assert_eq!(b["stop_sequences"], json!(["X"]));
    }

    #[test]
    fn usage_keeps_cache_split_and_retention_variants() {
        let u = parse_usage(&json!({
            "input_tokens": 50, "output_tokens": 70,
            "cache_creation_input_tokens": 300, "cache_read_input_tokens": 1000,
            "cache_creation": {"ephemeral_5m_input_tokens": 100, "ephemeral_1h_input_tokens": 200}
        }));
        assert_eq!(
            (
                u.input_tokens,
                u.output_tokens,
                u.cache_creation_input_tokens,
                u.cache_read_input_tokens
            ),
            (50, 70, 300, 1000)
        );
        let v = &u.provider_usage[CACHE_WRITE_BY_VARIANT];
        assert_eq!((v["5m"].as_u64(), v["1h"].as_u64()), (Some(100), Some(200)));
        assert!(parse_usage(&json!({"input_tokens": 1, "output_tokens": 1}))
            .provider_usage
            .is_null());
    }

    #[test]
    fn streams_thinking_signature_text_and_fragmented_tool_use() {
        let r = sse(&[
            ("message_start", json!({"type":"message_start","message":{"id":"msg_1","model":"vendor-claude","usage":{"input_tokens":40,"cache_read_input_tokens":900,"output_tokens":1}}})),
            ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking"}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":0})),
            ("content_block_start", json!({"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"RED"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":1})),
            ("content_block_start", json!({"type":"content_block_start","index":2,"content_block":{"type":"text"}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"voilà 🌍"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":2})),
            ("content_block_start", json!({"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"tu_9","name":"read"}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"th\":\"x\"}"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":3})),
            ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":55}})),
            ("message_stop", json!({"type":"message_stop"})),
        ])
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        let MessageContent::Blocks(b) = &r.message.content else {
            panic!()
        };
        assert!(
            matches!(&b[0], ContentBlock::Thinking { thinking, signature } if thinking == "hm" && signature == "SIG")
        );
        assert_eq!(b.len(), 4);
        assert!(matches!(&b[2], ContentBlock::Text { text } if text == "voilà 🌍"));
        assert!(
            matches!(&b[3], ContentBlock::ToolUse { id, input, .. } if id == "tu_9" && *input == json!({"path":"x"}))
        );
        assert_eq!(
            (
                r.usage.input_tokens,
                r.usage.cache_read_input_tokens,
                r.usage.output_tokens
            ),
            (40, 900, 55)
        );
    }

    #[test]
    fn error_events_and_refusal() {
        let e = sse(&[
            (
                "message_start",
                json!({"type":"message_start","message":{"id":"m","model":"x"}}),
            ),
            (
                "error",
                json!({"type":"error","error":{"message":"overloaded"}}),
            ),
        ])
        .unwrap_err();
        assert!(e.to_string().contains("overloaded"));
        let r = sse(&[
            ("message_start", json!({"type":"message_start","message":{"id":"m","model":"x"}})),
            ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":1}})),
            ("message_stop", json!({"type":"message_stop"})),
        ])
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::ContentFilter);
        // Cut stream.
        assert!(sse(&[(
            "message_start",
            json!({"type":"message_start","message":{"id":"m","model":"x"}})
        )])
        .is_err());
    }

    #[test]
    fn non_streaming_message() {
        let resp = json!({
            "id": "msg_2", "model": "vendor-claude", "stop_reason": "tool_use",
            "content": [
                {"type":"thinking","thinking":"t","signature":"S"},
                {"type":"text","text":"hi"},
                {"type":"tool_use","id":"tu","name":"read","input":{"a":1}}
            ],
            "usage": {"input_tokens": 5, "output_tokens": 6, "cache_read_input_tokens": 7}
        });
        let mut acc = StreamAccumulator::new();
        for e in response_to_events(&resp).unwrap() {
            acc.process_event(e);
        }
        let r = acc.into_response().unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        let MessageContent::Blocks(b) = &r.message.content else {
            panic!()
        };
        assert!(matches!(&b[0], ContentBlock::Thinking { signature, .. } if signature == "S"));
        assert_eq!(r.usage.cache_read_input_tokens, 7);
        assert!(response_to_events(&json!({"type":"error","error":{"message":"bad"}})).is_err());
    }
}
