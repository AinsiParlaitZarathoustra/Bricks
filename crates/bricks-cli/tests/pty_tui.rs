//! The interface in a real pseudo-terminal: terminal modes set and
//! restored, bracketed paste, a prompt sent with Enter, resize, Ctrl+C
//! during a run, quitting.
//!
//! The test plays the terminal emulator: it answers the cursor-position
//! and device-attribute queries the interface sends at start-up.

#![cfg(unix)]

mod common;

use common::*;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Tty {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    output: Arc<Mutex<Vec<u8>>>,
    child: Box<dyn Child + Send + Sync>,
}

impl Tty {
    fn start(p: &Project, args: &[&str]) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bricks"));
        cmd.args(args);
        cmd.cwd(p.path());
        cmd.env("BRICKS_HOME", p.home.path());
        cmd.env("TERM", "xterm-256color");
        let child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
        let output = Arc::new(Mutex::new(Vec::new()));
        let mut reader = pty.master.try_clone_reader().unwrap();
        {
            let (output, writer) = (output.clone(), writer.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let chunk = &buf[..n];
                    // Answer the terminal queries, like an emulator.
                    let text = String::from_utf8_lossy(chunk);
                    for _ in 0..text.matches("\x1b[6n").count() {
                        let _ = writer.lock().unwrap().write_all(b"\x1b[1;1R");
                    }
                    if text.contains("\x1b[c") {
                        let _ = writer.lock().unwrap().write_all(b"\x1b[?62c");
                    }
                    let _ = writer.lock().unwrap().flush();
                    output.lock().unwrap().extend_from_slice(chunk);
                }
            });
        }
        Tty {
            master: pty.master,
            writer,
            output,
            child,
        }
    }

    fn send(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).to_string()
    }

    fn wait_for(&self, needle: &str, secs: u64) {
        let start = Instant::now();
        while !self.text().contains(needle) {
            assert!(
                start.elapsed() < Duration::from_secs(secs),
                "`{needle}` not seen; output:\n{}",
                self.text().escape_debug()
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn wait_exit(&mut self, secs: u64) -> u32 {
        let start = Instant::now();
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s.exit_code();
            }
            assert!(start.elapsed() < Duration::from_secs(secs), "did not exit");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

#[test]
fn the_interface_runs_in_a_terminal_and_restores_it() {
    let m = model(vec![
        text("Salut depuis le modèle"),
        text("Salut encore"),
        HANG.to_string(),
    ]);
    let p = Project::new(&m.url, "");
    let mut tty = Tty::start(&p, &["tui"]);
    tty.wait_for("Ask Bricks", 20);
    let out = tty.text();
    assert!(out.contains("\x1b[?2004h"), "bracketed paste enabled");

    // A multi-line paste is inserted, not sent.
    tty.send(b"\x1b[200~ligne 1\rligne 2\x1b[201~");
    std::thread::sleep(Duration::from_millis(500));
    assert!(m.seen.lock().unwrap().is_empty(), "a paste never submits");
    tty.wait_for("ligne 2", 5);
    // Ctrl+U clears; type and send.
    tty.send(b"\x15Bonjour \xc3\xa9t\xc3\xa9\r");
    tty.wait_for("Salut", 20);
    {
        let seen = m.seen.lock().unwrap();
        assert!(seen[0].to_string().contains("Bonjour été"), "{}", seen[0]);
    }

    // Resize: the interface keeps working.
    tty.master
        .resize(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));

    // Ctrl+C during a run cancels it.
    let before = m.seen.lock().unwrap().len();
    tty.send(b"attends\r");
    let start = Instant::now();
    while m.seen.lock().unwrap().len() <= before {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the run did not start"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    std::thread::sleep(Duration::from_millis(200));
    tty.send(b"\x03");
    tty.wait_for("cancelled", 20);

    // Quit: Ctrl+C twice when idle. The terminal modes are restored.
    std::thread::sleep(Duration::from_millis(300));
    tty.send(b"\x03");
    std::thread::sleep(Duration::from_millis(200));
    tty.send(b"\x03");
    let code = tty.wait_exit(20);
    assert_eq!(code, 0, "{}", tty.text().escape_debug());
    let out = tty.text();
    assert!(
        out.contains("\x1b[?2004l"),
        "bracketed paste disabled at exit"
    );
    assert!(out.contains("\x1b[?25h"), "cursor shown at exit");
    assert!(
        out.contains("bricks resume"),
        "the session id is given back"
    );
}

#[test]
fn without_a_terminal_the_interface_refuses_and_points_to_run() {
    let m = model(vec![]);
    let p = Project::new(&m.url, "");
    let out = p.run(&["tui"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("bricks run"));
}
