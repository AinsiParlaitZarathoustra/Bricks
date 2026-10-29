//! Recipe gaps of the Bash session: cursors of a task's output, spawn errors,
//! individual interruption, sequential and concurrent state changes, load and
//! the job quota under concurrent yields. Real processes (Unix). Barriers are
//! observable files or counts; the only clocks are upper bounds.
//!
//! Complements `shell_session.rs` (state, isolation, timeouts, cleanup) and
//! the unit tests of `src/bash.rs` (yield, timeout after yield).

#![cfg(unix)]

use bricks_tools::bash::BashTool;
use bricks_tools::jobs::{JobOwner, JobRegistry, JobSettings, JobTool, JobsHandle};
use bricks_tools::permissions::AllowAll;
use bricks_tools::shell::bash::RunStatus;
use bricks_tools::shell::{self, ShellConfig, procs};
use bricks_tools::tool_report::ToolStatus;
use bricks_tools::{CostTracker, Extensions, Tool, ToolContext};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn config() -> ShellConfig {
    ShellConfig {
        term_grace: Duration::from_millis(500),
        drain: Duration::from_millis(300),
        progress_every: Duration::from_millis(200),
        base_dir: std::env::temp_dir().join("bricks-shell-recipe"),
        ..ShellConfig::default()
    }
}

struct Fixture {
    id: String,
    sessions: Arc<shell::ShellSessions>,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let id = format!("recipe-{}", uuid::Uuid::new_v4());
        Self {
            sessions: shell::session(&id, &config()),
            id,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn cwd(&self) -> &Path {
        self.dir.path()
    }

    fn dir_str(&self) -> String {
        self.cwd().display().to_string()
    }

    async fn spawn(&self, cmd: &str) -> shell::bash::Command {
        self.sessions
            .spawn_command(cmd, None, self.cwd(), &config(), Duration::from_secs(60))
            .await
            .unwrap()
    }

    async fn run(&self, cmd: &str) -> shell::bash::RunOutcome {
        let mut c = self.spawn(cmd).await;
        c.finish().await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        shell::close_session_now(&self.id);
    }
}

fn code(o: &shell::bash::RunOutcome) -> i32 {
    match o.status {
        RunStatus::Exited { code } => code,
        ref s => panic!("not exited: {s:?}"),
    }
}

/// A tool context of a session, with the test shell configuration and,
/// optionally, a job registry and an owner (the quota then applies).
fn tool_ctx(dir: &Path, sid: &str, jobs: Option<&Arc<JobRegistry>>) -> ToolContext {
    let extensions = Extensions::default();
    extensions.insert(config());
    if let Some(reg) = jobs {
        extensions.insert(JobsHandle(Arc::clone(reg)));
        extensions.insert(JobOwner {
            agent_id: "agent-recipe".into(),
            root_run_id: "run-recipe".into(),
            workspace: dir.to_path_buf(),
        });
    }
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: sid.to_string(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions,
    }
}

/// `(line number, text)` of the complete lines of a `Job` output page: the
/// header says the range (`lines a–b of n`), the body holds the lines.
fn page_lines(content: &str) -> Vec<(u64, String)> {
    let header = content.lines().next().unwrap_or_default();
    let Some(range) = regex::Regex::new(r"lines (\d+)–(\d+) of")
        .unwrap()
        .captures(header)
    else {
        return Vec::new();
    };
    let first: u64 = range[1].parse().unwrap();
    let last: u64 = range[2].parse().unwrap();
    let body: Vec<&str> = content.lines().skip(1).collect();
    let shown: Vec<&str> = body
        .into_iter()
        .take_while(|l| !l.starts_with("[line in progress"))
        .collect();
    assert_eq!(shown.len() as u64, last - first + 1, "{content}");
    shown
        .into_iter()
        .enumerate()
        .map(|(i, t)| (first + i as u64, t.to_string()))
        .collect()
}

/// The `next offset` of a `Job` output page.
fn next_offset(content: &str) -> u64 {
    regex::Regex::new(r"next offset (\d+)")
        .unwrap()
        .captures(content)
        .expect(content)[1]
        .parse()
        .unwrap()
}

/// A job's output read two lines at a time, following `next offset`, while
/// the job runs and after it ended: every line once, in order, none lost.
#[tokio::test]
async fn job_output_pages_follow_the_cursor_without_omission() {
    let dir = tempfile::tempdir().unwrap();
    let sid = format!("cursor-{}", uuid::Uuid::new_v4());
    let reg = JobRegistry::new(JobSettings::default(), None);
    let c = tool_ctx(dir.path(), &sid, Some(&reg));
    let r = BashTool::with_jobs(Arc::clone(&reg))
        .execute(
            json!({
                "command": "for i in 1 2 3 4 5 6 7; do echo line$i; sleep 0.15; done",
                "yield_time_ms": 100
            }),
            &c,
        )
        .await;
    assert!(!r.is_error, "{}", r.content);
    let id = r.metadata.as_ref().unwrap()["job_id"]
        .as_str()
        .unwrap()
        .to_string();
    let task = reg.view(&id, None).unwrap().task;
    let job = JobTool::new(Arc::clone(&reg));

    let mut offset = 0u64;
    let mut seen: Vec<(u64, String)> = Vec::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages <= 500, "the cursor never reached the end");
        // Read the state first: a page read after the end is the last one.
        let finished = task.state().is_finished();
        let out = job
            .execute(
                json!({ "action": "output", "job_id": id, "offset": offset, "limit": 2 }),
                &c,
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        let lines = page_lines(&out.content);
        seen.extend(lines.iter().cloned());
        offset = next_offset(&out.content);
        if finished && lines.is_empty() {
            break;
        }
        task.wait_finished(Duration::from_millis(20)).await;
    }
    let numbers: Vec<u64> = seen.iter().map(|(n, _)| *n).collect();
    assert_eq!(numbers, (1..=7).collect::<Vec<u64>>(), "{seen:?}");
    let texts: Vec<&str> = seen.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(
        texts,
        [
            "line1", "line2", "line3", "line4", "line5", "line6", "line7"
        ]
    );
    assert!(
        pages >= 4,
        "two lines a page: at least four pages, got {pages}"
    );
    reg.stop_all().await;
    shell::close_session(&sid).await;
}

/// A shell that cannot be started is an error of the call, not a panic, and
/// nothing is left running.
#[tokio::test]
async fn a_shell_that_cannot_start_is_an_error_of_the_call() {
    let dir = tempfile::tempdir().unwrap();
    let sid = format!("spawn-fail-{}", uuid::Uuid::new_v4());
    let broken = ShellConfig {
        bash_path: Some(PathBuf::from("/nonexistent/bricks-test-bash")),
        ..config()
    };
    let sessions = shell::session(&sid, &broken);
    let spawned = sessions
        .spawn_command(
            "echo never",
            None,
            dir.path(),
            &broken,
            Duration::from_secs(5),
        )
        .await;
    assert!(spawned.is_err(), "a missing shell started a command");

    let reg = JobRegistry::new(JobSettings::default(), None);
    let c = tool_ctx(dir.path(), &sid, Some(&reg));
    c.extensions.insert(broken.clone());
    let r = BashTool::with_jobs(Arc::clone(&reg))
        .execute(json!({ "command": "echo never" }), &c)
        .await;
    assert!(r.is_error, "{}", r.content);
    let rep = r.to_report();
    assert_eq!(rep.status, ToolStatus::Failure, "{}", r.content);
    assert!(
        rep.termination.as_deref().is_some_and(|t| t.starts_with("cannot launch")),
        "{:?}",
        rep.termination
    );
    assert!(!r.content.contains("never\n"), "{}", r.content);
    assert_eq!(reg.running(), 0, "nothing was registered as a job");
    shell::close_session(&sid).await;
}

/// Stopping one command of the session does not stop its neighbour: the
/// quick one ends with its output and its state.
#[tokio::test]
async fn interrupting_one_command_leaves_the_other_running() {
    let f = Fixture::new();
    let mut long = f.spawn("sleep 30").await;
    let mut quick = f.spawn("sleep 0.6; echo quick-done").await;
    let long_pid = long.task().pid;

    long.task().stop(Duration::from_secs(2)).await;
    let o_long = long.finish().await;
    assert_eq!(o_long.status, RunStatus::Cancelled);
    assert!(procs::wait_gone(&[long_pid], Duration::from_secs(3)).await);

    let o_quick = quick.finish().await;
    assert_eq!(code(&o_quick), 0, "{}", o_quick.stderr.text);
    assert_eq!(o_quick.stdout.text, "quick-done\n");
}

/// `cd`, `export` and `unset` in sequence: each call sees what the previous
/// ones left, and an unset variable is gone for the next calls.
#[tokio::test]
async fn cd_export_and_unset_run_in_sequence() {
    let f = Fixture::new();
    std::fs::create_dir(f.cwd().join("sub")).unwrap();
    assert_eq!(code(&f.run("export GONE=1").await), 0);
    assert_eq!(code(&f.run("cd sub && export INSIDE=yes").await), 0);
    assert_eq!(code(&f.run("unset GONE; cd ..").await), 0);
    let o = f.run("echo \"[${GONE-}][${INSIDE-}]\"; pwd -P").await;
    let canon = std::fs::canonicalize(f.cwd()).unwrap();
    assert_eq!(o.stdout.text, format!("[][yes]\n{}\n", canon.display()));
}

/// Two commands that run together each export a different variable: both
/// changes are kept, merged field by field.
#[tokio::test]
async fn distinct_deltas_of_two_concurrent_commands_are_both_kept() {
    let f = Fixture::new();
    let d = f.dir_str();
    let wait_for = |other: &str| {
        format!(
            "n=0; while [ ! -e '{d}/{other}' ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n+1)); done"
        )
    };
    let first = format!("export FIRST=one; touch '{d}/fa'; {}", wait_for("fb"));
    let second = format!("export SECOND=two; touch '{d}/fb'; {}", wait_for("fa"));
    let (a, b) = tokio::join!(f.run(&first), f.run(&second));
    assert_eq!(code(&a), 0, "{}", a.stderr.text);
    assert_eq!(code(&b), 0, "{}", b.stderr.text);
    assert_eq!(
        f.run("echo \"$FIRST $SECOND\"").await.stdout.text,
        "one two\n"
    );
}

/// The same variable exported by two commands: the one launched last wins,
/// whichever of the two ends first.
#[tokio::test]
async fn the_last_launched_export_wins_in_both_end_orders() {
    // `true`: the first launched ends last. `false`: the second ends last.
    for first_ends_last in [true, false] {
        let f = Fixture::new();
        let gate = f.cwd().join("gate");
        let d = f.dir_str();
        let waiting = format!(
            "n=0; while [ ! -e '{}' ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n+1)); done",
            gate.display()
        );
        if first_ends_last {
            let mut first = f.spawn(&format!("export WHO=first; {waiting}")).await;
            let second = f.run("export WHO=second").await;
            assert_eq!(code(&second), 0);
            std::fs::write(&gate, "").unwrap();
            assert_eq!(code(&first.finish().await), 0);
        } else {
            let mut first = f.spawn("export WHO=first").await;
            let mut second = f.spawn(&format!("export WHO=second; {waiting}")).await;
            assert_eq!(code(&first.finish().await), 0);
            std::fs::write(&gate, "").unwrap();
            assert_eq!(code(&second.finish().await), 0);
        }
        let who = f.run("echo \"$WHO\"").await.stdout.text;
        assert_eq!(who, "second\n", "first_ends_last={first_ends_last} in {d}");
    }
}

/// Ten commands of one agent, each waiting for the nine others to have
/// started: all finish. Serialised, the first would wait alone.
#[tokio::test]
async fn ten_concurrent_commands_of_one_agent_run_side_by_side() {
    let f = Fixture::new();
    let d = f.dir_str();
    let cmd = |i: u32| {
        format!(
            "touch '{d}/t{i}'; n=0; while [ $(ls '{d}' | grep -c '^t') -lt 10 ] && [ $n -lt 300 ]; do sleep 0.05; n=$((n+1)); done; echo \"{i} $(ls '{d}' | grep -c '^t')\""
        )
    };
    let mut commands = Vec::new();
    for i in 0..10 {
        commands.push(f.spawn(&cmd(i)).await);
    }
    for (i, c) in commands.iter_mut().enumerate() {
        let o = c.finish().await;
        assert_eq!(o.stdout.text, format!("{i} 10\n"), "{}", o.stderr.text);
    }
}

/// Load: 8, 16 then 32 commands of one session, each started with a yield
/// and all handed over at once under a quota of 32. Every one is a task,
/// none is lost, and the registry counts exactly what was launched.
#[tokio::test]
async fn loads_of_8_16_and_32_commands_are_all_handed_over_under_a_quota_of_32() {
    let dir = tempfile::tempdir().unwrap();
    let sid = format!("load-{}", uuid::Uuid::new_v4());
    let reg = JobRegistry::new(
        JobSettings {
            max_jobs: 32,
            ..Default::default()
        },
        None,
    );
    let c = tool_ctx(dir.path(), &sid, Some(&reg));
    let d = dir.path().display().to_string();
    let started = std::time::Instant::now();
    for n in [8usize, 16, 32] {
        let mut handles = Vec::new();
        for i in 0..n {
            let ctx = c.clone();
            let reg_for_wave = Arc::clone(&reg);
            let cmd = format!("touch '{d}/l{n}_{i}'; sleep 1.5; echo done-{n}-{i}");
            handles.push(tokio::spawn(async move {
                BashTool::with_jobs(Arc::clone(&reg_for_wave))
                    .execute(json!({ "command": cmd, "yield_time_ms": 100 }), &ctx)
                    .await
            }));
        }
        let mut running = 0;
        for h in handles {
            let r: ToolResultLike = h.await.unwrap().into();
            if r.running {
                running += 1;
            } else {
                panic!("not handed over: {}", r.content);
            }
        }
        assert_eq!(running, n);
        assert_eq!(reg.running(), n, "the registry counts the wave's jobs");
        // Let this wave end before the next starts: a finished job gives its
        // place back, so the next wave is not refused by the quota.
        for t in shell::session(&sid, &config()).tasks() {
            assert!(t.wait_finished(Duration::from_secs(20)).await);
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
    shell::close_session(&sid).await;
}

/// Calls beyond the quota of running jobs wait for their command instead of
/// starting a task: never more hand-overs than places.
#[tokio::test]
async fn concurrent_yields_never_hand_over_more_than_the_quota() {
    let dir = tempfile::tempdir().unwrap();
    let sid = format!("quota-{}", uuid::Uuid::new_v4());
    let reg = JobRegistry::new(
        JobSettings {
            max_jobs: 8,
            ..Default::default()
        },
        None,
    );
    let mut handles = Vec::new();
    for _ in 0..40 {
        let ctx = tool_ctx(dir.path(), &sid, Some(&reg));
        let reg_for_call = Arc::clone(&reg);
        handles.push(tokio::spawn(async move {
            BashTool::with_jobs(reg_for_call)
                .execute(json!({ "command": "sleep 2", "yield_time_ms": 50 }), &ctx)
                .await
        }));
    }
    let mut handed = 0;
    let mut waited = 0;
    for h in handles {
        let r: ToolResultLike = h.await.unwrap().into();
        if r.running {
            handed += 1;
        } else {
            waited += 1;
            assert!(r.content.contains("max_jobs"), "{}", r.content);
        }
    }
    assert_eq!(handed, 8, "exactly the places are handed over");
    assert_eq!(waited, 32);
    shell::close_session(&sid).await;
}

/// The part of a tool result these tests read.
struct ToolResultLike {
    running: bool,
    content: String,
}

impl From<bricks_tools::ToolResult> for ToolResultLike {
    fn from(r: bricks_tools::ToolResult) -> Self {
        Self {
            running: r
                .report
                .as_ref()
                .is_some_and(|rep| rep.status == ToolStatus::Running),
            content: r.content,
        }
    }
}
