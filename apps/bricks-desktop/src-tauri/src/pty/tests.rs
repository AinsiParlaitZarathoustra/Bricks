//! Terminal sessions on a real PTY, with a channel standing in for Tauri.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::{EventSink, OpenParams, PtyManager, ShellSpec};
use crate::contract::{IpcErrorCode, PtyEvent, PtyInfo};

const WAIT: Duration = Duration::from_secs(15);

fn sh() -> ShellSpec {
    ShellSpec { program: "/bin/sh".into(), args: Vec::new() }
}

fn open(manager: &PtyManager, cwd: &Path, project: &str) -> (PtyInfo, Receiver<PtyEvent>) {
    let (tx, rx) = mpsc::channel::<PtyEvent>();
    let sink: EventSink = Arc::new(move |event: PtyEvent| tx.send(event).is_ok());
    let params = OpenParams {
        project_id: project.into(),
        session_id: None,
        cwd: cwd.to_path_buf(),
        cols: 80,
        rows: 24,
    };
    let info = manager.open(params, &sh(), sink).expect("terminal opens");
    (info, rx)
}

/// Collect output until `Exit`. Returns the text and the exit code.
fn run_until_exit(rx: &Receiver<PtyEvent>) -> (String, Option<i32>) {
    let mut output = Vec::new();
    loop {
        match rx.recv_timeout(WAIT).expect("an event arrives") {
            PtyEvent::Data { data } => {
                output.extend(STANDARD.decode(data).expect("valid base64"));
            }
            PtyEvent::Exit { code } => return (String::from_utf8_lossy(&output).into_owned(), code),
        }
    }
}

#[test]
fn shell_runs_in_the_project_folder_and_exits_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::default();
    let (info, rx) = open(&manager, dir.path(), "p1");
    assert_eq!(info.shell, "/bin/sh");
    assert_eq!(info.project_id, "p1");

    manager.write(&info.id, "pwd\n").unwrap();
    manager.write(&info.id, "exit\n").unwrap();
    let (output, code) = run_until_exit(&rx);

    let folder = dir.path().file_name().unwrap().to_string_lossy().into_owned();
    assert!(output.contains(&folder), "output was {output:?}");
    assert_eq!(code, Some(0));
    assert!(manager.list(None).is_empty(), "an exited terminal leaves the registry");
    let err = manager.write(&info.id, "echo late\n").unwrap_err();
    assert_eq!(err.code, IpcErrorCode::NotFound);
}

#[test]
fn resize_is_accepted_and_listing_filters_by_project() {
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::default();
    let (a, _rx_a) = open(&manager, dir.path(), "p1");
    let (b, _rx_b) = open(&manager, dir.path(), "p2");

    manager.resize(&a.id, 120, 40).unwrap();
    // Out-of-range sizes are clamped, not rejected.
    manager.resize(&a.id, 0, 0).unwrap();

    let ids = |list: Vec<PtyInfo>| list.into_iter().map(|p| p.id).collect::<Vec<_>>();
    assert_eq!(ids(manager.list(Some("p1"))), vec![a.id.clone()]);
    assert_eq!(ids(manager.list(Some("p2"))), vec![b.id.clone()]);
    assert_eq!(ids(manager.list(None)), vec![a.id.clone(), b.id.clone()]);
    assert!(manager.list(Some("unknown")).is_empty());

    let err = manager.resize("missing", 80, 24).unwrap_err();
    assert_eq!(err.code, IpcErrorCode::NotFound);
    manager.kill_all();
}

#[test]
fn kill_ends_the_shell_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::default();
    let (info, rx) = open(&manager, dir.path(), "p1");

    manager.kill(&info.id);
    // Killed by a signal: no exit code.
    let (_, code) = run_until_exit(&rx);
    assert_eq!(code, None);

    manager.kill(&info.id);
    manager.kill("never-existed");
    assert!(manager.list(None).is_empty());
}

#[test]
fn consumer_that_stops_listening_gets_the_terminal_killed() {
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::default();
    let sink: EventSink = Arc::new(|_event: PtyEvent| false);
    let params = OpenParams {
        project_id: "p1".into(),
        session_id: None,
        cwd: dir.path().to_path_buf(),
        cols: 80,
        rows: 24,
    };
    let info = manager.open(params, &sh(), sink).unwrap();
    // Any output makes the pump try to deliver, and the sink refuses it.
    manager.write(&info.id, "echo hello\n").unwrap();

    let deadline = Instant::now() + WAIT;
    while !manager.list(None).is_empty() {
        assert!(Instant::now() < deadline, "the terminal was never reaped");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn opening_in_a_missing_folder_fails_with_folder_missing() {
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::default();
    let (tx, _rx) = mpsc::channel::<PtyEvent>();
    let sink: EventSink = Arc::new(move |event: PtyEvent| tx.send(event).is_ok());
    let params = OpenParams {
        project_id: "p1".into(),
        session_id: None,
        cwd: dir.path().join("gone"),
        cols: 80,
        rows: 24,
    };
    let err = manager.open(params, &sh(), sink).unwrap_err();
    assert_eq!(err.code, IpcErrorCode::FolderMissing);
    assert!(manager.list(None).is_empty());
}
