//! Real runner contracts: prompts, interactive decisions and injected input.
use async_trait::async_trait;
use cersei_agent::Agent;
use cersei_agent::control::scripted::{Reply, Script, ScriptedProvider};
use cersei_agent::events::{AgentEvent, AgentStream};
use cersei_tools::permissions::{InteractivePolicy, PermissionDecision};
use cersei_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use cersei_types::{ContentBlock, Role};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::Semaphore;

struct Probe {
    ran: Arc<AtomicUsize>,
    gate: Option<Arc<Semaphore>>,
}
#[async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        "Probe"
    }
    fn description(&self) -> &str {
        "Record execution"
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object"})
    }
    async fn execute(&self, _: Value, _: &ToolContext) -> ToolResult {
        self.ran.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            let _permit = gate.acquire().await.unwrap();
        }
        ToolResult::success("probe result")
    }
}

fn setup(
    replies: Vec<Reply>,
    gate: Option<Arc<Semaphore>>,
    interactive: bool,
) -> (Arc<Agent>, Arc<Script>, Arc<AtomicUsize>) {
    let script = Script::new(replies);
    let ran = Arc::new(AtomicUsize::new(0));
    let mut builder = Agent::builder()
        .provider(ScriptedProvider::new("test", script.clone()))
        .tool(Probe {
            ran: ran.clone(),
            gate,
        })
        .system_prompt("BASE")
        .append_system_prompt("APPEND-MARK")
        .auto_compact(false);
    if interactive {
        builder = builder.permission_policy(InteractivePolicy::via_stream());
    }
    (Arc::new(builder.build().unwrap()), script, ran)
}

async fn next(stream: &mut AgentStream) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
}
async fn permission(stream: &mut AgentStream) -> String {
    loop {
        if let AgentEvent::PermissionRequired(r) = next(stream).await {
            return r.id;
        }
    }
}
async fn finish(stream: &mut AgentStream) {
    loop {
        match next(stream).await {
            AgentEvent::Complete(_) => return,
            AgentEvent::Error(e) => panic!("{e}"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn appended_prompt_reaches_each_request_once_including_without_base() {
    let (agent, script, _) = setup(vec![Reply::text("done")], None, false);
    agent.run("go").await.unwrap();
    assert_eq!(
        script.requests()[0].1.system.as_deref(),
        Some("BASE\n\nAPPEND-MARK")
    );
    let script = Script::new(vec![Reply::text("done")]);
    let agent = Agent::builder()
        .provider(ScriptedProvider::new("test", script.clone()))
        .append_system_prompt("ONLY-APPEND")
        .build()
        .unwrap();
    agent.run("go").await.unwrap();
    assert_eq!(
        script.requests()[0].1.system.as_deref(),
        Some("ONLY-APPEND")
    );
}

#[tokio::test]
async fn parallel_permissions_are_matched_by_id_and_never_execute_before_approval() {
    let (agent, script, ran) = setup(
        vec![
            Reply::tools(vec![("a", "Probe", json!({})), ("b", "Probe", json!({}))]),
            Reply::text("done"),
        ],
        None,
        true,
    );
    let mut stream = agent.run_stream("go");
    let a = permission(&mut stream).await;
    let b = permission(&mut stream).await;
    assert_ne!(a, b);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    stream.respond_permission("unknown".into(), PermissionDecision::Allow);
    stream.respond_permission(b.clone(), PermissionDecision::Deny("refused".into()));
    stream.respond_permission(b, PermissionDecision::Allow); // already answered
    stream.respond_permission(a, PermissionDecision::AllowOnce);
    finish(&mut stream).await;
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    let reqs = script.requests();
    let results: Vec<_> = reqs[1]
        .1
        .messages
        .iter()
        .flat_map(|m| m.content_blocks())
        .collect();
    assert!(results.iter().any(|b| matches!(
        b,
        ContentBlock::ToolResult {
            is_error: Some(true),
            ..
        }
    )));
}

#[tokio::test]
async fn cancellation_unblocks_permission_and_next_run_works() {
    let (agent, _, ran) = setup(
        vec![Reply::tool("a", "Probe", json!({})), Reply::text("next")],
        None,
        true,
    );
    let mut stream = agent.run_stream("go");
    permission(&mut stream).await;
    stream.inject_message("retained on cancellation".into());
    stream.cancel();
    loop {
        if let AgentEvent::Error(_) = next(&mut stream).await {
            break;
        }
    }
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    assert!(
        agent
            .messages()
            .iter()
            .any(|m| m.get_text().as_deref() == Some("retained on cancellation"))
    );
    assert!(
        agent
            .raw_history()
            .iter()
            .any(|m| m.get_text().as_deref() == Some("retained on cancellation"))
    );
    // Dropping an old stream must not cancel a later run.
    let mut later = agent.run_stream("again");
    drop(stream);
    finish(&mut later).await;
}

#[tokio::test]
async fn dropped_stream_cancels_a_waiting_permission_without_executing() {
    let ran = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // Observe the worker even after the consumer is gone.
    let script = Script::new(vec![Reply::tool("a", "Probe", json!({}))]);
    let agent2 = Arc::new(
        Agent::builder()
            .provider(ScriptedProvider::new("test", script))
            .tool(Probe {
                ran: ran.clone(),
                gate: None,
            })
            .permission_policy(InteractivePolicy::via_stream())
            .on_event(move |e| {
                let _ = tx.send(e.clone());
            })
            .build()
            .unwrap(),
    );
    let mut stream = agent2.run_stream("go");
    permission(&mut stream).await;
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(rx.recv().await, Some(AgentEvent::Error(_))) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn deferred_policy_without_a_stream_denies_calls() {
    let (agent, _, ran) = setup(
        vec![Reply::tool("a", "Probe", json!({})), Reply::text("done")],
        None,
        true,
    );
    agent.run("go").await.unwrap();
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn injected_messages_follow_tool_results_in_order_without_queue_loss() {
    let gate = Arc::new(Semaphore::new(0));
    let (agent, script, _) = setup(
        vec![Reply::tool("a", "Probe", json!({})), Reply::text("done")],
        Some(gate.clone()),
        true,
    );
    let mut stream = agent.run_stream("go");
    let id = permission(&mut stream).await;
    for i in 0..100 {
        stream.inject_message(format!("injected-{i}"));
    }
    stream.inject_message("  ".into());
    stream.respond_permission(id, PermissionDecision::Allow);
    gate.add_permits(1);
    finish(&mut stream).await;
    let reqs = script.requests();
    let messages = &reqs[1].1.messages;
    let result = messages
        .iter()
        .position(|m| {
            m.content_blocks()
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
        })
        .unwrap();
    for i in 0..100 {
        assert_eq!(messages[result + 1 + i].role, Role::User);
        assert_eq!(
            messages[result + 1 + i].get_text().as_deref(),
            Some(format!("injected-{i}").as_str())
        );
    }
    assert_eq!(messages.len(), result + 101);
    let raw = agent.raw_history();
    for i in 0..100 {
        assert_eq!(
            raw.iter()
                .filter(|m| m.get_text().as_deref() == Some(format!("injected-{i}").as_str()))
                .count(),
            1
        );
    }
}

struct GatedProvider {
    inner: ScriptedProvider,
    gate: Arc<Semaphore>,
    entered: Arc<Semaphore>,
}
#[async_trait]
impl cersei_provider::Provider for GatedProvider {
    fn name(&self) -> &str {
        "gated"
    }
    fn context_window(&self, _: &str) -> u64 {
        100_000
    }
    async fn complete(
        &self,
        request: cersei_provider::CompletionRequest,
    ) -> cersei_types::Result<cersei_provider::CompletionStream> {
        self.entered.add_permits(1);
        self.gate.acquire().await.unwrap().forget();
        cersei_provider::Provider::complete(&self.inner, request).await
    }
}

#[tokio::test]
async fn injection_during_final_generation_causes_another_request() {
    let script = Script::new(vec![Reply::text("first"), Reply::text("second")]);
    let gate = Arc::new(Semaphore::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let agent = Arc::new(
        Agent::builder()
            .provider(GatedProvider {
                inner: ScriptedProvider::new("test", script.clone()),
                gate: gate.clone(),
                entered: entered.clone(),
            })
            .auto_compact(false)
            .build()
            .unwrap(),
    );
    let mut stream = agent.run_stream("go");
    tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    stream.inject_message("follow-up".into());
    gate.add_permits(2);
    finish(&mut stream).await;
    let reqs = script.requests();
    assert_eq!(reqs.len(), 2);
    assert!(
        !reqs[0]
            .1
            .messages
            .iter()
            .any(|m| m.get_text().as_deref() == Some("follow-up"))
    );
    assert_eq!(
        reqs[1].1.messages.last().unwrap().get_text().as_deref(),
        Some("follow-up")
    );
    stream.inject_message("too-late".into());
    assert!(
        !agent
            .messages()
            .iter()
            .any(|m| m.get_text().as_deref() == Some("too-late"))
    );
}

#[tokio::test]
async fn changed_preview_requires_a_new_decision_and_cannot_reuse_the_old_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.txt");
    let script = Script::new(vec![
        Reply::tool(
            "write",
            "Write",
            json!({
                "file_path": path, "content": "model content"
            }),
        ),
        Reply::text("done"),
    ]);
    let agent = Arc::new(
        Agent::builder()
            .provider(ScriptedProvider::new("test", script))
            .tool(cersei_tools::file_write::FileWriteTool)
            .permission_policy(InteractivePolicy::via_stream())
            .auto_compact(false)
            .build()
            .unwrap(),
    );
    let mut stream = agent.run_stream("go");
    let first = permission(&mut stream).await;
    assert!(!path.exists());
    std::fs::write(&path, "external content").unwrap();
    stream.respond_permission(first.clone(), PermissionDecision::Allow);
    let second = permission(&mut stream).await;
    assert_ne!(first, second);
    stream.respond_permission(first, PermissionDecision::Allow);
    stream.respond_permission(
        second,
        PermissionDecision::Deny("keep external edit".into()),
    );
    finish(&mut stream).await;
    assert_eq!(std::fs::read_to_string(path).unwrap(), "external content");
}

#[tokio::test]
async fn cancelling_before_the_worker_starts_does_not_get_reset() {
    let (agent, _, ran) = setup(vec![Reply::tool("a", "Probe", json!({}))], None, false);
    let mut stream = agent.run_stream("go");
    stream.cancel();
    loop {
        if let AgentEvent::Error(_) = next(&mut stream).await {
            break;
        }
    }
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}
