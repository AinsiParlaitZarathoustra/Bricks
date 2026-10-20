//! Auxiliary calls — compaction (manual, automatic, overflow recovery) and
//! remote token counting — can be cancelled and are bounded in time
//! (Sprint 13.1, lot C). Spy providers and a local TCP server that never
//! answers; deadlines checked on Tokio's virtual clock, never waited for.

mod common;

use async_trait::async_trait;
use bricks_agent::compact::{COMPACTION_ATTEMPT_TIMEOUT, CompactionPlan, compact_history_with};
use bricks_agent::events::AgentEvent;
use bricks_agent::{Agent, CompactionOutcome, ContextPolicy};
use bricks_provider::{CompletionRequest, CompletionStream, ModelInfo, ModelLimits, Provider};
use bricks_types::*;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

// ─── Spy provider ────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Answer {
    /// `complete` never returns (a server that sends no headers).
    NoHeaders,
    /// One fragment, then a fragment every 10 s, never finished.
    Trickle,
    /// A complete answer.
    Text(&'static str),
}

#[derive(Default)]
struct Spy {
    answers: parking_lot::Mutex<VecDeque<Answer>>,
    /// `complete` calls.
    requests: AtomicUsize,
    /// `count_request_tokens` calls.
    counts: AtomicUsize,
    /// Pending futures or streams the consumer dropped.
    dropped: AtomicUsize,
    entered: Notify,
    /// The counting endpoint never answers.
    count_hangs: bool,
}

struct DropMark(Arc<Spy>);
impl Drop for DropMark {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct SpyProvider(Arc<Spy>);

fn usage() -> Usage {
    Usage {
        input_tokens: 50,
        output_tokens: 7,
        ..Default::default()
    }
}

#[async_trait]
impl Provider for SpyProvider {
    fn name(&self) -> &str {
        "spy"
    }
    fn context_window(&self, _: &str) -> u64 {
        100_000
    }
    fn model_info(&self) -> Option<ModelInfo> {
        Some(ModelInfo {
            selection: "spy/m".into(),
            protocol: "anthropic_messages",
            limits: ModelLimits {
                max_input_tokens: 100_000,
                max_output_tokens: 4_000,
                context_window_tokens: None,
            },
            reasoning_resent: false,
            token_counting: true,
        })
    }
    async fn count_request_tokens(&self, _: &CompletionRequest) -> Result<Option<u64>> {
        self.0.counts.fetch_add(1, Ordering::SeqCst);
        if self.0.count_hangs {
            let _mark = DropMark(self.0.clone());
            self.0.entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(Some(42))
    }
    async fn complete(&self, _: CompletionRequest) -> Result<CompletionStream> {
        self.0.requests.fetch_add(1, Ordering::SeqCst);
        let answer = self
            .0
            .answers
            .lock()
            .pop_front()
            .unwrap_or(Answer::Text("done"));
        let (tx, rx) = mpsc::channel(8);
        match answer {
            Answer::NoHeaders => {
                let _mark = DropMark(self.0.clone());
                self.0.entered.notify_one();
                std::future::pending::<()>().await;
                unreachable!()
            }
            Answer::Trickle => {
                let spy = self.0.clone();
                tokio::spawn(async move {
                    let _ = tx
                        .send(StreamEvent::MessageStart {
                            id: "m".into(),
                            model: "spy".into(),
                            usage: None,
                        })
                        .await;
                    spy.entered.notify_one();
                    loop {
                        let sent = tx
                            .send(StreamEvent::TextDelta {
                                index: 0,
                                text: "partial ".into(),
                            })
                            .await;
                        if sent.is_err() {
                            // The reader went away: the stream ends here.
                            spy.dropped.fetch_add(1, Ordering::SeqCst);
                            return;
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                            _ = tx.closed() => {
                                spy.dropped.fetch_add(1, Ordering::SeqCst);
                                return;
                            }
                        }
                    }
                });
            }
            Answer::Text(text) => {
                tokio::spawn(async move {
                    for ev in [
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
                            text: text.into(),
                        },
                        StreamEvent::ContentBlockStop { index: 0 },
                        StreamEvent::MessageDelta {
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Some(usage()),
                        },
                        StreamEvent::MessageStop,
                    ] {
                        let _ = tx.send(ev).await;
                    }
                });
            }
        }
        Ok(CompletionStream::new(rx))
    }
}

fn spy(answers: Vec<Answer>, count_hangs: bool) -> Arc<Spy> {
    Arc::new(Spy {
        answers: parking_lot::Mutex::new(answers.into()),
        count_hangs,
        ..Default::default()
    })
}

fn long_history() -> Vec<Message> {
    let mut v = Vec::new();
    for i in 0..12 {
        v.push(Message::user(format!(
            "step {i}: {}",
            "detail ".repeat(300)
        )));
        v.push(Message::assistant(format!(
            "done {i}: {}",
            "result ".repeat(300)
        )));
    }
    v
}

fn agent_with(spy: &Arc<Spy>, history: Vec<Message>, policy: ContextPolicy) -> Arc<Agent> {
    Arc::new(
        Agent::builder()
            .provider(SpyProvider(spy.clone()))
            .with_messages(history)
            .context_policy(policy)
            .working_dir(std::env::temp_dir())
            .build()
            .unwrap(),
    )
}

/// Messages compared through their serialised form.
fn same(a: &[Message], b: &[Message]) -> bool {
    serde_json::to_value(a).unwrap() == serde_json::to_value(b).unwrap()
}

fn plan() -> CompactionPlan {
    CompactionPlan {
        keep_recent_messages: 4,
        max_recent_tokens: 4_000,
        input_limit: 100_000,
        summary_max_tokens: 2_000,
        min_gain: 0.1,
        frame_tokens: 0,
        instructions: None,
    }
}

fn estimator() -> bricks_agent::context::Estimator {
    bricks_agent::context::Estimator::new(&ContextPolicy::default(), false, 1.0)
}

// ─── Compaction ──────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn a_trickling_summary_stream_fails_at_the_absolute_deadline() {
    let s = spy(vec![Answer::Trickle], false);
    let p = SpyProvider(s.clone());
    let history = long_history();
    let start = tokio::time::Instant::now();
    let run = compact_history_with(
        &p,
        &history,
        &estimator(),
        &plan(),
        &tokio_util::sync::CancellationToken::new(),
        start + COMPACTION_ATTEMPT_TIMEOUT,
    )
    .await;
    // Fragments every 10 s never push the deadline back.
    let took = start.elapsed();
    assert!(
        took >= COMPACTION_ATTEMPT_TIMEOUT
            && took < COMPACTION_ATTEMPT_TIMEOUT + Duration::from_secs(1),
        "{took:?}"
    );
    match &run.outcome {
        CompactionOutcome::Failed { reason } => assert!(reason.contains("300 s"), "{reason}"),
        o => panic!("{o:?}"),
    }
    assert!(run.messages.is_none(), "no partial summary");
    assert!(run.usage.is_none(), "no usage invented");
    // The stream's producer saw its reader go away.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(s.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(s.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_summary_server_without_headers_times_out_too() {
    let s = spy(vec![Answer::NoHeaders], false);
    let p = SpyProvider(s.clone());
    let run = compact_history_with(
        &p,
        &long_history(),
        &estimator(),
        &plan(),
        &tokio_util::sync::CancellationToken::new(),
        tokio::time::Instant::now() + COMPACTION_ATTEMPT_TIMEOUT,
    )
    .await;
    assert!(
        matches!(run.outcome, CompactionOutcome::Failed { .. }),
        "{:?}",
        run.outcome
    );
    assert_eq!(
        s.dropped.load(Ordering::SeqCst),
        1,
        "the pending request was dropped"
    );
}

#[tokio::test]
async fn a_blocked_manual_compaction_is_cancelled_and_the_next_one_works() {
    let s = spy(
        vec![
            Answer::NoHeaders,
            Answer::Trickle,
            Answer::Text("SUMMARY: all steps"),
        ],
        false,
    );
    let history = long_history();
    let agent = agent_with(&s, history.clone(), ContextPolicy::default());

    // 1. No headers: cancelled while waiting for the response.
    for expected_drops in [1usize, 2] {
        let a = agent.clone();
        let entered = s.entered.notified();
        let task = tokio::spawn(async move { a.compact().await });
        entered.await;
        agent.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the cancellation is honoured")
            .unwrap();
        assert_eq!(outcome, CompactionOutcome::Cancelled);
        // 2nd round: cancelled while the stream trickles.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(s.dropped.load(Ordering::SeqCst), expected_drops);
        assert!(same(&agent.messages(), &history), "history intact");
        assert!(agent.compaction_snapshots().is_empty());
        assert_eq!(agent.context_status().totals.compaction_requests, 0);
    }
    // No request was sent after a cancellation.
    assert_eq!(s.requests.load(Ordering::SeqCst), 2);

    // The next compaction is not poisoned by the cancelled token.
    let outcome = agent.compact().await;
    assert!(outcome.is_compacted(), "{outcome:?}");
    assert_eq!(s.requests.load(Ordering::SeqCst), 3);
    let st = agent.context_status();
    assert_eq!(st.totals.compaction_requests, 1, "usage counted once");
    assert_eq!(st.totals.input_tokens, 50);
    assert_eq!(agent.compaction_snapshots().len(), 1);
}

#[tokio::test]
async fn a_cancelled_run_stops_its_overflow_compaction_and_sends_nothing_else() {
    // The first request is refused as too long; the summary call hangs.
    struct Refusing(Arc<Spy>, AtomicUsize);
    #[async_trait]
    impl Provider for Refusing {
        fn name(&self) -> &str {
            "refusing"
        }
        fn context_window(&self, _: &str) -> u64 {
            100_000
        }
        async fn complete(&self, r: CompletionRequest) -> Result<CompletionStream> {
            if self.1.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(CerseiError::ContextOverflow {
                    used: 200_000,
                    limit: 100_000,
                });
            }
            SpyProvider(self.0.clone()).complete(r).await
        }
    }
    let s = spy(vec![Answer::NoHeaders], false);
    let provider = Arc::new(Refusing(s.clone(), AtomicUsize::new(0)));
    struct Shared(Arc<Refusing>);
    #[async_trait]
    impl Provider for Shared {
        fn name(&self) -> &str {
            "refusing"
        }
        fn context_window(&self, _: &str) -> u64 {
            100_000
        }
        async fn complete(&self, r: CompletionRequest) -> Result<CompletionStream> {
            self.0.complete(r).await
        }
    }
    let history = long_history();
    let agent = Arc::new(
        Agent::builder()
            .provider(Shared(provider.clone()))
            .with_messages(history.clone())
            .working_dir(std::env::temp_dir())
            .build()
            .unwrap(),
    );
    let entered = s.entered.notified();
    let mut stream = agent.run_stream("continue");
    entered.await;
    agent.cancel();
    let mut outcomes = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the run ends")
        {
            Some(AgentEvent::CompactionResult { outcome, .. }) => outcomes.push(outcome),
            Some(AgentEvent::Error(e)) => {
                assert!(e.to_lowercase().contains("cancel"), "{e}");
                break;
            }
            Some(AgentEvent::Complete(_)) => panic!("cancelled"),
            Some(_) => {}
            None => panic!("no terminal event"),
        }
    }
    assert_eq!(outcomes, [CompactionOutcome::Cancelled]);
    // Refused request + summary call; no resend after the cancellation.
    assert_eq!(provider.1.load(Ordering::SeqCst), 2);
    assert_eq!(s.dropped.load(Ordering::SeqCst), 1);
    assert!(agent.compaction_snapshots().is_empty());
    // The history is the original one plus the prompt.
    assert!(same(&agent.messages()[..history.len()], &history));
}

/// A real configured provider against a local server that reads the request
/// and never answers: the manual compaction is cancelled while waiting for
/// the headers, and its connection is closed.
#[tokio::test]
async fn a_real_http_compaction_without_headers_is_cancelled() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen_tx, seen_rx) = std::sync::mpsc::channel::<()>();
    let (closed_tx, closed_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 65536];
            let _ = sock.read(&mut buf);
            let _ = seen_tx.send(());
            // Never answers; wait for the client to close the connection.
            loop {
                match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let _ = closed_tx.send(());
        }
    });
    let url = format!("http://127.0.0.1:{port}/v1");
    let history = long_history();
    let agent = Arc::new(
        Agent::builder()
            .provider(common::provider(&url, "chat_completions", 100_000))
            .with_messages(history.clone())
            .working_dir(std::env::temp_dir())
            .build()
            .unwrap(),
    );
    let a = agent.clone();
    let task = tokio::spawn(async move { a.compact().await });
    tokio::task::spawn_blocking(move || seen_rx.recv_timeout(Duration::from_secs(20)))
        .await
        .unwrap()
        .expect("the summary request reached the server");
    agent.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("cancelled in time")
        .unwrap();
    assert_eq!(outcome, CompactionOutcome::Cancelled);
    assert!(same(&agent.messages(), &history));
    tokio::task::spawn_blocking(move || closed_rx.recv_timeout(Duration::from_secs(20)))
        .await
        .unwrap()
        .expect("the connection was closed, not left orphaned");
}

// ─── Remote token counting ───────────────────────────────────────────────────

fn always_preflight() -> ContextPolicy {
    ContextPolicy {
        preflight_ratio: 0.0,
        ..Default::default()
    }
}

#[tokio::test(start_paused = true)]
async fn a_blocked_count_falls_back_to_the_local_estimate_after_30_s() {
    let s = spy(vec![Answer::Text("answer")], true);
    let agent = agent_with(&s, Vec::new(), always_preflight());
    let mut stream = agent.run_stream("hello");
    let start = tokio::time::Instant::now();
    let mut notes = Vec::new();
    let output = loop {
        match stream.next().await.expect("open") {
            AgentEvent::Status(m) => notes.push(m),
            AgentEvent::Complete(o) => break o,
            AgentEvent::Error(e) => panic!("{e}"),
            _ => {}
        }
    };
    assert_eq!(output.text(), "answer");
    assert!(
        !notes
            .iter()
            .any(|m| m.contains("Pre-flight") || m.contains("local estimate used")),
        "pre-flight diagnostics stay out of the UI: {notes:?}"
    );
    let at = start.elapsed();
    assert!(
        at >= Duration::from_secs(30) && at < Duration::from_secs(31),
        "{at:?}"
    );
    assert_eq!(s.counts.load(Ordering::SeqCst), 1);
    assert_eq!(s.dropped.load(Ordering::SeqCst), 1, "the count was dropped");
    assert_eq!(s.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelling_a_blocked_count_stops_the_run_without_fallback() {
    let s = spy(vec![], true);
    let agent = agent_with(&s, Vec::new(), always_preflight());
    let entered = s.entered.notified();
    let mut stream = agent.run_stream("hello");
    entered.await;
    agent.cancel();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the run ends")
        {
            Some(AgentEvent::Status(m)) => {
                assert!(
                    !m.contains("local estimate"),
                    "no fallback after the stop: {m}"
                )
            }
            Some(AgentEvent::Error(e)) => {
                assert!(e.to_lowercase().contains("cancel"), "{e}");
                break;
            }
            Some(AgentEvent::Complete(_)) => panic!("cancelled"),
            Some(_) => {}
            None => panic!("no terminal event"),
        }
    }
    assert_eq!(s.requests.load(Ordering::SeqCst), 0, "nothing sent");
    assert_eq!(s.dropped.load(Ordering::SeqCst), 1);
}
