//! The jobs surface, through the tools: `Bash` (background, handed over,
//! refusals without a registry) and `Job` (argument rules, owners, listing,
//! output pages, wait, stop). Real processes (Unix); waits poll observable
//! state, and the only clocks are upper bounds.

#![cfg(unix)]

use bricks_tools::bash::BashTool;
use bricks_tools::jobs::{
    JobOwner, JobRegistry, JobSettings, JobTool, JobsHandle, NO_ACCESSIBLE_JOB,
};
use bricks_tools::permissions::AllowAll;
use bricks_tools::shell::procs;
use bricks_tools::tool_report::ToolStatus;
use bricks_tools::{CostTracker, Extensions, Tool, ToolContext, ToolResult};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn settings() -> JobSettings {
    JobSettings {
        stop_grace_ms: 300,
        ..Default::default()
    }
}

/// A tool context of `agent` on the session registry `reg`, working in `dir`.
fn ctx(dir: &Path, reg: &Arc<JobRegistry>, agent: &str) -> ToolContext {
    let extensions = Extensions::default();
    extensions.insert(JobsHandle(Arc::clone(reg)));
    extensions.insert(JobOwner {
        agent_id: agent.into(),
        root_run_id: format!("run-{agent}"),
        workspace: dir.to_path_buf(),
    });
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: format!("jobs-surface-{}", uuid::Uuid::new_v4()),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions,
    }
}

async fn background(reg: &Arc<JobRegistry>, c: &ToolContext, command: &str) -> ToolResult {
    BashTool::with_jobs(Arc::clone(reg))
        .execute(json!({ "command": command, "background": true }), c)
        .await
}

async fn yielded(reg: &Arc<JobRegistry>, c: &ToolContext, command: &str, yield_ms: u64) -> ToolResult {
    BashTool::with_jobs(Arc::clone(reg))
        .execute(json!({ "command": command, "yield_time_ms": yield_ms }), c)
        .await
}

async fn job(reg: &Arc<JobRegistry>, c: &ToolContext, input: Value) -> ToolResult {
    JobTool::new(Arc::clone(reg)).execute(input, c).await
}

fn id_of(r: &ToolResult) -> String {
    r.metadata
        .as_ref()
        .and_then(|m| m["job_id"].as_str())
        .unwrap_or_else(|| panic!("no job_id: {}", r.content))
        .to_string()
}

/// The lines of a page after its header.
fn body(content: &str) -> Vec<&str> {
    content.lines().skip(1).collect()
}

/// Polls `Job` with `input` until `ok` holds on its text, up to 5 s; returns
/// the last reading.
async fn poll(reg: &Arc<JobRegistry>, c: &ToolContext, input: Value, ok: impl Fn(&str) -> bool) -> String {
    let started = Instant::now();
    loop {
        let text = job(reg, c, input.clone()).await.content;
        if ok(&text) || started.elapsed() > Duration::from_secs(5) {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn names(tools: &[Box<dyn Tool>]) -> Vec<String> {
    tools.iter().map(|t| t.name().to_string()).collect()
}

// ─── Argument rules: refused, never corrected ────────────────────────────────

#[tokio::test]
async fn job_arguments_out_of_range_or_unknown_are_refused_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(settings(), None);
    let c = ctx(dir.path(), &reg, "agent_a");

    let empty = job(&reg, &c, json!({})).await;
    assert!(!empty.is_error, "{}", empty.content);
    assert!(empty.content.contains("You have no jobs"), "{}", empty.content);

    let cases: Vec<(Value, &str)> = vec![
        (json!({ "action": "explode" }), "explode"),
        (json!({ "action": "output", "job_id": "job_x", "stream": "stdin" }), "stdin"),
        (json!({ "bogus": 1 }), "bogus"),
        (json!({ "action": "output", "job_id": "job_x", "limit": 0 }), "must be between 1 and 2000 (got 0)"),
        (json!({ "action": "output", "job_id": "job_x", "limit": 2001 }), "must be between 1 and 2000 (got 2001)"),
        (json!({ "action": "wait", "job_id": "job_x", "timeout_ms": 0 }), "must be between 1 and 600000 (got 0)"),
        (json!({ "action": "wait", "job_id": "job_x", "timeout_ms": 600001 }), "must be between 1 and 600000 (got 600001)"),
        (json!({ "action": "output", "job_id": "job_x", "offset": -1 }), "-1"),
        (json!({ "action": "output", "job_id": "job_x", "tail": true, "offset": 3 }), "not both"),
        (json!({ "action": "status" }), "needs a non-empty `job_id`"),
        (json!({ "action": "output", "job_id": "" }), "needs a non-empty `job_id`"),
        (json!({ "action": "wait", "job_id": "   " }), "needs a non-empty `job_id`"),
        (json!({ "action": "stop" }), "needs a non-empty `job_id`"),
    ];
    for (input, needle) in cases {
        let r = job(&reg, &c, input.clone()).await;
        assert!(r.is_error, "{input}: accepted: {}", r.content);
        assert!(r.content.contains(needle), "{input}: {}", r.content);
    }
    // The range edges are accepted: the error, if any, is about the job id.
    for input in [
        json!({ "action": "output", "job_id": "job_x", "limit": 1 }),
        json!({ "action": "output", "job_id": "job_x", "limit": 2000 }),
        json!({ "action": "wait", "job_id": "job_x", "timeout_ms": 1 }),
        json!({ "action": "wait", "job_id": "job_x", "timeout_ms": 600000 }),
    ] {
        let r = job(&reg, &c, input.clone()).await;
        assert_eq!(r.content, NO_ACCESSIBLE_JOB, "{input}");
    }
    assert!(reg.views(None).is_empty(), "a refused call started nothing");
}

// ─── Owners ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn background_and_handed_over_jobs_are_followed_by_their_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(settings(), None);
    let a = ctx(dir.path(), &reg, "agent_a");
    let b = ctx(dir.path(), &reg, "agent_b");

    let bg = id_of(&background(&reg, &a, "sleep 30").await);
    let y = yielded(&reg, &a, "sleep 30", 100).await;
    assert_eq!(y.report.as_ref().unwrap().status, ToolStatus::Running, "{}", y.content);
    let handed = id_of(&y);

    let list_a = job(&reg, &a, json!({})).await.content;
    assert!(list_a.contains(&bg) && list_a.contains(&handed), "{list_a}");
    assert!(job(&reg, &b, json!({})).await.content.contains("You have no jobs"));

    for id in [bg.as_str(), handed.as_str(), "job_000000000000"] {
        for action in ["status", "output", "wait", "stop"] {
            let r = job(&reg, &b, json!({ "action": action, "job_id": id, "timeout_ms": 50 })).await;
            assert!(r.is_error, "{action} {id}: {}", r.content);
            assert_eq!(r.content, NO_ACCESSIBLE_JOB, "{action} {id}");
        }
    }
    // Nothing the other agent tried reached the jobs.
    let status = job(&reg, &a, json!({ "action": "status", "job_id": bg })).await.content;
    assert!(status.contains("[running]"), "{status}");
    reg.stop_all().await;
}

// ─── Listing and status ──────────────────────────────────────────────────────

#[tokio::test]
async fn status_shows_live_raw_logs_then_durable_copies_never_before_they_exist() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let durable = dir.path().join("durable");
    let reg = JobRegistry::new(settings(), Some(durable.clone()));
    let a = ctx(&work, &reg, "agent_a");

    let id = id_of(&background(&reg, &a, "echo one; echo oops >&2; sleep 30").await);
    let status_input = json!({ "action": "status", "job_id": id });
    let status_now = poll(&reg, &a, status_input.clone(), |t| {
        t.contains("stdout 1 lines") && t.contains("stderr 1 lines")
    })
    .await;
    let first = status_now.lines().next().unwrap();
    assert!(first.starts_with(&format!("{id} [running]")), "{status_now}");
    assert!(first.contains("stdout 1 lines / 4 B"), "{first}");
    assert!(first.contains("stderr 1 lines / 5 B"), "{first}");
    assert!(first.contains(&format!("cwd {}", work.display())), "{first}");
    assert!(first.ends_with("echo one; echo oops >&2; sleep 30"), "{first}");

    let logs = status_now.lines().nth(1).unwrap();
    assert!(logs.starts_with("raw logs: "), "{logs}");
    assert!(!logs.contains("kept"), "{logs}");
    for path in logs.trim_start_matches("raw logs: ").split(", ") {
        assert!(Path::new(path).exists(), "live log {path} missing");
    }
    assert!(!durable.join(&id).exists(), "no durable copy before the end");

    let list = job(&reg, &a, json!({})).await.content;
    assert!(list.contains(&format!("{id} [running]")) && list.contains("stdout 1 lines / 4 B"), "{list}");

    job(&reg, &a, json!({ "action": "stop", "job_id": id })).await;
    let ended = poll(&reg, &a, status_input, |t| t.contains("raw logs (kept): ")).await;
    let kept = ended.lines().nth(1).unwrap();
    assert!(kept.starts_with("raw logs (kept): "), "{ended}");
    let copies: Vec<&str> = kept.trim_start_matches("raw logs (kept): ").split(", ").collect();
    assert_eq!(copies.len(), 2, "{kept}");
    for path in &copies {
        assert!(Path::new(path).starts_with(durable.join(&id)), "{path}");
        assert!(Path::new(path).exists(), "{path}");
    }
    assert!(std::fs::read_to_string(copies[0]).unwrap().contains("one"));
    assert!(std::fs::read_to_string(copies[1]).unwrap().contains("oops"));
    let list = job(&reg, &a, json!({})).await.content;
    assert!(list.contains(&format!("{id} [stopped]")), "{list}");
}

// ─── Pages ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn offset_pages_advance_and_an_empty_page_keeps_its_offset() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(settings(), None);
    let a = ctx(dir.path(), &reg, "agent_a");
    let id = id_of(&background(&reg, &a, "echo l1; echo l2; echo l3; echo l4; echo l5").await);
    assert!(job(&reg, &a, json!({ "action": "wait", "job_id": id, "timeout_ms": 10000 })).await.content.contains("[completed"));

    let page = |offset: u64, limit: u64| {
        let reg = Arc::clone(&reg);
        let a = a.clone();
        let id = id.clone();
        async move {
            job(&reg, &a, json!({ "action": "output", "job_id": id, "offset": offset, "limit": limit })).await
        }
    };
    let p = page(0, 2).await;
    assert!(p.content.starts_with("stdout: lines 1–2 of 5 complete"), "{}", p.content);
    assert!(p.content.contains("; next offset 2"), "{}", p.content);
    assert_eq!(body(&p.content), ["l1", "l2"]);

    let p = page(2, 2).await;
    assert!(p.content.contains("lines 3–4 of 5"), "{}", p.content);
    assert!(p.content.contains("; next offset 4"), "{}", p.content);
    assert_eq!(body(&p.content), ["l3", "l4"]);

    let p = page(4, 2).await;
    assert!(p.content.contains("lines 5–5 of 5"), "{}", p.content);
    assert_eq!(body(&p.content), ["l5"]);
    assert!(p.content.contains("; next offset 5"), "{}", p.content);

    // At the end: an empty page, not an error, and the offset stays.
    let p = page(5, 2).await;
    assert!(!p.is_error, "{}", p.content);
    assert!(p.content.contains("no complete line from offset 5 (5 complete"), "{}", p.content);
    assert!(p.content.contains("; next offset 5"), "{}", p.content);
    assert!(body(&p.content).is_empty());

    let p = page(0, 200).await;
    assert_eq!(body(&p.content), ["l1", "l2", "l3", "l4", "l5"]);

    // Tail: the last `limit` lines, computed at the read.
    let t = job(&reg, &a, json!({ "action": "output", "job_id": id, "tail": true, "limit": 2 })).await;
    assert!(t.content.contains("lines 4–5 of 5"), "{}", t.content);
    assert_eq!(body(&t.content), ["l4", "l5"]);
    assert!(t.content.contains("; next offset 5"), "{}", t.content);
    let t = job(&reg, &a, json!({ "action": "output", "job_id": id, "tail": true })).await;
    assert_eq!(body(&t.content), ["l1", "l2", "l3", "l4", "l5"]);

    // The other stream is its own: nothing on stderr.
    let err = job(&reg, &a, json!({ "action": "output", "job_id": id, "stream": "stderr" })).await;
    assert!(err.content.starts_with("stderr: no complete line from offset 0 (0 complete, 0 bytes received)"), "{}", err.content);
    reg.stop_all().await;
}

#[tokio::test]
async fn evicted_lines_are_reported_and_the_raw_log_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(
        JobSettings {
            output_buffer_bytes: 64,
            ..settings()
        },
        None,
    );
    let a = ctx(dir.path(), &reg, "agent_a");
    // Read while the job runs (its raw log is there), after its 20 lines.
    let seq = "for i in $(seq 1 20); do printf 'line-%02d\\n' $i; done; sleep 30";
    let id = id_of(&background(&reg, &a, seq).await);
    poll(&reg, &a, json!({ "action": "status", "job_id": id }), |t| t.contains("stdout 20 lines")).await;

    let p = job(&reg, &a, json!({ "action": "output", "job_id": id, "offset": 0, "limit": 200 })).await;
    let evicted: u64 = regex::Regex::new(r"(\d+) earlier line\(s\) no longer in memory")
        .unwrap()
        .captures(&p.content)
        .unwrap_or_else(|| panic!("no eviction notice: {}", p.content))[1]
        .parse()
        .unwrap();
    assert!((1..20).contains(&evicted), "{}", p.content);
    assert!(p.content.contains(&format!("(the {evicted} before this page are among them)")), "{}", p.content);
    let first = evicted + 1;
    assert!(p.content.contains(&format!("lines {first}–20 of 20")), "{}", p.content);
    assert_eq!(body(&p.content).first().copied(), Some(format!("line-{first:02}").as_str()));

    // The raw log holds every line.
    let raw = regex::Regex::new(r"raw log: ([^;\s]+)")
        .unwrap()
        .captures(&p.content)
        .unwrap()[1]
        .to_string();
    let text = std::fs::read_to_string(&raw).unwrap();
    assert!(text.contains("line-01") && text.contains("line-20"), "{raw}");
    reg.stop_all().await;
}

#[tokio::test]
async fn a_first_line_longer_than_the_page_is_cut_on_a_character_and_the_cursor_moves_past_it() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(
        JobSettings {
            max_page_bytes: 131,
            ..settings()
        },
        None,
    );
    let a = ctx(dir.path(), &reg, "agent_a");
    // 60 pairs of a 2-byte and a 4-byte character: 360 bytes on one line.
    let long = "printf 'é😀%.0s' $(seq 1 60); echo; echo second";
    let id = id_of(&background(&reg, &a, long).await);
    job(&reg, &a, json!({ "action": "wait", "job_id": id, "timeout_ms": 10000 })).await;

    let p = job(&reg, &a, json!({ "action": "output", "job_id": id, "offset": 0, "limit": 200 })).await;
    assert!(p.content.contains("the line shown was truncated to fit the page"), "{}", p.content);
    assert!(p.content.contains("; next offset 1"), "{}", p.content);
    let shown = body(&p.content);
    assert_eq!(shown.len(), 1, "{}", p.content);
    let cut = shown[0]
        .strip_suffix(" [truncated to fit the page; the whole line is in the raw log]")
        .unwrap_or_else(|| panic!("marker missing: {}", shown[0]));
    let whole = "é😀".repeat(60);
    assert!(whole.starts_with(cut), "the shown part is a prefix of the line");
    assert!(!cut.is_empty() && cut.len() <= 131, "{} bytes", cut.len());

    // The next page starts past the long line.
    let next = job(&reg, &a, json!({ "action": "output", "job_id": id, "offset": 1, "limit": 200 })).await;
    assert_eq!(body(&next.content), ["second"], "{}", next.content);
    assert!(next.content.contains("; next offset 2"), "{}", next.content);
    reg.stop_all().await;
}

#[tokio::test]
async fn a_partial_line_is_shown_apart_and_does_not_move_the_offset() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(settings(), None);
    let a = ctx(dir.path(), &reg, "agent_a");
    let id = id_of(&background(&reg, &a, "printf 'full\\npart'; sleep 30").await);
    let page = poll(
        &reg,
        &a,
        json!({ "action": "output", "job_id": id, "offset": 0 }),
        |t| t.contains("[line in progress"),
    )
    .await;
    assert!(page.contains("lines 1–1 of 1 complete"), "{page}");
    assert!(page.contains("; next offset 1"), "{page}");
    assert_eq!(body(&page)[0], "full", "{page}");
    assert!(body(&page).contains(&"part"), "{page}");

    // The partial line is not a line: reading at the offset gives it apart again.
    let again = job(&reg, &a, json!({ "action": "output", "job_id": id, "offset": 1 })).await;
    assert!(again.content.contains("no complete line from offset 1"), "{}", again.content);
    assert!(again.content.contains("; next offset 1"), "{}", again.content);
    assert!(again.content.contains("[line in progress"), "{}", again.content);
    job(&reg, &a, json!({ "action": "stop", "job_id": id })).await;
    reg.stop_all().await;
}

// ─── Wait and stop ───────────────────────────────────────────────────────────

#[tokio::test]
async fn a_short_wait_leaves_the_job_running_and_stop_is_idempotent_for_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(settings(), None);
    let a = ctx(dir.path(), &reg, "agent_a");
    let id = id_of(&background(&reg, &a, "sleep 300 & echo $! > child.pid; wait").await);
    let child_file = dir.path().join("child.pid");
    let child: u32 = {
        let started = Instant::now();
        loop {
            if let Ok(t) = std::fs::read_to_string(&child_file)
                && let Ok(p) = t.trim().parse::<u32>()
            {
                break p;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "the child started");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };

    let w = job(&reg, &a, json!({ "action": "wait", "job_id": id, "timeout_ms": 200 })).await;
    assert!(!w.is_error, "{}", w.content);
    assert!(w.content.contains("Still running after 200 ms"), "{}", w.content);
    assert!(w.content.contains("the job was not stopped"), "{}", w.content);
    assert!(w.content.contains("[running]"), "{}", w.content);
    assert!(procs::is_running(child), "a timed-out wait stops nothing");

    let s1 = job(&reg, &a, json!({ "action": "stop", "job_id": id })).await;
    assert!(!s1.is_error && s1.content.contains("[stopped]"), "{}", s1.content);
    assert!(procs::wait_gone(&[child], Duration::from_secs(5)).await, "the child of the group ended");
    // Idempotent: a second stop is still a success, with the same state.
    let s2 = job(&reg, &a, json!({ "action": "stop", "job_id": id })).await;
    assert!(!s2.is_error && s2.content.contains("[stopped]"), "{}", s2.content);
    reg.stop_all().await;
}

// ─── Bash without a registry, or without an identity ────────────────────────

fn plain_ctx(dir: &Path) -> ToolContext {
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: format!("jobs-plain-{}", uuid::Uuid::new_v4()),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

#[tokio::test]
async fn bash_from_shell_refuses_what_may_become_a_job_before_anything_starts() {
    let dir = tempfile::tempdir().unwrap();
    let tools = bricks_tools::shell();
    let bash = tools.iter().find(|t| t.name() == "Bash").expect("Bash");
    assert!(
        !names(&tools).contains(&"Job".to_string()),
        "Job is not in shell(): {:?}",
        names(&tools)
    );
    let c = plain_ctx(dir.path());
    let marker = dir.path().join("ran");

    let bg = bash
        .execute(
            json!({ "command": format!("touch {}; sleep 1", marker.display()), "background": true }),
            &c,
        )
        .await;
    assert!(bg.is_error, "{}", bg.content);
    assert!(bg.content.contains("no job registry"), "{}", bg.content);

    // The default yield (10 s) is below the default timeout (120 s): the
    // command may become a job, so it is refused.
    let default = bash
        .execute(json!({ "command": format!("touch {}", marker.display()) }), &c)
        .await;
    assert!(default.is_error, "{}", default.content);
    assert!(default.content.contains("no job registry"), "{}", default.content);

    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!marker.exists(), "refused before anything started");

    // A yield at or past the timeout: the command cannot be handed over, so
    // it runs without a registry, and its timeout ends it.
    let quick = bash
        .execute(json!({ "command": "echo quick", "yield_time_ms": 5000, "timeout": 5000 }), &c)
        .await;
    assert!(!quick.is_error, "{}", quick.content);
    assert!(quick.content.contains("quick"), "{}", quick.content);
    let slow = bash
        .execute(json!({ "command": "sleep 30", "yield_time_ms": 300, "timeout": 300 }), &c)
        .await;
    assert_eq!(slow.report.as_ref().unwrap().status, ToolStatus::TimedOut, "{}", slow.content);
    bricks_tools::shell::close_session(&c.session_id).await;
}

#[tokio::test]
async fn a_registry_without_an_owner_refuses_before_anything_starts() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(settings(), None);
    let c = plain_ctx(dir.path());
    let marker = dir.path().join("ran");
    let bash = BashTool::with_jobs(Arc::clone(&reg));
    let r = bash
        .execute(
            json!({ "command": format!("touch {}; sleep 1", marker.display()), "background": true }),
            &c,
        )
        .await;
    assert!(r.is_error, "{}", r.content);
    assert!(r.content.contains("no identity"), "{}", r.content);
    let r = bash
        .execute(json!({ "command": format!("touch {}", marker.display()) }), &c)
        .await;
    assert!(r.is_error, "{}", r.content);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists(), "refused before anything started");
    assert!(reg.views(None).is_empty());
}

#[tokio::test]
async fn background_with_a_yield_is_refused_as_a_contradiction() {
    let dir = tempfile::tempdir().unwrap();
    let reg = JobRegistry::new(settings(), None);
    let c = ctx(dir.path(), &reg, "agent_a");
    let r = BashTool::with_jobs(Arc::clone(&reg))
        .execute(json!({ "command": "true", "background": true, "yield_time_ms": 100 }), &c)
        .await;
    assert!(r.is_error, "{}", r.content);
    assert!(r.content.contains("only applies to a foreground command"), "{}", r.content);
    assert!(reg.views(None).is_empty());
}

// ─── Surface ─────────────────────────────────────────────────────────────────

#[test]
fn the_three_old_task_tools_are_gone_from_every_set() {
    let old = ["BashTaskStatus", "BashTaskOutput", "BashTaskStop"];
    for (set_name, set) in [
        ("all", bricks_tools::all()),
        ("coding", bricks_tools::coding()),
    ] {
        let set = names(&set);
        for gone in old {
            assert!(!set.contains(&gone.to_string()), "{gone} in {set_name}: {set:?}");
        }
        assert!(set.contains(&"Job".to_string()), "{set_name}: {set:?}");
        assert!(set.contains(&"Bash".to_string()), "{set_name}: {set:?}");
    }
}
