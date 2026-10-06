//! `responses` adapter: `POST <base>/responses`.
//!
//! The Responses API is not Chat Completions with other field names: input is a
//! flat list of typed *items* (messages, `function_call`, `function_call_output`,
//! `reasoning`), the system prompt is `instructions`, tools are flat, and the
//! stream is a sequence of named events (`response.output_text.delta`, …)
//! ending in `response.completed` / `response.incomplete` / `response.failed`.
//!
//! Reasoning items are kept as [`ContentBlock::ProtocolItem`] and echoed back
//! verbatim, in order, on the next request — that is what lets a stateless
//! (`store = false`) multi-turn tool conversation continue.

use super::*;
use crate::CompletionRequest;
use cersei_types::{Result, StopReason, StreamEvent, Usage};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

const PROTOCOL: &str = "responses";

/// Block index of the primary block of output item `oi`.
fn primary(oi: usize) -> usize {
    oi * 2
}
/// Block index of the protocol item (verbatim copy) of output item `oi`.
fn protocol_slot(oi: usize) -> usize {
    oi * 2 + 1
}

// ─── Request ─────────────────────────────────────────────────────────────────

pub(crate) fn build_body(ctx: &BuildCtx, req: &CompletionRequest) -> Result<Value> {
    if !req.stop_sequences.is_empty() {
        return Err(unsupported(
            "the responses protocol has no stop-sequence parameter; remove stop_sequences",
        ));
    }

    let mut input: Vec<Value> = Vec::new();
    for msg in &req.messages {
        match msg.role {
            Role::System => {
                input.push(json!({ "role": "system", "content": msg.get_all_text() }));
            }
            Role::User => push_user(&mut input, msg)?,
            Role::Assistant => push_assistant(&mut input, msg),
        }
    }

    let mut body = json!({
        "model": ctx.api_model,
        "input": input,
        "max_output_tokens": ctx.max_tokens(req.max_tokens),
        "stream": ctx.stream,
    });
    if let Some(system) = &req.system {
        let s = plain_system(system);
        if !s.trim().is_empty() {
            body["instructions"] = json!(s);
        }
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if !req.tools.is_empty() {
        // Responses tools are flat: `{type, name, description, parameters}`.
        let tools = crate::adapt::adapt_tools(&req.tools, crate::adapt::SchemaDialect::OpenAiLoose)
            .into_iter()
            .map(|t| {
                let f = &t["function"];
                json!({
                    "type": "function",
                    "name": f["name"],
                    "description": f["description"],
                    "parameters": f["parameters"],
                })
            })
            .collect::<Vec<_>>();
        body["tools"] = Value::Array(tools);
        if req.options.get::<String>("tool_choice").as_deref() == Some("required") {
            body["tool_choice"] = json!("required");
        }
    }
    Ok(body)
}

fn push_user(out: &mut Vec<Value>, msg: &Message) -> Result<()> {
    let MessageContent::Blocks(blocks) = &msg.content else {
        out.push(json!({ "role": "user", "content": msg.get_all_text() }));
        return Ok(());
    };

    for block in blocks {
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } = block
        {
            out.push(json!({
                "type": "function_call_output",
                "call_id": tool_use_id,
                "output": tool_result_text(content, PROTOCOL)?,
            }));
        }
    }

    let mut parts: Vec<Value> = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } if !text.is_empty() => {
                parts.push(json!({ "type": "input_text", "text": text }));
            }
            ContentBlock::Image { source } => {
                let part = if let Some(id) = &source.file_id {
                    json!({ "type": "input_image", "file_id": id })
                } else if let Some(data) = &source.data {
                    let mt = source.media_type.as_deref().unwrap_or("image/png");
                    json!({ "type": "input_image", "image_url": data_url(mt, data) })
                } else if let Some(url) = &source.url {
                    json!({ "type": "input_image", "image_url": url })
                } else {
                    return Err(unsupported("an image block has no data, url or file_id"));
                };
                parts.push(part);
            }
            ContentBlock::Document { source, title, .. } => {
                let part = if let Some(id) = &source.file_id {
                    json!({ "type": "input_file", "file_id": id })
                } else if let Some(data) = &source.data {
                    let mt = source.media_type.as_deref().unwrap_or("application/pdf");
                    json!({
                        "type": "input_file",
                        "filename": document_filename(title.as_deref(), Some(mt)),
                        "file_data": data_url(mt, data),
                    })
                } else if let Some(url) = &source.url {
                    json!({ "type": "input_file", "file_url": url })
                } else {
                    return Err(unsupported("a document block has no data, url or file_id"));
                };
                parts.push(part);
            }
            ContentBlock::Audio { .. } => {
                return Err(unsupported(
                    "the responses protocol cannot carry audio input",
                ))
            }
            ContentBlock::Video { .. } => {
                return Err(unsupported(
                    "the responses protocol cannot carry video input",
                ))
            }
            _ => {}
        }
    }
    match parts.as_slice() {
        [] => {}
        [only] if only["type"] == "input_text" => {
            out.push(json!({ "role": "user", "content": only["text"].clone() }));
        }
        _ => out.push(json!({ "role": "user", "content": parts })),
    }
    Ok(())
}

fn push_assistant(out: &mut Vec<Value>, msg: &Message) {
    let mut text = String::new();
    let flush = |out: &mut Vec<Value>, text: &mut String| {
        if !text.is_empty() {
            out.push(json!({ "role": "assistant", "content": std::mem::take(text) }));
        }
    };
    for block in blocks_of(msg) {
        match block {
            ContentBlock::Text { text: t } => text.push_str(&t),
            ContentBlock::ProtocolItem { protocol, item } if protocol == PROTOCOL => {
                flush(out, &mut text);
                out.push(item);
            }
            ContentBlock::ToolUse { id, name, input } => {
                flush(out, &mut text);
                out.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": input.to_string(),
                }));
            }
            // Thinking text has no input form here (the reasoning item itself
            // carries it); other protocols' items are not ours to send.
            _ => {}
        }
    }
    flush(out, &mut text);
}

// ─── Usage ───────────────────────────────────────────────────────────────────

/// Normalize a Responses `usage` object. `input_tokens` includes cached tokens
/// and `output_tokens` includes reasoning tokens; see the chat adapter.
pub(crate) fn parse_usage(u: &Value) -> Usage {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let input = n(&u["input_tokens"]);
    let cached = n(&u["input_tokens_details"]["cached_tokens"]);
    Usage {
        input_tokens: input.saturating_sub(cached),
        output_tokens: n(&u["output_tokens"]),
        cache_read_input_tokens: cached,
        reasoning_tokens: n(&u["output_tokens_details"]["reasoning_tokens"]),
        provider_usage: u.clone(),
        ..Default::default()
    }
}

// ─── Stream state machine ────────────────────────────────────────────────────

enum Item {
    Message { started: bool },
    Reasoning { started: bool },
    Call { started: bool, saw_delta: bool },
}

#[derive(Default)]
pub(crate) struct StreamState {
    items: HashMap<usize, Item>,
    call_args: BTreeMap<usize, String>,
    started: bool,
    has_call: bool,
    executable_call: bool,
    terminal: bool,
}

impl StreamState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_data(&mut self, data: &str) -> Vec<StreamEvent> {
        let Ok(json) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        self.on_event(&json)
    }

    fn start(&mut self, out: &mut Vec<StreamEvent>, id: &str, model: &str) {
        if !self.started {
            self.started = true;
            out.push(StreamEvent::MessageStart {
                id: id.to_string(),
                model: model.to_string(),
                usage: None,
            });
        }
    }

    fn on_event(&mut self, ev: &Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        let kind = ev["type"].as_str().unwrap_or("");
        let oi = ev["output_index"].as_u64().unwrap_or(0) as usize;

        if !self.started && kind != "response.created" && kind != "error" {
            self.start(&mut out, "", "");
        }

        match kind {
            "response.created" | "response.in_progress" => {
                let r = &ev["response"];
                self.start(
                    &mut out,
                    r["id"].as_str().unwrap_or(""),
                    r["model"].as_str().unwrap_or(""),
                );
            }
            "response.output_item.added" => self.item_added(&mut out, oi, &ev["item"]),
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(d) = ev["delta"].as_str() {
                    self.ensure_message(&mut out, oi);
                    out.push(StreamEvent::TextDelta {
                        index: primary(oi),
                        text: d.to_string(),
                    });
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(d) = ev["delta"].as_str() {
                    self.ensure_reasoning(&mut out, oi);
                    out.push(StreamEvent::ThinkingDelta {
                        index: primary(oi),
                        thinking: d.to_string(),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(d) = ev["delta"].as_str() {
                    if let Some(Item::Call { saw_delta, .. }) = self.items.get_mut(&oi) {
                        *saw_delta = true;
                    }
                    self.call_args.entry(oi).or_default().push_str(d);
                    out.push(StreamEvent::InputJsonDelta {
                        index: primary(oi),
                        partial_json: d.to_string(),
                    });
                }
            }
            "response.output_item.done" => self.item_done(&mut out, oi, &ev["item"]),
            "response.completed" | "response.incomplete" => {
                self.terminal = true;
                let r = &ev["response"];
                self.start(
                    &mut out,
                    r["id"].as_str().unwrap_or(""),
                    r["model"].as_str().unwrap_or(""),
                );
                if let Some(msg) = r["error"]["message"].as_str() {
                    out.push(StreamEvent::Error {
                        message: msg.to_string(),
                    });
                }
                let reason = r["incomplete_details"]["reason"].as_str();
                out.extend(self.terminal_events(
                    kind == "response.incomplete",
                    reason,
                    r["usage"].is_object().then(|| parse_usage(&r["usage"])),
                ));
            }
            "response.failed" => {
                self.terminal = true;
                let msg = ev["response"]["error"]["message"]
                    .as_str()
                    .unwrap_or("response failed")
                    .to_string();
                out.push(StreamEvent::Error { message: msg });
                out.push(StreamEvent::MessageStop);
            }
            "error" => {
                self.terminal = true;
                let msg = ev["message"]
                    .as_str()
                    .or_else(|| ev["error"]["message"].as_str())
                    .unwrap_or("stream error")
                    .to_string();
                out.push(StreamEvent::Error { message: msg });
            }
            _ => {}
        }
        out
    }

    fn ensure_message(&mut self, out: &mut Vec<StreamEvent>, oi: usize) {
        let item = self
            .items
            .entry(oi)
            .or_insert(Item::Message { started: false });
        if let Item::Message { started } = item {
            if !*started {
                *started = true;
                out.push(StreamEvent::ContentBlockStart {
                    index: primary(oi),
                    block_type: "text".into(),
                    id: None,
                    name: None,
                });
            }
        }
    }

    fn ensure_reasoning(&mut self, out: &mut Vec<StreamEvent>, oi: usize) {
        let item = self
            .items
            .entry(oi)
            .or_insert(Item::Reasoning { started: false });
        if let Item::Reasoning { started } = item {
            if !*started {
                *started = true;
                out.push(StreamEvent::ContentBlockStart {
                    index: primary(oi),
                    block_type: "thinking".into(),
                    id: None,
                    name: None,
                });
            }
        }
    }

    fn ensure_call(&mut self, out: &mut Vec<StreamEvent>, oi: usize, item: &Value) {
        let entry = self.items.entry(oi).or_insert(Item::Call {
            started: false,
            saw_delta: false,
        });
        if let Item::Call { started, .. } = entry {
            if !*started {
                *started = true;
                let id = item["call_id"]
                    .as_str()
                    .or_else(|| item["id"].as_str())
                    .unwrap_or("");
                let name = item["name"].as_str().unwrap_or("");
                out.push(StreamEvent::ContentBlockStart {
                    index: primary(oi),
                    block_type: "tool_use".into(),
                    id: Some(id.to_string()),
                    name: Some(name.to_string()),
                });
            }
        }
    }

    fn item_added(&mut self, out: &mut Vec<StreamEvent>, oi: usize, item: &Value) {
        match item["type"].as_str() {
            Some("message") => self.ensure_message(out, oi),
            Some("reasoning") => self.ensure_reasoning(out, oi),
            Some("function_call") => {
                self.has_call = true;
                self.items.insert(
                    oi,
                    Item::Call {
                        started: false,
                        saw_delta: false,
                    },
                );
                self.ensure_call(out, oi, item);
            }
            _ => {} // built-in tool calls etc.: no block
        }
    }

    fn item_done(&mut self, out: &mut Vec<StreamEvent>, oi: usize, item: &Value) {
        match item["type"].as_str() {
            Some("message") => {
                // Servers that never sent deltas deliver the text here.
                let had_started =
                    matches!(self.items.get(&oi), Some(Item::Message { started: true }));
                if !had_started {
                    let text: String = item["content"]
                        .as_array()
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|p| {
                                    p["text"].as_str().or_else(|| p["refusal"].as_str())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if !text.is_empty() {
                        self.ensure_message(out, oi);
                        out.push(StreamEvent::TextDelta {
                            index: primary(oi),
                            text,
                        });
                    }
                }
                if matches!(self.items.get(&oi), Some(Item::Message { started: true })) {
                    out.push(StreamEvent::ContentBlockStop { index: primary(oi) });
                }
            }
            Some("reasoning") => {
                if matches!(self.items.get(&oi), Some(Item::Reasoning { started: true })) {
                    out.push(StreamEvent::ContentBlockStop { index: primary(oi) });
                }
                out.push(StreamEvent::ProtocolItem {
                    index: protocol_slot(oi),
                    protocol: PROTOCOL.to_string(),
                    item: item.clone(),
                });
            }
            Some("function_call") => {
                self.has_call = true;
                self.ensure_call(out, oi, item);
                let saw_delta = matches!(
                    self.items.get(&oi),
                    Some(Item::Call {
                        saw_delta: true,
                        ..
                    })
                );
                if !saw_delta {
                    if let Some(args) = item["arguments"].as_str() {
                        self.call_args.entry(oi).or_default().push_str(args);
                        out.push(StreamEvent::InputJsonDelta {
                            index: primary(oi),
                            partial_json: args.to_string(),
                        });
                    }
                }
                let args = self.call_args.get(&oi).map(String::as_str).unwrap_or("");
                let routable = item["call_id"]
                    .as_str()
                    .or_else(|| item["id"].as_str())
                    .is_some_and(|s| !s.is_empty())
                    && item["name"].as_str().is_some_and(|s| !s.is_empty());
                if routable && args_are_executable(args) {
                    self.executable_call = true;
                }
                out.push(StreamEvent::ContentBlockStop { index: primary(oi) });
            }
            _ => {}
        }
    }

    /// The closing events for a finished response.
    fn terminal_events(
        &mut self,
        incomplete: bool,
        incomplete_reason: Option<&str>,
        usage: Option<Usage>,
    ) -> Vec<StreamEvent> {
        let stop = if self.executable_call {
            // A runnable call must be dispatched whatever the status says.
            StopReason::ToolUse
        } else if incomplete {
            match incomplete_reason {
                Some("content_filter") => StopReason::ContentFilter,
                _ => StopReason::MaxTokens,
            }
        } else if self.has_call {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        vec![
            StreamEvent::MessageDelta {
                stop_reason: Some(stop),
                usage,
            },
            StreamEvent::MessageStop,
        ]
    }

    /// Called when the byte stream ends. A stream with no terminal event was
    /// cut off: that is an error, not a clean turn.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        if self.terminal {
            return Vec::new();
        }
        vec![StreamEvent::Error {
            message: "stream ended without response.completed (the response is incomplete)".into(),
        }]
    }
}

/// Convert a complete (non-streamed) response into events.
pub(crate) fn response_to_events(json: &Value) -> Result<Vec<StreamEvent>> {
    if let Some(msg) = json["error"]["message"].as_str() {
        return Err(CerseiError::Provider(msg.to_string()));
    }
    if json["status"].as_str() == Some("failed") {
        return Err(CerseiError::Provider("response failed".into()));
    }
    let mut state = StreamState::new();
    let mut out = Vec::new();
    state.start(
        &mut out,
        json["id"].as_str().unwrap_or(""),
        json["model"].as_str().unwrap_or(""),
    );
    for (oi, item) in json["output"].as_array().into_iter().flatten().enumerate() {
        match item["type"].as_str() {
            Some("reasoning") => {
                let text: String = item["summary"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(item["content"].as_array().into_iter().flatten())
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    state.ensure_reasoning(&mut out, oi);
                    out.push(StreamEvent::ThinkingDelta {
                        index: primary(oi),
                        thinking: text,
                    });
                }
                state.item_done(&mut out, oi, item);
            }
            Some("message") | Some("function_call") => state.item_done(&mut out, oi, item),
            _ => {}
        }
    }
    let incomplete = json["status"].as_str() == Some("incomplete");
    out.extend(
        state.terminal_events(
            incomplete,
            json["incomplete_details"]["reason"].as_str(),
            json["usage"]
                .is_object()
                .then(|| parse_usage(&json["usage"])),
        ),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Compat;
    use crate::StreamAccumulator;
    use cersei_types::{ToolDefinition, ToolResultContent};

    fn compat() -> crate::config::ResolvedCompat {
        Compat::resolve(&Compat::default(), &Compat::default())
    }

    fn body(r: &CompletionRequest, stream: bool) -> Result<Value> {
        let c = compat();
        build_body(
            &BuildCtx {
                api_model: "vendor-expert",
                compat: &c,
                max_output_tokens: 5000,
                stream,
            },
            r,
        )
    }

    fn run(events: &[Value]) -> cersei_types::Result<crate::CompletionResponse> {
        let mut st = StreamState::new();
        let mut acc = StreamAccumulator::new();
        for e in events {
            for ev in st.on_data(&e.to_string()) {
                acc.process_event(ev);
            }
        }
        for ev in st.finish() {
            acc.process_event(ev);
        }
        acc.into_response()
    }

    #[test]
    fn request_shape_is_responses_not_chat() {
        let mut r = CompletionRequest::new("expert");
        r.system = Some("be brief".into());
        r.messages.push(Message::user("hi"));
        r.max_tokens = 100_000;
        r.tools.push(ToolDefinition {
            name: "read".into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
        });
        r.options.set("tool_choice", "required");
        let b = body(&r, true).unwrap();
        assert_eq!(b["model"], "vendor-expert");
        assert_eq!(b["instructions"], "be brief");
        assert_eq!(b["input"][0], json!({"role":"user","content":"hi"}));
        assert!(b.get("messages").is_none() && b.get("max_tokens").is_none());
        assert_eq!(b["max_output_tokens"], 5000);
        assert_eq!(
            b["tools"][0],
            json!({"type":"function","name":"read","description":"d","parameters":{"type":"object"}})
        );
        assert_eq!(b["tool_choice"], "required");
        assert_eq!(b["stream"], true);
    }

    #[test]
    fn stop_sequences_are_refused() {
        let mut r = CompletionRequest::new("expert");
        r.stop_sequences = vec!["x".into()];
        assert!(matches!(body(&r, true), Err(CerseiError::Unsupported(_))));
    }

    #[test]
    fn tool_exchange_with_reasoning_item_is_echoed_in_order() {
        let reasoning =
            json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"ENC"});
        let mut r = CompletionRequest::new("expert");
        r.messages.push(Message::user("go"));
        r.messages.push(Message::assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "summary text".into(),
                signature: String::new(),
            },
            ContentBlock::ProtocolItem {
                protocol: "responses".into(),
                item: reasoning.clone(),
            },
            ContentBlock::Text {
                text: "working".into(),
            },
            ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "read".into(),
                input: json!({"p":1}),
            },
            // Another protocol's item must not leak.
            ContentBlock::ProtocolItem {
                protocol: "other".into(),
                item: json!({"type":"x"}),
            },
        ]));
        r.messages
            .push(Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: ToolResultContent::Text("done".into()),
                is_error: None,
            }]));
        let input = body(&r, true).unwrap()["input"].as_array().unwrap().clone();
        assert_eq!(input[1], reasoning, "verbatim, including encrypted_content");
        assert_eq!(input[2], json!({"role":"assistant","content":"working"}));
        assert_eq!(
            input[3],
            json!({"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"p\":1}"})
        );
        assert_eq!(
            input[4],
            json!({"type":"function_call_output","call_id":"call_1","output":"done"})
        );
        assert_eq!(input.len(), 5);
    }

    #[test]
    fn multimodal_items() {
        let mut r = CompletionRequest::new("expert");
        r.messages.push(Message::user_blocks(vec![
            ContentBlock::Text { text: "see".into() },
            ContentBlock::image_base64("image/png", "QUJD"),
            ContentBlock::image_file_id("file-i"),
            ContentBlock::document_base64("application/pdf", "JVBE"),
            ContentBlock::document_url("https://x/d.pdf"),
            ContentBlock::document_file_id("file-d"),
        ]));
        let parts = body(&r, true).unwrap()["input"][0]["content"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(parts[0], json!({"type":"input_text","text":"see"}));
        assert_eq!(parts[1]["image_url"], "data:image/png;base64,QUJD");
        assert_eq!(parts[2], json!({"type":"input_image","file_id":"file-i"}));
        assert_eq!(parts[3]["file_data"], "data:application/pdf;base64,JVBE");
        assert_eq!(parts[3]["filename"], "document.pdf");
        assert_eq!(
            parts[4],
            json!({"type":"input_file","file_url":"https://x/d.pdf"})
        );
        assert_eq!(parts[5], json!({"type":"input_file","file_id":"file-d"}));
        for bad in [
            ContentBlock::audio_base64("audio/wav", "AA=="),
            ContentBlock::video_url("https://x/v"),
        ] {
            let mut r = CompletionRequest::new("expert");
            r.messages.push(Message::user_blocks(vec![bad]));
            assert!(matches!(body(&r, true), Err(CerseiError::Unsupported(_))));
        }
    }

    #[test]
    fn usage_is_normalized() {
        let u = parse_usage(
            &json!({"input_tokens": 500, "input_tokens_details": {"cached_tokens": 200}, "output_tokens": 90, "output_tokens_details": {"reasoning_tokens": 60}}),
        );
        assert_eq!(
            (
                u.input_tokens,
                u.cache_read_input_tokens,
                u.output_tokens,
                u.reasoning_tokens
            ),
            (300, 200, 90, 60)
        );
    }

    fn text_stream() -> Vec<Value> {
        vec![
            json!({"type":"response.created","response":{"id":"resp_1","model":"vendor-expert"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1"}}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"Bonjour "}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"🌍"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message"}}),
            json!({"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":12,"output_tokens":4}}}),
        ]
    }

    #[test]
    fn streams_text() {
        let r = run(&text_stream()).unwrap();
        assert_eq!(r.message.get_all_text(), "Bonjour 🌍");
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (12, 4));
        assert_eq!(r.message.id.as_deref(), Some("resp_1"));
    }

    #[test]
    fn streams_reasoning_then_fragmented_tool_call() {
        let reasoning = json!({"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"hmm"}],"encrypted_content":"ENC"});
        let r = run(&[
            json!({"type":"response.created","response":{"id":"r"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"hm"}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"m"}),
            json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"é"}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"\"}"}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{\"path\":\"é\"}"}}),
            json!({"type":"response.completed","response":{"usage":{"input_tokens":30,"output_tokens":20,"output_tokens_details":{"reasoning_tokens":12}}}}),
        ])
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        let MessageContent::Blocks(b) = &r.message.content else {
            panic!()
        };
        assert!(matches!(&b[0], ContentBlock::Thinking { thinking, .. } if thinking == "hmm"));
        assert!(
            matches!(&b[1], ContentBlock::ProtocolItem { protocol, item } if protocol == "responses" && item["encrypted_content"] == "ENC")
        );
        assert!(
            matches!(&b[2], ContentBlock::ToolUse { id, name, input } if id == "call_1" && name == "read" && *input == json!({"path":"é"}))
        );
        assert_eq!(b.len(), 3, "no placeholder blocks");
        assert_eq!(r.usage.reasoning_tokens, 12);
        assert_eq!(r.usage.output_tokens, 20);
    }

    #[test]
    fn call_arguments_delivered_only_at_item_done() {
        let r = run(&[
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"c","name":"n","arguments":"{\"a\":1}"}}),
            json!({"type":"response.completed","response":{}}),
        ])
        .unwrap();
        let calls = r.message.get_tool_use_blocks();
        assert!(
            matches!(calls[0], ContentBlock::ToolUse { input, .. } if *input == json!({"a":1}))
        );
        assert_eq!(r.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn incomplete_maps_to_max_tokens_or_filter_and_runnable_call_wins() {
        let r = run(&[
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message"}}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"cut"}),
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message"}}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
        ])
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::MaxTokens);
        let f = run(&[
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter"}}}),
        ])
        .unwrap();
        assert_eq!(f.stop_reason, StopReason::ContentFilter);
        let w = run(&[
            json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"c","name":"n","arguments":"{}"}}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
        ])
        .unwrap();
        assert_eq!(w.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn failures_and_cut_streams_are_errors() {
        let e = run(&[json!({"type":"response.failed","response":{"error":{"message":"boom"}}})])
            .unwrap_err();
        assert!(e.to_string().contains("boom"));
        let e = run(&[json!({"type":"error","message":"rate"})]).unwrap_err();
        assert!(e.to_string().contains("rate"));
        // Text but no terminal event: incomplete.
        let mut cut = text_stream();
        cut.pop();
        assert!(run(&cut)
            .unwrap_err()
            .to_string()
            .contains("without response.completed"));
        assert!(run(&[]).is_err());
    }

    #[test]
    fn non_streaming_response() {
        let resp = json!({
            "id": "resp_9", "model": "vendor-expert", "status": "completed",
            "output": [
                {"type":"reasoning","id":"rs","summary":[{"type":"summary_text","text":"because"}],"encrypted_content":"E"},
                {"type":"message","content":[{"type":"output_text","text":"answer"}]},
                {"type":"function_call","call_id":"c1","name":"read","arguments":"{\"p\":2}"}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5, "input_tokens_details": {"cached_tokens": 4}}
        });
        let mut acc = StreamAccumulator::new();
        for e in response_to_events(&resp).unwrap() {
            acc.process_event(e);
        }
        let r = acc.into_response().unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(r.message.get_all_text(), "answer");
        assert_eq!(
            (r.usage.input_tokens, r.usage.cache_read_input_tokens),
            (6, 4)
        );
        let MessageContent::Blocks(b) = &r.message.content else {
            panic!()
        };
        assert!(b
            .iter()
            .any(|x| matches!(x, ContentBlock::ProtocolItem { .. })));
        assert!(response_to_events(&json!({"error":{"message":"bad"}})).is_err());
        assert!(response_to_events(&json!({"status":"failed"})).is_err());
    }
}
