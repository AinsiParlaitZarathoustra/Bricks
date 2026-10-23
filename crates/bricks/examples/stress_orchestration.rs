//! # Phase 3 Stress Test — Sub-Agent Orchestration
//!
//! Tests AgentTool (sub-agent spawning), coordinator mode and message
//! passing between agents.
//!
//! ```bash
//! cargo run --example phase3_stress_test --release
//! ```

use bricks::prelude::*;
use bricks::provider::CompletionStream;
use std::sync::Arc;
use tokio::sync::mpsc;

// ── Echo provider for testing ────────────────────────────────────────────────

struct EchoProvider;

#[async_trait]
impl Provider for EchoProvider {
    fn name(&self) -> &str {
        "echo"
    }
    fn context_window(&self, _: &str) -> u64 {
        200_000
    }
    async fn complete(
        &self,
        req: bricks::provider::CompletionRequest,
    ) -> bricks_types::Result<CompletionStream> {
        // The task the agent received (its first user message): the engine
        // may add its own follow-up messages (nudges) after it.
        let prompt = req
            .messages
            .iter()
            .find(|m| m.role == bricks_types::Role::User)
            .and_then(|m| m.get_text())
            .unwrap_or("")
            .to_string();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::MessageStart {
                    id: "1".into(),
                    model: "echo".into(),
                    usage: None,
                })
                .await;
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
                    text: format!("[Echo] {}", &prompt[..prompt.floor_char_boundary(100)]),
                })
                .await;
            let _ = tx.send(StreamEvent::ContentBlockStop { index: 0 }).await;
            let _ = tx
                .send(StreamEvent::MessageDelta {
                    stop_reason: Some(StopReason::EndTurn),
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

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { run().await });
}

async fn run() {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($name:expr, $cond:expr) => {
            if $cond {
                passed += 1;
                println!("  \x1b[32m✓\x1b[0m {}", $name);
            } else {
                failed += 1;
                println!("  \x1b[31m✗\x1b[0m {}", $name);
            }
        };
    }

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║  Phase 3 — Sub-Agent Orchestration Stress Test   ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    let ctx = ToolContext {
        working_dir: std::env::temp_dir(),
        session_id: "phase3".into(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    };

    // ── 1. AgentTool ─────────────────────────────────────────────────────
    println!("  1. AgentTool (Sub-Agent Spawning)");
    println!("  ─────────────────────────────────");
    {
        use bricks_agent::agent_tool::AgentTool;

        let agent_tool = AgentTool::new(|| Box::new(EchoProvider), bricks::tools::filesystem());

        // Basic spawn
        let r = agent_tool
            .execute(
                serde_json::json!({
                    "description": "test sub-agent",
                    "prompt": "Hello from parent agent"
                }),
                &ctx,
            )
            .await;
        check!("Sub-agent spawns and returns", !r.is_error);
        check!("Sub-agent echoes prompt", r.content.contains("Echo"));
        check!("Sub-agent has metadata", r.metadata.is_some());

        if let Some(meta) = &r.metadata {
            check!("Metadata has turns", meta["turns"].is_number());
            check!("Metadata has tool_calls", meta["tool_calls"].is_number());
        }

        // With custom system prompt
        let r = agent_tool
            .execute(
                serde_json::json!({
                    "description": "custom system",
                    "prompt": "Do something",
                    "system_prompt": "You are a Rust expert."
                }),
                &ctx,
            )
            .await;
        check!("Custom system prompt works", !r.is_error);

        // Multiple sub-agents (parallel)
        let a1 = agent_tool.execute(
            serde_json::json!({
                "description": "worker 1", "prompt": "Task A"
            }),
            &ctx,
        );
        let a2 = agent_tool.execute(
            serde_json::json!({
                "description": "worker 2", "prompt": "Task B"
            }),
            &ctx,
        );
        let (r1, r2) = tokio::join!(a1, a2);
        check!(
            "Parallel sub-agents both complete",
            !r1.is_error && !r2.is_error
        );
        check!("Worker 1 got Task A", r1.content.contains("Task A"));
        check!("Worker 2 got Task B", r2.content.contains("Task B"));
        println!();
    }

    // ── 2. Coordinator Mode ──────────────────────────────────────────────
    println!("  2. Coordinator Mode");
    println!("  ───────────────────");
    {
        use bricks_agent::coordinator::*;

        // Tool filtering
        let all_tools = bricks::tools::all();
        let all_count = all_tools.len();

        let worker_tools = filter_tools_for_mode(bricks::tools::all(), AgentMode::Worker);
        let coord_tools = filter_tools_for_mode(bricks::tools::all(), AgentMode::Coordinator);

        check!("Workers have fewer tools", worker_tools.len() < all_count);
        check!(
            "Coordinator keeps all tools",
            coord_tools.len() == all_count
        );

        // Workers can't use SendMessage
        let has_send = worker_tools.iter().any(|t| t.name() == "SendMessage");
        check!("Workers can't SendMessage", !has_send);

        // Workers still have core tools
        let has_bash = worker_tools.iter().any(|t| t.name() == "Bash");
        let has_read = worker_tools.iter().any(|t| t.name() == "Read");
        check!("Workers have Bash", has_bash);
        check!("Workers have Read", has_read);

        // Coordinator prompt
        let prompt = coordinator_system_prompt();
        check!(
            "Coordinator prompt mentions orchestrator",
            prompt.contains("orchestrator")
        );

        // Context listing
        let ctx_text = coordinator_context(&all_tools);
        check!(
            "Context lists available tools",
            ctx_text.contains("Read") && ctx_text.contains("Bash")
        );
        println!();
    }

    // ── 3. Message Passing ───────────────────────────────────────────────
    println!("  3. Inter-Agent Message Passing");
    println!("  ──────────────────────────────");
    {
        let send = bricks_tools::send_message::SendMessageTool;

        // Agent A sends to Agent B
        let ctx_a = ToolContext {
            session_id: "agent-alpha".into(),
            ..ctx.clone()
        };
        // Outside an agent runtime there is no recipient: nothing is claimed sent.
        let r = send
            .execute(
                serde_json::json!({"to": "agent-beta", "content": "Found 3 bugs"}),
                &ctx_a,
            )
            .await;
        check!(
            "SendMessage refuses without a recipient",
            r.is_error && r.content.contains("nothing was sent")
        );

        println!();
    }

    // ── 4. Full orchestration scenario ───────────────────────────────────
    println!("  4. Full Orchestration Scenario");
    println!("  ──────────────────────────────");
    {
        use bricks_agent::agent_tool::AgentTool;
        // The coordinator's three pieces of work.
        let tasks: Vec<(String, String)> = ["Lint check", "Test suite", "Build release"]
            .iter()
            .enumerate()
            .map(|(i, d)| (format!("work-{i}"), d.to_string()))
            .collect();

        // Spawn 3 parallel workers
        let _agent_tool = AgentTool::new(|| Box::new(EchoProvider), bricks::tools::coding());
        let mut handles = Vec::new();
        for (id, desc) in &tasks {
            let desc = desc.clone();
            let id = id.clone();
            let at = AgentTool::new(|| Box::new(EchoProvider), bricks::tools::coding());
            let ctx_clone = ctx.clone();
            handles.push(tokio::spawn(async move {
                let r = at
                    .execute(
                        serde_json::json!({
                            "description": desc.clone(),
                            "prompt": format!("Execute: {}", desc)
                        }),
                        &ctx_clone,
                    )
                    .await;
                (id, r)
            }));
        }

        let results: Vec<_> = futures::future::join_all(handles).await;
        let all_ok = results
            .iter()
            .all(|r| r.as_ref().map(|(_, r)| !r.is_error).unwrap_or(false));
        if !all_ok {
            for (_, r) in results
                .iter()
                .filter_map(|r| r.as_ref().ok())
                .filter(|(_, r)| r.is_error)
            {
                eprintln!(
                    "    worker error: {}",
                    &r.content[..r.content.len().min(200)]
                );
            }
        }
        check!("All 3 workers completed", all_ok);

        let completed = results
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .filter(|(_, r)| !r.is_error)
            .count();
        check!(&format!("{}/3 tasks completed", completed), completed >= 3);
        println!();
    }

    // ── Summary ──────────────────────────────────────────────────────────
    println!("╔══════════════════════════════════════════════════╗");
    if failed == 0 {
        println!(
            "║  \x1b[32mALL {} CHECKS PASSED\x1b[0m                              ║",
            passed
        );
    } else {
        println!(
            "║  \x1b[31m{} PASSED, {} FAILED\x1b[0m                               ║",
            passed, failed
        );
    }
    println!("╚══════════════════════════════════════════════════╝\n");
    if failed > 0 {
        std::process::exit(1);
    }
}
