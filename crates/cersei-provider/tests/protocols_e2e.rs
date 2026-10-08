//! End-to-end tests of the configured providers against a scripted server:
//! real HTTP, real SSE framing, the real adapters and accumulator. Nothing is
//! stubbed on the Bricks side and no paid API is ever called.

mod common;

use cersei_provider::{
    CompletionRequest, CompletionResponse, ConfiguredProvider, Provider, ProviderRegistry,
};
use cersei_types::*;
use common::*;
use serde_json::{json, Value};

const KEY: &str = "sk-test-key-123456";

const EXTRA: &str = r#"
[[providers.models]]
id = "chat_ns"
name = "Chat non-streaming"
api_model = "vendor-chat-ns"
tool_calls = true
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "resp_ns"
name = "Resp non-streaming"
api_model = "vendor-resp-ns"
protocol = "responses"
tool_calls = true
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "anth_ns"
name = "Anth non-streaming"
api_model = "vendor-anth-ns"
protocol = "anthropic_messages"
tool_calls = true
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "think"
name = "Think"
api_model = "vendor-think"
streaming = true
tool_calls = true
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
[providers.models.parameters]
temperature = 0.5
reasoning = { summary = "auto", effort = "model-default" }
[providers.models.reasoning]
default = "deep"
[[providers.models.reasoning.profiles]]
id = "fast"
label = "Rapide"
parameters = { reasoning = { effort = "low" } }
[[providers.models.reasoning.profiles]]
id = "deep"
parameters = { reasoning = { effort = "high" } }
[[providers.models.reasoning.profiles]]
id = "off"
label = "Aucun"
remove = ["/reasoning", "/temperature"]
parameters = { thinking = { type = "disabled" } }
[[providers.models.reasoning.profiles]]
id = "ultra"
parameters = { reasoning = { effort = "high" }, extra_budget = 99999 }

[[providers.models]]
id = "plain"
name = "No profiles"
api_model = "vendor-plain"
streaming = true
tool_calls = true
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
[providers.models.reasoning]
profiles = []

[[providers.models]]
id = "videomodel"
name = "Declares video"
api_model = "vendor-video"
streaming = true
input_modalities = ["text", "video"]
output_modalities = ["text", "audio"]
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers]]
id = "local"
name = "Local server"
endpoint = "ENDPOINT/openai"
protocol = "chat_completions"
auth = "none"
[[providers.models]]
id = "m"
name = "M"
api_model = "local-m"
streaming = true
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
"#;

fn setup(replies: Vec<Reply>) -> (Mock, ProviderRegistry) {
    let mock = serve(replies);
    let extra = EXTRA.replace("ENDPOINT", &mock.base);
    let reg = registry_for(&mock.base, &extra);
    (mock, reg)
}

fn provider(reg: &ProviderRegistry, sel: &str) -> ConfiguredProvider {
    // Keys come through `api_key_env` and an injected lookup: no key, not even
    // a fake one, is written into a fixture.
    model(reg, sel)
        .provider()
        .env(|_| Some(KEY.to_string()))
        .build()
        .unwrap()
}

fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "read_file".into(),
        description: "Read a file".into(),
        input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
    }]
}

fn first_request(msgs: Vec<Message>, with_tools: bool) -> CompletionRequest {
    let mut r = CompletionRequest::new("ignored-local-id");
    r.system = Some("You are helpful.".into());
    r.messages = msgs;
    r.max_tokens = 1000;
    if with_tools {
        r.tools = tools();
    }
    r
}

async fn run(p: &ConfiguredProvider, r: CompletionRequest) -> Result<CompletionResponse> {
    p.complete(r).await?.collect().await
}

fn history_after(first: Vec<Message>, resp: &CompletionResponse, tool_id: &str) -> Vec<Message> {
    let mut msgs = first;
    msgs.push(resp.message.clone());
    msgs.push(Message::user_blocks(vec![ContentBlock::ToolResult {
        tool_use_id: tool_id.into(),
        content: ToolResultContent::Text("contenu du fichier — é 🌍".into()),
        is_error: None,
    }]));
    msgs
}

fn text_reply(proto: &str) -> Reply {
    let events = match proto {
        "chat" => vec![
            (None, json!({"choices":[{"delta":{"content":"Voilà."}}]})),
            (
                None,
                json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}),
            ),
        ],
        "resp" => vec![
            (
                Some("response.created"),
                json!({"type":"response.created","response":{"id":"r2"}}),
            ),
            (
                Some("response.output_item.added"),
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message"}}),
            ),
            (
                Some("response.output_text.delta"),
                json!({"type":"response.output_text.delta","output_index":0,"delta":"Voilà."}),
            ),
            (
                Some("response.output_item.done"),
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message"}}),
            ),
            (
                Some("response.completed"),
                json!({"type":"response.completed","response":{"usage":{"input_tokens":3,"output_tokens":2}}}),
            ),
        ],
        _ => vec![
            (
                Some("message_start"),
                json!({"type":"message_start","message":{"id":"m2","model":"x","usage":{"input_tokens":3,"output_tokens":1}}}),
            ),
            (
                Some("content_block_start"),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text"}}),
            ),
            (
                Some("content_block_delta"),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Voilà."}}),
            ),
            (
                Some("content_block_stop"),
                json!({"type":"content_block_stop","index":0}),
            ),
            (
                Some("message_delta"),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
            ),
            (Some("message_stop"), json!({"type":"message_stop"})),
        ],
    };
    let mut text = sse_events(&events);
    if proto == "chat" {
        text.push_str("data: [DONE]\n\n");
    }
    Reply::Sse {
        chunks: fragment(&text, 5),
    }
}

// ─── Multi-turn tool exchange, per protocol ──────────────────────────────────

fn chat_tool_turn() -> Reply {
    let text = sse_events(&[
        (
            None,
            json!({"id":"c1","model":"vendor-chat","choices":[{"delta":{"reasoning_content":"Je cherche 🌍"}}]}),
        ),
        (None, json!({"choices":[{"delta":{"content":"Je lis."}}]})),
        (
            None,
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}),
        ),
        (
            None,
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"é🌍.txt\"}"}}]},"finish_reason":"tool_calls"}]}),
        ),
        (
            None,
            json!({"choices":[],"usage":{"prompt_tokens":2000,"completion_tokens":500,"prompt_tokens_details":{"cached_tokens":1000},"completion_tokens_details":{"reasoning_tokens":200}}}),
        ),
    ]) + "data: [DONE]\n\n";
    Reply::Sse {
        chunks: fragment(&text, 7),
    }
}

#[tokio::test]
async fn chat_completions_tool_exchange_over_two_turns() {
    let (mock, reg) = setup(vec![chat_tool_turn(), text_reply("chat")]);
    let p = provider(&reg, "mock/chat");
    let first = vec![Message::user("Lis é🌍.txt")];

    let r1 = run(&p, first_request(first.clone(), true)).await.unwrap();
    assert_eq!(r1.stop_reason, StopReason::ToolUse);
    assert_eq!(r1.message.get_all_text(), "Je lis.");
    let calls = r1.message.get_tool_use_blocks();
    let ContentBlock::ToolUse { id, name, input } = calls[0] else {
        panic!()
    };
    assert_eq!(
        (id.as_str(), name.as_str(), input.clone()),
        ("call_1", "read_file", json!({"path":"é🌍.txt"}))
    );
    assert_eq!(
        (
            r1.usage.input_tokens,
            r1.usage.cache_read_input_tokens,
            r1.usage.output_tokens,
            r1.usage.reasoning_tokens
        ),
        (1000, 1000, 500, 200)
    );

    let r2 = run(&p, first_request(history_after(first, &r1, "call_1"), true))
        .await
        .unwrap();
    assert_eq!(r2.message.get_all_text(), "Voilà.");

    let reqs = mock.requests();
    assert_eq!(reqs[0].path, "/v1/chat/completions");
    assert_eq!(reqs[0].headers["authorization"], format!("Bearer {KEY}"));
    assert_eq!(
        reqs[0].body["model"], "vendor-chat",
        "api_model, never the request's model string"
    );
    assert_eq!(reqs[0].body["stream"], true);
    assert_eq!(reqs[0].body["tools"][0]["function"]["name"], "read_file");
    let m = reqs[1].body["messages"].as_array().unwrap();
    assert_eq!(m[2]["role"], "assistant");
    assert_eq!(m[2]["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        m[2]["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"é🌍.txt\"}"
    );
    assert_eq!(
        m[3],
        json!({"role":"tool","tool_call_id":"call_1","content":"contenu du fichier — é 🌍"})
    );
}

fn resp_tool_turn() -> Reply {
    let reasoning = json!({"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"Je cherche"}],"encrypted_content":"ENC-ÉÉ"});
    let text = sse_events(&[
        (
            Some("response.created"),
            json!({"type":"response.created","response":{"id":"resp_1","model":"vendor-resp"}}),
        ),
        (
            Some("response.output_item.added"),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}),
        ),
        (
            Some("response.reasoning_summary_text.delta"),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"Je cherche"}),
        ),
        (
            Some("response.output_item.done"),
            json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
        ),
        (
            Some("response.output_item.added"),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_r1","name":"read_file"}}),
        ),
        (
            Some("response.function_call_arguments.delta"),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"é🌍"}),
        ),
        (
            Some("response.function_call_arguments.delta"),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":".txt\"}"}),
        ),
        (
            Some("response.output_item.done"),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_r1","name":"read_file","arguments":"{\"path\":\"é🌍.txt\"}"}}),
        ),
        (
            Some("response.completed"),
            json!({"type":"response.completed","response":{"usage":{"input_tokens":50,"output_tokens":30,"output_tokens_details":{"reasoning_tokens":10}}}}),
        ),
    ]);
    Reply::Sse {
        chunks: fragment(&text, 9),
    }
}

#[tokio::test]
async fn responses_tool_exchange_echoes_the_reasoning_item() {
    let (mock, reg) = setup(vec![resp_tool_turn(), text_reply("resp")]);
    let p = provider(&reg, "mock/resp");
    let first = vec![Message::user("Lis é🌍.txt")];

    let r1 = run(&p, first_request(first.clone(), true)).await.unwrap();
    assert_eq!(r1.stop_reason, StopReason::ToolUse);
    let ContentBlock::ToolUse { id, input, .. } = r1.message.get_tool_use_blocks()[0] else {
        panic!()
    };
    assert_eq!(
        (id.as_str(), input.clone()),
        ("call_r1", json!({"path":"é🌍.txt"}))
    );
    assert_eq!(r1.usage.reasoning_tokens, 10);

    let r2 = run(
        &p,
        first_request(history_after(first, &r1, "call_r1"), true),
    )
    .await
    .unwrap();
    assert_eq!(r2.message.get_all_text(), "Voilà.");

    let reqs = mock.requests();
    assert_eq!(reqs[0].path, "/v1/responses");
    assert_eq!(reqs[0].body["instructions"], "You are helpful.");
    assert_eq!(reqs[0].body["model"], "vendor-resp");
    assert_eq!(reqs[0].body["tools"][0]["name"], "read_file");
    let input = reqs[1].body["input"].as_array().unwrap();
    assert_eq!(input[1]["type"], "reasoning");
    assert_eq!(
        input[1]["encrypted_content"], "ENC-ÉÉ",
        "opaque data returns verbatim"
    );
    assert_eq!(
        input[2],
        json!({"type":"function_call","call_id":"call_r1","name":"read_file","arguments":"{\"path\":\"é🌍.txt\"}"})
    );
    assert_eq!(
        input[3],
        json!({"type":"function_call_output","call_id":"call_r1","output":"contenu du fichier — é 🌍"})
    );
}

fn anth_tool_turn() -> Reply {
    let text = sse_events(&[
        (
            Some("message_start"),
            json!({"type":"message_start","message":{"id":"msg_1","model":"vendor-anth","usage":{"input_tokens":100,"cache_creation_input_tokens":1000,"cache_read_input_tokens":2000,"output_tokens":1,"cache_creation":{"ephemeral_5m_input_tokens":600,"ephemeral_1h_input_tokens":400}}}}),
        ),
        (
            Some("content_block_start"),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}),
        ),
        (
            Some("content_block_delta"),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Je cherche 🌍"}}),
        ),
        (
            Some("content_block_delta"),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG-éé"}}),
        ),
        (
            Some("content_block_stop"),
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            Some("content_block_start"),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}),
        ),
        (
            Some("content_block_delta"),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"é"}}),
        ),
        (
            Some("content_block_delta"),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"🌍.txt\"}"}}),
        ),
        (
            Some("content_block_stop"),
            json!({"type":"content_block_stop","index":1}),
        ),
        (
            Some("message_delta"),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":50}}),
        ),
        (Some("message_stop"), json!({"type":"message_stop"})),
    ]);
    Reply::Sse {
        chunks: fragment(&text, 11),
    }
}

#[tokio::test]
async fn anthropic_tool_exchange_keeps_thinking_signature() {
    let (mock, reg) = setup(vec![anth_tool_turn(), text_reply("anth")]);
    let p = provider(&reg, "mock/anth");
    let first = vec![Message::user("Lis é🌍.txt")];

    let r1 = run(&p, first_request(first.clone(), true)).await.unwrap();
    assert_eq!(r1.stop_reason, StopReason::ToolUse);
    let ContentBlock::ToolUse { id, input, .. } = r1.message.get_tool_use_blocks()[0] else {
        panic!()
    };
    assert_eq!(
        (id.as_str(), input.clone()),
        ("toolu_1", json!({"path":"é🌍.txt"}))
    );
    assert_eq!(
        (
            r1.usage.input_tokens,
            r1.usage.cache_read_input_tokens,
            r1.usage.cache_creation_input_tokens,
            r1.usage.output_tokens
        ),
        (100, 2000, 1000, 50)
    );

    let r2 = run(
        &p,
        first_request(history_after(first, &r1, "toolu_1"), true),
    )
    .await
    .unwrap();
    assert_eq!(r2.message.get_all_text(), "Voilà.");

    let reqs = mock.requests();
    assert_eq!(reqs[0].path, "/v1/messages");
    assert_eq!(reqs[0].headers["x-api-key"], KEY);
    assert!(!reqs[0].headers.contains_key("authorization"));
    assert_eq!(reqs[0].headers["anthropic-version"], "2023-06-01");
    assert_eq!(reqs[0].body["model"], "vendor-anth");
    let m = reqs[1].body["messages"].as_array().unwrap();
    assert_eq!(
        m[1]["content"][0],
        json!({"type":"thinking","thinking":"Je cherche 🌍","signature":"SIG-éé"})
    );
    assert_eq!(m[1]["content"][1]["type"], "tool_use");
    assert_eq!(m[2]["content"][0]["type"], "tool_result");
    assert_eq!(m[2]["content"][0]["tool_use_id"], "toolu_1");
}

// ─── Non-streaming ───────────────────────────────────────────────────────────

#[tokio::test]
async fn non_streaming_models_use_a_blocking_request_for_every_protocol() {
    let chat = json!({"id":"c","model":"x","choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":"ok","tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}]}}],"usage":{"prompt_tokens":5,"completion_tokens":2}});
    let resp = json!({"id":"r","model":"x","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"ok"}]},{"type":"function_call","call_id":"c1","name":"read_file","arguments":"{\"path\":\"a\"}"}],"usage":{"input_tokens":5,"output_tokens":2}});
    let anth = json!({"id":"m","model":"x","stop_reason":"tool_use","content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"c1","name":"read_file","input":{"path":"a"}}],"usage":{"input_tokens":5,"output_tokens":2}});
    for (sel, body, path) in [
        ("mock/chat_ns", chat, "/v1/chat/completions"),
        ("mock/resp_ns", resp, "/v1/responses"),
        ("mock/anth_ns", anth, "/v1/messages"),
    ] {
        let (mock, reg) = setup(vec![Reply::Json {
            status: "200 OK",
            headers: vec![],
            body: body.to_string(),
        }]);
        let r = run(
            &provider(&reg, sel),
            first_request(vec![Message::user("hi")], true),
        )
        .await
        .unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse, "{sel}");
        assert_eq!(r.message.get_all_text(), "ok", "{sel}");
        assert_eq!(r.message.get_tool_use_blocks().len(), 1, "{sel}");
        let reqs = mock.requests();
        assert_eq!(reqs[0].path, path);
        assert_eq!(reqs[0].body["stream"], false, "{sel}");
        assert!(reqs[0].body.get("stream_options").is_none());
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

async fn error_of(reg: &ProviderRegistry, sel: &str) -> CerseiError {
    let p = provider(reg, sel);
    match p
        .complete(first_request(vec![Message::user("hi")], false))
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("complete() returned Ok for a failing response"),
    }
}

#[tokio::test]
async fn http_statuses_are_typed_and_retryable_from_complete_itself() {
    for sel in ["mock/chat", "mock/resp", "mock/anth"] {
        let (_, reg) = setup(vec![Reply::Json {
            status: "429 Too Many Requests",
            headers: vec![("Retry-After", "7")],
            body: r#"{"error":"slow down"}"#.into(),
        }]);
        match error_of(&reg, sel).await {
            CerseiError::RateLimit { retry_after, .. } => {
                assert_eq!(retry_after, Some(std::time::Duration::from_secs(7)))
            }
            other => panic!("{sel}: {other:?}"),
        }
        // A 503's delay reaches the error too, in seconds or as a date.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(600);
        let date: &'static str = Box::leak(httpdate::fmt_http_date(later).into_boxed_str());
        for value in ["12", date] {
            let (_, reg) = setup(vec![Reply::Json {
                status: "503 Service Unavailable",
                headers: vec![("Retry-After", value)],
                body: r#"{"error":"busy"}"#.into(),
            }]);
            let e = error_of(&reg, sel).await;
            assert!(
                e.is_retryable() && e.http_status() == Some(503),
                "{sel}: {e:?}"
            );
            let d = e
                .retry_after()
                .unwrap_or_else(|| panic!("{sel} {value}: {e:?}"));
            if value == "12" {
                assert_eq!(d, std::time::Duration::from_secs(12));
            } else {
                assert!(d > std::time::Duration::from_secs(590), "{d:?}");
            }
        }
        // A definitive error keeps its delay and stays definitive.
        let (_, reg) = setup(vec![Reply::Json {
            status: "400 Bad Request",
            headers: vec![("Retry-After", "5")],
            body: "{}".into(),
        }]);
        let e = error_of(&reg, sel).await;
        assert!(
            !e.is_retryable() && e.retry_after().is_some(),
            "{sel}: {e:?}"
        );
        let (_, reg) = setup(vec![Reply::Json {
            status: "529 Overloaded",
            headers: vec![],
            body: "{}".into(),
        }]);
        let e = error_of(&reg, sel).await;
        assert!(e.is_retryable(), "{sel}: {e:?}");
        let (_, reg) = setup(vec![Reply::Json {
            status: "401 Unauthorized",
            headers: vec![],
            body: r#"{"error":"bad key"}"#.into(),
        }]);
        let e = error_of(&reg, sel).await;
        assert!(
            matches!(e, CerseiError::ProviderStatus { status: 401, .. }) && !e.is_retryable(),
            "{sel}: {e:?}"
        );
    }
}

#[tokio::test]
async fn an_error_body_that_echoes_the_key_is_redacted() {
    let body = format!(r#"{{"error":"invalid key {KEY} supplied"}}"#);
    let (_, reg) = setup(vec![Reply::Json {
        status: "401 Unauthorized",
        headers: vec![],
        body,
    }]);
    let e = error_of(&reg, "mock/chat").await.to_string();
    assert!(!e.contains(KEY), "{e}");
    assert!(e.contains("***"), "{e}");
}

#[tokio::test]
async fn connection_refused_is_a_retryable_transport_error_without_the_url() {
    // Bind then drop to get a port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let reg = registry_for(&format!("http://127.0.0.1:{port}"), "");
    let e = error_of(&reg, "mock/chat").await;
    assert!(matches!(e, CerseiError::Http(_)), "{e:?}");
    assert!(e.is_retryable());
    assert!(
        !e.to_string().contains(&port.to_string()),
        "the URL is stripped from transport errors: {e}"
    );
}

#[tokio::test]
async fn a_connection_dropped_mid_stream_becomes_a_stream_error() {
    let partial = sse_events(&[(None, json!({"choices":[{"delta":{"content":"par"}}]}))]);
    let (_, reg) = setup(vec![Reply::Truncated {
        chunks: vec![partial.into_bytes()],
    }]);
    let e = run(
        &provider(&reg, "mock/chat"),
        first_request(vec![Message::user("hi")], false),
    )
    .await
    .unwrap_err();
    assert!(matches!(e, CerseiError::Provider(_)), "{e:?}");
}

#[tokio::test]
async fn api_errors_inside_a_stream_fail_the_turn() {
    let chat = sse_events(&[(None, json!({"error":{"message":"overloaded now"}}))]);
    let resp = sse_events(&[(
        Some("response.failed"),
        json!({"type":"response.failed","response":{"error":{"message":"overloaded now"}}}),
    )]);
    let anth = sse_events(&[
        (
            Some("message_start"),
            json!({"type":"message_start","message":{"id":"m","model":"x"}}),
        ),
        (
            Some("error"),
            json!({"type":"error","error":{"message":"overloaded now"}}),
        ),
    ]);
    for (sel, text) in [
        ("mock/chat", chat),
        ("mock/resp", resp),
        ("mock/anth", anth),
    ] {
        let (_, reg) = setup(vec![Reply::Sse {
            chunks: fragment(&text, 8),
        }]);
        let e = run(
            &provider(&reg, sel),
            first_request(vec![Message::user("hi")], false),
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("overloaded now"), "{sel}: {e}");
    }
}

// ─── Authentication ──────────────────────────────────────────────────────────

#[tokio::test]
async fn auth_modes_and_custom_headers() {
    let (mock, reg) = setup(vec![text_reply("chat"), text_reply("chat")]);
    // `none`: no credential header at all.
    run(
        &provider(&reg, "local/m"),
        first_request(vec![Message::user("hi")], false),
    )
    .await
    .unwrap();
    {
        let reqs = mock.requests();
        assert_eq!(
            reqs[0].path, "/openai/chat/completions",
            "gateway prefix kept"
        );
        assert!(
            !reqs[0].headers.contains_key("authorization")
                && !reqs[0].headers.contains_key("x-api-key")
        );
    }

    // api_key_header with a custom header name and an extra static header.
    let text = format!(
        r#"
schema_version = 1
[[providers]]
id = "p"
name = "P"
endpoint = "{base}/gw"
protocol = "chat_completions"
auth = "api_key_header"
auth_header = "api-key"
api_key_env = "BRICKS_MOCK_API_KEY"
headers = {{ "x-team" = "blue" }}
[[providers.models]]
id = "m"
name = "M"
api_model = "am"
streaming = true
[providers.models.limits]
max_input_tokens = 10
max_output_tokens = 10
"#,
        base = mock.base
    );
    let reg2 = ProviderRegistry::from_toml_str(&text, "t").unwrap();
    run(
        &provider(&reg2, "p/m"),
        first_request(vec![Message::user("hi")], false),
    )
    .await
    .unwrap();
    let reqs = mock.requests();
    assert_eq!(reqs[1].headers["api-key"], KEY);
    assert_eq!(reqs[1].headers["x-team"], "blue");
    assert!(!reqs[1].headers.contains_key("authorization"));
}

#[test]
fn secret_resolution_from_the_environment_reference() {
    let text = r#"
schema_version = 1
[[providers]]
id = "p"
name = "P"
endpoint = "http://127.0.0.1:1"
protocol = "chat_completions"
auth = "bearer"
api_key_env = "BRICKS_TEST_KEY_VAR"
[[providers.models]]
id = "m"
name = "M"
api_model = "am"
[providers.models.limits]
max_input_tokens = 10
max_output_tokens = 10
"#;
    let reg = ProviderRegistry::from_toml_str(text, "t").unwrap();
    let m = reg.resolve("p/m").unwrap();
    // Unset or blank -> an actionable error naming the variable, never a value.
    for value in [None, Some("   ".to_string())] {
        let v = value.clone();
        let e = m.provider().env(move |_| v.clone()).build().unwrap_err();
        assert!(matches!(e, CerseiError::Auth(_)));
        assert!(e.to_string().contains("BRICKS_TEST_KEY_VAR"), "{e}");
    }
    let p = m
        .provider()
        .env(|name| (name == "BRICKS_TEST_KEY_VAR").then(|| "  sk-from-env-9999\n".to_string()))
        .build()
        .unwrap();
    assert!(!format!("{p:?}").contains("sk-from-env-9999"));
}

// ─── Reasoning profiles ──────────────────────────────────────────────────────

async fn body_for(
    reg: &ProviderRegistry,
    mock: &Mock,
    p: &ConfiguredProvider,
    option: Option<&str>,
) -> Value {
    let mut r = first_request(vec![Message::user("hi")], false);
    if let Some(id) = option {
        r.options.set("reasoning_profile", id);
    }
    let before = mock.requests().len();
    let _ = reg;
    run(p, r).await.unwrap();
    mock.requests()[before].body.clone()
}

#[tokio::test]
async fn profiles_are_merged_removed_and_selected_as_configured() {
    let (mock, reg) = setup(vec![
        text_reply("chat"),
        text_reply("chat"),
        text_reply("chat"),
        text_reply("chat"),
    ]);
    let p = provider(&reg, "mock/think");

    // Default profile `deep`: model parameters first, profile on top (recursive merge).
    let b = body_for(&reg, &mock, &p, None).await;
    assert_eq!(b["reasoning"], json!({"summary":"auto","effort":"high"}));
    assert_eq!(b["temperature"], 0.5);
    assert!(b.get("thinking").is_none());

    // Per-request selection: `fast`.
    let b = body_for(&reg, &mock, &p, Some("fast")).await;
    assert_eq!(b["reasoning"]["effort"], "low");

    // `off` removes inherited options, then sends its own mechanism.
    let b = body_for(&reg, &mock, &p, Some("off")).await;
    assert!(b.get("reasoning").is_none() && b.get("temperature").is_none());
    assert_eq!(b["thinking"], json!({"type":"disabled"}));

    // `ultra` is a local alias: the name never reaches the wire, only the parameters.
    let b = body_for(&reg, &mock, &p, Some("ultra")).await;
    assert_eq!(b["reasoning"]["effort"], "high");
    assert_eq!(b["extra_budget"], 99999);
    assert!(!b.to_string().contains("ultra"));
    // Structural content is untouched by all of it.
    assert_eq!(b["model"], "vendor-think");
    assert_eq!(b["messages"][1], json!({"role":"user","content":"hi"}));
}

#[tokio::test]
async fn unknown_profile_and_empty_profile_lists_never_send_a_request() {
    let (mock, reg) = setup(vec![]);
    let p = provider(&reg, "mock/think");
    let mut r = first_request(vec![Message::user("hi")], false);
    r.options.set("reasoning_profile", "mythic");
    let e = p.complete(r).await.err().unwrap();
    assert!(matches!(e, CerseiError::Config(_)));
    assert!(
        e.to_string().contains("unknown reasoning profile `mythic`")
            && e.to_string().contains("fast, deep, off, ultra"),
        "{e}"
    );

    let plain = provider(&reg, "mock/plain");
    let mut r = first_request(vec![Message::user("hi")], false);
    r.options.set("reasoning_profile", "high");
    let e = plain.complete(r).await.err().unwrap();
    assert!(e.to_string().contains("exposes no profiles"), "{e}");
    assert!(mock.requests().is_empty(), "nothing was sent");

    // Builder-level selection fails at build time, not mid-run.
    assert!(model(&reg, "mock/think")
        .provider()
        .reasoning_profile("mythic")
        .build()
        .is_err());
    let built = model(&reg, "mock/think")
        .provider()
        .env(|_| Some(KEY.to_string()))
        .reasoning_profile("fast")
        .build()
        .unwrap();
    assert!(format!("{built:?}").contains("fast"));
}

// ─── Modalities ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn untransportable_or_undeclared_input_is_refused_before_sending() {
    let (mock, reg) = setup(vec![]);
    let cases: Vec<(&str, ContentBlock, &str)> = vec![
        (
            "mock/anth",
            ContentBlock::audio_base64("audio/wav", "AA=="),
            "audio block cannot be sent",
        ),
        (
            "mock/resp",
            ContentBlock::audio_base64("audio/wav", "AA=="),
            "audio block cannot be sent",
        ),
        (
            "mock/chat",
            ContentBlock::video_url("https://x/v.mp4"),
            "video block cannot be sent",
        ),
        (
            "mock/videomodel",
            ContentBlock::video_url("https://x/v.mp4"),
            "declared, but the `chat_completions` adapter cannot transport it",
        ),
        (
            "mock/chat",
            ContentBlock::audio_url("https://x/a.wav"),
            "cannot carry audio as URL",
        ),
        (
            "mock/chat",
            ContentBlock::document_bytes("application/zip", b"PK"),
            "document_mime_types",
        ),
    ];
    for (sel, block, expect) in cases {
        let r = first_request(
            vec![Message::user_blocks(vec![
                ContentBlock::Text { text: "x".into() },
                block,
            ])],
            false,
        );
        let e = provider(&reg, sel).complete(r).await.err().unwrap();
        assert!(matches!(e, CerseiError::Unsupported(_)), "{sel}: {e:?}");
        assert!(e.to_string().contains(expect), "{sel}: {e}");
        assert!(e.to_string().contains(sel), "the error names the model");
    }
    // Requested outputs that no adapter carries (declared or not).
    let mut r = first_request(vec![Message::user("x")], false);
    r.output_modalities = vec![Modality::Audio];
    let e = provider(&reg, "mock/videomodel")
        .complete(r)
        .await
        .err()
        .unwrap();
    assert!(
        e.to_string()
            .contains("declared, but the `chat_completions` adapter cannot transport audio output"),
        "{e}"
    );
    // Tools on a model that does not declare them.
    let e = provider(&reg, "local/m")
        .complete(first_request(vec![Message::user("x")], true))
        .await
        .err()
        .unwrap();
    assert!(e.to_string().contains("tool_calls"), "{e}");
    assert!(mock.requests().is_empty(), "no request left the process");
}

#[tokio::test]
async fn supported_media_goes_on_the_wire_in_each_protocol_form() {
    let (mock, reg) = setup(vec![
        text_reply("chat"),
        text_reply("resp"),
        text_reply("anth"),
    ]);
    let msg = || {
        Message::user_blocks(vec![
            ContentBlock::Text {
                text: "regarde".into(),
            },
            ContentBlock::image_base64("image/png", "QUJD"),
            ContentBlock::document_base64("application/pdf", "JVBE"),
        ])
    };
    for sel in ["mock/chat", "mock/resp", "mock/anth"] {
        run(&provider(&reg, sel), first_request(vec![msg()], false))
            .await
            .unwrap();
    }
    let reqs = mock.requests();
    let chat = reqs[0].body["messages"][1]["content"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(chat[1]["type"], "image_url");
    assert_eq!(chat[2]["type"], "file");
    let resp = reqs[1].body["input"][0]["content"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(resp[1]["type"], "input_image");
    assert_eq!(resp[2]["type"], "input_file");
    let anth = reqs[2].body["messages"][0]["content"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(anth[1]["type"], "image");
    assert_eq!(anth[2]["type"], "document");
}

// ─── Limits and cost ─────────────────────────────────────────────────────────

#[tokio::test]
async fn limits_cap_the_output_and_report_the_prompt_budget() {
    let (mock, reg) = setup(vec![text_reply("chat")]);
    let p = provider(&reg, "mock/chat");
    // 96000 input, 4000 output, 100000 shared window: min(96000, 100000-4000).
    assert_eq!(p.context_window("anything"), 96000);
    let mut r = first_request(vec![Message::user("hi")], false);
    r.max_tokens = 50_000;
    run(&p, r).await.unwrap();
    assert_eq!(mock.requests()[0].body["max_tokens"], 4000);
}

#[tokio::test]
async fn cost_is_estimated_from_known_prices_only() {
    // chat: input 1000 (uncached) * $1 + cached 1000 * $0.1 + output 500 * $2 per million.
    let (_, reg) = setup(vec![chat_tool_turn()]);
    let r = run(
        &provider(&reg, "mock/chat"),
        first_request(vec![Message::user("hi")], true),
    )
    .await
    .unwrap();
    let e = r.usage.cost_estimate.expect("priced model");
    assert!((e.amount_usd - 0.0021).abs() < 1e-12, "{e:?}");
    assert!(!e.partial && e.tariff == "default");
    assert_eq!(r.usage.cost_usd, Some(e.amount_usd));

    // anthropic: cache write split by retention (400 at the 1h price, 600 generic).
    let (_, reg) = setup(vec![anth_tool_turn()]);
    let r = run(
        &provider(&reg, "mock/anth"),
        first_request(vec![Message::user("hi")], true),
    )
    .await
    .unwrap();
    let e = r.usage.cost_estimate.expect("priced model");
    let expected = 100.0 * 3.0 / 1e6
        + 50.0 * 15.0 / 1e6
        + 2000.0 * 0.3 / 1e6
        + 400.0 * 6.0 / 1e6
        + 600.0 * 3.75 / 1e6;
    assert!(
        (e.amount_usd - expected).abs() < 1e-12,
        "{e:?} vs {expected}"
    );
    assert!(!e.partial);

    // A model with no prices: unknown, never zero.
    let (_, reg) = setup(vec![resp_tool_turn()]);
    let r = run(
        &provider(&reg, "mock/resp"),
        first_request(vec![Message::user("hi")], true),
    )
    .await
    .unwrap();
    assert!(r.usage.cost_estimate.is_none() && r.usage.cost_usd.is_none());

    // Media in the request: the estimate says it is partial.
    let (_, reg) = setup(vec![chat_tool_turn()]);
    let msg = Message::user_blocks(vec![
        ContentBlock::Text { text: "x".into() },
        ContentBlock::image_base64("image/png", "QUJD"),
    ]);
    let r = run(&provider(&reg, "mock/chat"), first_request(vec![msg], true))
        .await
        .unwrap();
    let e = r.usage.cost_estimate.unwrap();
    assert!(
        e.partial && e.unpriced.iter().any(|u| u.starts_with("image")),
        "{e:?}"
    );
}

#[tokio::test]
async fn an_unknown_tariff_is_refused_at_build_time() {
    let (_, reg) = setup(vec![]);
    let e = model(&reg, "mock/chat")
        .provider()
        .tariff("offpeak")
        .build()
        .unwrap_err();
    assert!(e.to_string().contains("unknown tariff `offpeak`"), "{e}");
    let e = model(&reg, "mock/resp")
        .provider()
        .tariff("x")
        .build()
        .unwrap_err();
    assert!(e.to_string().contains("no pricing"), "{e}");
}
