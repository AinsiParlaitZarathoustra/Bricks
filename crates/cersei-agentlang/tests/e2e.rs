use cersei_agentlang::interp::{run_program, EvalCtx};
use cersei_agentlang::registry::{DispatchHandle, VecToolDispatch};
use cersei_agentlang::RunAgentTemplateTool;
use cersei_tools::file_read::FileReadTool;
use cersei_tools::file_write::FileWriteTool;
use cersei_tools::permissions::AllowAll;
use cersei_tools::{CostTracker, Extensions, Tool, ToolContext};
use cersei_vms::{Mailbox, SandboxId};
use serde_json::json;
use std::sync::Arc;

fn base_ctx() -> ToolContext {
    ToolContext {
        working_dir: std::env::temp_dir(),
        session_id: "e2e".into(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

#[tokio::test]
async fn end_to_end_real_file_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.txt");
    let dst = dir.path().join("out.txt");
    std::fs::write(&src, "hello agentlang").unwrap();

    let dispatch = Arc::new(VecToolDispatch::new(vec![
        Arc::new(FileReadTool) as Arc<dyn Tool>,
        Arc::new(FileWriteTool) as Arc<dyn Tool>,
    ]));
    let ctx = base_ctx();
    let mut ev = EvalCtx::new(&ctx, dispatch)
        .with_var("src", json!(src.to_str().unwrap()))
        .with_var("dst", json!(dst.to_str().unwrap()));

    let program = "$data = io.read($src)\nio.write($dst, content: $data)";
    run_program(program, &mut ev).await.unwrap();

    // FileReadTool decorates output with line numbers (cat -n style), so this
    // is a wiring roundtrip rather than a byte-exact copy.
    let written = std::fs::read_to_string(&dst).unwrap();
    assert!(written.contains("hello agentlang"), "got: {written:?}");
}

#[tokio::test]
async fn agent_send_publishes_to_mailbox() {
    let mailbox = Mailbox::new();
    let mut sub = mailbox.subscribe("worker-2");

    let dispatch = Arc::new(VecToolDispatch::new(vec![]));
    let ctx = base_ctx();
    let mut ev = EvalCtx::new(&ctx, dispatch)
        .with_mailbox(Arc::new(mailbox.clone()))
        .with_self_id(SandboxId::from("sender"))
        .with_var("msg", json!({ "hello": "there" }));

    run_program(r#"agent.send(to: "worker-2", $msg)"#, &mut ev)
        .await
        .unwrap();

    let env = sub.try_recv().unwrap().expect("a message");
    assert_eq!(env.from, SandboxId::from("sender"));
    assert_eq!(env.payload, json!({ "hello": "there" }));
}

#[tokio::test]
async fn run_agent_template_tool_surface() {
    let dispatch: Arc<dyn cersei_agentlang::ToolDispatch> = Arc::new(VecToolDispatch::new(vec![]));
    let ctx = base_ctx();
    ctx.extensions.insert(DispatchHandle(dispatch));

    let out = RunAgentTemplateTool
        .execute(json!({ "program": "agent.tools.list()" }), &ctx)
        .await;
    assert!(!out.is_error, "{}", out.content);
    let meta = out.metadata.unwrap();
    assert_eq!(meta["value"], json!([]));
}
