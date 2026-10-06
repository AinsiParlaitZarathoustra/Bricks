//! Ctrl+C in a real terminal (PTY): the run is cancelled, the exit code is
//! 130, and the session stays stored and resumable.

#![cfg(unix)]

mod common;

use common::*;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::time::{Duration, Instant};

#[test]
fn ctrl_c_cancels_the_run_and_keeps_the_session() {
    let m = model(vec![HANG.to_string()]);
    let p = Project::new(&m.url, "");
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bricks"));
    cmd.args(["run", "--json", "Attends"]);
    cmd.cwd(p.path());
    cmd.env("BRICKS_HOME", p.home.path());
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().unwrap();
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let output = output.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                output.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
    }
    // Wait until the model was asked (the run is in flight).
    let start = Instant::now();
    while m.seen.lock().unwrap().is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the run never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut writer = pty.master.take_writer().unwrap();
    writer.write_all(b"\x03").unwrap();
    writer.flush().unwrap();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "did not exit after Ctrl+C"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.exit_code(), 130);
    std::thread::sleep(Duration::from_millis(100));
    let text = String::from_utf8_lossy(&output.lock().unwrap()).to_string();
    assert!(text.contains("\"outcome\":\"cancelled\""), "{text}");
    // The session (with the prompt) was stored.
    let sessions = std::fs::read_dir(p.sessions_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .count();
    assert!(sessions >= 1);
}

use std::io::Write;
