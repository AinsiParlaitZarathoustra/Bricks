//! # Stress test — Agent Skills
//!
//! The `bricks-skills` service and the `Skill` tool, on temporary folders
//! only (an injected home: no personal skill of the machine is read).
//! Built-in documents, the five sources and their priorities, collisions,
//! trust, incompatible documents, activation, resources, the tool contract,
//! an agent's single `Skill` registration, and discovery at scale.
//!
//! ```bash
//! cargo run --release -p cersei --example stress_skills
//! ```

use cersei::prelude::*;
use cersei::skills::{ProjectDecision, ResolveError, SkillAccess, SkillsService, standard_roots};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

fn write(root: &Path, dir: &str, front: &str, body: &str) {
    let d = root.join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("SKILL.md"), format!("---\n{front}---\n{body}")).unwrap();
}

struct Nothing;

#[async_trait]
impl Provider for Nothing {
    fn name(&self) -> &str {
        "none"
    }
    fn context_window(&self, _: &str) -> u64 {
        100_000
    }
    async fn complete(&self, _: CompletionRequest) -> Result<cersei::provider::CompletionStream> {
        Err(CerseiError::Provider("not used".into()))
    }
}

fn main() {
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
    println!("║  Agent Skills — Stress Test                      ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    // ── 1. Built-in skills ───────────────────────────────────────────────
    println!("  1. Built-in skills");
    println!("  ──────────────────");
    {
        let svc = SkillsService::standard(&project, Some(&home));
        let access = SkillAccess::new(Arc::new(svc));
        let names: Vec<String> = access
            .catalog()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        for n in ["commit", "debug", "remember", "simplify", "stuck", "verify"] {
            check!(
                format!("built-in `{n}` is listed"),
                names.iter().any(|x| x == n)
            );
        }
        for n in &names {
            let a = access.activate(n).unwrap();
            check!(
                format!("`{n}` parses with the common parser, no template"),
                !a.body.contains("$ARGUMENTS") && a.dir.is_none()
            );
        }
        check!(
            "historical aliases are gone (`mem`, `diagnose`)",
            access.activate("mem").is_err() && access.activate("diagnose").is_err()
        );
        check!(
            "no `loop` built-in (no scheduler in Bricks)",
            access.activate("loop").is_err()
        );
    }
    println!();

    // ── 2. Sources and priorities ────────────────────────────────────────
    println!("  2. Sources, priorities, collisions");
    println!("  ──────────────────────────────────");
    {
        let ok = |n: &str, d: &str| format!("name: {n}\ndescription: {d}\n");
        write(
            &project.join(".agents/skills"),
            "shared",
            &ok("shared", "p-agents"),
            "A",
        );
        write(
            &project.join(".claude/skills"),
            "shared",
            &ok("shared", "p-claude"),
            "B",
        );
        write(
            &home.join(".agents/skills"),
            "shared",
            &ok("shared", "h-agents"),
            "C",
        );
        write(
            &home.join(".claude/skills"),
            "shared",
            &ok("shared", "h-claude"),
            "D",
        );
        write(
            &home.join(".claude/skills"),
            "verify",
            &ok("verify", "personal verify"),
            "E",
        );
        write(
            &project.join(".cersei/skills"),
            "legacy",
            &ok("legacy", "old"),
            "F",
        );
        std::fs::create_dir_all(project.join(".claude/commands")).unwrap();
        std::fs::write(project.join(".claude/commands/cmd.md"), "Do $ARGUMENTS").unwrap();
        let svc = Arc::new(SkillsService::standard(&project, Some(&home)));
        let access = SkillAccess::new(Arc::clone(&svc));
        let desc = |a: &SkillAccess, n: &str| a.activate(n).map(|x| x.description).ok();
        check!(
            "pending project: personal wins",
            desc(&access, "shared").as_deref() == Some("h-agents")
        );
        check!(
            "a personal skill shadows a built-in one",
            desc(&access, "verify").as_deref() == Some("personal verify")
        );
        access.decide_project(ProjectDecision::Granted);
        check!(
            "approved project: .agents first",
            desc(&access, "shared").as_deref() == Some("p-agents")
        );
        let reg = svc.registry();
        check!(
            "every homonym stays visible for diagnostics (4)",
            reg.candidates.iter().filter(|c| c.key == "shared").count() == 4
        );
        check!(
            ".cersei/skills and .claude/commands are not read",
            access.activate("legacy").is_err() && access.activate("cmd").is_err()
        );
        let child = access.clone();
        access.decide_project(ProjectDecision::Denied);
        check!(
            "a sub-agent's access follows the session's decision",
            desc(&child, "shared").as_deref() == Some("h-agents")
        );
    }
    println!();

    // ── 3. Validation ────────────────────────────────────────────────────
    println!("  3. Validation and incompatible documents");
    println!("  ────────────────────────────────────────");
    {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("h/.agents/skills");
        write(
            &root,
            "données",
            "name: données\ndescription: unicode\n",
            "x",
        );
        write(
            &root,
            "fork",
            "name: fork\ndescription: d\ncontext: fork\n",
            "x",
        );
        write(&root, "wrong-dir", "name: other\ndescription: d\n", "x");
        std::fs::create_dir_all(root.join("crlf")).unwrap();
        std::fs::write(
            root.join("crlf/SKILL.md"),
            "---\r\nname: crlf\r\ndescription: >-\r\n  two\r\n  lines\r\n---\r\nbody\r\n",
        )
        .unwrap();
        let svc = SkillsService::new(
            standard_roots(&t.path().join("p"), Some(&t.path().join("h"))),
            &[],
            None,
        );
        let a = SkillAccess::new(Arc::new(svc));
        check!("Unicode name accepted", a.activate("données").is_ok());
        check!(
            "CRLF and folded description",
            a.activate("crlf").map(|x| x.description).ok().as_deref() == Some("two lines")
        );
        check!(
            "`context: fork` → incompatible, not activable",
            matches!(
                a.activate("fork"),
                Err(cersei::skills::ActivationError::Resolve(
                    ResolveError::Incompatible { .. }
                ))
            )
        );
        check!(
            "name must match its directory",
            a.activate("other").is_err() && a.activate("wrong-dir").is_err()
        );
    }
    println!();

    // ── 4. The Skill tool ────────────────────────────────────────────────
    println!("  4. Skill tool");
    println!("  ─────────────");
    {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("h/.agents/skills");
        write(
            &root,
            "list",
            "name: list\ndescription: named list\n",
            "LIST-BODY",
        );
        write(
            &root,
            "pdf",
            "name: pdf\ndescription: PDFs\nallowed-tools: Bash Read\n",
            "Use scripts/x.sh",
        );
        std::fs::create_dir_all(root.join("pdf/scripts")).unwrap();
        std::fs::write(root.join("pdf/scripts/x.sh"), "SCRIPT-CONTENT").unwrap();
        let svc = SkillsService::new(
            standard_roots(&t.path().join("p"), Some(&t.path().join("h"))),
            &[],
            None,
        );
        let session = cersei::tools::skill_tool::SkillSession::new(SkillAccess::new(Arc::new(svc)));
        let ext = Extensions::default();
        ext.insert(session.clone());
        let ctx = ToolContext {
            working_dir: t.path().to_path_buf(),
            session_id: "stress-skills".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: ext,
        };
        let tool = cersei::tools::skill_tool::SkillTool::new();
        check!(
            "read-only permission level",
            tool.permission_level() == PermissionLevel::ReadOnly
        );
        let r = rt.block_on(tool.execute(serde_json::json!({"skill": "list"}), &ctx));
        check!(
            "`skill: list` without action lists",
            r.content.contains("can be activated")
        );
        let r = rt.block_on(tool.execute(
            serde_json::json!({"action": "activate", "skill": "list"}),
            &ctx,
        ));
        check!(
            "a skill named `list` is activable",
            r.content.contains("LIST-BODY")
        );
        let r = rt.block_on(tool.execute(
            serde_json::json!({"skill": "pdf", "args": "report.pdf"}),
            &ctx,
        ));
        check!(
            "resources listed, not read",
            r.content.contains("scripts/x.sh") && !r.content.contains("SCRIPT-CONTENT")
        );
        check!(
            "allowed-tools reported as not applied",
            r.content.contains("does not apply it")
        );
        check!(
            "the request is separate",
            r.content
                .contains("Request to apply it to (from the call): report.pdf")
        );
        check!(
            "only proposed by the tool (the engine commits)",
            session.ledger.lock().active().count() == 0
        );
    }
    println!();

    // ── 5. Agent registration ────────────────────────────────────────────
    println!("  5. Agent");
    println!("  ────────");
    {
        let svc = SkillsService::standard(&project, Some(&home));
        let access = SkillAccess::new(Arc::new(svc));
        let agent = Agent::builder()
            .provider(Nothing)
            .tools(cersei::tools::coding())
            .skills(access.clone())
            .build()
            .unwrap();
        check!(
            "`Skill` registered once",
            agent
                .tool_list()
                .iter()
                .filter(|t| t.name() == "Skill")
                .count()
                == 1
        );
        let agent = Agent::builder()
            .provider(Nothing)
            .tool(cersei::tools::skill_tool::SkillTool::new())
            .skills(access)
            .build()
            .unwrap();
        check!(
            "an existing `Skill` tool is not duplicated",
            agent
                .tool_list()
                .iter()
                .filter(|t| t.name() == "Skill")
                .count()
                == 1
        );
        let plain = Agent::builder().provider(Nothing).build().unwrap();
        check!(
            "no service: no `Skill` tool",
            plain.tool_by_name("Skill").is_none()
        );
    }
    println!();

    // ── 6. Discovery at scale ────────────────────────────────────────────
    println!("  6. Discovery at scale");
    println!("  ─────────────────────");
    {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("h/.agents/skills");
        for i in 0..300 {
            let n = format!("skill-{i:03}");
            write(
                &root,
                &n,
                &format!("name: {n}\ndescription: number {i}\n"),
                &"x".repeat(2_000),
            );
        }
        let start = Instant::now();
        let svc = SkillsService::new(
            standard_roots(&t.path().join("p"), Some(&t.path().join("h"))),
            &[],
            None,
        );
        let elapsed = start.elapsed();
        let a = SkillAccess::new(Arc::new(svc));
        let cat = a.catalog();
        println!("    300 skills discovered in {} ms", elapsed.as_millis());
        check!("catalog bounded (100 entries)", cat.entries.len() == 100);
        check!("partial catalog says so", cat.omitted == 200);
        check!(
            "catalog carries no instructions",
            !cat.render().unwrap_or_default().contains(&"x".repeat(100))
        );
        let start = Instant::now();
        a.service.reload();
        println!("    reload in {} ms", start.elapsed().as_millis());
        check!("discovery under 2 s", elapsed.as_millis() < 2_000);
    }
    println!();

    let total = passed + failed;
    if failed == 0 {
        println!("╔══════════════════════════════════════════════════╗");
        println!("║  \x1b[32mALL {total} CHECKS PASSED\x1b[0m                              ║");
        println!("╚══════════════════════════════════════════════════╝");
    } else {
        println!("  \x1b[31m{failed} of {total} checks FAILED\x1b[0m");
        std::process::exit(1);
    }
}
