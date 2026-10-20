//! The lifetime of a root run's commands, through the controller and real
//! processes. A foreground command handed over as a task (past
//! `yield_time_ms`) ends with its run; an explicit `background: true` job
//! outlives the run and ends with the session. The project's `[bash]
//! yield_time_ms` reaches the Bash tool.

#![cfg(unix)]

use bricks_agent::BricksConfig;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::*;
use bricks_tools::shell::procs;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

async fn open(dir: &Path, replies: Vec<Reply>, yield_time_ms: u64) -> (Controller, EventStream) {
    let catalog = ScriptedCatalog::new(&["a"], Script::new(replies));
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.bash.yield_time_ms = yield_time_ms;
    bricks.permissions = ApprovalRules {
        execute: Action::Allow,
        ..Default::default()
    };
    let mut cfg = EngineConfig::new(dir, catalog, bricks, dir.join(".sessions"));
    cfg.interactive = false;
    Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap()
}

/// The next event matching `pred`.
async fn until(events: &mut EventStream, pred: impl Fn(&Event) -> bool) -> Event {
    loop {
        let e = tokio::time::timeout(Duration::from_secs(30), events.next())
            .await
            .expect("the event within 30 s")
            .expect("the stream open");
        if pred(&e.event) {
            return e.event;
        }
    }
}

fn finished(call: &'static str) -> impl Fn(&Event) -> bool {
    move |e| matches!(e, Event::ToolFinished { tool_call_id, .. } if tool_call_id == call)
}

/// The pid a command wrote to `path` when it started.
async fn pid_in(path: &Path) -> u32 {
    for _ in 0..400 {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse()
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no pid written to {}", path.display());
}

/// A shell command that records its pid and sleeps.
fn sleeper(pidfile: &Path) -> String {
    format!(
        "sh -c 'echo $$ > \"{}\"; exec sleep 300'",
        pidfile.display()
    )
}

#[tokio::test]
async fn a_handed_over_command_ends_with_its_run_and_a_background_job_with_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let ordinary_pid = dir.path().join("ordinary.pid");
    let background_pid = dir.path().join("background.pid");
    let (ctl, mut events) = open(
        dir.path(),
        vec![
            Reply::tool("ord", "Bash", json!({ "command": sleeper(&ordinary_pid) })),
            Reply::tool(
                "bg",
                "Bash",
                json!({ "command": sleeper(&background_pid), "background": true }),
            ),
            Reply::hang(),
        ],
        300,
    )
    .await;
    ctl.send(Command::Submit {
        prompt: Prompt::text("start the jobs"),
    })
    .unwrap();

    // Handed over after the project's 300 ms, not the default 10 s.
    let Event::ToolFinished {
        is_error,
        duration_ms,
        ..
    } = until(&mut events, finished("ord")).await
    else {
        unreachable!()
    };
    assert!(
        !is_error,
        "the handed-over command is a task, not a failure"
    );
    assert!(
        duration_ms < 5_000,
        "yield_time_ms not applied: {duration_ms} ms"
    );

    until(&mut events, finished("bg")).await;
    let ordinary = pid_in(&ordinary_pid).await;
    let background = pid_in(&background_pid).await;
    assert!(procs::is_running(ordinary), "the handed-over command runs");
    assert!(procs::is_running(background), "the background job runs");

    ctl.send(Command::Cancel).unwrap();
    until(&mut events, |e| matches!(e, Event::RunFinished { .. })).await;
    assert!(
        procs::wait_gone(&[ordinary], Duration::from_secs(5)).await,
        "the handed-over command outlived its cancelled run"
    );
    assert!(
        procs::is_running(background),
        "a background job must outlive its run"
    );

    ctl.close().await;
    assert!(
        procs::wait_gone(&[background], Duration::from_secs(5)).await,
        "the background job outlived its session"
    );
}

#[tokio::test]
async fn the_project_yield_time_reaches_the_bash_tool() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let (ctl, mut events) = open(
        dir.path(),
        vec![
            Reply::tool("ord", "Bash", json!({ "command": sleeper(&pidfile) })),
            Reply::hang(),
        ],
        200,
    )
    .await;
    ctl.send(Command::Submit {
        prompt: Prompt::text("go"),
    })
    .unwrap();
    let Event::ToolFinished {
        is_error,
        duration_ms,
        ..
    } = until(&mut events, finished("ord")).await
    else {
        unreachable!()
    };
    assert!(!is_error);
    assert!(
        duration_ms < 5_000,
        "yield_time_ms not applied: {duration_ms} ms"
    );
    let pid = pid_in(&pidfile).await;
    ctl.send(Command::Cancel).unwrap();
    until(&mut events, |e| matches!(e, Event::RunFinished { .. })).await;
    assert!(procs::wait_gone(&[pid], Duration::from_secs(5)).await);
    ctl.close().await;
}
