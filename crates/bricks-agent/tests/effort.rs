//! Reasoning profiles through the real controller and real providers: the
//! profiles come from a providers TOML (labels ≠ ids, parameters of two
//! protocols), and every HTTP body is captured by a local server. Checks
//! the default, a switch, the reset to the model's default, a profile named
//! `default`, refusals, a switch during a stream and during a retry, and
//! the accounting of a response sent before a switch.

#[allow(dead_code)]
mod common;

use bricks_agent::BricksConfig;
use bricks_agent::permission::{PermissionProfile, SessionPolicy};
use bricks_agent::control::*;
use bricks_provider::ProviderRegistry;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ─── Local server ────────────────────────────────────────────────────────────

/// What the server answers, in order (then `Text` forever).
#[derive(Clone)]
enum Answer {
    /// A plain answer, with usage.
    Text,
    /// A tool call whose stream stops half-way until `release` is notified.
    HeldTool,
    /// 503, retryable.
    Unavailable,
}

struct Server {
    url: String,
    bodies: Arc<Mutex<Vec<Value>>>,
    /// Notified when the held stream started (its first bytes were sent).
    started: Arc<tokio::sync::Notify>,
    release: Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    /// Notified when a 503 was sent.
    refused: Arc<tokio::sync::Notify>,
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|e| format!("data: {e}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n"
}

fn serve(answers: Vec<Answer>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(tokio::sync::Notify::new());
    let refused = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(std::sync::Mutex::new(None));
    let (b, s, r, rel) = (
        bodies.clone(),
        started.clone(),
        refused.clone(),
        release.clone(),
    );
    std::thread::spawn(move || {
        let mut answers = answers.into_iter();
        while let Ok((mut sock, _)) = listener.accept() {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
            let head_end = loop {
                match sock.read(&mut tmp) {
                    Ok(0) | Err(_) => break None,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(p + 4);
                }
            };
            let Some(head_end) = head_end else { continue };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while buf.len() < head_end + len {
                match sock.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            b.lock()
                .unwrap()
                .push(serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null));
            let usage = serde_json::json!({"prompt_tokens": 1000, "completion_tokens": 10});
            let tool = |id: &str| {
                vec![
                    serde_json::json!({"choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{
                        "index": 0, "id": id, "type": "function",
                        "function": {"name": "Glob", "arguments": "{\"pattern\":\"*.zz\"}"}}]}}]}),
                    serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}], "usage": usage}),
                ]
            };
            match answers.next().unwrap_or(Answer::Text) {
                Answer::Unavailable => {
                    let _ = sock.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbusy",
                    );
                    let _ = sock.flush();
                    drop(sock);
                    r.notify_one();
                }
                Answer::HeldTool => {
                    let ev = tool("t_held");
                    let _ = sock.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                    );
                    let _ = sock.write_all(format!("data: {}\n\n", ev[0]).as_bytes());
                    let _ = sock.flush();
                    let (tx, rx) = std::sync::mpsc::channel();
                    *rel.lock().unwrap() = Some(tx);
                    s.notify_one();
                    let _ = rx.recv_timeout(Duration::from_secs(10));
                    let _ =
                        sock.write_all(format!("data: {}\n\ndata: [DONE]\n\n", ev[1]).as_bytes());
                    let _ = sock.flush();
                    let _ = sock.shutdown(std::net::Shutdown::Write);
                }
                Answer::Text => {
                    let body = sse(&[
                        serde_json::json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": "ok"}}]}),
                        serde_json::json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}),
                    ]);
                    let _ = sock.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    );
                    let _ = sock.flush();
                    let _ = sock.shutdown(std::net::Shutdown::Write);
                }
            }
        }
    });
    Server {
        url: format!("http://127.0.0.1:{port}/v1"),
        bodies,
        started,
        release,
        refused,
    }
}

// ─── Configuration ───────────────────────────────────────────────────────────

/// Three models: `a` (chat completions; labels differ from ids; a profile
/// literally named `default`; declared default `high`), `c` (Anthropic
/// messages, thinking parameters, no declared default), `plain` (no
/// profiles).
fn registry(url: &str) -> Arc<ProviderRegistry> {
    let toml = format!(
        r#"
schema_version = 1

[[providers]]
id = "local"
name = "Local"
endpoint = "{url}"
protocol = "chat_completions"
auth = "none"

[[providers.models]]
id = "a"
name = "Example Model"
api_model = "model-a"
streaming = true
[providers.models.limits]
max_input_tokens = 100000
max_output_tokens = 4096
[providers.models.reasoning]
default = "high"
[[providers.models.reasoning.profiles]]
id = "low"
label = "Low"
parameters = {{ reasoning_effort = "low" }}
[[providers.models.reasoning.profiles]]
id = "medium"
label = "Medium"
parameters = {{ reasoning_effort = "medium" }}
[[providers.models.reasoning.profiles]]
id = "high"
label = "High"
parameters = {{ reasoning_effort = "high" }}
[[providers.models.reasoning.profiles]]
id = "default"
label = "Profile named default"
parameters = {{ reasoning_effort = "minimal" }}

[[providers.models]]
id = "c"
name = "Claude-like"
api_model = "model-c"
protocol = "anthropic_messages"
auth = "none"
streaming = true
[providers.models.limits]
max_input_tokens = 100000
max_output_tokens = 8192
[[providers.models.reasoning.profiles]]
id = "Deep_Think"
label = "Deep"
parameters = {{ thinking = {{ type = "enabled", budget_tokens = 2048 }} }}

[[providers.models]]
id = "plain"
name = "Plain"
api_model = "model-plain"
streaming = true
[providers.models.limits]
max_input_tokens = 100000
max_output_tokens = 4096
"#
    );
    Arc::new(ProviderRegistry::from_toml_str(&toml, "effort-test.toml").expect("valid config"))
}

struct Env {
    ctl: Controller,
    events: EventStream,
    server: Server,
    _dir: tempfile::TempDir,
}

async fn open(answers: Vec<Answer>) -> Env {
    let server = serve(answers);
    let dir = tempfile::tempdir().unwrap();
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("local/a".into());
    bricks.semantic.lsp.enabled = false;
    let mut cfg = EngineConfig::new(
        dir.path(),
        registry(&server.url),
        bricks,
        dir.path().join(".sessions"),
    );
    cfg.permissions = Some(common::default_store(
        dir.path(),
        &SessionPolicy::chosen(PermissionProfile::Execute),
    ));
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    let opened = next(&mut events).await;
    let Event::SessionOpened {
        reasoning,
        effective_reasoning,
        ..
    } = opened.event
    else {
        panic!("{opened:?}")
    };
    assert_eq!(reasoning, None, "no override");
    assert_eq!(
        effective_reasoning.as_deref(),
        Some("high"),
        "the declared default"
    );
    Env {
        ctl,
        events,
        server,
        _dir: dir,
    }
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(20), events.next())
        .await
        .expect("an event within 20 s")
        .expect("stream open")
}

async fn until(events: &mut EventStream, kind: &str) -> Envelope {
    loop {
        let e = next(events).await;
        if e.event.kind() == kind {
            return e;
        }
    }
}

async fn run(env: &mut Env, text: &str) -> RunOutcome {
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text(text),
        })
        .unwrap();
    let e = until(&mut env.events, "run_finished").await;
    env.ctl.wait_idle().await;
    match e.event {
        Event::RunFinished { outcome, .. } => outcome,
        _ => unreachable!(),
    }
}

fn effort(body: &Value) -> Option<String> {
    body.get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn bodies(env: &Env) -> Vec<Value> {
    env.server.bodies.lock().unwrap().clone()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_declared_default_is_sent_then_a_switch_and_the_reset_reach_the_wire() {
    let mut env = open(vec![]).await;
    run(&mut env, "one").await;
    assert_eq!(effort(&bodies(&env)[0]).as_deref(), Some("high"));

    env.ctl
        .send(Command::SetReasoning {
            profile: Some("medium".into()),
        })
        .unwrap();
    let Event::ReasoningChanged {
        requested,
        previous,
        previous_label,
        effective,
        effective_label,
        applies,
        ..
    } = until(&mut env.events, "reasoning_changed").await.event
    else {
        unreachable!()
    };
    assert_eq!(requested.as_deref(), Some("medium"));
    assert_eq!(
        (previous.as_deref(), previous_label.as_deref()),
        (Some("high"), Some("High"))
    );
    assert_eq!(
        (effective.as_deref(), effective_label.as_deref()),
        (Some("medium"), Some("Medium"))
    );
    assert_eq!(applies, "next_run");
    run(&mut env, "two").await;
    assert_eq!(effort(&bodies(&env)[1]).as_deref(), Some("medium"));

    // Back to the model's default: not "keep medium", not "no reasoning".
    env.ctl
        .send(Command::SetReasoning { profile: None })
        .unwrap();
    until(&mut env.events, "reasoning_changed").await;
    let snap = env.ctl.snapshot();
    assert_eq!(snap.reasoning, None);
    assert_eq!(snap.effective_reasoning.as_deref(), Some("high"));
    run(&mut env, "three").await;
    assert_eq!(effort(&bodies(&env)[2]).as_deref(), Some("high"));
}

#[tokio::test]
async fn a_profile_named_default_is_not_the_reset() {
    let mut env = open(vec![]).await;
    env.ctl
        .send(Command::SetReasoning {
            profile: Some("default".into()),
        })
        .unwrap();
    until(&mut env.events, "reasoning_changed").await;
    run(&mut env, "x").await;
    assert_eq!(effort(&bodies(&env)[0]).as_deref(), Some("minimal"));
    assert_eq!(env.ctl.snapshot().reasoning.as_deref(), Some("default"));
    env.ctl
        .send(Command::SetReasoning { profile: None })
        .unwrap();
    until(&mut env.events, "reasoning_changed").await;
    run(&mut env, "y").await;
    assert_eq!(effort(&bodies(&env)[1]).as_deref(), Some("high"));
}

#[tokio::test]
async fn unknown_profiles_and_models_without_profiles_change_nothing() {
    let mut env = open(vec![]).await;
    let e = env
        .ctl
        .send(Command::SetReasoning {
            profile: Some("ultra".into()),
        })
        .unwrap_err();
    assert!(
        e.contains("ultra") && e.contains("low, medium, high, default"),
        "{e}"
    );
    until(&mut env.events, "command_rejected").await;
    assert_eq!(env.ctl.snapshot().reasoning, None);
    assert!(bodies(&env).is_empty(), "no provider call");

    // Another protocol, a mixed-case id and its parameters.
    env.ctl
        .send(Command::SetModel {
            model: Some("local/c".into()),
            reasoning: Some("Deep_Think".into()),
        })
        .unwrap();
    until(&mut env.events, "model_changed").await;
    run(&mut env, "x").await;
    let b = bodies(&env)[0].clone();
    assert_eq!(b["thinking"]["budget_tokens"], 2048, "{b}");
    assert!(b.get("reasoning_effort").is_none());

    env.ctl
        .send(Command::SetModel {
            model: Some("local/plain".into()),
            reasoning: None,
        })
        .unwrap();
    let Event::ModelChanged {
        effective_reasoning,
        ..
    } = until(&mut env.events, "model_changed").await.event
    else {
        unreachable!()
    };
    assert_eq!(effective_reasoning, None, "no artificial `none` profile");
    let e = env
        .ctl
        .send(Command::SetReasoning {
            profile: Some("low".into()),
        })
        .unwrap_err();
    assert_eq!(e, "No reasoning profiles configured for this model.");
    run(&mut env, "y").await;
    let b = bodies(&env)[1].clone();
    assert!(
        b.get("reasoning_effort").is_none() && b.get("thinking").is_none(),
        "{b}"
    );
}

#[tokio::test]
async fn a_switch_during_a_stream_applies_to_the_next_request_only() {
    let mut env = open(vec![Answer::HeldTool, Answer::Text]).await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), env.server.started.notified())
        .await
        .unwrap();
    env.ctl
        .send(Command::SetReasoning {
            profile: Some("low".into()),
        })
        .unwrap();
    let Event::ReasoningChanged { applies, .. } =
        until(&mut env.events, "reasoning_changed").await.event
    else {
        unreachable!()
    };
    assert_eq!(applies, "next_turn");
    let history_before = env.ctl.history().await.len();
    env.server
        .release
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(())
        .unwrap();
    let e = until(&mut env.events, "run_finished").await;
    assert!(matches!(
        e.event,
        Event::RunFinished {
            outcome: RunOutcome::Succeeded,
            ..
        }
    ));
    env.ctl.wait_idle().await;
    let b = bodies(&env);
    assert_eq!(
        effort(&b[0]).as_deref(),
        Some("high"),
        "the request in flight"
    );
    assert_eq!(effort(&b[1]).as_deref(), Some("low"), "the next request");
    // The conversation went on (the tool call, its result, the answer).
    assert!(env.ctl.history().await.len() > history_before);

    // The response sent before the switch counts once in the totals, but
    // does not measure the context of the new selection.
    let snap = env.ctl.snapshot();
    assert_eq!(snap.context.totals.requests, 2);
    assert_eq!(snap.context.totals.input_tokens, 2000);
}

#[tokio::test]
async fn an_old_response_does_not_restore_an_invalidated_measurement() {
    let mut env = open(vec![Answer::HeldTool, Answer::Text]).await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), env.server.started.notified())
        .await
        .unwrap();
    env.ctl
        .send(Command::SetReasoning {
            profile: Some("low".into()),
        })
        .unwrap();
    until(&mut env.events, "reasoning_changed").await;
    env.server
        .release
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(())
        .unwrap();
    // The first `context` event follows the old response.
    let Event::Context { status } = until(&mut env.events, "context").await.event else {
        unreachable!()
    };
    assert_eq!(
        status.context_used.provenance,
        bricks_agent::Provenance::Estimated,
        "measured under the previous selection: not a measurement of this one"
    );
    assert_eq!(status.context_used.measured_tokens, 0);
    assert_eq!(status.totals.requests, 1, "counted once all the same");
    until(&mut env.events, "run_finished").await;
    env.ctl.wait_idle().await;
    // The next response, sent with the new selection, measures again (its
    // own answer is estimated on top: `mixed`).
    let used = env.ctl.snapshot().context.context_used;
    assert!(used.measured_tokens > 0, "{used:?}");
    assert_ne!(used.provenance, bricks_agent::Provenance::Estimated);
}

#[tokio::test]
async fn a_retried_request_keeps_its_original_selection() {
    let mut env = open(vec![Answer::Unavailable, Answer::Text]).await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), env.server.refused.notified())
        .await
        .unwrap();
    // During the backoff before the retry.
    env.ctl
        .send(Command::SetReasoning {
            profile: Some("low".into()),
        })
        .unwrap();
    until(&mut env.events, "run_finished").await;
    env.ctl.wait_idle().await;
    let b = bodies(&env);
    assert_eq!(b.len(), 2);
    assert_eq!(effort(&b[0]).as_deref(), Some("high"));
    assert_eq!(
        effort(&b[1]).as_deref(),
        Some("high"),
        "a retry is the same request"
    );
    run(&mut env, "next").await;
    assert_eq!(effort(&bodies(&env)[2]).as_deref(), Some("low"));
}

#[tokio::test]
async fn a_switch_keeps_the_conversation_and_cancellation_stays_immediate() {
    let mut env = open(vec![Answer::Text, Answer::HeldTool]).await;
    run(&mut env, "hello").await;
    let before = serde_json::to_string(&env.ctl.history().await).unwrap();
    let policy = env.ctl.snapshot().permission_policy.clone();
    env.ctl
        .send(Command::SetReasoning {
            profile: Some("low".into()),
        })
        .unwrap();
    until(&mut env.events, "reasoning_changed").await;
    assert_eq!(
        serde_json::to_string(&env.ctl.history().await).unwrap(),
        before
    );
    assert_eq!(env.ctl.snapshot().permission_policy, policy);
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), env.server.started.notified())
        .await
        .unwrap();
    let t = std::time::Instant::now();
    env.ctl.send(Command::Cancel).unwrap();
    let Event::RunFinished { outcome, .. } = until(&mut env.events, "run_finished").await.event
    else {
        unreachable!()
    };
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    let tx = env.server.release.lock().unwrap().take();
    if let Some(tx) = tx {
        let _ = tx.send(());
    }
}

#[test]
fn the_command_is_serialised_explicitly() {
    let reset: Command = serde_json::from_str(r#"{"type":"set_reasoning"}"#).unwrap();
    assert_eq!(reset, Command::SetReasoning { profile: None });
    let null: Command = serde_json::from_str(r#"{"type":"set_reasoning","profile":null}"#).unwrap();
    assert_eq!(null, Command::SetReasoning { profile: None });
    let named: Command =
        serde_json::from_str(r#"{"type":"set_reasoning","profile":"default"}"#).unwrap();
    assert_eq!(
        named,
        Command::SetReasoning {
            profile: Some("default".into())
        }
    );
    // The former meaning of `set_model` is unchanged.
    let m: Command = serde_json::from_str(r#"{"type":"set_model","model":"p/m"}"#).unwrap();
    assert_eq!(
        m,
        Command::SetModel {
            model: Some("p/m".into()),
            reasoning: None
        }
    );
}
