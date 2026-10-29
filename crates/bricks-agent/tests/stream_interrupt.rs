//! A turn stopped while its answer still streams (cancelled, failed by the
//! provider, or cut off): the calls already started keep a complete
//! `tool_use` and exactly one result — the real one when it finished, an
//! interruption otherwise — an incomplete call is neither run nor recorded,
//! and nothing runs after the stop. The provider is synchronised with the
//! tools by signals, never by pauses.

use async_trait::async_trait;
use bricks_agent::Agent;
use bricks_agent::events::AgentEvent;
use bricks_memory::{JsonlMemory, Memory};
use bricks_provider::{CompletionRequest, CompletionStream, Provider};
use bricks_tools::permissions::AllowAll;
use bricks_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use bricks_types::*;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

const SESSION: &str = "interrupt";

#[derive(Clone, Copy, PartialEq)]
enum Ending {
    /// The stream stays open; the test cancels the run.
    Cancel,
    /// The provider reports an error mid-stream.
    Error,
    /// The stream closes without a terminal event.
    Closed,
    /// The answer ends normally; the test cancels the run while `b` runs.
    Finished,
}

#[derive(Default)]
struct Shared {
    /// Call `a` ran to its end.
    a_done: Notify,
    /// Call `b` started running.
    b_started: Notify,
    /// The runner received every event of the first answer.
    streamed: Notify,
    /// Call `b` was stopped (its future dropped).
    b_dropped: AtomicBool,
    executions: Mutex<Vec<u64>>,
    requests: Mutex<Vec<Vec<Message>>>,
    count: AtomicUsize,
}

struct Interrupted {
    shared: Arc<Shared>,
    ending: Ending,
}

async fn tool_block(tx: &mpsc::Sender<StreamEvent>, index: usize, id: &str, n: u64) {
    let _ = tx
        .send(StreamEvent::ContentBlockStart {
            index,
            block_type: "tool_use".into(),
            id: Some(id.into()),
            name: Some("Probe".into()),
        })
        .await;
    let _ = tx
        .send(StreamEvent::InputJsonDelta {
            index,
            partial_json: json!({ "n": n }).to_string(),
        })
        .await;
    let _ = tx.send(StreamEvent::ContentBlockStop { index }).await;
}

async fn text_block(tx: &mpsc::Sender<StreamEvent>, index: usize, text: &str) {
    let _ = tx
        .send(StreamEvent::ContentBlockStart {
            index,
            block_type: "text".into(),
            id: None,
            name: None,
        })
        .await;
    let _ = tx
        .send(StreamEvent::TextDelta {
            index,
            text: text.into(),
        })
        .await;
    let _ = tx.send(StreamEvent::ContentBlockStop { index }).await;
}

#[async_trait]
impl Provider for Interrupted {
    fn name(&self) -> &str {
        "interrupted"
    }
    fn context_window(&self, _: &str) -> u64 {
        200_000
    }
    fn async_tool_calls(&self) -> bool {
        true
    }
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream> {
        self.shared.requests.lock().push(request.messages.clone());
        let first = self.shared.count.fetch_add(1, Ordering::SeqCst) == 0;
        let shared = Arc::clone(&self.shared);
        let ending = self.ending;
        // One slot: a send returns once the runner took the previous event.
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::MessageStart {
                    id: "m".into(),
                    model: "interrupted".into(),
                    usage: None,
                })
                .await;
            if !first {
                text_block(&tx, 0, "done").await;
                let _ = tx
                    .send(StreamEvent::MessageDelta {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: None,
                    })
                    .await;
                let _ = tx.send(StreamEvent::MessageStop).await;
                return;
            }
            text_block(&tx, 0, "Looking.").await;
            // 1. A first call, closed, run to its end while the stream is open.
            tool_block(&tx, 1, "a", 1).await;
            shared.a_done.notified().await;
            // 2. A second complete call, still running.
            tool_block(&tx, 2, "b", 2).await;
            shared.b_started.notified().await;
            // 3. A third call, never closed.
            let _ = tx
                .send(StreamEvent::ContentBlockStart {
                    index: 3,
                    block_type: "tool_use".into(),
                    id: Some("c".into()),
                    name: Some("Probe".into()),
                })
                .await;
            let _ = tx
                .send(StreamEvent::InputJsonDelta {
                    index: 3,
                    partial_json: r#"{"n": 3"#.into(),
                })
                .await;
            if ending == Ending::Finished {
                let _ = tx
                    .send(StreamEvent::MessageDelta {
                        stop_reason: Some(StopReason::ToolUse),
                        usage: None,
                    })
                    .await;
                let _ = tx.send(StreamEvent::MessageStop).await;
            }
            // Returns once the runner took the event above.
            let _ = tx.send(StreamEvent::Ping).await;
            shared.streamed.notify_one();
            match ending {
                Ending::Cancel => {
                    // Open until the runner lets go of it.
                    tx.closed().await;
                }
                Ending::Error => {
                    let _ = tx
                        .send(StreamEvent::Error {
                            message: "overloaded".into(),
                        })
                        .await;
                }
                Ending::Closed | Ending::Finished => {}
            }
        });
        Ok(CompletionStream::new(rx))
    }
}

struct DropFlag(Arc<Shared>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.b_dropped.store(true, Ordering::SeqCst);
    }
}

struct Probe(Arc<Shared>);

#[async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        "Probe"
    }
    fn description(&self) -> &str {
        "answers with its number"
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"n": {"type": "integer"}}})
    }
    async fn execute(&self, input: Value, _: &ToolContext) -> ToolResult {
        let n = input["n"].as_u64().unwrap_or(0);
        self.0.executions.lock().push(n);
        match n {
            1 => {
                self.0.a_done.notify_one();
                ToolResult::success("probe 1")
            }
            2 => {
                let _flag = DropFlag(Arc::clone(&self.0));
                self.0.b_started.notify_one();
                std::future::pending::<()>().await;
                ToolResult::success("probe 2")
            }
            _ => ToolResult::success(format!("probe {n}")),
        }
    }
}

struct Run {
    shared: Arc<Shared>,
    agent: Arc<Agent>,
    events: Vec<AgentEvent>,
    memory_dir: tempfile::TempDir,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

async fn collect(stream: &mut bricks_agent::events::AgentStream, events: &mut Vec<AgentEvent>) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("an event within 20 s");
        match event {
            Some(e @ (AgentEvent::Error(_) | AgentEvent::Complete(_))) => {
                events.push(e);
                break;
            }
            Some(e) => events.push(e),
            None => break,
        }
    }
}

async fn interrupted_run(ending: Ending) -> Run {
    let shared = Arc::new(Shared::default());
    let wd = tempfile::tempdir().unwrap();
    let raw = tempfile::tempdir().unwrap();
    let memory_dir = tempfile::tempdir().unwrap();
    let agent = Arc::new(
        Agent::builder()
            .provider(Interrupted {
                shared: Arc::clone(&shared),
                ending,
            })
            .tool(Probe(Arc::clone(&shared)))
            .working_dir(wd.path())
            .raw_output_dir(raw.path())
            .memory(JsonlMemory::new(memory_dir.path()))
            .session_id(SESSION)
            .permission_policy(AllowAll)
            .build()
            .unwrap(),
    );
    let mut stream = agent.run_stream("go");
    if matches!(ending, Ending::Cancel | Ending::Finished) {
        tokio::time::timeout(Duration::from_secs(20), shared.streamed.notified())
            .await
            .expect("the answer streamed");
        agent.cancel();
    }
    let mut events = Vec::new();
    collect(&mut stream, &mut events).await;
    assert!(
        matches!(events.last(), Some(AgentEvent::Error(_))),
        "the run fails: {:?}",
        events.last()
    );
    Run {
        shared,
        agent,
        events,
        memory_dir,
        _dirs: (wd, raw),
    }
}

/// `(id, text, is_error)` of every tool result.
fn results(messages: &[Message]) -> Vec<(String, String, bool)> {
    messages
        .iter()
        .flat_map(|m| m.content_blocks())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let text = match content {
                    ToolResultContent::Text(t) => t,
                    ToolResultContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect(),
                };
                Some((tool_use_id, text, is_error.unwrap_or(false)))
            }
            _ => None,
        })
        .collect()
}

fn tool_use_ids(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|m| m.content_blocks())
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id),
            _ => None,
        })
        .collect()
}

/// The history after the stop: the prompt, the answer with its closed text
/// and its two started calls, then their two results, in order.
fn check_history(messages: &[Message], why: &str) {
    assert_eq!(messages.len(), 3, "{messages:#?}");
    assert_eq!(messages[1].role, Role::Assistant);
    let answer = messages[1].content_blocks();
    assert!(
        matches!(&answer[0], ContentBlock::Text { text } if text == "Looking."),
        "{answer:?}"
    );
    assert_eq!(tool_use_ids(messages), ["a", "b"], "c was never complete");
    for block in &answer {
        if let ContentBlock::ToolUse { input, .. } = block {
            assert!(input.get("__parse_error").is_none(), "{input}");
        }
    }
    assert_eq!(messages[2].role, Role::User);
    let results = results(&messages[2..]);
    let ids: Vec<&str> = results.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(ids, ["a", "b"], "one result per call, in order");
    let (_, a, a_error) = &results[0];
    assert!(a.contains("probe 1") && !a_error, "a keeps its result: {a}");
    let (_, b, b_error) = &results[1];
    assert!(*b_error, "b was interrupted");
    assert!(b.contains(why), "{b}");
    assert!(b.contains("any effect it already had remains"), "{b}");
    assert!(!b.contains("probe 2"), "{b}");
}

async fn check_interrupted(run: &Run, why: &str) {
    // Nothing ran after the stop, b was stopped, c never ran.
    assert_eq!(*run.shared.executions.lock(), [1, 2]);
    assert!(run.shared.b_dropped.load(Ordering::SeqCst));
    // Each call started and published once; c not at all.
    let starts: Vec<&str> = run
        .events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolStart { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(starts, ["a", "b"]);
    let ends: Vec<(&str, bool)> = run
        .events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolEnd { id, is_error, .. } => Some((id.as_str(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        ends,
        [("a", false)],
        "a published once, with its real state"
    );

    let active = run.agent.messages();
    let raw = run.agent.raw_history();
    check_history(&active, why);
    check_history(&raw, why);

    // The saved session is the pair as it is in the agent.
    let stored = JsonlMemory::new(run.memory_dir.path())
        .load_histories(SESSION)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&stored.active).unwrap(),
        serde_json::to_value(&active).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&stored.raw).unwrap(),
        serde_json::to_value(&raw).unwrap()
    );

    // The next turn sends that history, valid, and reruns nothing.
    let mut stream = run.agent.run_stream("again");
    let mut events = Vec::new();
    collect(&mut stream, &mut events).await;
    assert!(
        matches!(events.last(), Some(AgentEvent::Complete(_))),
        "{:?}",
        events.last()
    );
    let requests = run.shared.requests.lock().clone();
    assert_eq!(requests.len(), 2);
    let sent = &requests[1];
    assert_eq!(tool_use_ids(sent), ["a", "b"]);
    let ids: Vec<String> = results(sent).into_iter().map(|r| r.0).collect();
    assert_eq!(ids, ["a", "b"]);
    check_history(&sent[..3], why);
    assert_eq!(*run.shared.executions.lock(), [1, 2], "no call ran again");
}

#[tokio::test]
async fn a_cancelled_stream_keeps_started_calls_and_their_results() {
    let run = interrupted_run(Ending::Cancel).await;
    check_interrupted(&run, "cancelled by the user").await;
}

#[tokio::test]
async fn a_provider_error_keeps_started_calls_and_their_results() {
    let run = interrupted_run(Ending::Error).await;
    check_interrupted(&run, "interrupted by an error").await;
}

#[tokio::test]
async fn a_stream_closed_early_keeps_started_calls_and_their_results() {
    let run = interrupted_run(Ending::Closed).await;
    check_interrupted(&run, "interrupted by an error").await;
}

#[tokio::test]
async fn a_cancel_after_the_answer_keeps_the_calls_finished_while_it_streamed() {
    let run = interrupted_run(Ending::Finished).await;
    check_interrupted(&run, "cancelled by the user").await;
}
