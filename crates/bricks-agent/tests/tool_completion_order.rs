//! Tool calls of one round are published as they finish (Sprint 13.1, lot
//! B): a fast call's end, result and applied edit are seen while a slower
//! sibling is still blocked; the conversation keeps the order of the calls;
//! a cancellation keeps what already finished. Spy provider, gated tools,
//! no network and no sleeps deciding the outcome.

use async_trait::async_trait;
use bricks_agent::Agent;
use bricks_agent::events::AgentEvent;
use bricks_provider::{CompletionRequest, CompletionStream, Provider};
use bricks_tools::permissions::{AllowAll, DenyAll};
use bricks_tools::preview::{ChangeKind, ChangePreview, FileChange};
use bricks_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use bricks_types::*;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};

// ─── Spy provider ────────────────────────────────────────────────────────────

type Calls = Vec<(&'static str, &'static str, Value)>;

struct Spy {
    steps: parking_lot::Mutex<VecDeque<Calls>>,
    requests: AtomicUsize,
}

impl Spy {
    fn new(steps: Vec<Calls>) -> Arc<Self> {
        Arc::new(Self {
            steps: parking_lot::Mutex::new(steps.into()),
            requests: AtomicUsize::new(0),
        })
    }
}

struct SpyProvider(Arc<Spy>);

#[async_trait]
impl Provider for SpyProvider {
    fn name(&self) -> &str {
        "spy"
    }
    fn context_window(&self, _: &str) -> u64 {
        200_000
    }
    async fn complete(&self, _: CompletionRequest) -> Result<CompletionStream> {
        self.0.requests.fetch_add(1, Ordering::SeqCst);
        // Script exhausted: a final answer.
        let calls = self.0.steps.lock().pop_front().unwrap_or_default();
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::MessageStart {
                    id: "m".into(),
                    model: "spy".into(),
                    usage: None,
                })
                .await;
            let has_calls = !calls.is_empty();
            if !has_calls {
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
            for (index, (id, name, input)) in calls.into_iter().enumerate() {
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index,
                        block_type: "tool_use".into(),
                        id: Some(id.into()),
                        name: Some(name.into()),
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::InputJsonDelta {
                        index,
                        partial_json: input.to_string(),
                    })
                    .await;
                let _ = tx.send(StreamEvent::ContentBlockStop { index }).await;
            }
            let _ = tx
                .send(StreamEvent::MessageDelta {
                    stop_reason: Some(if has_calls {
                        StopReason::ToolUse
                    } else {
                        StopReason::EndTurn
                    }),
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

// ─── Tools ───────────────────────────────────────────────────────────────────

/// Blocks until its gate gets a permit; records whether it finished or
/// was dropped while waiting.
struct Slow {
    gate: Arc<Semaphore>,
    finished: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

struct OnDrop(Arc<AtomicBool>, Arc<AtomicBool>);
impl Drop for OnDrop {
    fn drop(&mut self) {
        if !self.0.load(Ordering::SeqCst) {
            self.1.store(true, Ordering::SeqCst);
        }
    }
}

#[async_trait]
impl Tool for Slow {
    fn name(&self) -> &str {
        "Slow"
    }
    fn description(&self) -> &str {
        "waits for its gate"
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"n": {"type": "integer"}}})
    }
    async fn execute(&self, input: Value, _: &ToolContext) -> ToolResult {
        let _guard = OnDrop(self.finished.clone(), self.dropped.clone());
        let _permit = self.gate.acquire().await.unwrap();
        self.finished.store(true, Ordering::SeqCst);
        ToolResult::success(format!("slow {}", input["n"]))
    }
}

/// Answers at once (an error when `fail`).
struct Fast;

#[async_trait]
impl Tool for Fast {
    fn name(&self) -> &str {
        "Fast"
    }
    fn description(&self) -> &str {
        "answers at once"
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"n": {"type": "integer"}, "fail": {"type": "boolean"}}})
    }
    async fn execute(&self, input: Value, _: &ToolContext) -> ToolResult {
        if input["fail"].as_bool() == Some(true) {
            return ToolResult::error("fast failed");
        }
        ToolResult::success(format!("fast {}", input["n"]))
    }
}

/// Writes a new file; its preview names the change, so the engine reports
/// the edit once the call succeeds.
struct Make;

#[async_trait]
impl Tool for Make {
    fn name(&self) -> &str {
        "Make"
    }
    fn description(&self) -> &str {
        "creates a file"
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"path": {"type": "string"}}})
    }
    async fn preview(&self, input: &Value, ctx: &ToolContext) -> Option<ChangePreview> {
        let path = input["path"].as_str()?.to_string();
        Some(ChangePreview {
            files: vec![FileChange {
                absolute: ctx.working_dir.join(&path),
                path,
                kind: ChangeKind::Create,
                before_sha256: None,
                diff: "+a\n+b\n".into(),
                added: 2,
                removed: 0,
                binary: false,
            }],
            refusal: None,
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let path = ctx.working_dir.join(input["path"].as_str().unwrap());
        std::fs::write(path, "a\nb\n").unwrap();
        ToolResult::success("created")
    }
}

struct Fixture {
    agent: Arc<Agent>,
    gate: Arc<Semaphore>,
    finished: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
    spy: Arc<Spy>,
    _dir: tempfile::TempDir,
}

fn fixture(steps: Vec<Calls>, allow: bool) -> Fixture {
    let spy = Spy::new(steps);
    let gate = Arc::new(Semaphore::new(0));
    let finished = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let dir = tempfile::tempdir().unwrap();
    let builder = Agent::builder()
        .provider(SpyProvider(spy.clone()))
        .tool(Slow {
            gate: gate.clone(),
            finished: finished.clone(),
            dropped: dropped.clone(),
        })
        .tool(Fast)
        .tool(Make)
        .working_dir(dir.path());
    let builder = if allow {
        builder.permission_policy(AllowAll)
    } else {
        builder.permission_policy(DenyAll)
    };
    Fixture {
        agent: Arc::new(builder.build().unwrap()),
        gate,
        finished,
        dropped,
        spy,
        _dir: dir,
    }
}

/// The tool results of the first user message following a tool round.
fn round_results(messages: &[Message]) -> Vec<(String, String, bool)> {
    messages
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
        .collect()
}

/// Every tool call has exactly one result, right after its call.
fn coherent(messages: &[Message]) {
    let mut open: Vec<String> = Vec::new();
    let mut answered: Vec<String> = Vec::new();
    for m in messages {
        for b in m.content_blocks() {
            match b {
                ContentBlock::ToolUse { id, .. } => open.push(id),
                ContentBlock::ToolResult { tool_use_id, .. } => {
                    assert!(open.contains(&tool_use_id), "result without call");
                    assert!(!answered.contains(&tool_use_id), "two results");
                    open.retain(|i| *i != tool_use_id);
                    answered.push(tool_use_id);
                }
                _ => {}
            }
        }
    }
    assert!(open.is_empty(), "unanswered: {open:?}");
}

async fn next_tool_end(stream: &mut bricks_agent::AgentStream) -> (String, bool, String) {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("an event")
            .expect("the stream is open")
        {
            AgentEvent::ToolEnd {
                id,
                is_error,
                result,
                ..
            } => return (id, is_error, result),
            AgentEvent::Error(e) => panic!("run failed: {e}"),
            AgentEvent::Complete(_) => panic!("completed before the tool end"),
            _ => {}
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_fast_call_ends_while_a_slow_sibling_is_still_blocked() {
    let f = fixture(
        vec![vec![
            ("slow", "Slow", json!({"n": 1})),
            ("fast", "Fast", json!({"n": 2})),
        ]],
        true,
    );
    let mut stream = f.agent.run_stream("go");
    // The slow call cannot finish: its gate has no permit yet.
    let (id, is_error, result) = next_tool_end(&mut stream).await;
    assert_eq!(id, "fast");
    assert!(!is_error);
    assert_eq!(result, "fast 2");
    assert!(
        !f.finished.load(Ordering::SeqCst),
        "the slow call is still blocked"
    );
    // No second request while the round is unresolved.
    assert_eq!(f.spy.requests.load(Ordering::SeqCst), 1);

    f.gate.add_permits(1);
    let (id, _, result) = next_tool_end(&mut stream).await;
    assert_eq!(id, "slow");
    assert_eq!(result, "slow 1");
    let mut ends = 2;
    let output = loop {
        match stream.next().await.expect("open") {
            AgentEvent::ToolEnd { .. } => ends += 1,
            AgentEvent::Complete(o) => break o,
            AgentEvent::Error(e) => panic!("{e}"),
            _ => {}
        }
    };
    assert_eq!(ends, 2, "one tool end per call");
    // The conversation keeps the order of the calls, not of the ends.
    let msgs = f.agent.messages();
    coherent(&msgs);
    let ids: Vec<String> = round_results(&msgs).into_iter().map(|r| r.0).collect();
    assert_eq!(ids, ["slow", "fast"]);
    let raw_ids: Vec<String> = round_results(&f.agent.raw_history())
        .into_iter()
        .map(|r| r.0)
        .collect();
    assert_eq!(raw_ids, ["slow", "fast"]);
    let records: Vec<&str> = output.tool_calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(records, ["slow", "fast"]);
    // The slow call's duration was measured when it ended, the fast one's
    // is not inflated by the wait.
    let fast = output.tool_calls.iter().find(|c| c.id == "fast").unwrap();
    let slow = output.tool_calls.iter().find(|c| c.id == "slow").unwrap();
    assert!(fast.duration <= slow.duration);
    assert_eq!(f.spy.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_applied_edit_is_reported_before_the_round_ends() {
    let f = fixture(
        vec![vec![
            ("slow", "Slow", json!({"n": 1})),
            ("make", "Make", json!({"path": "new.txt"})),
        ]],
        true,
    );
    let mut stream = f.agent.run_stream("go");
    let mut edit = None;
    loop {
        match stream.next().await.expect("open") {
            AgentEvent::EditApplied {
                tool_call_id,
                files,
                ..
            } => edit = Some((tool_call_id, files)),
            AgentEvent::ToolEnd { id, .. } => {
                assert_eq!(id, "make");
                break;
            }
            AgentEvent::Error(e) => panic!("{e}"),
            _ => {}
        }
    }
    let (id, files) = edit.expect("the edit is reported with its call");
    assert_eq!(id, "make");
    assert_eq!(files.iter().map(|f| f.added).sum::<usize>(), 2);
    assert!(!f.finished.load(Ordering::SeqCst));
    f.gate.add_permits(1);
    let mut edits = 1;
    loop {
        match stream.next().await.expect("open") {
            AgentEvent::EditApplied { .. } => edits += 1,
            AgentEvent::Complete(_) => break,
            AgentEvent::Error(e) => panic!("{e}"),
            _ => {}
        }
    }
    assert_eq!(edits, 1, "counted once");
}

#[tokio::test]
async fn ends_in_reverse_order_keep_a_deterministic_history() {
    // Three calls; the first one finishes last.
    let f = fixture(
        vec![vec![
            ("a", "Slow", json!({"n": 1})),
            ("b", "Fast", json!({"n": 2})),
            ("c", "Fast", json!({"n": 3, "fail": true})),
        ]],
        true,
    );
    let mut stream = f.agent.run_stream("go");
    let mut seen = Vec::new();
    for _ in 0..2 {
        seen.push(next_tool_end(&mut stream).await.0);
    }
    seen.sort();
    assert_eq!(seen, ["b", "c"]);
    f.gate.add_permits(1);
    assert_eq!(next_tool_end(&mut stream).await.0, "a");
    while let Some(ev) = stream.next().await {
        if matches!(ev, AgentEvent::Complete(_)) {
            break;
        }
    }
    let msgs = f.agent.messages();
    coherent(&msgs);
    let results = round_results(&msgs);
    let ids: Vec<&str> = results.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    // The failure carries the engine's failure-streak note, in its result.
    let (_, text, is_error) = &results[2];
    assert!(*is_error);
    assert!(text.contains("fast failed"), "{text}");
    assert!(text.contains("--- notes ---"), "{text}");
}

#[tokio::test]
async fn refusals_unknown_tools_and_errors_end_once_each() {
    let f = fixture(
        vec![vec![
            ("w", "Make", json!({"path": "x.txt"})),
            ("u", "Nope", json!({})),
            ("e", "Fast", json!({"fail": true})),
        ]],
        false,
    );
    let output = {
        let mut stream = f.agent.run_stream("go");
        let mut ends: Vec<(String, bool)> = Vec::new();
        let out = loop {
            match stream.next().await.expect("open") {
                AgentEvent::ToolEnd { id, is_error, .. } => ends.push((id, is_error)),
                AgentEvent::EditApplied { .. } => panic!("nothing was written"),
                AgentEvent::Complete(o) => break o,
                AgentEvent::Error(e) => panic!("{e}"),
                _ => {}
            }
        };
        ends.sort();
        assert_eq!(
            ends,
            [("e".into(), true), ("u".into(), true), ("w".into(), true)]
        );
        out
    };
    assert_eq!(output.tool_calls.len(), 3);
    coherent(&f.agent.messages());
    assert!(!f._dir.path().join("x.txt").exists(), "denied: not written");
}

#[tokio::test]
async fn a_cancellation_keeps_finished_calls_and_stops_the_others() {
    let f = fixture(
        vec![vec![
            ("slow", "Slow", json!({"n": 1})),
            ("make", "Make", json!({"path": "kept.txt"})),
        ]],
        true,
    );
    let mut stream = f.agent.run_stream("go");
    let mut edits = 0;
    loop {
        match stream.next().await.expect("open") {
            AgentEvent::EditApplied { .. } => edits += 1,
            AgentEvent::ToolEnd { id, .. } => {
                assert_eq!(id, "make");
                break;
            }
            _ => {}
        }
    }
    f.agent.cancel();
    let mut ends_after = 0;
    loop {
        match stream.next().await {
            Some(AgentEvent::ToolEnd { .. }) => ends_after += 1,
            Some(AgentEvent::EditApplied { .. }) => edits += 1,
            Some(AgentEvent::Error(e)) => {
                assert!(e.to_lowercase().contains("cancel"), "{e}");
                break;
            }
            Some(AgentEvent::Complete(_)) => panic!("the run was cancelled"),
            Some(_) => {}
            None => panic!("no terminal event"),
        }
    }
    assert_eq!(ends_after, 0, "the stopped call gets no invented end");
    assert_eq!(edits, 1, "the applied edit is reported once");
    // The slow call was stopped (dropped), not left running.
    assert!(f.dropped.load(Ordering::SeqCst));
    assert!(!f.finished.load(Ordering::SeqCst));
    // The finished write keeps its success; the stopped call says so.
    let msgs = f.agent.messages();
    coherent(&msgs);
    let results = round_results(&msgs);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "slow");
    assert!(results[0].2 && results[0].1.contains("cancelled by the user"));
    assert_eq!(results[1].0, "make");
    assert!(!results[1].2, "{}", results[1].1);
    assert!(results[1].1.contains("created"));
    let raw = round_results(&f.agent.raw_history());
    assert_eq!(raw.len(), 2);
    assert!(f._dir.path().join("kept.txt").exists());
    // No further request after the cancellation, and the next run works.
    assert_eq!(f.spy.requests.load(Ordering::SeqCst), 1);
    let out = f.agent.run("again").await.unwrap();
    assert_eq!(out.message.get_text().unwrap_or_default(), "done");
    coherent(&f.agent.messages());
}
