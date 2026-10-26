//! # Phase 2 Stress Test — All Tools Validation
//!
//! Tests every tool that can run without external APIs or git repos.
//! Validates schemas, execution, and the tool registry.
//!
//! ```bash
//! cargo run --example phase2_stress_test --release
//! ```

use bricks::prelude::*;
use std::sync::Arc;

fn ctx(dir: &std::path::Path) -> ToolContext {
    // The agent's identity: Bash records the jobs it starts under it.
    let extensions = Extensions::default();
    extensions.insert(bricks_tools::jobs::JobOwner {
        agent_id: "phase2-test".into(),
        root_run_id: "run_phase2".into(),
        workspace: dir.to_path_buf(),
    });
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: "phase2-test".into(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions,
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
    println!("║  Phase 2 — All Tools Stress Test                 ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    let tmp = tempfile::tempdir().unwrap();
    let ctx = ctx(tmp.path());

    // ── 1. Tool Registry ─────────────────────────────────────────────────
    println!("  1. Tool Registry");
    println!("  ────────────────");
    {
        let all = bricks::tools::all();
        let fs = bricks::tools::filesystem();
        let sh = bricks::tools::shell();
        let web = bricks::tools::web();
        let plan = bricks::tools::planning();
        check!(
            &format!("all() returns {} tools", all.len()),
            all.len() == 16
        );
        check!(
            &format!("filesystem() = {} tools", fs.len()),
            fs.len() == 10
        );
        check!(&format!("shell() = {} tools", sh.len()), sh.len() == 2);
        check!(&format!("web() = {} tools", web.len()), web.len() == 2);
        check!(
            &format!("planning() = {} tools", plan.len()),
            plan.len() == 1
        );
        check!("none() is empty", bricks::tools::none().is_empty());

        // All tools have valid schemas
        let mut all_valid = true;
        for tool in &all {
            let schema = tool.input_schema();
            if !schema.is_object() {
                println!("    BAD SCHEMA: {} → {:?}", tool.name(), schema);
                all_valid = false;
            }
        }
        check!("All tools have valid JSON schemas", all_valid);

        // All tools have unique names
        let names: Vec<String> = all.iter().map(|t| t.name().to_string()).collect();
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        check!(
            &format!("All {} tool names unique", names.len()),
            names.len() == unique.len()
        );

        // Print tool inventory
        println!("\n    Tool inventory:");
        for tool in &all {
            println!(
                "      {:<18} {:?} / {:?}",
                tool.name(),
                tool.permission_level(),
                tool.category()
            );
        }
        println!();
    }

    // ── 2. Filesystem Tools ──────────────────────────────────────────────
    println!("  2. Filesystem Tools");
    println!("  ───────────────────");
    {
        // Write a file
        let tools = bricks::tools::filesystem();
        let write_tool = tools.iter().find(|t| t.name() == "Write").unwrap();
        let r = write_tool
            .execute(
                serde_json::json!({
                    "file_path": tmp.path().join("test.txt").display().to_string(),
                    "content": "Hello, Phase 2!\nLine 2\nLine 3\n"
                }),
                &ctx,
            )
            .await;
        check!("Write creates file", !r.is_error);

        // Read it back
        let read_tool = tools.iter().find(|t| t.name() == "Read").unwrap();
        let r = read_tool
            .execute(
                serde_json::json!({
                    "file_path": tmp.path().join("test.txt").display().to_string()
                }),
                &ctx,
            )
            .await;
        check!(
            "Read returns content",
            r.content.contains("Hello, Phase 2!")
        );

        // Edit it
        let edit_tool = tools.iter().find(|t| t.name() == "Edit").unwrap();
        let r = edit_tool
            .execute(
                serde_json::json!({
                    "file_path": tmp.path().join("test.txt").display().to_string(),
                    "old_string": "Hello, Phase 2!",
                    "new_string": "Hello, Bricks!"
                }),
                &ctx,
            )
            .await;
        check!("Edit replaces string", !r.is_error);

        // Verify edit
        let r = read_tool
            .execute(
                serde_json::json!({
                    "file_path": tmp.path().join("test.txt").display().to_string()
                }),
                &ctx,
            )
            .await;
        check!("Edit persisted", r.content.contains("Bricks"));

        // Glob
        for i in 0..5 {
            std::fs::write(
                tmp.path().join(format!("mod_{}.rs", i)),
                format!("fn f{}() {{}}", i),
            )
            .unwrap();
        }
        let glob_tool = tools.iter().find(|t| t.name() == "Glob").unwrap();
        let r = glob_tool
            .execute(
                serde_json::json!({
                    "pattern": "**/*.rs",
                    "path": tmp.path().display().to_string()
                }),
                &ctx,
            )
            .await;
        check!("Glob finds .rs files", r.content.contains("mod_0.rs"));

        // Grep
        let grep_tool = tools.iter().find(|t| t.name() == "Grep").unwrap();
        let r = grep_tool
            .execute(
                serde_json::json!({
                    "pattern": "fn f",
                    "path": tmp.path().display().to_string()
                }),
                &ctx,
            )
            .await;
        check!("Grep finds pattern", r.content.contains("fn f"));

        println!();
    }

    // ── 3. Shell Tools ───────────────────────────────────────────────────
    println!("  3. Shell Tools");
    println!("  ──────────────");
    {
        // Bash on a job registry: a call with the default yield (10 s) and
        // timeout (120 s) may hand its command over as a job, so it needs one.
        let tools = bricks_tools::shell_with_jobs(bricks_tools::jobs::JobRegistry::new(
            Default::default(),
            None,
        ));
        let bash = tools.iter().find(|t| t.name() == "Bash").unwrap();
        let r = bash
            .execute(serde_json::json!({"command": "echo 'Phase2 OK'"}), &ctx)
            .await;
        check!("Bash executes", r.content.contains("Phase2 OK"));

        let r = bash
            .execute(serde_json::json!({"command": "false"}), &ctx)
            .await;
        check!("Bash reports errors", r.is_error);
        println!();
    }

    // ── 4. Planning Tools ────────────────────────────────────────────────
    println!("  4. Planning Tools");
    println!("  ─────────────────");
    {
        let todo = bricks_tools::todo_write::TodoWriteTool;
        let r = todo
            .execute(
                serde_json::json!({
                    "todos": [
                        {"content": "Task A", "status": "in_progress", "activeForm": "Doing A"},
                        {"content": "Task B", "status": "pending", "activeForm": "Doing B"},
                    ]
                }),
                &ctx,
            )
            .await;
        check!(
            "TodoWrite creates items",
            r.content.contains("0/2 completed")
        );
        let todos = bricks_tools::todo_write::get_todos("phase2-test");
        check!("Todos persisted in registry", todos.len() == 2);
        println!();
    }

    // ── 5. Orchestration Tools ───────────────────────────────────────────
    println!("  5. Orchestration Tools");
    println!("  ──────────────────────");
    {
        let send = bricks_agent::agents::SendMessageTool;
        let r = send
            .execute(
                serde_json::json!({"to": "worker-1", "content": "Start task"}),
                &ctx,
            )
            .await;
        // Outside an agent runtime there is no recipient: nothing is claimed sent.
        check!(
            "SendMessage refuses without a recipient",
            r.is_error && r.content.contains("nothing was sent")
        );
        println!();
    }

    // ── 6. Web Tools (schema-only, no live API) ──────────────────────────
    println!("  6. Web Tools (schema validation)");
    println!("  ────────────────────────────────");
    {
        let wf = bricks_tools::web_fetch::WebFetchTool;
        check!(
            "WebFetch has url in schema",
            wf.input_schema()["properties"]["url"].is_object()
        );
        check!(
            "WebFetch is ReadOnly",
            wf.permission_level() == PermissionLevel::ReadOnly
        );

        let ws = bricks_tools::web_search::WebSearchTool;
        check!(
            "WebSearch has query in schema",
            ws.input_schema()["properties"]["query"].is_object()
        );
        check!(
            "WebSearch is ReadOnly",
            ws.permission_level() == PermissionLevel::ReadOnly
        );
        println!();
    }

    // ── 7. Benchmark: tool dispatch speed ───────────────────────────────
    println!("  7. Performance (tool dispatch speed)");
    println!("  ─────────────────────────────────────");
    {
        let tools = bricks::tools::filesystem();
        let read = tools.iter().find(|t| t.name() == "Read").unwrap();

        // Create test file
        let test_file = tmp.path().join("perf_test.txt");
        std::fs::write(&test_file, "x\n".repeat(500)).unwrap();

        let iters = 100u32;
        let start = std::time::Instant::now();
        for _ in 0..iters {
            read.execute(
                serde_json::json!({"file_path": test_file.display().to_string()}),
                &ctx,
            )
            .await;
        }
        let avg_us = start.elapsed().as_micros() / iters as u128;
        check!(
            &format!("Read avg {}μs (< 500μs target)", avg_us),
            avg_us < 500
        );

        let edit = tools.iter().find(|t| t.name() == "Edit").unwrap();
        let start = std::time::Instant::now();
        for _ in 0..iters {
            std::fs::write(&test_file, "Hello, world!\nLine 2\n").unwrap();
            edit.execute(
                serde_json::json!({
                    "file_path": test_file.display().to_string(),
                    "old_string": "Hello, world!",
                    "new_string": "Hello, Bricks!"
                }),
                &ctx,
            )
            .await;
        }
        let avg_us = start.elapsed().as_micros() / iters as u128;
        check!(
            &format!("Edit avg {}μs (< 500μs target)", avg_us),
            avg_us < 500
        );

        let glob_tool = tools.iter().find(|t| t.name() == "Glob").unwrap();
        let start = std::time::Instant::now();
        for _ in 0..iters {
            glob_tool
                .execute(
                    serde_json::json!({
                        "pattern": "**/*.rs",
                        "path": tmp.path().display().to_string()
                    }),
                    &ctx,
                )
                .await;
        }
        let avg_us = start.elapsed().as_micros() / iters as u128;
        check!(
            &format!("Glob avg {}μs (< 500μs target)", avg_us),
            avg_us < 500
        );
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
