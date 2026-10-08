//! The stop contract and the sub-agent invariants (sprint 8), with a spy
//! provider: it counts the providers built and the requests sent, and
//! answers from a script. No network.
//!
//! * An empty task builds no provider and sends nothing.
//! * A final answer ends the run; tools being available is no obligation.
//! * Real multi-step work goes on until its answer.
//! * There is no turn limit: a run goes on until its answer. The other
//!   stops (cut answers, no progress) end the run explicitly, with a
//!   coherent history.
//! * Repeating calls is told apart from progress by arguments and results.
//! * A child never has more permissions or tools than its parent, never a
//!   delegation tool, and is cancelled with its parent.

use async_trait::async_trait;
use cersei_agent::agent_tool::AgentTool;
use cersei_agent::delegate::{run_batch, DelegateConfig, DelegateTask, ToolsetFactory};
use cersei_agent::delegate_tool::DelegateTool;
use cersei_agent::subagent::{ChildStatus, DelegationDepth, RunCancellation};
use cersei_agent::{Agent, Termination, UserInput};
use cersei_provider::{CompletionRequest, CompletionStream, Provider};
use cersei_tools::permissions::{AllowAll, DenyAll, PermissionPolicy};
use cersei_tools::{
    CostTracker, Extensions, PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult,
};
use cersei_types::*;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ─── Spy provider ────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Step {
    text: Option<String>,
    calls: Vec<(String, String, Value)>,
    stop: Option<StopReason>,
    hang: bool,
}

fn say(text: &str) -> Step {
    Step {
        text: Some(text.into()),
        ..Default::default()
    }
}

fn call(id: &str, name: &str, input: Value) -> Step {
    Step {
        calls: vec![(id.into(), name.into(), input)],
        ..Default::default()
    }
}

fn hang() -> Step {
    Step {
        hang: true,
        ..Default::default()
    }
}

#[derive(Default)]
struct Spy {
    steps: parking_lot::Mutex<VecDeque<Step>>,
    built: AtomicUsize,
    requests: parking_lot::Mutex<Vec<CompletionRequest>>,
    /// Streams the consumer dropped while they hung (cancelled requests).
    dropped: AtomicUsize,
    /// Answer when the script is exhausted (else a loud error).
    fallback: parking_lot::Mutex<Option<Step>>,
}

impl Spy {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: parking_lot::Mutex::new(steps.into()),
            ..Default::default()
        })
    }
    fn provider(self: &Arc<Self>) -> SpyProvider {
        self.built.fetch_add(1, Ordering::SeqCst);
        SpyProvider(self.clone())
    }
    fn factory(self: &Arc<Self>) -> impl Fn() -> Box<dyn Provider> + Send + Sync + 'static {
        let spy = self.clone();
        move || Box::new(spy.provider()) as Box<dyn Provider>
    }
    fn built(&self) -> usize {
        self.built.load(Ordering::SeqCst)
    }
    fn sent(&self) -> usize {
        self.requests.lock().len()
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
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionStream> {
        self.0.requests.lock().push(req);
        let step = self
            .0
            .steps
            .lock()
            .pop_front()
            .or_else(|| self.0.fallback.lock().clone());
        let Some(step) = step else {
            return Err(CerseiError::Provider(
                "spy: unexpected request (script exhausted)".into(),
            ));
        };
        let spy = self.0.clone();
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::MessageStart {
                    id: "m".into(),
                    model: "spy".into(),
                    usage: None,
                })
                .await;
            if step.hang {
                tx.closed().await;
                spy.dropped.fetch_add(1, Ordering::SeqCst);
                return;
            }
            let mut index = 0;
            if let Some(t) = step.text {
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index,
                        block_type: "text".into(),
                        id: None,
                        name: None,
                    })
                    .await;
                let _ = tx.send(StreamEvent::TextDelta { index, text: t }).await;
                let _ = tx.send(StreamEvent::ContentBlockStop { index }).await;
                index += 1;
            }
            let has_calls = !step.calls.is_empty();
            for (id, name, input) in step.calls {
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index,
                        block_type: "tool_use".into(),
                        id: Some(id),
                        name: Some(name),
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::InputJsonDelta {
                        index,
                        partial_json: input.to_string(),
                    })
                    .await;
                let _ = tx.send(StreamEvent::ContentBlockStop { index }).await;
                index += 1;
            }
            let stop = step.stop.unwrap_or(if has_calls {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            });
            let _ = tx
                .send(StreamEvent::MessageDelta {
                    stop_reason: Some(stop),
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

/// Returns its `n` argument (so different calls give different results)
/// or a fixed error when `fail` is set.
struct Probe;

#[async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        "Probe"
    }
    fn description(&self) -> &str {
        "Echoes n."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::FileSystem
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"n": {"type": "integer"}, "fail": {"type": "boolean"}}})
    }
    async fn execute(&self, input: Value, _: &ToolContext) -> ToolResult {
        if input["fail"].as_bool() == Some(true) {
            return ToolResult::error("no such thing");
        }
        ToolResult::success(format!("value {}", input["n"]))
    }
}

/// A tool named like a delegation tool: must never reach a child.
struct Named(&'static str);

#[async_trait]
impl Tool for Named {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "named"
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }
    async fn execute(&self, _: Value, _: &ToolContext) -> ToolResult {
        ToolResult::success("ran")
    }
}

fn agent(spy: &Arc<Spy>) -> Agent {
    Agent::builder()
        .provider(spy.provider())
        .tool(Probe)
        .working_dir(std::env::temp_dir())
        .build()
        .unwrap()
}

fn ctx(permissions: Arc<dyn PermissionPolicy>, dir: &std::path::Path) -> ToolContext {
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: "parent".into(),
        permissions,
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

fn no_tools() -> ToolsetFactory {
    Arc::new(Vec::new)
}

/// No assistant tool call left without its result.
fn coherent(agent: &Agent) -> bool {
    let msgs = agent.messages();
    let mut open: Vec<String> = Vec::new();
    for m in &msgs {
        for b in m.content_blocks() {
            match b {
                ContentBlock::ToolUse { id, .. } => open.push(id),
                ContentBlock::ToolResult { tool_use_id, .. } => open.retain(|i| *i != tool_use_id),
                _ => {}
            }
        }
    }
    open.is_empty()
}

// ─── 1. Empty tasks ──────────────────────────────────────────────────────────

const BLANKS: &[&str] = &["", "   ", "\n\t", "\u{3000}\u{00A0}", "\u{200B}\u{FEFF}"];

#[tokio::test]
async fn empty_tasks_build_no_provider_and_send_nothing() {
    let spy = Spy::new(vec![]);
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(Arc::new(AllowAll), dir.path());
    let agent_tool = AgentTool::new(spy.factory(), vec![Box::new(Probe) as Box<dyn Tool>]);
    let pf = {
        let spy = spy.clone();
        Arc::new(move || Box::new(spy.provider()) as Box<dyn Provider + Send + Sync>)
            as cersei_agent::delegate::ProviderFactory
    };
    let delegate = DelegateTool::new(pf.clone(), no_tools());

    for blank in BLANKS {
        let r = agent_tool
            .execute(json!({"description": "x", "prompt": blank}), &c)
            .await;
        assert!(
            r.is_error && r.content.contains("empty"),
            "{blank:?}: {}",
            r.content
        );
        let r = delegate.execute(json!({"goal": blank}), &c).await;
        assert!(
            r.is_error && r.content.contains("empty goal"),
            "{}",
            r.content
        );
    }
    // One invalid task in a batch: nothing starts, the valid one included.
    let r = delegate
        .execute(
            json!({"tasks": [{"goal": "lis a.txt"}, {"goal": " \u{200B} "}]}),
            &c,
        )
        .await;
    assert!(
        r.is_error && r.content.contains("task(s) 2 of 2"),
        "{}",
        r.content
    );
    // Impossible limits.
    let r = agent_tool
        .execute(
            json!({"description": "x", "prompt": "ok", "max_turns": 0}),
            &c,
        )
        .await;
    assert!(
        r.is_error && r.content.contains("max_turns"),
        "{}",
        r.content
    );
    let r = agent_tool
        .execute(
            json!({"description": "x", "prompt": "ok", "max_turns": 10_000}),
            &c,
        )
        .await;
    // Turn limits were removed: the former argument is refused, explained.
    assert!(
        r.is_error && r.content.contains("was removed"),
        "{}",
        r.content
    );
    // A model change would be silently ignored: refused instead.
    let r = agent_tool
        .execute(
            json!({"description": "x", "prompt": "ok", "model": "other/m"}),
            &c,
        )
        .await;
    assert!(r.is_error && r.content.contains("model"), "{}", r.content);

    // The public API, bypassing the tools.
    let mut cfg = DelegateConfig::new(pf.clone(), no_tools());
    cfg.tasks = vec![DelegateTask::new("ok"), DelegateTask::new("\u{2003}")];
    assert!(matches!(
        run_batch(cfg).await,
        Err(CerseiError::InvalidInput(_))
    ));
    let mut cfg = DelegateConfig::new(pf.clone(), no_tools());
    cfg.tasks = vec![DelegateTask::new("ok")];
    cfg.max_concurrent = 0;
    assert!(run_batch(cfg).await.is_err());
    // The documented no-op stays one.
    let cfg = DelegateConfig::new(pf, no_tools());
    assert!(run_batch(cfg).await.unwrap().is_empty());

    // An empty run of an agent.
    let a = Agent::builder()
        .provider(SpyProvider(spy.clone()))
        .build()
        .unwrap();
    for blank in BLANKS {
        assert!(matches!(
            a.run(blank).await,
            Err(CerseiError::InvalidInput(_))
        ));
    }

    assert_eq!(spy.built(), 0, "no provider built for an empty task");
    assert_eq!(spy.sent(), 0, "no request sent for an empty task");
}

/// An image without text is a real prompt: sent, without an empty text
/// block.
#[tokio::test]
async fn an_image_only_prompt_is_sent() {
    let spy = Spy::new(vec![say("Un carré rouge.")]);
    let a = agent(&spy);
    let input = UserInput {
        text: "  ".into(),
        attachments: vec![ContentBlock::Image {
            source: ImageSource {
                source_type: "base64".into(),
                media_type: Some("image/png".into()),
                data: Some("iVBORw0KGgo=".into()),
                url: None,
                file_id: None,
            },
        }],
    };
    let out = a.run_input(&input).await.unwrap();
    assert!(out.is_complete());
    let reqs = spy.requests.lock();
    assert_eq!(reqs.len(), 1);
    let blocks = reqs[0].messages[0].content_blocks();
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert!(matches!(blocks[0], ContentBlock::Image { .. }));
}

// ─── 2–5. Final answers and real work ────────────────────────────────────────

#[tokio::test]
async fn a_simple_request_is_one_request_without_forced_tools() {
    let spy = Spy::new(vec![say("4")]);
    let out = agent(&spy).run("2+2 ?").await.unwrap();
    assert_eq!(out.termination, Termination::Completed);
    assert_eq!((out.turns, spy.sent()), (1, 1));
    let req = &spy.requests.lock()[0];
    assert!(!req.tools.is_empty(), "tools were available");
    assert!(req.options.get::<String>("tool_choice").is_none());
}

#[tokio::test]
async fn one_read_then_the_answer_stops_there() {
    let spy = Spy::new(vec![call("c1", "Probe", json!({"n": 1})), say("Lu : 1.")]);
    let a = agent(&spy);
    let out = a.run("lis 1").await.unwrap();
    assert_eq!(out.termination, Termination::Completed);
    assert_eq!((out.turns, spy.sent()), (2, 2));
    assert_eq!(out.text(), "Lu : 1.");
    // Nothing the engine added between the result and the answer.
    let last_req = &spy.requests.lock()[1];
    let text: String = last_req
        .messages
        .iter()
        .filter_map(|m| m.get_text())
        .collect();
    assert!(!text.contains("[system]"), "{text}");
}

#[tokio::test]
async fn multi_step_work_goes_on_until_its_answer() {
    let spy = Spy::new(vec![
        call("c1", "Probe", json!({"n": 1})),
        call("c2", "Probe", json!({"n": 2})),
        call("c3", "Probe", json!({"fail": true})),
        call("c4", "Probe", json!({"n": 3})),
        say("Trois valeurs lues."),
    ]);
    let out = agent(&spy).run("lis tout").await.unwrap();
    assert_eq!(out.termination, Termination::Completed);
    assert_eq!((out.turns, spy.sent()), (5, 5));
    assert_eq!(out.tool_calls.len(), 4);
}

// ─── 6. Stops ────────────────────────────────────────────────────────────────

/// No turn limit (removed in 0.4.8): 150 turns of real progress, far past
/// the former defaults (10 for the SDK, 50 for the CLI, 30/100 for
/// sub-agents), then the answer ends the run.
#[tokio::test]
async fn there_is_no_turn_limit() {
    let mut steps: Vec<Step> = (1..=150)
        .map(|n| call(&format!("c{n}"), "Probe", json!({"n": n})))
        .collect();
    steps.push(say("Fini."));
    let spy = Spy::new(steps);
    let a = agent(&spy);
    let out = tokio::time::timeout(Duration::from_secs(60), a.run("longue tâche"))
        .await
        .expect("the test harness's own timeout")
        .unwrap();
    assert_eq!(out.termination, Termination::Completed);
    assert_eq!((out.turns, spy.sent()), (151, 151));
    assert_eq!(out.tool_calls.len(), 150);
    assert!(coherent(&a), "every call has its result");
}

#[tokio::test]
async fn cut_answers_are_continued_a_bounded_number_of_times() {
    let cut = |t: &str| Step {
        text: Some(t.into()),
        stop: Some(StopReason::MaxTokens),
        ..Default::default()
    };
    // A cut call (arguments incomplete) is answered, never run.
    let cut_call = Step {
        calls: vec![("c1".into(), "Probe".into(), json!({"n": 1}))],
        stop: Some(StopReason::MaxTokens),
        ..Default::default()
    };
    let spy = Spy::new(vec![cut_call, cut("b"), cut("c"), cut("d"), say("jamais")]);
    let a = agent(&spy);
    let out = a.run("long").await.unwrap();
    assert_eq!(
        out.termination,
        Termination::OutputTruncated { continuations: 3 }
    );
    assert_eq!(spy.sent(), 4, "the first answer and three continuations");
    assert!(out.tool_calls.is_empty(), "a cut call never runs");
    assert!(coherent(&a));
}

#[tokio::test]
async fn repeated_failures_end_without_endless_retries() {
    let spy = Spy::new(vec![]);
    *spy.fallback.lock() = Some(call("c", "Probe", json!({"fail": true})));
    let a = agent(&spy);
    let out = a.run("cherche").await.unwrap();
    assert_eq!(out.termination, Termination::NoProgress { repeats: 5 });
    assert_eq!(
        spy.sent(),
        6,
        "told once after the 4th identical round, stopped at the 6th"
    );
    assert!(coherent(&a));
    // The warning is in the history once.
    let warnings = a
        .messages()
        .iter()
        .flat_map(|m| m.content_blocks())
        .filter(|b| matches!(b, ContentBlock::Text { text } if text.contains("repeated earlier")))
        .count();
    assert_eq!(warnings, 1);
}

// ─── 7. Loop or progress ─────────────────────────────────────────────────────

#[tokio::test]
async fn different_calls_of_one_tool_are_progress() {
    let mut steps: Vec<Step> = (1..=12)
        .map(|n| call(&format!("c{n}"), "Probe", json!({"n": n})))
        .collect();
    steps.push(say("Douze valeurs."));
    let spy = Spy::new(steps);
    let a = agent(&spy);
    let out = a.run("lis douze").await.unwrap();
    assert_eq!(out.termination, Termination::Completed);
    assert_eq!(spy.sent(), 13);
    assert!(!a
        .messages()
        .iter()
        .flat_map(|m| m.content_blocks())
        .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("repeated earlier"))));
}

#[tokio::test]
async fn an_alternating_cycle_with_the_same_results_is_a_loop() {
    let spy = Spy::new(vec![]);
    let a_call = call("a", "Probe", json!({"n": 1}));
    let b_call = call("b", "Probe", json!({"n": 2}));
    for _ in 0..10 {
        spy.steps.lock().push_back(a_call.clone());
        spy.steps.lock().push_back(b_call.clone());
    }
    let out = agent(&spy).run("cycle").await.unwrap();
    assert_eq!(out.termination, Termination::NoProgress { repeats: 5 });
    assert_eq!(
        spy.sent(),
        7,
        "A B are new; the five rounds after them repeat"
    );
}

// ─── 8. Sub-agents: permissions, tools, recursion ────────────────────────────

#[tokio::test]
async fn a_child_has_its_parents_permissions_not_more() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("child.txt");
    let spy = Spy::new(vec![
        call(
            "w1",
            "Write",
            json!({"file_path": target.display().to_string(), "content": "x"}),
        ),
        say("Écriture refusée."),
    ]);
    let tool = AgentTool::new(spy.factory(), cersei_tools::filesystem());
    let c = ctx(Arc::new(DenyAll), dir.path());
    let r = tool
        .execute(json!({"description": "w", "prompt": "écris child.txt"}), &c)
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(
        !target.exists(),
        "the parent's DenyAll applies to the child"
    );
    let result = spy.requests.lock()[1]
        .messages
        .iter()
        .flat_map(|m| m.content_blocks())
        .find_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => Some(format!("{content:?}")),
            _ => None,
        })
        .unwrap();
    assert!(result.contains("Permission denied"), "{result}");
}

#[tokio::test]
async fn a_child_never_gets_more_tools_nor_a_delegation_tool() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(Arc::new(AllowAll), dir.path());

    // No tools given: none, not the standard set.
    let spy = Spy::new(vec![say("ok")]);
    let tool = AgentTool::new(spy.factory(), vec![]);
    let r = tool
        .execute(json!({"description": "x", "prompt": "fais"}), &c)
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(spy.requests.lock()[0].tools.is_empty());

    // Factories that return delegation tools: removed on both paths.
    let spy = Spy::new(vec![say("ok"), say("ok")]);
    let tf: ToolsetFactory = Arc::new(|| {
        vec![
            Box::new(Named("Agent")) as Box<dyn Tool>,
            Box::new(Named("delegate")),
            Box::new(Probe),
        ]
    });
    let tool = AgentTool::with_toolset(spy.factory(), tf.clone());
    tool.execute(json!({"description": "x", "prompt": "fais"}), &c)
        .await;
    let pf = {
        let spy = spy.clone();
        Arc::new(move || Box::new(spy.provider()) as Box<dyn Provider + Send + Sync>)
            as cersei_agent::delegate::ProviderFactory
    };
    DelegateTool::new(pf, tf)
        .execute(json!({"goal": "fais"}), &c)
        .await;
    for req in spy.requests.lock().iter() {
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Probe"]);
    }

    // A tool the registry cannot rebuild is refused, not silently dropped.
    let spy = Spy::new(vec![]);
    let tool = AgentTool::new(spy.factory(), vec![Box::new(Probe) as Box<dyn Tool>]);
    let r = tool
        .execute(json!({"description": "x", "prompt": "fais"}), &c)
        .await;
    assert!(r.is_error && r.content.contains("Probe"), "{}", r.content);
    assert_eq!(spy.built(), 0);
}

#[tokio::test]
async fn a_child_cannot_delegate_on_either_path() {
    let dir = tempfile::tempdir().unwrap();
    let spy = Spy::new(vec![]);
    let c = ctx(Arc::new(AllowAll), dir.path());
    c.extensions.insert(DelegationDepth(1));
    let r = AgentTool::new(spy.factory(), vec![])
        .execute(json!({"description": "x", "prompt": "fais"}), &c)
        .await;
    assert!(r.is_error, "{}", r.content);
    let pf = {
        let spy = spy.clone();
        Arc::new(move || Box::new(spy.provider()) as Box<dyn Provider + Send + Sync>)
            as cersei_agent::delegate::ProviderFactory
    };
    let r = DelegateTool::new(pf, no_tools())
        .execute(json!({"goal": "fais"}), &c)
        .await;
    assert!(r.is_error && r.content.contains("depth"), "{}", r.content);
    assert_eq!((spy.built(), spy.sent()), (0, 0));
}

#[tokio::test]
async fn a_child_that_stops_without_an_answer_is_not_a_success() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(Arc::new(AllowAll), dir.path());
    // The child keeps failing the same way: stopped for no progress.
    let spy = Spy::new(vec![Step {
        text: Some("Je commence…".into()),
        calls: vec![("c1".into(), "Probe".into(), json!({"fail": true}))],
        ..Default::default()
    }]);
    *spy.fallback.lock() = Some(call("c", "Probe", json!({"fail": true})));
    let tf: ToolsetFactory = Arc::new(|| vec![Box::new(Probe) as Box<dyn Tool>]);
    let r = AgentTool::with_toolset(spy.factory(), tf.clone())
        .execute(json!({"description": "x", "prompt": "fais"}), &c)
        .await;
    assert!(r.is_error);
    assert!(r.content.contains("incomplete"), "{}", r.content);
    assert_eq!(r.metadata.as_ref().unwrap()["status"], "incomplete");

    // An empty final answer is not a completed task either.
    let spy = Spy::new(vec![Step::default()]);
    let r = AgentTool::with_toolset(spy.factory(), tf)
        .execute(json!({"description": "x", "prompt": "fais"}), &c)
        .await;
    assert!(r.is_error, "{}", r.content);
}

// ─── 9. Cancellation ─────────────────────────────────────────────────────────

#[tokio::test]
async fn cancelling_the_parent_stops_its_child_and_starts_nothing_more() {
    // Parent: one Agent call; child: a request that never ends.
    let spy = Spy::new(vec![
        call(
            "a1",
            "Agent",
            json!({"description": "x", "prompt": "attends"}),
        ),
        hang(),
    ]);
    let tool = AgentTool::with_toolset(spy.factory(), no_tools());
    let parent = Arc::new(
        Agent::builder()
            .provider(spy.provider())
            .tool(tool)
            .build()
            .unwrap(),
    );
    let run = {
        let parent = parent.clone();
        tokio::spawn(async move { parent.run("délègue").await })
    };
    for _ in 0..200 {
        if spy.sent() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(spy.sent(), 2, "the child's request is in flight");
    parent.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("the parent stops promptly")
        .unwrap();
    assert!(matches!(result, Err(CerseiError::Cancelled)));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        spy.dropped.load(Ordering::SeqCst),
        1,
        "the child's request was dropped"
    );
    assert_eq!(spy.sent(), 2, "no request after the cancellation");
}

#[tokio::test]
async fn a_cancelled_batch_starts_no_further_child() {
    let spy = Spy::new(vec![hang(), say("b"), say("c")]);
    let pf = {
        let spy = spy.clone();
        Arc::new(move || Box::new(spy.provider()) as Box<dyn Provider + Send + Sync>)
            as cersei_agent::delegate::ProviderFactory
    };
    let token = tokio_util::sync::CancellationToken::new();
    let mut cfg = DelegateConfig::new(pf, no_tools());
    cfg.tasks = vec![
        DelegateTask::new("a"),
        DelegateTask::new("b"),
        DelegateTask::new("c"),
    ];
    cfg.max_concurrent = 1;
    cfg.cancel = Some(token.clone());
    let batch = tokio::spawn(run_batch(cfg));
    for _ in 0..200 {
        if spy.sent() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    token.cancel();
    let results = tokio::time::timeout(Duration::from_secs(5), batch)
        .await
        .expect("the batch stops promptly")
        .unwrap()
        .unwrap();
    let statuses: Vec<ChildStatus> = results.iter().map(|r| r.status.clone()).collect();
    assert_eq!(statuses, vec![ChildStatus::Cancelled; 3]);
    assert_eq!(spy.sent(), 1, "no child after the cancellation");
    assert_eq!(spy.built(), 1);

    // Already cancelled: nothing is built.
    let spy2 = Spy::new(vec![]);
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(Arc::new(AllowAll), dir.path());
    let t = tokio_util::sync::CancellationToken::new();
    t.cancel();
    c.extensions.insert(RunCancellation(t));
    let r = AgentTool::new(spy2.factory(), vec![])
        .execute(json!({"description": "x", "prompt": "fais"}), &c)
        .await;
    assert!(r.is_error);
    assert_eq!(spy2.built(), 0);
}

/// The historical delegation paths pass no turn budget either: each child
/// works past its former default (10 for `AgentTool`, 30 for `delegate`)
/// and gives its answer.
#[tokio::test]
async fn historical_delegation_paths_have_no_turn_limit() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(Arc::new(AllowAll), dir.path());
    let work = |n: usize| {
        let mut v: Vec<Step> = (1..=n)
            .map(|i| call(&format!("c{i}"), "Probe", json!({"n": i})))
            .collect();
        v.push(say("Fini."));
        v
    };
    let tf: ToolsetFactory = Arc::new(|| vec![Box::new(Probe) as Box<dyn Tool>]);
    let limit = Duration::from_secs(60);

    let spy = Spy::new(work(40));
    let tool = AgentTool::with_toolset(spy.factory(), tf.clone());
    let r = tokio::time::timeout(
        limit,
        tool.execute(json!({"description": "x", "prompt": "fais"}), &c),
    )
    .await
    .unwrap();
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(spy.sent(), 41, "AgentTool: past its former default of 10");

    let spy = Spy::new(work(45));
    let pf = spy.factory();
    let pf: cersei_agent::delegate::ProviderFactory = Arc::new(move || pf());
    let tool = DelegateTool::new(pf.clone(), tf.clone());
    let r = tokio::time::timeout(limit, tool.execute(json!({"goal": "fais"}), &c))
        .await
        .unwrap();
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        spy.sent(),
        46,
        "DelegateTool: past its former default of 30"
    );

    let spy = Spy::new(work(35));
    let pf = spy.factory();
    let pf: cersei_agent::delegate::ProviderFactory = Arc::new(move || pf());
    let mut cfg = DelegateConfig::new(pf, tf);
    cfg.tasks = vec![DelegateTask::new("fais")];
    let out = tokio::time::timeout(limit, run_batch(cfg))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out[0].turns, 36, "run_batch: past its former default of 30");
}
