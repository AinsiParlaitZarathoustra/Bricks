//! Retries with the real runner, a spy provider and Tokio's virtual clock:
//! the server's `Retry-After` governs the wait when it is longer than the
//! local backoff, the notice says the same duration, five retries at most,
//! cancellation during a wait, definitive errors, and the stream boundary.

use cersei_agent::events::AgentEvent;
use cersei_agent::Agent;
use cersei_provider::{CompletionRequest, CompletionStream, Provider};
use cersei_types::{CerseiError, StreamEvent};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

enum Step {
    Fail(CerseiError),
    /// A stream answering `text`.
    Text(&'static str),
    /// A stream that emits text, then a transient-looking error.
    TextThenError,
}

struct Spy {
    steps: Mutex<VecDeque<Step>>,
    calls: Arc<Mutex<Vec<Instant>>>,
}

fn stream(events: Vec<StreamEvent>) -> CompletionStream {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        for e in events {
            if tx.send(e).await.is_err() {
                return;
            }
        }
    });
    CompletionStream::new(rx)
}

fn text_events(t: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::MessageStart {
            id: "m".into(),
            model: "spy".into(),
            usage: None,
        },
        StreamEvent::ContentBlockStart {
            index: 0,
            block_type: "text".into(),
            id: None,
            name: None,
        },
        StreamEvent::TextDelta {
            index: 0,
            text: t.into(),
        },
        StreamEvent::ContentBlockStop { index: 0 },
    ]
}

#[async_trait::async_trait]
impl Provider for Spy {
    fn name(&self) -> &str {
        "spy"
    }
    fn context_window(&self, _: &str) -> u64 {
        100_000
    }
    async fn complete(&self, _: CompletionRequest) -> cersei_types::Result<CompletionStream> {
        self.calls.lock().unwrap().push(Instant::now());
        match self.steps.lock().unwrap().pop_front() {
            Some(Step::Fail(e)) => Err(e),
            Some(Step::Text(t)) => {
                let mut ev = text_events(t);
                ev.push(StreamEvent::MessageStop);
                Ok(stream(ev))
            }
            Some(Step::TextThenError) => {
                let mut ev = text_events("partial ");
                ev.push(StreamEvent::Error {
                    message: "overloaded_error: please retry".into(),
                });
                Ok(stream(ev))
            }
            None => Err(CerseiError::Provider("no more steps".into())),
        }
    }
}

struct Run {
    agent: Arc<Agent>,
    calls: Arc<Mutex<Vec<Instant>>>,
    /// Status notices from the run's event stream and from the agent's
    /// callbacks (the two channels).
    stream_notices: Arc<Mutex<Vec<String>>>,
    callback_notices: Arc<Mutex<Vec<String>>>,
    cancel: tokio_util::sync::CancellationToken,
}

fn run_with(steps: Vec<Step>) -> Run {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let callback_notices = Arc::new(Mutex::new(Vec::new()));
    let cancel = tokio_util::sync::CancellationToken::new();
    let cb = callback_notices.clone();
    let agent = Agent::builder()
        .provider(Spy {
            steps: Mutex::new(steps.into()),
            calls: calls.clone(),
        })
        .model("spy")
        .max_turns(2)
        .cancel_token(cancel.clone())
        .on_event(move |e| {
            if let AgentEvent::Status(s) = e {
                if s.contains("Retrying") {
                    cb.lock().unwrap().push(s.clone());
                }
            }
        })
        .build()
        .unwrap();
    Run {
        agent: Arc::new(agent),
        calls,
        stream_notices: Arc::new(Mutex::new(Vec::new())),
        callback_notices,
        cancel,
    }
}

/// Run to the end, collecting the stream's retry notices.
async fn finish(r: &Run) -> Result<String, CerseiError> {
    let mut s = r.agent.run_stream("ping");
    let mut text = String::new();
    let mut error = None;
    while let Some(e) = s.next().await {
        match e {
            AgentEvent::Status(m) if m.contains("Retrying") => {
                r.stream_notices.lock().unwrap().push(m)
            }
            AgentEvent::TextDelta(t) => text.push_str(&t),
            AgentEvent::Error(m) => error = Some(m),
            _ => {}
        }
    }
    match error {
        Some(m) if m.contains("Cancelled") => Err(CerseiError::Cancelled),
        Some(m) => Err(CerseiError::Provider(m)),
        None => Ok(text),
    }
}

fn status(code: u16, after: Option<u64>) -> CerseiError {
    CerseiError::from_http_status(code, after.map(Duration::from_secs), "busy")
}

fn gaps(calls: &[Instant]) -> Vec<u128> {
    calls
        .windows(2)
        .map(|w| (w[1] - w[0]).as_millis())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_503_waits_for_the_servers_delay_then_succeeds() {
    let r = run_with(vec![Step::Fail(status(503, Some(12))), Step::Text("pong")]);
    let out = finish(&r).await.unwrap();
    assert!(out.contains("pong"), "{out}");
    let calls = r.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        gaps(&calls),
        vec![12000],
        "the server's delay, not the backoff"
    );
    let expected = "Service unavailable (HTTP 503). Retrying in 12000 ms... (retry 1/5)";
    assert_eq!(
        *r.stream_notices.lock().unwrap(),
        vec![expected.to_string()]
    );
    assert_eq!(
        *r.callback_notices.lock().unwrap(),
        vec![expected.to_string()],
        "same notice"
    );
}

#[tokio::test(start_paused = true)]
async fn a_shorter_or_absent_server_delay_keeps_the_backoff() {
    let r = run_with(vec![
        Step::Fail(status(429, Some(0))),
        Step::Fail(status(502, None)),
        Step::Text("pong"),
    ]);
    finish(&r).await.unwrap();
    let g = gaps(&r.calls.lock().unwrap());
    assert!((1000..1250).contains(&g[0]), "{g:?}");
    assert!((2000..2500).contains(&g[1]), "{g:?}");
    // What was waited is what was said.
    let notices = r.stream_notices.lock().unwrap().clone();
    assert_eq!(
        notices[0],
        format!(
            "Rate limited (HTTP 429). Retrying in {} ms... (retry 1/5)",
            g[0]
        )
    );
    assert_eq!(
        notices[1],
        format!(
            "Temporary provider error (HTTP 502). Retrying in {} ms... (retry 2/5)",
            g[1]
        )
    );
}

#[tokio::test(start_paused = true)]
async fn five_retries_then_the_error() {
    let steps = (0..10).map(|_| Step::Fail(status(503, None))).collect();
    let r = run_with(steps);
    assert!(finish(&r).await.is_err());
    assert_eq!(
        r.calls.lock().unwrap().len(),
        6,
        "the first call and five retries"
    );
    let n = r.stream_notices.lock().unwrap().clone();
    assert_eq!(n.len(), 5);
    assert!(n[4].ends_with("(retry 5/5)"), "{n:?}");
}

#[tokio::test(start_paused = true)]
async fn definitive_errors_are_not_retried() {
    for e in [
        status(401, Some(5)),
        status(400, Some(5)),
        CerseiError::from_http_status(429, Some(Duration::from_secs(5)), "insufficient_quota"),
    ] {
        let r = run_with(vec![Step::Fail(e), Step::Text("never")]);
        assert!(finish(&r).await.is_err());
        assert_eq!(r.calls.lock().unwrap().len(), 1);
        assert!(r.stream_notices.lock().unwrap().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn cancelling_during_a_servers_wait_sends_nothing_more() {
    let r = run_with(vec![
        Step::Fail(status(429, Some(3600))),
        Step::Text("never"),
    ]);
    let agent = r.agent.clone();
    let task = tokio::spawn(async move { agent.run("ping").await });
    // Well into the hour the server asked for.
    tokio::time::sleep(Duration::from_secs(60)).await;
    r.cancel.cancel();
    let out = task.await.unwrap();
    assert!(matches!(out, Err(CerseiError::Cancelled)), "{out:?}");
    tokio::time::sleep(Duration::from_secs(7200)).await;
    assert_eq!(r.calls.lock().unwrap().len(), 1, "no call after the cancel");
}

#[tokio::test(start_paused = true)]
async fn an_error_after_streamed_text_is_not_retried_by_the_backoff() {
    let r = run_with(vec![Step::TextThenError, Step::Text("again")]);
    let out = finish(&r).await;
    assert_eq!(r.calls.lock().unwrap().len(), 1, "no second call: {out:?}");
    assert!(r.stream_notices.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_connection_error_has_no_status_in_its_notice() {
    // Nothing listens on this port; the first notice is read, then the run
    // is cancelled (no multi-second wait).
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let provider = cersei_provider::ProviderRegistry::from_toml_str(
        &format!(
            r#"schema_version = 1
[[providers]]
id = "t"
name = "T"
endpoint = "http://127.0.0.1:{port}/v1"
protocol = "chat_completions"
auth = "none"
[[providers.models]]
id = "m"
name = "M"
api_model = "m"
streaming = true
tool_calls = true
[providers.models.limits]
max_input_tokens = 100000
max_output_tokens = 1000
"#
        ),
        "t.toml",
    )
    .unwrap()
    .resolve("t/m")
    .unwrap()
    .build_provider()
    .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let agent = Arc::new(
        Agent::builder()
            .provider(provider)
            .model("m")
            .max_turns(1)
            .cancel_token(cancel.clone())
            .build()
            .unwrap(),
    );
    let mut s = agent.run_stream("ping");
    let notice = loop {
        match tokio::time::timeout(Duration::from_secs(10), s.next())
            .await
            .unwrap()
        {
            Some(AgentEvent::Status(m)) if m.contains("Retrying") => break m,
            Some(_) => {}
            None => panic!("no retry notice"),
        }
    };
    cancel.cancel();
    assert!(
        notice.starts_with("Temporary connection error. Retrying in ")
            && notice.ends_with(" ms... (retry 1/5)"),
        "{notice}"
    );
    assert!(
        !notice.contains("HTTP") && !notice.contains("127.0.0.1"),
        "{notice}"
    );
}
