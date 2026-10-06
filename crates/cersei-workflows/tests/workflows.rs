//! Integration tests for cersei-workflows: IR round-trip, sequential execution,
//! parallel + branch, streaming, ToolStep, and suspend/resume.

use std::sync::Arc;

use cersei_tools::{Tool, ToolContext, ToolResult};
use cersei_workflows::prelude::*;
use cersei_workflows::{Condition, JoinStrategy, StepOutcome, WorkflowEvent};
use serde_json::{json, Value};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn registry_with_text_steps() -> Arc<StepRegistry> {
    let reg = StepRegistry::new();
    reg.register(Arc::new(FnStep::new(
        "upper",
        |input: Value, _ctx| async move {
            let s = input.get("message").and_then(|v| v.as_str()).unwrap_or("");
            Ok(json!({ "message": s.to_uppercase() }))
        },
    )));
    reg.register(Arc::new(FnStep::new(
        "emphasize",
        |input: Value, _ctx| async move {
            let s = input.get("message").and_then(|v| v.as_str()).unwrap_or("");
            Ok(json!({ "message": format!("{}!!!", s) }))
        },
    )));
    reg
}

// ─── 1. IR round-trip ─────────────────────────────────────────────────────────

#[test]
fn ir_round_trips_through_json() {
    let def = WorkflowBuilder::new("rt")
        .then("upper")
        .then("emphasize")
        .commit();
    let json = serde_json::to_string(&def).unwrap();
    let back: WorkflowDef = serde_json::from_str(&json).unwrap();
    assert_eq!(
        def, back,
        "WorkflowDef must survive a JSON round-trip losslessly"
    );
}

// ─── 2. Sequential execution (Mastra docs example shape) ──────────────────────

#[tokio::test]
async fn sequential_pipeline_runs_in_order() {
    let reg = registry_with_text_steps();
    let def = WorkflowBuilder::new("pipe")
        .then("upper")
        .then("emphasize")
        .commit();
    let wf = Workflow::compile(def, &reg).unwrap();

    let result = wf.start(json!({ "message": "hello world" })).await.unwrap();
    assert_eq!(result.status, RunStatus::Success);
    assert_eq!(
        result.result.unwrap(),
        json!({ "message": "HELLO WORLD!!!" })
    );
    // Both steps recorded.
    assert_eq!(result.steps.len(), 2);
    assert!(result
        .steps
        .values()
        .all(|s| s.status == RunStatus::Success));
}

// ─── 3. Parallel + join ───────────────────────────────────────────────────────

#[tokio::test]
async fn parallel_fans_out_and_joins() {
    let reg = StepRegistry::new();
    reg.register(Arc::new(FnStep::new("a", |_in, _ctx| async move {
        Ok(json!({ "from": "a" }))
    })));
    reg.register(Arc::new(FnStep::new("b", |_in, _ctx| async move {
        Ok(json!({ "from": "b" }))
    })));

    let def = WorkflowBuilder::new("par")
        .parallel(&["a", "b"], JoinStrategy::AllOrFail)
        .commit();
    let wf = Workflow::compile(def, &reg).unwrap();

    let result = wf.start(json!({})).await.unwrap();
    assert_eq!(result.status, RunStatus::Success);
    let arr = result.result.unwrap();
    assert!(arr.is_array());
    assert_eq!(arr.as_array().unwrap().len(), 2);
}

// ─── 4. Branch: first matching arm wins ───────────────────────────────────────

#[tokio::test]
async fn branch_takes_first_matching_arm() {
    let reg = StepRegistry::new();
    reg.register(Arc::new(FnStep::new("paid", |_in, _ctx| async move {
        Ok(json!({ "tier": "paid" }))
    })));
    reg.register(Arc::new(FnStep::new("free", |_in, _ctx| async move {
        Ok(json!({ "tier": "free" }))
    })));

    let def = WorkflowBuilder::new("branch")
        .branch(vec![
            (
                Condition::Eq {
                    path: "current/premium".into(),
                    value: json!(true),
                },
                "paid",
            ),
            (Condition::Always, "free"),
        ])
        .commit();
    let wf = Workflow::compile(def, &reg).unwrap();

    let result = wf.start(json!({ "premium": true })).await.unwrap();
    assert_eq!(result.status, RunStatus::Success);
    // Only the "paid" arm ran — "free" never executed.
    let ran: Vec<String> = result.steps.keys().cloned().collect();
    assert!(ran.iter().any(|k| k.starts_with("paid")));
    assert!(!ran.iter().any(|k| k.starts_with("free")));
}

// ─── 5. Streaming: events are ordered and serializable ────────────────────────

#[tokio::test]
async fn stream_emits_ordered_serializable_events() {
    let reg = registry_with_text_steps();
    let def = WorkflowBuilder::new("stream")
        .then("upper")
        .then("emphasize")
        .commit();
    let wf = Workflow::compile(def, &reg).unwrap();

    let mut stream = wf.stream(json!({ "message": "hi" }));
    let mut started = 0;
    let mut completed = 0;
    let mut saw_workflow_completed = false;

    while let Some(event) = stream.next().await {
        // UI contract: every event must serialize.
        serde_json::to_value(&event).expect("event must serialize for the UI");
        match event {
            WorkflowEvent::StepStarted { .. } => started += 1,
            WorkflowEvent::StepCompleted { .. } => completed += 1,
            WorkflowEvent::WorkflowCompleted { result } => {
                assert_eq!(result.status, RunStatus::Success);
                saw_workflow_completed = true;
            }
            _ => {}
        }
    }
    assert_eq!(started, 2);
    assert_eq!(completed, 2);
    assert!(saw_workflow_completed);
}

// ─── 6. ToolStep wraps a Cersei tool ──────────────────────────────────────────

struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "echoes its input text"
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "text": { "type": "string" } } })
    }
    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("");
        ToolResult::success(format!("echo: {}", text))
    }
}

#[tokio::test]
async fn tool_step_runs_a_tool() {
    let reg = StepRegistry::new();
    reg.register(Arc::new(ToolStep::new("echo", Arc::new(EchoTool))));

    let def = WorkflowBuilder::new("tool").then("echo").commit();
    let wf = Workflow::compile(def, &reg).unwrap();

    let result = wf.start(json!({ "text": "ping" })).await.unwrap();
    assert_eq!(result.status, RunStatus::Success);
    assert_eq!(
        result.result.unwrap().get("content").unwrap(),
        &json!("echo: ping")
    );
}

// ─── 7. Suspend / resume ──────────────────────────────────────────────────────

#[tokio::test]
async fn suspend_then_resume_completes() {
    let reg = StepRegistry::new();
    // A gate step that suspends until resume data arrives, then passes it through.
    reg.register(Arc::new(FnStep::with_outcome(
        "gate",
        |input: Value, ctx| async move {
            match ctx.resume_data {
                Some(data) => Ok(StepOutcome::Done(json!({ "approved": data }))),
                None => Ok(StepOutcome::Suspended {
                    resume_schema: json!({ "type": "boolean" }),
                    payload: input,
                }),
            }
        },
    )));
    reg.register(Arc::new(FnStep::new(
        "finish",
        |input: Value, _ctx| async move { Ok(json!({ "final": input })) },
    )));

    let def = WorkflowBuilder::new("gate_flow")
        .then("gate")
        .then("finish")
        .commit();
    let wf = Workflow::compile(def, &reg).unwrap();

    // First run suspends at the gate.
    let suspended = wf.start(json!({ "request": 1 })).await.unwrap();
    assert_eq!(suspended.status, RunStatus::Suspended);
    assert_eq!(suspended.suspended.len(), 1);
    let node_id = suspended.suspended[0].node_id.clone();

    // Resume with approval — the workflow continues to completion.
    let resumed = wf
        .resume(&suspended.run_id, &node_id, json!(true))
        .await
        .unwrap();
    assert_eq!(resumed.status, RunStatus::Success);
    assert_eq!(
        resumed.result.unwrap(),
        json!({ "final": { "approved": true } })
    );
}

// ─── 8. Unknown step id fails compilation ─────────────────────────────────────

#[test]
fn compile_rejects_unknown_step() {
    let reg = StepRegistry::new();
    let def = WorkflowBuilder::new("bad").then("does_not_exist").commit();
    let err = match Workflow::compile(def, &reg) {
        Ok(_) => panic!("expected compile to reject unknown step"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("unknown step"));
}
