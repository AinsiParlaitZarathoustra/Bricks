//! The stop contract, with a spy
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

use async_trait::async_trait;
use bricks_agent::{Agent, Termination, UserInput};
use bricks_provider::{CompletionRequest, CompletionStream, Provider};
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use bricks_types::*;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

// ─── Spy provider ────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Step {
    text: Option<String>,
    calls: Vec<(String, String, Value)>,
    stop: Option<StopReason>,
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

#[derive(Default)]
struct Spy {
    steps: parking_lot::Mutex<VecDeque<Step>>,
    built: AtomicUsize,
    requests: parking_lot::Mutex<Vec<CompletionRequest>>,
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
            return Err(BricksError::Provider(
                "spy: unexpected request (script exhausted)".into(),
            ));
        };
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::MessageStart {
                    id: "m".into(),
                    model: "spy".into(),
                    usage: None,
                })
                .await;
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

fn agent(spy: &Arc<Spy>) -> Agent {
    Agent::builder()
        .provider(spy.provider())
        .tool(Probe)
        .working_dir(std::env::temp_dir())
        .build()
        .unwrap()
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
    // An empty run of an agent.
    let a = Agent::builder()
        .provider(SpyProvider(spy.clone()))
        .build()
        .unwrap();
    for blank in BLANKS {
        assert!(matches!(
            a.run(blank).await,
            Err(BricksError::InvalidInput(_))
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
        skills: Vec::new(),
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
async fn a_run_goes_on_until_its_answer() {
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
    assert!(
        !a.messages()
            .iter()
            .flat_map(|m| m.content_blocks())
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("repeated earlier")))
    );
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
async fn a_stored_delegate_call_stays_readable_and_is_unknown_now() {
    let old = vec![
        Message::user("délègue la lecture"),
        Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "d1".into(),
            name: "delegate".into(),
            input: json!({"goal": "lis a.txt"}),
        }]),
        Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "d1".into(),
            content: ToolResultContent::Text("Task 1/1 [completed]: a.txt lu".into()),
            is_error: None,
        }]),
        Message::assistant("Fait."),
    ];
    let spy = Spy::new(vec![
        call("c2", "delegate", json!({"goal": "lis b.txt"})),
        say("Je ne peux pas déléguer ici."),
    ]);
    let a = Agent::builder()
        .provider(spy.provider())
        .tool(Probe)
        .working_dir(std::env::temp_dir())
        .with_messages(old)
        .build()
        .unwrap();
    let out = a.run("reprends").await.unwrap();
    assert_eq!(out.termination, Termination::Completed);

    let reqs = spy.requests.lock();
    // The old call is in the first request, as it was written.
    let old_call = reqs[0]
        .messages
        .iter()
        .flat_map(|m| m.content_blocks())
        .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "delegate"));
    assert!(old_call, "the stored call stays readable");

    // A new call to it is refused as an unknown tool: no sub-agent, no provider.
    let refused = reqs[1]
        .messages
        .iter()
        .flat_map(|m| m.content_blocks())
        .find_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } if tool_use_id == "c2" => Some(format!("{content:?}")),
            _ => None,
        })
        .expect("the new call has its result");
    assert!(refused.contains("'delegate' not found"), "{refused}");
    assert_eq!(spy.built(), 1, "only the parent's provider was built");
    assert!(
        !bricks_tools::all().iter().any(|t| t.name() == "delegate"),
        "delegate is not in the standard tool set"
    );
    assert!(!bricks_agent::subagent::DELEGATION_TOOLS.contains(&"delegate"));
    assert!(!bricks_agent::system_prompt::SPAWNING_TOOLS.contains(&"delegate"));
}
