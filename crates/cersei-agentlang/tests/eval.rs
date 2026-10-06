use async_trait::async_trait;
use cersei_agentlang::interp::{run_program, EvalCtx, Limits};
use cersei_agentlang::registry::ToolDispatch;
use cersei_agentlang::ProgramError;
use cersei_tools::permissions::{AllowAll, AllowReadOnly, PermissionPolicy};
use cersei_tools::{CostTracker, Extensions, ToolContext, ToolResult};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Records every dispatched call and returns a canned result per tool name.
#[derive(Default)]
struct MockDispatch {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl ToolDispatch for MockDispatch {
    async fn call(&self, name: &str, input: Value, _ctx: &ToolContext) -> ToolResult {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_string(), input.clone()));
        match name {
            "Read" => ToolResult::success("FILE CONTENTS"),
            "Write" => ToolResult::success("wrote ok"),
            "WebFetch" => ToolResult::success("<html>"),
            other => ToolResult::success(format!("ran {other}")),
        }
    }
    fn has(&self, name: &str) -> bool {
        matches!(
            name,
            "Read" | "Write" | "WebFetch" | "Glob" | "Grep" | "Bash"
        )
    }
    fn list(&self) -> Vec<String> {
        vec!["Read".into(), "Write".into()]
    }
}

fn ctx_with(policy: Arc<dyn PermissionPolicy>) -> ToolContext {
    ToolContext {
        working_dir: std::env::temp_dir(),
        session_id: "test-session".into(),
        permissions: policy,
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

#[tokio::test]
async fn io_read_dispatches_correct_input() {
    let dispatch = Arc::new(MockDispatch::default());
    let ctx = ctx_with(Arc::new(AllowAll));
    let mut ev = EvalCtx::new(&ctx, dispatch.clone()).with_var("f", json!("/tmp/a.txt"));
    let out = run_program("io.read($f)", &mut ev).await.unwrap();
    assert_eq!(out, json!("FILE CONTENTS"));
    let calls = dispatch.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "Read");
    assert_eq!(calls[0].1, json!({ "file_path": "/tmp/a.txt" }));
}

#[tokio::test]
async fn io_write_denied_under_read_only() {
    let dispatch = Arc::new(MockDispatch::default());
    let ctx = ctx_with(Arc::new(AllowReadOnly));
    let mut ev = EvalCtx::new(&ctx, dispatch.clone());
    let err = run_program(r#"io.write("/tmp/a", content: "x")"#, &mut ev)
        .await
        .unwrap_err();
    match err {
        ProgramError::Runtime(e) => {
            assert_eq!(e.kind, cersei_agentlang::RuntimeErrorKind::PermissionDenied)
        }
        other => panic!("expected permission denied, got {other}"),
    }
    // The write must not have reached the dispatch layer.
    assert!(dispatch.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn chaining_threads_previous_result_into_pipe_target() {
    let dispatch = Arc::new(MockDispatch::default());
    let ctx = ctx_with(Arc::new(AllowAll));
    let mut ev = EvalCtx::new(&ctx, dispatch.clone());
    // read returns "FILE CONTENTS"; chained write should receive it as `content`.
    run_program(r#"io.read("/in").write("/out")"#, &mut ev)
        .await
        .unwrap();
    let calls = dispatch.calls.lock().unwrap();
    assert_eq!(calls[0].0, "Read");
    assert_eq!(calls[1].0, "Write");
    assert_eq!(
        calls[1].1,
        json!({ "file_path": "/out", "content": "FILE CONTENTS" })
    );
}

#[tokio::test]
async fn step_limit_aborts() {
    let dispatch = Arc::new(MockDispatch::default());
    let ctx = ctx_with(Arc::new(AllowAll));
    let mut ev = EvalCtx::new(&ctx, dispatch).with_limits(Limits { max_steps: 1 });
    let err = run_program("io.read('/a')\nio.read('/b')", &mut ev)
        .await
        .unwrap_err();
    match err {
        ProgramError::Runtime(e) => {
            assert_eq!(
                e.kind,
                cersei_agentlang::RuntimeErrorKind::StepLimitExceeded
            )
        }
        other => panic!("expected step limit, got {other}"),
    }
}

#[tokio::test]
async fn unknown_builtin_errors() {
    let dispatch = Arc::new(MockDispatch::default());
    let ctx = ctx_with(Arc::new(AllowAll));
    let mut ev = EvalCtx::new(&ctx, dispatch);
    let err = run_program("io.teleport('/a')", &mut ev).await.unwrap_err();
    assert!(err.to_string().contains("unknown builtin"), "{err}");
}

#[tokio::test]
async fn tools_list_returns_names() {
    let dispatch = Arc::new(MockDispatch::default());
    let ctx = ctx_with(Arc::new(AllowAll));
    let mut ev = EvalCtx::new(&ctx, dispatch);
    let out = run_program("agent.tools.list()", &mut ev).await.unwrap();
    assert_eq!(out, json!(["Read", "Write"]));
}
