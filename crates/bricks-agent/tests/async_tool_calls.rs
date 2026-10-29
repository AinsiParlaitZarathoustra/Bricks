//! `async_tool_calls`: a call starts as soon as it is complete in the stream,
//! while the model still writes the rest; off, it waits for the whole answer.
//! The provider holds its stream open until the first call ran (or a short
//! wait passed), so the outcome never depends on timing luck.

use async_trait::async_trait;
use bricks_agent::Agent;
use bricks_agent::events::AgentEvent;
use bricks_provider::{CompletionRequest, CompletionStream, Provider};
use bricks_tools::permissions::AllowAll;
use bricks_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use bricks_types::*;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

struct Shared {
    /// The first call ran.
    ran: Notify,
    ran_flag: AtomicBool,
    /// The first call had run when the stream ended.
    ran_before_end: AtomicBool,
    requests: AtomicUsize,
}

struct Holding {
    shared: Arc<Shared>,
    early: bool,
    /// How the answer with calls ends.
    stop: StopReason,
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

#[async_trait]
impl Provider for Holding {
    fn name(&self) -> &str {
        "holding"
    }
    fn context_window(&self, _: &str) -> u64 {
        200_000
    }
    fn async_tool_calls(&self) -> bool {
        self.early
    }
    async fn complete(&self, _: CompletionRequest) -> Result<CompletionStream> {
        let first = self.shared.requests.fetch_add(1, Ordering::SeqCst) == 0;
        let shared = Arc::clone(&self.shared);
        let stop = self.stop.clone();
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::MessageStart {
                    id: "m".into(),
                    model: "holding".into(),
                    usage: None,
                })
                .await;
            if first {
                tool_block(&tx, 0, "a", 1).await;
                // The model is still writing: wait for the first call, briefly.
                if !shared.ran_flag.load(Ordering::SeqCst) {
                    let _ = tokio::time::timeout(Duration::from_millis(500), shared.ran.notified())
                        .await;
                }
                shared
                    .ran_before_end
                    .store(shared.ran_flag.load(Ordering::SeqCst), Ordering::SeqCst);
                tool_block(&tx, 1, "b", 2).await;
            } else {
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index: 0,
                        block_type: "text".into(),
                        id: None,
                        name: None,
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::TextDelta {
                        index: 0,
                        text: "done".into(),
                    })
                    .await;
                let _ = tx.send(StreamEvent::ContentBlockStop { index: 0 }).await;
            }
            let _ = tx
                .send(StreamEvent::MessageDelta {
                    stop_reason: Some(if first { stop } else { StopReason::EndTurn }),
                    usage: Some(Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        ..Default::default()
                    }),
                })
                .await;
            let _ = tx.send(StreamEvent::MessageStop).await;
        });
        Ok(CompletionStream::new(rx))
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
        if input["n"] == 1 {
            self.0.ran_flag.store(true, Ordering::SeqCst);
            self.0.ran.notify_one();
        }
        ToolResult::success(format!("probe {}", input["n"]))
    }
}

struct Outcome {
    shared: Arc<Shared>,
    starts: Vec<String>,
    results: Vec<(String, String, bool)>,
}

async fn run(early: bool, stop: StopReason) -> Outcome {
    let shared = Arc::new(Shared {
        ran: Notify::new(),
        ran_flag: AtomicBool::new(false),
        ran_before_end: AtomicBool::new(false),
        requests: AtomicUsize::new(0),
    });
    let dir = tempfile::tempdir().unwrap();
    let agent = Arc::new(
        Agent::builder()
            .provider(Holding {
                shared: Arc::clone(&shared),
                early,
                stop,
            })
            .tool(Probe(Arc::clone(&shared)))
            .working_dir(dir.path())
            .permission_policy(AllowAll)
            .build()
            .unwrap(),
    );
    let mut stream = agent.run_stream("go");
    let mut starts = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("an event")
            .expect("the stream is open")
        {
            AgentEvent::ToolStart { id, .. } => starts.push(id),
            AgentEvent::Complete(_) => break,
            AgentEvent::Error(e) => panic!("run failed: {e}"),
            _ => {}
        }
    }
    let results = agent
        .messages()
        .iter()
        .flat_map(|m| m.content_blocks())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content: ToolResultContent::Text(t),
                is_error,
            } => Some((tool_use_id, t, is_error.unwrap_or(false))),
            _ => None,
        })
        .collect();
    Outcome {
        shared,
        starts,
        results,
    }
}

#[tokio::test]
async fn on_a_call_runs_before_the_answer_ends() {
    let o = run(true, StopReason::ToolUse).await;
    assert!(o.shared.ran_before_end.load(Ordering::SeqCst));
    assert_eq!(o.starts, ["a", "b"], "one start per call, in stream order");
    let ids: Vec<&str> = o.results.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(ids, ["a", "b"], "results in the order of the calls");
    assert!(o.results[0].1.ends_with("probe 1"), "{}", o.results[0].1);
    assert!(o.results[1].1.ends_with("probe 2"), "{}", o.results[1].1);
    assert_eq!(o.shared.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn off_calls_wait_for_the_whole_answer() {
    let o = run(false, StopReason::ToolUse).await;
    assert!(!o.shared.ran_before_end.load(Ordering::SeqCst));
    assert_eq!(o.starts, ["a", "b"]);
    let ids: Vec<&str> = o.results.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
}

#[tokio::test]
async fn a_cut_answer_keeps_the_results_of_the_calls_that_ran() {
    let o = run(true, StopReason::MaxTokens).await;
    assert!(o.shared.ran_before_end.load(Ordering::SeqCst));
    let ran: Vec<(&str, bool)> = o
        .results
        .iter()
        .map(|r| (r.0.as_str(), r.2 || !r.1.contains("probe")))
        .collect();
    assert_eq!(ran, [("a", false), ("b", false)], "{:?}", o.results);
}

#[tokio::test]
async fn off_a_cut_answer_runs_nothing() {
    let o = run(false, StopReason::MaxTokens).await;
    assert!(!o.shared.ran_flag.load(Ordering::SeqCst));
    assert!(o.results.iter().all(|r| r.2 && r.1.contains("was not run")));
}
