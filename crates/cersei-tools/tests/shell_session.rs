//! The persistent shell, proven with real processes (Unix).
//!
//! Synchronisation is on observable states (replies, PIDs, files), never on
//! long fixed sleeps; timeouts under test are short.

#![cfg(unix)]

use cersei_tools::shell::bash::{RunOutcome, RunStatus};
use cersei_tools::shell::{self, procs, ShellConfig, ShellSessions};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn config() -> ShellConfig {
    ShellConfig {
        term_grace: Duration::from_millis(500),
        drain: Duration::from_millis(300),
        progress_every: Duration::from_millis(200),
        base_dir: std::env::temp_dir().join("bricks-shell-tests"),
        capture: cersei_tools::shell::capture::CaptureLimits {
            head_bytes: 256 * 1024,
            tail_bytes: 256 * 1024,
            raw_memory_bytes: 512 * 1024,
        },
        ..ShellConfig::default()
    }
}

struct Fixture {
    id: String,
    sessions: Arc<ShellSessions>,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let id = format!("test-{}", uuid::Uuid::new_v4());
        Self {
            sessions: shell::session(&id, &config()),
            id,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn cwd(&self) -> &Path {
        self.dir.path()
    }

    async fn run(&self, cmd: &str) -> RunOutcome {
        self.run_with(cmd, None, Duration::from_secs(20)).await
    }

    async fn run_with(&self, cmd: &str, stdin: Option<&str>, timeout: Duration) -> RunOutcome {
        let b = self.sessions.bash(self.cwd(), &config()).await.unwrap();
        b.run(cmd, stdin, timeout, None).await.unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        shell::close_session_now(&self.id);
    }
}

fn code(o: &RunOutcome) -> i32 {
    match o.status {
        RunStatus::Exited { code } => code,
        ref s => panic!("not exited: {s:?}"),
    }
}

#[tokio::test]
async fn exported_variables_and_cwd_persist_between_calls() {
    let f = Fixture::new();
    std::fs::create_dir(f.cwd().join("sous dossier é")).unwrap();
    assert_eq!(code(&f.run("export MA_VAR=\"Bricks42\"").await), 0);
    let o = f.run("echo \"$MA_VAR\"").await;
    assert_eq!(o.stdout.text, "Bricks42\n");

    let o = f.run("cd 'sous dossier é'").await;
    let canon = std::fs::canonicalize(f.cwd().join("sous dossier é")).unwrap();
    assert_eq!(
        std::fs::canonicalize(o.cwd.as_ref().unwrap()).unwrap(),
        canon
    );
    let o = f.run("pwd -P").await;
    assert_eq!(o.stdout.text.trim_end(), canon.to_str().unwrap());
}

#[tokio::test]
async fn functions_aliases_and_a_venv_persist() {
    let f = Fixture::new();
    f.run("greet() { echo \"hello $1\"; }").await;
    assert_eq!(f.run("greet Bricks").await.stdout.text, "hello Bricks\n");
    // Alias defined in one request, used in the next.
    f.run("alias ll='echo listed'").await;
    assert_eq!(f.run("ll").await.stdout.text, "listed\n");
    // A non-exported variable and `declare` persist too: the driver runs at
    // the shell's top level, not inside a function.
    f.run("declare -i counter=41; plain=yes").await;
    assert_eq!(
        f.run("echo $((counter+1)) $plain").await.stdout.text,
        "42 yes\n"
    );

    let venv = f
        .run("python3 -m venv .venv && source .venv/bin/activate && echo ok")
        .await;
    assert_eq!(code(&venv), 0, "{}", venv.stderr.text);
    let o = f.run("echo \"$VIRTUAL_ENV\"; command -v python").await;
    let canon = std::fs::canonicalize(f.cwd()).unwrap();
    assert!(o.stdout.text.contains(".venv"), "{}", o.stdout.text);
    assert!(
        o.stdout
            .text
            .lines()
            .nth(1)
            .unwrap()
            .contains(".venv/bin/python"),
        "{} (cwd {})",
        o.stdout.text,
        canon.display()
    );
    assert_eq!(
        code(&f.run("deactivate && test -z \"$VIRTUAL_ENV\"").await),
        0
    );
}

#[tokio::test]
async fn sessions_are_isolated() {
    let a = Fixture::new();
    let b = Fixture::new();
    a.run("export ONLY_A=1; cd /").await;
    let o = b.run("echo \"[$ONLY_A]\"; pwd").await;
    assert_eq!(o.stdout.text.lines().next(), Some("[]"));
    assert_ne!(o.stdout.text.lines().nth(1), Some("/"));
    // The process environment of Bricks itself is untouched.
    assert!(std::env::var("ONLY_A").is_err());
}

#[tokio::test]
async fn command_text_is_transported_exactly() {
    let f = Fixture::new();
    let cmd = "printf '%s|' \"a  b\" 'c\"d' $'e\\tf'\necho\ncat <<'EOF'\n$HOME is not expanded \"here\"\n  indented\nEOF\nx=$(echo 'multi\nline')\necho \"$x\"";
    let o = f.run(cmd).await;
    assert_eq!(code(&o), 0, "{}", o.stderr.text);
    assert_eq!(
        o.stdout.text,
        "a  b|c\"d|e\tf|\n$HOME is not expanded \"here\"\n  indented\nmulti\nline\n"
    );
}

#[tokio::test]
async fn a_subshell_does_not_change_the_parent() {
    let f = Fixture::new();
    let before = f.run("pwd").await.stdout.text;
    f.run("(cd / && export SUB=1)").await;
    let o = f.run("pwd; echo \"[$SUB]\"").await;
    assert_eq!(o.stdout.text, format!("{before}[]\n"));
}

#[tokio::test]
async fn output_that_imitates_markers_or_the_protocol_is_ordinary_output() {
    let f = Fixture::new();
    let o = f
        .run("echo __ABSTRACT_STATE_7f2a9b__; printf 'END\\0001\\0000\\000/\\000'; printf 'READY\\000' >&2; echo after")
        .await;
    assert_eq!(code(&o), 0);
    assert!(o.stdout.text.starts_with("__ABSTRACT_STATE_7f2a9b__\n"));
    assert!(o.stdout.text.ends_with("after\n"), "{:?}", o.stdout.text);
    // The session is in sync: the next reply belongs to the next command.
    assert_eq!(f.run("echo next").await.stdout.text, "next\n");
}

#[tokio::test]
async fn exit_codes_and_syntax_errors() {
    let f = Fixture::new();
    assert_eq!(code(&f.run("true").await), 0);
    assert_eq!(code(&f.run("false").await), 1);
    let o = f.run("definitely_not_a_command_bricks").await;
    assert_eq!(code(&o), 127);
    assert!(
        o.stderr.text.contains("command not found"),
        "{}",
        o.stderr.text
    );
    let o = f.run("if then fi (").await;
    assert_ne!(code(&o), 0);
    assert!(o.stderr.text.contains("syntax error"), "{}", o.stderr.text);
    // A syntax error does not end the session.
    assert_eq!(f.run("echo alive").await.stdout.text, "alive\n");
    // `return` ends the command with its status.
    assert_eq!(code(&f.run("return 3; echo never").await), 3);
}

#[tokio::test]
async fn exit_and_a_lost_channel_end_the_shell_explicitly() {
    let f = Fixture::new();
    f.run("export KEEP=1").await;
    let o = f.run("echo bye; exit 7").await;
    assert_eq!(
        o.status,
        RunStatus::ShellEnded {
            code: Some(7),
            signal: None
        }
    );
    assert_eq!(o.stdout.text, "bye\n");
    assert!(o.notes.iter().any(|n| n.contains("state")), "{:?}", o.notes);

    // The next command runs in a fresh shell and the reset is reported.
    let o = f.run("echo \"[$KEEP]\"").await;
    assert_eq!(o.stdout.text, "[]\n");
    assert!(f.sessions.take_reset_notice().is_some());

    // The shell killed under its own feet (channel lost).
    let o = f.run("kill -9 $$").await;
    assert!(
        matches!(
            o.status,
            RunStatus::ShellEnded {
                signal: Some(9),
                ..
            }
        ),
        "{:?}",
        o.status
    );
    assert_eq!(f.run("echo again").await.stdout.text, "again\n");
}

#[tokio::test]
async fn large_simultaneous_outputs_do_not_deadlock() {
    let f = Fixture::new();
    // 2 MiB on each stream, interleaved: far beyond pipe capacity.
    let o = f
        .run("for i in $(seq 1 16384); do echo \"out line $i padded to be long enough........................................................................................\"; echo \"err line $i padded to be long enough........................................................................................\" >&2; done")
        .await;
    assert_eq!(code(&o), 0);
    assert_eq!(o.stdout.raw_bytes, o.stderr.raw_bytes);
    assert!(o.stdout.raw_bytes > 2_000_000);
    assert!(o.stdout.text.contains("out line 1 "));
    assert!(o.stdout.text.contains("out line 16384 "));
    // Beyond the in-memory head/tail, the gap is stated and the raw is on disk.
    assert!(o.stdout.text.contains("bytes of output omitted"));
    let raw = std::fs::metadata(o.stdout.raw_path.as_ref().unwrap()).unwrap();
    assert_eq!(raw.len(), o.stdout.raw_bytes);
}

#[tokio::test]
async fn no_output_utf8_and_fragmented_escapes() {
    let f = Fixture::new();
    let o = f.run("true").await;
    assert!(o.stdout.text.is_empty() && o.stderr.text.is_empty());
    // A multi-byte character and an escape sequence split across writes.
    let o = f
        .run("printf '\\xe6\\x97'; sleep 0.05; printf '\\xa5 \\033[3'; sleep 0.05; printf '1mrouge\\033[0m\\r\\n'")
        .await;
    assert_eq!(o.stdout.text, "日 rouge\n");
}

#[tokio::test]
async fn a_timeout_returns_partial_output_and_stops_the_tree() {
    let f = Fixture::new();
    f.run("export SURVIVES=yes").await;
    let o = f
        .run_with(
            "echo before-out; echo before-err >&2; sleep 300 & echo \"bg=$!\"; sh -c 'echo \"child=$$\"; exec sleep 300'",
            None,
            Duration::from_millis(800),
        )
        .await;
    assert_eq!(
        o.status,
        RunStatus::TimedOut {
            shell_destroyed: false
        }
    );
    assert!(o.stdout.text.contains("before-out"));
    assert!(o.stderr.text.contains("before-err"));
    let pids: Vec<u32> = o
        .stdout
        .text
        .lines()
        .filter_map(|l| l.split('=').nth(1).and_then(|p| p.parse().ok()))
        .collect();
    assert_eq!(pids.len(), 2, "{}", o.stdout.text);
    assert!(
        procs::wait_gone(&pids, Duration::from_secs(3)).await,
        "{pids:?} still running"
    );
    assert!(!o.terminated.targeted.is_empty());
    // The shell survived with its state.
    assert_eq!(f.run("echo $SURVIVES").await.stdout.text, "yes\n");
}

#[tokio::test]
async fn a_builtin_loop_that_ignores_signals_destroys_the_shell_explicitly() {
    let f = Fixture::new();
    f.run("export GONE=1").await;
    let o = f
        .run_with("while :; do :; done", None, Duration::from_millis(300))
        .await;
    assert_eq!(
        o.status,
        RunStatus::TimedOut {
            shell_destroyed: true
        }
    );
    assert!(o.notes.iter().any(|n| n.contains("destroyed")));
    let o = f.run("echo \"[$GONE]\"").await;
    assert_eq!(o.stdout.text, "[]\n");
    assert!(f.sessions.take_reset_notice().is_some());
}

#[tokio::test]
async fn reading_stdin_never_consumes_the_control_channel() {
    let f = Fixture::new();
    // Default: stdin is empty; `read` and `cat` return at once.
    let o = f
        .run("read x; echo \"got[$x] rc=$?\"; cat; echo done")
        .await;
    assert_eq!(o.stdout.text, "got[] rc=1\ndone\n");
    // Explicit input.
    let o = f
        .run_with(
            "read a; read b; echo \"$b-$a\"",
            Some("one\ntwo\n"),
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(o.stdout.text, "two-one\n");
    // A program that tries the terminal has none (no controlling tty).
    let o = f
        .run("exec 3</dev/tty && echo has-tty || echo no-tty")
        .await;
    assert!(o.stdout.text.contains("no-tty"), "{}", o.stdout.text);
    assert_eq!(
        f.run("echo still-in-sync").await.stdout.text,
        "still-in-sync\n"
    );
}

#[tokio::test]
async fn progress_is_reported_while_a_command_runs() {
    let f = Fixture::new();
    let seen = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let s = seen.clone();
    let b = f.sessions.bash(f.cwd(), &config()).await.unwrap();
    let o = b
        .run(
            "sleep 0.9",
            None,
            Duration::from_secs(5),
            Some(Arc::new(move |m| s.lock().push(m))),
        )
        .await
        .unwrap();
    assert_eq!(code(&o), 0);
    let seen = seen.lock();
    assert!(seen.len() >= 2, "{seen:?}");
    assert!(seen.iter().any(|m| m.contains("no output")), "{seen:?}");
}

#[tokio::test]
async fn a_cancelled_command_is_interrupted_and_the_shell_stays_usable() {
    let f = Fixture::new();
    f.run("export STAY=1").await;
    let b = f.sessions.bash(f.cwd(), &config()).await.unwrap();
    let pidfile = f.cwd().join("pid");
    let cmd = format!(
        "sh -c 'echo $$ > \"{}\"; exec sleep 300'",
        pidfile.display()
    );
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        b.run(&cmd, None, Duration::from_secs(60), None),
    )
    .await;
    let pid: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(procs::wait_gone(&[pid], Duration::from_secs(5)).await);
    assert_eq!(f.run("echo $STAY").await.stdout.text, "1\n");
}

#[tokio::test]
async fn leftover_processes_of_a_finished_command_are_stopped() {
    let f = Fixture::new();
    let o = f.run("sleep 300 & echo \"pid=$!\"").await;
    let pid: u32 = o
        .stdout
        .text
        .trim()
        .trim_start_matches("pid=")
        .parse()
        .unwrap();
    assert!(
        o.notes.iter().any(|n| n.contains("background: true")),
        "{:?}",
        o.notes
    );
    assert!(procs::wait_gone(&[pid], Duration::from_secs(3)).await);
}

#[tokio::test]
async fn a_background_server_is_supervised_and_independent() {
    let f = Fixture::new();
    f.run("export FROM_SHELL=inherited; served() { echo function-inherited; }")
        .await;
    let task = f
        .sessions
        .start_task(
            "served; echo \"$FROM_SHELL\" >&2; exec python3 -u -m http.server 0 --bind 127.0.0.1",
            None,
            Some(regex::Regex::new(r"port (\d+)").unwrap()),
            f.cwd(),
            &config(),
        )
        .await
        .unwrap();
    task.wait_ready(Duration::from_secs(15)).await;
    let st = task.status();
    let port: u16 = match &st.readiness {
        cersei_tools::shell::background::Readiness::Ready { line, .. } => {
            regex::Regex::new(r"port (\d+)")
                .unwrap()
                .captures(line)
                .unwrap()[1]
                .parse()
                .unwrap()
        }
        other => panic!("not ready: {other:?} {st:?}"),
    };
    // The next foreground command runs while the task keeps going.
    let o = f
        .run(&format!("python3 -c \"import urllib.request as u; print(u.urlopen('http://127.0.0.1:{port}/').status)\""))
        .await;
    assert_eq!(o.stdout.text, "200\n", "{}", o.stderr.text);
    assert!(
        !o.stdout.text.contains("Serving"),
        "task output must not leak"
    );

    // Paged logs with a cursor.
    let page = task.read(cersei_tools::shell::background::Stream::Stdout, 0, 10);
    assert_eq!(page.lines[0].1, "function-inherited");
    let err = task.read(cersei_tools::shell::background::Stream::Stderr, 0, 1);
    assert_eq!(err.lines[0].1, "inherited");
    assert_eq!(err.next_offset, 1);

    // Stop: idempotent, the port is released, the process is gone.
    let pid = task.pid;
    assert_eq!(
        task.stop(Duration::from_secs(2)).await,
        cersei_tools::shell::background::TaskState::Stopped
    );
    assert_eq!(
        task.stop(Duration::from_secs(2)).await,
        cersei_tools::shell::background::TaskState::Stopped
    );
    assert!(procs::wait_gone(&[pid], Duration::from_secs(3)).await);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[tokio::test]
async fn task_states_and_exit_codes() {
    let f = Fixture::new();
    let ok = f
        .sessions
        .start_task("echo done", None, None, f.cwd(), &config())
        .await
        .unwrap();
    let bad = f
        .sessions
        .start_task("exit 4", None, None, f.cwd(), &config())
        .await
        .unwrap();
    assert!(ok.wait_finished(Duration::from_secs(5)).await);
    assert!(bad.wait_finished(Duration::from_secs(5)).await);
    assert_eq!(
        ok.state(),
        cersei_tools::shell::background::TaskState::Completed { code: 0 }
    );
    assert_eq!(
        bad.state(),
        cersei_tools::shell::background::TaskState::Failed {
            code: Some(4),
            signal: None
        }
    );
    // Tasks belong to their session only.
    let other = Fixture::new();
    assert!(other.sessions.task(&ok.id).is_none());
}

#[tokio::test]
async fn closing_a_session_stops_tasks_and_shell_without_leftovers() {
    let f = Fixture::new();
    let b = f.sessions.bash(f.cwd(), &config()).await.unwrap();
    let shell_pid = b.pid();
    let t = f
        .sessions
        .start_task("sleep 300", None, None, f.cwd(), &config())
        .await
        .unwrap();
    let task_pid = t.pid;
    shell::close_session(&f.id).await;
    assert!(procs::wait_gone(&[shell_pid, task_pid], Duration::from_secs(5)).await);
    // Not even zombies: both were reaped.
    for pid in [shell_pid, task_pid] {
        let ps = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&ps.stdout).trim().is_empty(),
            "pid {pid} still listed"
        );
    }
    assert!(!f.sessions.dir().exists());
    // A new session with the same id starts clean.
    let fresh = shell::session(&f.id, &config());
    let b2 = fresh.bash(f.cwd(), &config()).await.unwrap();
    assert_ne!(b2.pid(), shell_pid);
    assert!(fresh.tasks().is_empty());
}
