//! `chat_completions` adapter: `POST <base>/chat/completions`.
//!
//! Request: `messages` with `system`/`user`/`assistant`/`tool` roles, tools as
//! `{type:"function", function:{…}}`. Response: `choices[0].message` (or
//! `delta` when streamed), `finish_reason`, `usage`.
//!
//! The stream reader keeps the hard-won behaviours of the previous OpenAI
//! client: tool-call slots are keyed by `index` (or by call id when a server
//! omits it), an empty id or name never reaches the dispatcher, tool calls are
//! flushed exactly once after the read loop, and a call the runner can run
//! wins over a `length` finish reason.

use super::*;
use crate::CompletionRequest;
use cersei_types::{
    DocumentSource, ImageSource, MediaSource, Result, StopReason, StreamEvent, Usage,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

const PROTOCOL: &str = "chat_completions";

// Block indexes: reasoning, then text, then one per tool-call slot.
const THINKING_INDEX: usize = 0;
const TEXT_INDEX: usize = 1;
const TOOL_BASE_INDEX: usize = 2;

// ─── Request ─────────────────────────────────────────────────────────────────

pub(crate) fn build_body(ctx: &BuildCtx, req: &CompletionRequest) -> Result<Value> {
    let mut messages: Vec<Value> = Vec::new();

    if let Some(system) = &req.system {
        messages.push(json!({ "role": "system", "content": plain_system(system) }));
    }

    for msg in &req.messages {
        match msg.role {
            Role::System => {
                messages.push(json!({ "role": "system", "content": msg.get_all_text() }));
            }
            Role::User => push_user(&mut messages, msg)?,
            Role::Assistant => push_assistant(&mut messages, msg, ctx),
        }
    }

    let mut body = json!({
        "model": ctx.api_model,
        "messages": messages,
        "stream": ctx.stream,
    });
    body[&ctx.compat.max_tokens_field] = json!(ctx.max_tokens(req.max_tokens));
    if ctx.stream && ctx.compat.stream_usage {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if !req.stop_sequences.is_empty() {
        body["stop"] = json!(req.stop_sequences);
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(crate::adapt::adapt_tools(
            &req.tools,
            crate::adapt::SchemaDialect::OpenAiLoose,
        ));
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

    // `role:"tool"` messages must directly follow the assistant turn that made
    // the calls, so tool results go first. `is_error` has no wire field here;
    // the runner already writes failures into the result text.
    for block in blocks {
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } = block
        {
            out.push(json!({
                "role": "tool",
                "tool_call_id": tool_use_id,
                "content": tool_result_text(content, PROTOCOL)?,
            }));
        }
    }

    let mut parts: Vec<Value> = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } if !text.is_empty() => {
                parts.push(json!({ "type": "text", "text": text }));
            }
            ContentBlock::Image { source } => parts.push(image_part(source)?),
            ContentBlock::Audio { source } => parts.push(audio_part(source)?),
            ContentBlock::Document { source, title, .. } => {
                parts.push(file_part(source, title.as_deref())?)
            }
            ContentBlock::Video { .. } => {
                return Err(unsupported("chat_completions cannot carry video input"))
            }
            _ => {}
        }
    }
    match parts.as_slice() {
        [] => {}
        // A lone text part collapses to a plain string.
        [only] if only["type"] == "text" => {
            out.push(json!({ "role": "user", "content": only["text"].clone() }));
        }
        _ => out.push(json!({ "role": "user", "content": parts })),
    }
    Ok(())
}

fn image_part(s: &ImageSource) -> Result<Value> {
    let url = match (&s.data, &s.url) {
        (Some(data), _) => data_url(s.media_type.as_deref().unwrap_or("image/png"), data),
        (None, Some(url)) => url.clone(),
        _ => {
            return Err(unsupported(
                "chat_completions carries images only as data or URL",
            ))
        }
    };
    Ok(json!({ "type": "image_url", "image_url": { "url": url } }))
}

fn audio_part(s: &MediaSource) -> Result<Value> {
    let (Some(data), Some(mt)) = (&s.data, s.media_type.as_deref()) else {
        return Err(unsupported(
            "chat_completions carries audio only as inline data with a media_type",
        ));
    };
    let format = audio_format(mt).ok_or_else(|| {
        unsupported(format!(
            "chat_completions accepts audio as wav or mp3; `{mt}` is not one of them"
        ))
    })?;
    Ok(json!({ "type": "input_audio", "input_audio": { "data": data, "format": format } }))
}

fn file_part(s: &DocumentSource, title: Option<&str>) -> Result<Value> {
    if let Some(id) = &s.file_id {
        return Ok(json!({ "type": "file", "file": { "file_id": id } }));
    }
    let Some(data) = &s.data else {
        return Err(unsupported(
            "chat_completions carries documents only as inline data or a file id",
        ));
    };
    let mt = s.media_type.as_deref().unwrap_or("application/pdf");
    Ok(json!({
        "type": "file",
        "file": {
            "filename": document_filename(title, Some(mt)),
            "file_data": data_url(mt, data),
        },
    }))
}

fn push_assistant(out: &mut Vec<Value>, msg: &Message, ctx: &BuildCtx) {
    let blocks = blocks_of(msg);
    let text: String = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let tool_calls: Vec<Value> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some(json!({
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": input.to_string() },
            })),
            _ => None,
        })
        .collect();

    if tool_calls.is_empty() {
        out.push(json!({ "role": "assistant", "content": text }));
        return;
    }
    let mut m = json!({ "role": "assistant", "tool_calls": tool_calls });
    if !text.is_empty() {
        m["content"] = json!(text);
    }
    // Some servers (reasoning models behind this protocol) reject a tool turn
    // whose reasoning was not echoed back. Opt-in per provider/model.
    if let Some(field) = &ctx.compat.reasoning_field {
        let thinking: String = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                _ => None,
            })
            .collect();
        if !thinking.is_empty() {
            m[field] = json!(thinking);
        }
    }
    out.push(m);
}

// ─── Usage ───────────────────────────────────────────────────────────────────

/// Normalize a Chat Completions `usage` object.
///
/// `prompt_tokens` *includes* cached tokens and `completion_tokens` *includes*
/// reasoning tokens, so cached tokens are moved to their own counter (making
/// `input_tokens` the uncached remainder) and reasoning tokens are reported
/// separately without being added to the output.
pub(crate) fn parse_usage(u: &Value) -> Usage {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let prompt = n(&u["prompt_tokens"]);
    let completion = n(&u["completion_tokens"]);
    let cached = u["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .or_else(|| u["prompt_cache_hit_tokens"].as_u64())
        .unwrap_or(0);
    let input = match u["prompt_cache_miss_tokens"].as_u64() {
        Some(miss) => miss,
        None => prompt.saturating_sub(cached),
    };
    Usage {
        input_tokens: input,
        output_tokens: completion,
        cache_read_input_tokens: cached,
        reasoning_tokens: n(&u["completion_tokens_details"]["reasoning_tokens"]),
        provider_usage: u.clone(),
        ..Default::default()
    }
}

fn stop_from_finish(reason: &str) -> Option<StopReason> {
    match reason {
        "stop" => Some(StopReason::EndTurn),
        "tool_calls" | "function_call" => Some(StopReason::ToolUse),
        "length" => Some(StopReason::MaxTokens),
        "content_filter" => Some(StopReason::ContentFilter),
        _ => None,
    }
}

// ─── Stream state machine ────────────────────────────────────────────────────

#[derive(Default)]
pub(crate) struct StreamState {
    // slot -> (id, name, arguments)
    tool_calls: BTreeMap<usize, (String, String, String)>,
    slot_for_id: HashMap<String, usize>,
    last_slot: Option<usize>,
    text_started: bool,
    thinking_started: bool,
    saw_done: bool,
    final_stop: Option<StopReason>,
    usage: Option<Usage>,
    message_id: String,
    model: String,
    started: bool,
}

impl StreamState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one SSE `data:` payload. Returns events to emit now.
    pub fn on_data(&mut self, data: &str) -> Vec<StreamEvent> {
        let data = data.trim();
        if data == "[DONE]" {
            self.saw_done = true;
            return Vec::new();
        }
        let Ok(json) = serde_json::from_str::<Value>(data) else {
            return Vec::new();
        };
        self.on_chunk(&json)
    }

    pub fn saw_done(&self) -> bool {
        self.saw_done
    }

    fn on_chunk(&mut self, json: &Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if !self.started {
            self.started = true;
            self.message_id = json["id"].as_str().unwrap_or("").to_string();
            self.model = json["model"].as_str().unwrap_or("").to_string();
            out.push(StreamEvent::MessageStart {
                id: self.message_id.clone(),
                model: self.model.clone(),
                usage: None,
            });
        }

        // An error object mid-stream (some servers send one as a data event).
        if let Some(msg) = json["error"]["message"].as_str() {
            out.push(StreamEvent::Error {
                message: msg.to_string(),
            });
            return out;
        }

        let choice = &json["choices"][0];
        let delta = &choice["delta"];
        if let Some(reason) = choice["finish_reason"].as_str() {
            if let Some(sr) = stop_from_finish(reason) {
                self.final_stop = Some(sr);
            }
        }

        // Reasoning text: `reasoning_content` (common) or `reasoning`.
        let reasoning = delta["reasoning_content"]
            .as_str()
            .or_else(|| delta["reasoning"].as_str())
            .filter(|s| !s.is_empty());
        if let Some(text) = reasoning {
            if !self.thinking_started {
                self.thinking_started = true;
                out.push(StreamEvent::ContentBlockStart {
                    index: THINKING_INDEX,
                    block_type: "thinking".into(),
                    id: None,
                    name: None,
                });
            }
            out.push(StreamEvent::ThinkingDelta {
                index: THINKING_INDEX,
                thinking: text.to_string(),
            });
        }

        if let Some(text) = delta["content"].as_str().filter(|s| !s.is_empty()) {
            if !self.text_started {
                self.text_started = true;
                out.push(StreamEvent::ContentBlockStart {
                    index: TEXT_INDEX,
                    block_type: "text".into(),
                    id: None,
                    name: None,
                });
            }
            out.push(StreamEvent::TextDelta {
                index: TEXT_INDEX,
                text: text.to_string(),
            });
        }

        if let Some(calls) = delta["tool_calls"].as_array() {
            for tc in calls {
                self.on_tool_delta(tc);
            }
        }

        if let Some(usage) = json["usage"].as_object() {
            self.usage = Some(parse_usage(&Value::Object(usage.clone())));
        }
        out
    }

    fn on_tool_delta(&mut self, tc: &Value) {
        let tc_id = tc["id"].as_str().filter(|s| !s.is_empty());
        // Only an explicit `index` is trusted. Servers that omit it
        // (llama.cpp, some proxies) get slots correlated by call id; an id-less
        // delta continues the slot most recently touched.
        let idx = match tc["index"].as_u64() {
            Some(i) => i as usize,
            None => match tc_id.and_then(|id| self.slot_for_id.get(id).copied()) {
                Some(slot) => slot,
                None if tc_id.is_some() => self
                    .tool_calls
                    .keys()
                    .next_back()
                    .map(|k| k + 1)
                    .unwrap_or(0),
                None => self.last_slot.unwrap_or(0),
            },
        };
        if let Some(id) = tc_id {
            self.slot_for_id.insert(id.to_string(), idx);
        }
        self.last_slot = Some(idx);
        let entry = self.tool_calls.entry(idx).or_default();
        // An empty string in a later delta never clobbers a good value.
        if let Some(id) = tc_id {
            entry.0 = id.to_string();
        }
        if let Some(name) = tc["function"]["name"].as_str().filter(|s| !s.is_empty()) {
            entry.1 = name.to_string();
        }
        if let Some(args) = tc["function"]["arguments"].as_str() {
            entry.2.push_str(args);
        }
    }

    /// Emit everything still pending. Runs on `[DONE]` and on plain EOF.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        if !self.started {
            out.push(StreamEvent::MessageStart {
                id: String::new(),
                model: String::new(),
                usage: None,
            });
        }
        let mut emitted = 0usize;
        let mut executable = 0usize;
        let mut rejected: Vec<String> = Vec::new();

        for (slot, (id, name, args)) in &self.tool_calls {
            // A call the dispatcher cannot route would wedge the conversation:
            // an empty id is echoed back as `"tool_call_id": ""` and rejected.
            if id.is_empty() || name.is_empty() {
                rejected.push(format!(
                    "slot {slot}: id={id:?} name={name:?} arguments={args:?}"
                ));
                continue;
            }
            let index = TOOL_BASE_INDEX + slot;
            out.push(StreamEvent::ContentBlockStart {
                index,
                block_type: "tool_use".into(),
                id: Some(id.clone()),
                name: Some(name.clone()),
            });
            out.push(StreamEvent::InputJsonDelta {
                index,
                partial_json: args.clone(),
            });
            out.push(StreamEvent::ContentBlockStop { index });
            emitted += 1;
            if args_are_executable(args) {
                executable += 1;
            }
        }

        // Report dropped calls in-band so valid siblings survive: an Error
        // event would discard the whole turn.
        if !rejected.is_empty() && emitted > 0 {
            tracing::warn!(
                rejected = rejected.len(),
                emitted,
                "provider emitted unusable tool call(s)"
            );
            let note = format!(
                "{}[bricks] dropped {} unusable tool call(s) (empty id or name): {}. {} valid \
                 call(s) were kept; re-issue the dropped one(s) if you still need them.",
                if self.text_started { "\n\n" } else { "" },
                rejected.len(),
                rejected.join("; "),
                emitted
            );
            if !self.text_started {
                self.text_started = true;
                out.push(StreamEvent::ContentBlockStart {
                    index: TEXT_INDEX,
                    block_type: "text".into(),
                    id: None,
                    name: None,
                });
            }
            out.push(StreamEvent::TextDelta {
                index: TEXT_INDEX,
                text: note,
            });
        }

        if self.thinking_started {
            out.push(StreamEvent::ContentBlockStop {
                index: THINKING_INDEX,
            });
        }
        if self.text_started {
            out.push(StreamEvent::ContentBlockStop { index: TEXT_INDEX });
        }

        // `finish_reason` says how generation ended, not what it produced.
        // A runnable call must be dispatched, and ToolUse is the only stop
        // reason that makes the runner dispatch it.
        let stop = match self.final_stop.take() {
            _ if executable > 0 => StopReason::ToolUse,
            Some(StopReason::EndTurn) if emitted > 0 => StopReason::ToolUse,
            Some(sr) => sr,
            None if emitted > 0 => StopReason::ToolUse,
            None => StopReason::EndTurn,
        };
        out.push(StreamEvent::MessageDelta {
            stop_reason: Some(stop),
            usage: self.usage.take(),
        });

        if !rejected.is_empty() && emitted == 0 {
            out.push(StreamEvent::Error {
                message: format!(
                    "provider emitted {} unusable tool call(s) (empty id or name): {}",
                    rejected.len(),
                    rejected.join("; ")
                ),
            });
        } else if !self.saw_done && emitted == 0 && !self.text_started && !self.thinking_started {
            // Cut short AND empty: without this the accumulator would report a
            // confident, empty EndTurn.
            out.push(StreamEvent::Error {
                message: "stream ended without [DONE] and produced no content".into(),
            });
        }
        out.push(StreamEvent::MessageStop);
        out
    }
}

/// Convert a complete (non-streamed) response body into the same events.
pub(crate) fn response_to_events(json: &Value) -> Result<Vec<StreamEvent>> {
    if let Some(msg) = json["error"]["message"].as_str() {
        return Err(CerseiError::Provider(msg.to_string()));
    }
    let message = &json["choices"][0]["message"];
    if !message.is_object() {
        return Err(CerseiError::Provider(
            "response has no choices[0].message".into(),
        ));
    }
    let mut delta = message.clone();
    if let Some(calls) = delta["tool_calls"].as_array_mut() {
        for (i, c) in calls.iter_mut().enumerate() {
            c["index"] = json!(i);
        }
    }
    let chunk = json!({
        "id": json["id"],
        "model": json["model"],
        "choices": [{ "delta": delta, "finish_reason": json["choices"][0]["finish_reason"] }],
        "usage": json["usage"],
    });
    let mut state = StreamState::new();
    let mut events = state.on_chunk(&chunk);
    state.saw_done = true;
    events.extend(state.finish());
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Compat;
    use crate::StreamAccumulator;
    use cersei_types::ToolDefinition;

    fn ctx<'a>(compat: &'a ResolvedCompat, stream: bool) -> BuildCtx<'a> {
        BuildCtx {
            api_model: "vendor-flash",
            compat,
            max_output_tokens: 8000,
            stream,
        }
    }

    fn compat() -> ResolvedCompat {
        Compat::resolve(&Compat::default(), &Compat::default())
    }

    fn req() -> CompletionRequest {
        CompletionRequest::new("flash")
    }

    fn run(chunks: &[&str]) -> cersei_types::Result<crate::CompletionResponse> {
        let mut state = StreamState::new();
        let mut acc = StreamAccumulator::new();
        for c in chunks {
            for e in state.on_data(c) {
                acc.process_event(e);
            }
        }
        for e in state.finish() {
            acc.process_event(e);
        }
        acc.into_response()
    }

    #[test]
    fn body_basics_and_api_model() {
        let mut r = req();
        r.system = Some(format!("stable{}dynamic", SYSTEM_PROMPT_DYNAMIC_BOUNDARY));
        r.messages.push(Message::user("hi"));
        r.max_tokens = 100_000;
        r.temperature = Some(0.2);
        r.stop_sequences = vec!["END".into()];
        let c = compat();
        let body = build_body(&ctx(&c, true), &r).unwrap();
        assert_eq!(body["model"], "vendor-flash", "api_model, not the local id");
        assert_eq!(
            body["max_tokens"], 8000,
            "capped at the declared max_output_tokens"
        );
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(
            body["messages"][0],
            json!({"role":"system","content":"stabledynamic"})
        );
        assert_eq!(body["messages"][1], json!({"role":"user","content":"hi"}));
        assert_eq!(body["stop"], json!(["END"]));
        assert!((body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn compat_renames_max_tokens_and_drops_stream_options() {
        let c = Compat::resolve(
            &Compat {
                max_tokens_field: Some("max_completion_tokens".into()),
                stream_usage: Some(false),
                ..Default::default()
            },
            &Compat::default(),
        );
        let body = build_body(&ctx(&c, true), &req()).unwrap();
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["max_completion_tokens"], 8000);
        assert!(body.get("stream_options").is_none());
        let non_stream = build_body(&ctx(&compat(), false), &req()).unwrap();
        assert_eq!(non_stream["stream"], false);
        assert!(non_stream.get("stream_options").is_none());
    }

    #[test]
    fn tools_and_forced_choice() {
        let mut r = req();
        r.tools.push(ToolDefinition {
            name: "read".into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
        });
        r.options.set("tool_choice", "required");
        let body = build_body(&ctx(&compat(), true), &r).unwrap();
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(body["tool_choice"], "required");
        // No tools -> no tool_choice even when requested.
        let mut r2 = req();
        r2.options.set("tool_choice", "required");
        assert!(build_body(&ctx(&compat(), true), &r2)
            .unwrap()
            .get("tool_choice")
            .is_none());
    }

    #[test]
    fn tool_exchange_serializes_in_wire_order() {
        let mut r = req();
        r.messages.push(Message::user("go"));
        r.messages.push(Message::assistant_blocks(vec![
            ContentBlock::Text {
                text: "calling".into(),
            },
            ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "read".into(),
                input: json!({"path":"a"}),
            },
            ContentBlock::ToolUse {
                id: "call_2".into(),
                name: "read".into(),
                input: json!({"path":"b"}),
            },
        ]));
        r.messages.push(Message::user_blocks(vec![
            ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: ToolResultContent::Text("A".into()),
                is_error: None,
            },
            ContentBlock::ToolResult {
                tool_use_id: "call_2".into(),
                content: ToolResultContent::Text("B".into()),
                is_error: Some(true),
            },
        ]));
        let body = build_body(&ctx(&compat(), true), &r).unwrap();
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m[1]["role"], "assistant");
        assert_eq!(m[1]["content"], "calling");
        assert_eq!(m[1]["tool_calls"][1]["id"], "call_2");
        assert_eq!(
            m[1]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a\"}"
        );
        assert_eq!(
            m[2],
            json!({"role":"tool","tool_call_id":"call_1","content":"A"})
        );
        assert_eq!(
            m[3],
            json!({"role":"tool","tool_call_id":"call_2","content":"B"})
        );
        assert_eq!(
            m.len(),
            4,
            "a tool-result-only user turn adds no empty user message"
        );
    }

    #[test]
    fn reasoning_is_echoed_only_when_configured() {
        let mut r = req();
        r.messages.push(Message::assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "plan".into(),
                signature: String::new(),
            },
            ContentBlock::ToolUse {
                id: "c".into(),
                name: "t".into(),
                input: json!({}),
            },
        ]));
        let off = build_body(&ctx(&compat(), true), &r).unwrap();
        assert!(off["messages"][0].get("reasoning_content").is_none());
        let c = Compat::resolve(
            &Compat {
                reasoning_field: Some("reasoning_content".into()),
                ..Default::default()
            },
            &Compat::default(),
        );
        let on = build_body(&ctx(&c, true), &r).unwrap();
        assert_eq!(on["messages"][0]["reasoning_content"], "plan");
    }

    #[test]
    fn multimodal_parts() {
        let mut r = req();
        r.messages.push(Message::user_blocks(vec![
            ContentBlock::Text {
                text: "look".into(),
            },
            ContentBlock::image_base64("image/png", "QUJD"),
            ContentBlock::image_url("https://x/y.png"),
            ContentBlock::audio_base64("audio/wav", "UklG"),
            ContentBlock::document_base64("application/pdf", "JVBE"),
            ContentBlock::document_file_id("file-9"),
        ]));
        let body = build_body(&ctx(&compat(), true), &r).unwrap();
        let parts = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,QUJD");
        assert_eq!(parts[2]["image_url"]["url"], "https://x/y.png");
        assert_eq!(
            parts[3]["input_audio"],
            json!({"data":"UklG","format":"wav"})
        );
        assert_eq!(
            parts[4]["file"]["file_data"],
            "data:application/pdf;base64,JVBE"
        );
        assert_eq!(parts[4]["file"]["filename"], "document.pdf");
        assert_eq!(parts[5]["file"]["file_id"], "file-9");
    }

    #[test]
    fn unsupported_media_is_an_error_not_a_drop() {
        for block in [
            ContentBlock::video_url("https://x/v.mp4"),
            ContentBlock::audio_url("https://x/a.wav"),
            ContentBlock::audio_base64("audio/ogg", "AA=="),
            ContentBlock::image_file_id("f"),
        ] {
            let mut r = req();
            r.messages.push(Message::user_blocks(vec![block]));
            assert!(matches!(
                build_body(&ctx(&compat(), true), &r),
                Err(CerseiError::Unsupported(_))
            ));
        }
        let mut r = req();
        r.messages
            .push(Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: ToolResultContent::Blocks(vec![ContentBlock::image_base64(
                    "image/png",
                    "QQ==",
                )]),
                is_error: None,
            }]));
        assert!(build_body(&ctx(&compat(), true), &r).is_err());
    }

    #[test]
    fn usage_is_normalized_without_double_counting() {
        let u = parse_usage(&json!({
            "prompt_tokens": 1000, "completion_tokens": 300,
            "prompt_tokens_details": {"cached_tokens": 400},
            "completion_tokens_details": {"reasoning_tokens": 120}
        }));
        assert_eq!(
            (u.input_tokens, u.cache_read_input_tokens),
            (600, 400),
            "cached tokens leave the input counter"
        );
        assert_eq!(
            (u.output_tokens, u.reasoning_tokens),
            (300, 120),
            "reasoning stays inside output"
        );
        assert_eq!(u.input_tokens + u.cache_read_input_tokens, 1000);
        // DeepSeek-style cache counters.
        let d = parse_usage(
            &json!({"prompt_tokens": 100, "completion_tokens": 5, "prompt_cache_hit_tokens": 70, "prompt_cache_miss_tokens": 30}),
        );
        assert_eq!((d.input_tokens, d.cache_read_input_tokens), (30, 70));
        // No details at all.
        let p = parse_usage(&json!({"prompt_tokens": 7, "completion_tokens": 2}));
        assert_eq!(
            (
                p.input_tokens,
                p.cache_read_input_tokens,
                p.reasoning_tokens
            ),
            (7, 0, 0)
        );
    }

    #[test]
    fn streams_text_with_usage() {
        let r = run(&[
            r#"{"id":"c1","model":"vendor-flash","choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo 🌍"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3}}"#,
            "[DONE]",
        ])
        .unwrap();
        assert_eq!(r.message.get_all_text(), "Hello 🌍");
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (10, 3));
        assert_eq!(r.message.id.as_deref(), Some("c1"));
    }

    #[test]
    fn streams_fragmented_tool_arguments_and_reasoning() {
        let r = run(&[
            r#"{"choices":[{"delta":{"reasoning_content":"think "}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"hard"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"read","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"é"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"glob","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ])
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        let MessageContent::Blocks(blocks) = &r.message.content else {
            panic!()
        };
        assert!(
            matches!(&blocks[0], ContentBlock::Thinking { thinking, .. } if thinking == "think hard")
        );
        let calls: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], ("call_a", "read", json!({"path": "é"})));
        assert_eq!(calls[1], ("call_b", "glob", json!({})));
    }

    #[test]
    fn indexless_servers_get_slots_by_id() {
        let r = run(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"x1","function":{"name":"a","arguments":"{\"k\":1}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"x2","function":{"name":"b","arguments":"{\"z\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"2}"}}]},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ])
        .unwrap();
        assert_eq!(
            r.stop_reason,
            StopReason::ToolUse,
            "runnable calls beat a `stop` finish reason"
        );
        let MessageContent::Blocks(b) = &r.message.content else {
            panic!()
        };
        let calls: Vec<_> = b
            .iter()
            .filter_map(|x| match x {
                ContentBlock::ToolUse { id, input, .. } => Some((id.clone(), input.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            vec![
                ("x1".to_string(), json!({"k":1})),
                ("x2".to_string(), json!({"z":2}))
            ]
        );
    }

    #[test]
    fn unusable_calls_are_reported_without_losing_siblings() {
        let r = run(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"","function":{"name":"x","arguments":"{}"}},{"index":1,"id":"ok","function":{"name":"y","arguments":"{}"}}]}}]}"#,
            "[DONE]",
        ])
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert!(r
            .message
            .get_all_text()
            .contains("dropped 1 unusable tool call"));
        assert_eq!(r.message.get_tool_use_blocks().len(), 1);
        // Only unusable calls -> the turn fails loudly.
        let e = run(&[r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"","function":{"name":"x","arguments":"{}"}}]}}]}"#, "[DONE]"]).unwrap_err();
        assert!(e.to_string().contains("unusable tool call"));
    }

    #[test]
    fn truncated_arguments_keep_the_length_reason_and_the_parse_error() {
        let r = run(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"w","arguments":"{\"a\":"}}]},"finish_reason":"length"}]}"#,
            "[DONE]",
        ])
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::MaxTokens);
        let calls = r.message.get_tool_use_blocks();
        let ContentBlock::ToolUse { input, .. } = calls[0] else {
            panic!()
        };
        assert!(input.get("__parse_error").is_some());
    }

    #[test]
    fn cut_stream_with_nothing_is_an_error() {
        assert!(run(&[]).is_err());
        assert!(
            run(&[r#"{"choices":[{"delta":{"content":"partial"}}]}"#]).is_ok(),
            "content received, EOF without DONE tolerated"
        );
    }

    #[test]
    fn in_stream_error_object_fails_the_turn() {
        let e = run(&[r#"{"error":{"message":"overloaded"}}"#]).unwrap_err();
        assert!(e.to_string().contains("overloaded"));
    }

    #[test]
    fn non_streaming_response_yields_the_same_result() {
        let body = json!({
            "id": "r1", "model": "vendor-flash",
            "choices": [{"finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": "ok", "reasoning_content": "why",
                "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read", "arguments": "{\"p\":1}"}}]
            }}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 8, "prompt_tokens_details": {"cached_tokens": 5}}
        });
        let mut acc = StreamAccumulator::new();
        for e in response_to_events(&body).unwrap() {
            acc.process_event(e);
        }
        let r = acc.into_response().unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(r.message.get_all_text(), "ok");
        assert_eq!(r.usage.cache_read_input_tokens, 5);
        assert_eq!(r.usage.input_tokens, 15);
        let MessageContent::Blocks(b) = &r.message.content else {
            panic!()
        };
        assert!(matches!(&b[0], ContentBlock::Thinking { thinking, .. } if thinking == "why"));
        assert!(response_to_events(&json!({"error": {"message": "nope"}})).is_err());
    }
}
