//! The session's commands and state, proven with real processes (Unix).
//!
//! Each command is its own process; what it changes in the session (working
//! directory, exported variables, functions, aliases) is published when it
//! ends normally. Synchronisation is on observable states (files, PIDs,
//! replies), never on long fixed sleeps; timeouts under test are short.

#![cfg(unix)]

use cersei_tools::shell::bash::{RunOutcome, RunStatus};
use cersei_tools::shell::{self, ShellConfig, ShellSessions, procs};
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
        let mut c = self
            .sessions
            .spawn_command(cmd, stdin, self.cwd(), &config(), timeout)
            .await
            .unwrap();
        c.finish().await
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
    assert_eq!(code(&f.run("unset MA_VAR").await), 0);
    assert_eq!(f.run("echo \"[$MA_VAR]\"").await.stdout.text, "[]\n");

    let o = f.run("cd 'sous dossier é'").await;
    let canon = std::fs::canonicalize(f.cwd().join("sous dossier é")).unwrap();
    assert_eq!(
        std::fs::canonicalize(o.cwd.as_ref().unwrap()).unwrap(),
        canon
    );
    let o = f.run("pwd -P").await;
    assert_eq!(o.stdout.text.trim_end(), canon.to_str().unwrap());
    // `cd -` goes back: OLDPWD is part of the directory state.
    f.run("cd -").await;
    let o = f.run("pwd -P").await;
    assert_eq!(
        std::fs::canonicalize(o.stdout.text.trim_end()).unwrap(),
        std::fs::canonicalize(f.cwd()).unwrap()
    );
}

#[tokio::test]
async fn functions_and_aliases_persist_plain_variables_do_not() {
    let f = Fixture::new();
    f.run("greet() { echo \"hello $1\"; }").await;
    assert_eq!(f.run("greet Bricks").await.stdout.text, "hello Bricks\n");
    f.run("alias ll='echo listed'").await;
    assert_eq!(f.run("ll").await.stdout.text, "listed\n");
    // Limit: a plain (non-exported) variable is not part of the session state.
    f.run("plain=yes").await;
    assert_eq!(f.run("echo \"[$plain]\"").await.stdout.text, "[]\n");
}

#[tokio::test]
async fn an_activated_environment_persists_its_path_and_variables() {
    let f = Fixture::new();
    // A virtualenv-like activation script, without needing python.
    let bin = f.cwd().join(".venv/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("fake-tool"), "#!/bin/sh\necho from-venv\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        bin.join("fake-tool"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(
        f.cwd().join(".venv/bin/activate"),
        "export VIRTUAL_ENV=\"$PWD/.venv\"\nPATH=\"$VIRTUAL_ENV/bin:$PATH\"\nexport PATH\ndeactivate() { unset VIRTUAL_ENV; }\n",
    )
    .unwrap();
    let act = f.run("source .venv/bin/activate && echo ok").await;
    assert_eq!(code(&act), 0, "{}", act.stderr.text);
    // The next command runs with the venv's PATH and VIRTUAL_ENV.
    let o = f.run("echo \"$VIRTUAL_ENV\"; fake-tool").await;
    assert!(o.stdout.text.contains(".venv"), "{}", o.stdout.text);
    assert!(o.stdout.text.ends_with("from-venv\n"), "{}", o.stdout.text);
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
    assert_eq!(f.run("echo alive").await.stdout.text, "alive\n");
    // `return` ends the command with its status.
    assert_eq!(code(&f.run("return 3; echo never").await), 3);
}

/// An exit that ends normally still publishes; an interrupted command (a
/// signal) publishes nothing: its changes are lost, and the result says so.
#[tokio::test]
async fn exit_publishes_and_a_killed_command_publishes_nothing() {
    let f = Fixture::new();
    let o = f.run("export KEEP=1; echo bye; exit 7").await;
    assert_eq!(o.status, RunStatus::Exited { code: 7 });
    assert_eq!(o.stdout.text, "bye\n");
    assert_eq!(f.run("echo \"[$KEEP]\"").await.stdout.text, "[1]\n");

    let o = f.run("export LOST=1; kill -9 $$").await;
    assert_eq!(o.status, RunStatus::Killed { signal: Some(9) });
    assert!(o.cwd.is_none());
    assert!(
        o.notes
            .iter()
            .any(|n| n.contains("environment changes not available")),
        "{:?}",
        o.notes
    );
    assert_eq!(
        f.run("echo \"[$LOST][$KEEP]\"").await.stdout.text,
        "[1]\n".replace("[1]", "[][1]")
    );
}

/// `exec` replaces the shell: nothing is recorded, nothing is published.
#[tokio::test]
async fn exec_publishes_nothing() {
    let f = Fixture::new();
    let o = f.run("export EXECVAR=1; exec true").await;
    assert_eq!(code(&o), 0);
    assert!(o.notes.iter().any(|n| n.contains("exec")), "{:?}", o.notes);
    assert_eq!(f.run("echo \"[$EXECVAR]\"").await.stdout.text, "[]\n");
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
    assert_eq!(o.status, RunStatus::TimedOut);
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
    // The session kept its state; the timed-out command published nothing.
    assert_eq!(f.run("echo $SURVIVES").await.stdout.text, "yes\n");
}

#[tokio::test]
async fn a_timed_out_loop_is_killed_and_the_session_keeps_its_state() {
    let f = Fixture::new();
    f.run("export GONE=1").await;
    let o = f
        .run_with("while :; do :; done", None, Duration::from_millis(300))
        .await;
    assert_eq!(o.status, RunStatus::TimedOut);
    assert_eq!(f.run("echo \"[$GONE]\"").await.stdout.text, "[1]\n");
}

#[tokio::test]
async fn stdin_is_empty_unless_given_and_there_is_no_tty() {
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
async fn a_cancelled_command_is_interrupted_and_the_session_stays_usable() {
    let f = Fixture::new();
    f.run("export STAY=1").await;
    let pidfile = f.cwd().join("pid");
    let cmd = format!(
        "sh -c 'echo $$ > \"{}\"; exec sleep 300'",
        pidfile.display()
    );
    let mut c = f
        .sessions
        .spawn_command(&cmd, None, f.cwd(), &config(), Duration::from_secs(60))
        .await
        .unwrap();
    // The caller gives up while the command runs: the handle is dropped.
    let _ = tokio::time::timeout(Duration::from_millis(500), c.finish()).await;
    drop(c);
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

/// Commands of one session run side by side: four of them, each waiting for
/// the other three to have started, all finish. Serialised, the first would
/// wait alone and see one file.
#[tokio::test]
async fn commands_of_one_session_run_side_by_side() {
    let f = Fixture::new();
    let d = f.cwd().display().to_string();
    let cmd = |i: u32| {
        format!(
            "touch '{d}/s{i}'; n=0; while [ $(find '{d}' -name 's*' | wc -l | tr -d ' ') -lt 4 ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n+1)); done; echo \"{i} $(find '{d}' -name 's*' | wc -l | tr -d ' ')\""
        )
    };
    let start = std::time::Instant::now();
    let (a, b, c, e) = tokio::join!(
        async { f.run(&cmd(0)).await },
        async { f.run(&cmd(1)).await },
        async { f.run(&cmd(2)).await },
        async { f.run(&cmd(3)).await },
    );
    for (i, o) in [a, b, c, e].iter().enumerate() {
        assert_eq!(o.stdout.text, format!("{i} 4\n"), "{}", o.stderr.text);
    }
    assert!(
        start.elapsed() < Duration::from_secs(8),
        "{:?}",
        start.elapsed()
    );
}

/// A command that waits does not hold back a quick one of the same session.
#[tokio::test]
async fn a_long_command_does_not_hold_back_a_quick_one() {
    let f = Fixture::new();
    let mut long = f
        .sessions
        .spawn_command(
            "sleep 30",
            None,
            f.cwd(),
            &config(),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    let start = std::time::Instant::now();
    let quick = f.run("echo quick").await;
    assert_eq!(quick.stdout.text, "quick\n");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
    long.task().stop(Duration::from_secs(2)).await;
    let o = long.finish().await;
    assert_eq!(o.status, RunStatus::Cancelled);
}

/// Two commands changing the directory: the one launched last wins, whatever
/// the order of their ends.
#[tokio::test]
async fn conflicting_directory_changes_follow_the_launch_order() {
    // `true`: the first launched ends last; `false`: the first launched ends first.
    for first_ends_last in [true, false] {
        let f = Fixture::new();
        std::fs::create_dir(f.cwd().join("a")).unwrap();
        std::fs::create_dir(f.cwd().join("b")).unwrap();
        let gate = f.cwd().join("gate");
        let d = f.cwd().display().to_string();
        let older_cmd = format!(
            "cd '{d}/a'; n=0; while [ ! -e '{}' ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n+1)); done",
            gate.display()
        );
        let newer_cmd = format!("cd '{d}/b'");
        if first_ends_last {
            let mut older = f
                .sessions
                .spawn_command(
                    &older_cmd,
                    None,
                    f.cwd(),
                    &config(),
                    Duration::from_secs(20),
                )
                .await
                .unwrap();
            let newer = f.run(&newer_cmd).await;
            assert_eq!(code(&newer), 0);
            std::fs::write(&gate, "").unwrap();
            assert_eq!(code(&older.finish().await), 0);
        } else {
            let mut newer = f
                .sessions
                .spawn_command(&format!("cd '{d}/b'; n=0; while [ ! -e '{}' ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n+1)); done", gate.display()), None, f.cwd(), &config(), Duration::from_secs(20))
                .await
                .unwrap();
            let older = f.run(&format!("cd '{d}/a'")).await;
            assert_eq!(code(&older), 0);
            std::fs::write(&gate, "").unwrap();
            assert_eq!(code(&newer.finish().await), 0);
        }
        // Launched last: `cd b` when the first launched ends last, `cd a` otherwise.
        let expected = if first_ends_last { "/b" } else { "/a" };
        let pwd = f.run("pwd -P").await.stdout.text;
        assert!(
            pwd.trim_end().ends_with(expected),
            "first_ends_last={first_ends_last}: {pwd}"
        );
    }
}

/// A background task starts from the state and never publishes into it.
#[tokio::test]
async fn background_tasks_never_publish_into_the_session() {
    let f = Fixture::new();
    std::fs::create_dir(f.cwd().join("sub")).unwrap();
    let t = f
        .sessions
        .start_task("cd sub; export BGVAR=1", None, None, f.cwd(), &config())
        .await
        .unwrap();
    assert!(t.wait_finished(Duration::from_secs(5)).await);
    let o = f.run("pwd -P; echo \"[$BGVAR]\"").await;
    let canon = std::fs::canonicalize(f.cwd()).unwrap();
    assert_eq!(o.stdout.text, format!("{}\n[]\n", canon.display()),);
}

/// The command of the test HTTP server (`tests/fixtures/http_server.py`):
/// a real child process, no reverse DNS lookup at start.
fn http_server(args: &[&str]) -> String {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/http_server.py");
    let mut cmd = format!("python3 -u '{}'", script.display());
    for a in args {
        cmd.push(' ');
        cmd.push_str(a);
    }
    cmd
}

#[tokio::test]
async fn a_background_server_is_supervised_and_independent() {
    let f = Fixture::new();
    f.run("export FROM_SHELL=inherited; served() { echo function-inherited; }")
        .await;
    let task = f
        .sessions
        .start_task(
            &format!(
                "served; echo \"$FROM_SHELL\" >&2; exec {}",
                http_server(&[])
            ),
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

/// The fixture never needs a name resolution: with `socket.getfqdn`
/// replaced by a function that fails at once, it still starts, answers and
/// stops (the failure would kill it before the readiness line).
#[tokio::test]
async fn the_test_server_starts_answers_and_stops_without_reverse_dns() {
    let f = Fixture::new();
    let task = f
        .sessions
        .start_task(
            &http_server(&["--forbid-getfqdn"]),
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
    let o = f
        .run(&format!("python3 -c \"import urllib.request as u; print(u.urlopen('http://127.0.0.1:{port}/').status)\""))
        .await;
    assert_eq!(o.stdout.text, "200\n", "{}", o.stderr.text);
    let err = task.read(cersei_tools::shell::background::Stream::Stderr, 0, 50);
    assert!(
        !err.lines.iter().any(|(_, l)| l.contains("getfqdn")),
        "{:?}",
        err.lines
    );
    let pid = task.pid;
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
async fn closing_a_session_stops_tasks_and_commands_without_leftovers() {
    let f = Fixture::new();
    let mut c = f
        .sessions
        .spawn_command(
            "sleep 300",
            None,
            f.cwd(),
            &config(),
            Duration::from_secs(600),
        )
        .await
        .unwrap();
    let cmd_pid = c.task().pid;
    let t = f
        .sessions
        .start_task("sleep 300", None, None, f.cwd(), &config())
        .await
        .unwrap();
    let task_pid = t.pid;
    shell::close_session(&f.id).await;
    assert!(procs::wait_gone(&[cmd_pid, task_pid], Duration::from_secs(5)).await);
    // Not even zombies: both were reaped.
    for pid in [cmd_pid, task_pid] {
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
    let _ = c.finish().await;
    // A new session with the same id starts clean.
    let fresh = shell::session(&f.id, &config());
    assert!(fresh.tasks().is_empty());
    assert_eq!(fresh.state().generation(), 0);
}

/// Four 2-second commands of one session take about 2 seconds together
/// (serialised, they would take 8).
#[tokio::test]
async fn four_two_second_commands_take_about_two_seconds() {
    let f = Fixture::new();
    let start = std::time::Instant::now();
    let (a, b, c, d) = tokio::join!(
        f.run("sleep 2"),
        f.run("sleep 2"),
        f.run("sleep 2"),
        f.run("sleep 2"),
    );
    for o in [a, b, c, d] {
        assert_eq!(code(&o), 0, "{}", o.stderr.text);
    }
    let took = start.elapsed();
    assert!(took >= Duration::from_secs(2), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}
