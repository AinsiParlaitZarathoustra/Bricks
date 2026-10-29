//! The shell tools through a real agent (scripted model, real processes).

#![cfg(unix)]

mod common;

use async_trait::async_trait;
use bricks_agent::Agent;
use bricks_agent::events::AgentEvent;
use bricks_compression::CompressionLevel;
use bricks_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use bricks_tools::jobs::JobRegistry;
use bricks_tools::shell::{ShellConfig, procs};
use bricks_types::*;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ─── Scripted model ──────────────────────────────────────────────────────────

fn tool_call(id: &str, tool: &str, args: Value) -> String {
    let first = json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "tool_calls": [{
        "index": 0, "id": id, "type": "function",
        "function": { "name": tool, "arguments": args.to_string() } }] } }] });
    let last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }] });
    format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n")
}

fn text(t: &str) -> String {
    let first =
        json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "content": t } }] });
    let last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] });
    format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n")
}

/// Serves `replies` in order (then plain "done" texts forever).
fn serve(replies: Vec<String>) -> String {
    let mut it = replies.into_iter();
    serve_with(move |_body| it.next().unwrap_or_else(|| text("done")))
}

/// Serves what `next` returns for each request, given the request body so
/// far: a reply can name a job id that an earlier tool result gave.
fn serve_with(mut next: impl FnMut(&str) -> String + Send + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((mut sock, _)) = listener.accept() {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
            let body_start = loop {
                let n = sock.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break None;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(p + 4);
                }
            };
            let Some(start) = body_start else { continue };
            let head = String::from_utf8_lossy(&buf[..start]).to_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            while buf.len() < start + len {
                let n = sock.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let request = String::from_utf8_lossy(&buf[start..]).to_string();
            let body = next(&request);
            let out = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(out.as_bytes());
            let _ = sock.shutdown(std::net::Shutdown::Write);
        }
    });
    format!("http://127.0.0.1:{port}/v1")
}

/// The job id (`job_` and 12 hex digits) named in `text`, if any.
fn job_id_in(text: &str) -> Option<String> {
    regex::Regex::new(r"job_[0-9a-f]{12}")
        .unwrap()
        .find(text)
        .map(|m| m.as_str().to_string())
}

fn tool_results(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .flat_map(|m| m.content_blocks())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content: ToolResultContent::Text(t),
                ..
            } => Some(t),
            _ => None,
        })
        .collect()
}

fn shell_extensions(progress_every: Duration) -> bricks_tools::Extensions {
    let ext = bricks_tools::Extensions::default();
    ext.insert(ShellConfig {
        progress_every,
        term_grace: Duration::from_millis(500),
        ..ShellConfig::default()
    });
    ext
}

fn agent(url: &str, work: &std::path::Path) -> bricks_agent::AgentBuilder {
    Agent::builder()
        .provider(common::provider(url, "chat_completions", 100_000))
        .tools(bricks_tools::shell_with_jobs(JobRegistry::new(Default::default(), None)))
        .working_dir(work)
        .raw_output_dir(work.join(".raw"))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn state_persists_across_turns_and_results_are_rendered_uniformly() {
    let work = tempfile::tempdir().unwrap();
    std::fs::create_dir(work.path().join("sub dir")).unwrap();
    let url = serve(vec![
        tool_call(
            "c1",
            "Bash",
            json!({ "command": "export MA_VAR=Bricks42; cd 'sub dir'" }),
        ),
        tool_call(
            "c2",
            "Bash",
            json!({ "command": "echo \"$MA_VAR\"; basename \"$PWD\"" }),
        ),
        tool_call(
            "c3",
            "Bash",
            json!({ "command": "foobar_missing_cmd_bricks" }),
        ),
        text("finished"),
    ]);
    let a = agent(&url, work.path()).build().unwrap();
    a.run("go").await.unwrap();
    let r = tool_results(&a);

    assert!(r[0].starts_with("✓ [Bash] Success ("), "{}", r[0]);
    assert!(
        r[0].ends_with(") — code 0\n(Command ran successfully with no output)"),
        "{}",
        r[0]
    );
    assert!(
        r[1].contains("--- stdout ---\nBricks42\nsub dir"),
        "{}",
        r[1]
    );
    let fail = &r[2];
    assert!(fail.starts_with("✗ [Bash] Failed ("), "{fail}");
    assert!(fail.contains(") — code 127\n--- stderr ---\n"), "{fail}");
    assert!(fail.contains("command not found"), "{fail}");
    assert!(
        fail.contains("--- suggestion ---\nCheck that the tool is installed"),
        "{fail}"
    );
    // The shell's `cd` did not move the agent's own working directory.
    assert!(!work.path().join("sub dir").join("sub dir").exists());
}

#[tokio::test]
async fn a_timeout_renders_partial_output_without_a_code() {
    let work = tempfile::tempdir().unwrap();
    let url = serve(vec![
        tool_call(
            "t1",
            "Bash",
            json!({ "command": "echo partial; echo err-part >&2; sleep 30", "timeout": 600 }),
        ),
        tool_call("t2", "Bash", json!({ "command": "echo alive" })),
        text("ok"),
    ]);
    let a = agent(&url, work.path()).build().unwrap();
    a.run("go").await.unwrap();
    let r = tool_results(&a);
    let t = &r[0];
    assert!(
        t.starts_with("⏱ [Bash] Interrupted after timeout of 0.6s ("),
        "{t}"
    );
    assert!(t.contains("no exit code"), "{t}");
    assert!(!t.contains("code 0") && !t.contains("code -1"), "{t}");
    assert!(
        t.contains("[Interrupted after timeout of 0.6s — partial output below]"),
        "{t}"
    );
    assert!(
        t.contains("--- stdout ---\npartial") && t.contains("--- stderr ---\nerr-part"),
        "{t}"
    );
    assert!(r[1].contains("alive"), "the session stays usable: {}", r[1]);
}

#[tokio::test]
async fn background_jobs_through_the_agent() {
    let work = tempfile::tempdir().unwrap();
    let server = format!(
        "python3 -u '{}'",
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bricks-tools/tests/fixtures/http_server.py")
            .display()
    );
    // Each reply is built from the request so far: the job's id comes from
    // the result of the Bash call that started it.
    let mut step = 0;
    let url = serve_with(move |request| {
        let id = job_id_in(request).unwrap_or_default();
        let reply = match step {
            0 => tool_call(
                "b1",
                "Bash",
                json!({
                    "command": server,
                    "background": true,
                    "ready_pattern": "port \\d+",
                    "ready_timeout": 15000
                }),
            ),
            1 => tool_call("b2", "Job", json!({ "action": "output", "job_id": id })),
            2 => tool_call("b3", "Job", json!({})),
            3 => tool_call("b4", "Job", json!({ "action": "stop", "job_id": id })),
            4 => tool_call("b5", "Job", json!({ "action": "stop", "job_id": id })),
            5 => tool_call(
                "b6",
                "Job",
                json!({ "action": "output", "job_id": "job_000000000000" }),
            ),
            _ => text("ok"),
        };
        step += 1;
        reply
    });
    let a = agent(&url, work.path()).build().unwrap();
    a.run("serve").await.unwrap();
    let r = tool_results(&a);
    let id = job_id_in(&r[0]).expect(&r[0]);
    assert!(r[0].starts_with("… [Bash] Running ("), "{}", r[0]);
    assert!(
        r[0].contains(&format!("Started job {id}")) && r[0].contains("ready after"),
        "{}",
        r[0]
    );
    let port: u16 = regex::Regex::new(r"port (\d+)")
        .unwrap()
        .captures(&r[1])
        .expect(&r[1])[1]
        .parse()
        .unwrap();
    assert!(r[1].contains("Serving HTTP"), "{}", r[1]);
    assert!(r[2].contains(&format!("{id} [running]")), "{}", r[2]);
    assert!(r[3].starts_with("✓ [Job] Success"), "{}", r[3]);
    assert!(r[3].contains(&format!("{id} [stopped]")), "{}", r[3]);
    // Stopping a stopped job is not an error.
    assert!(r[4].starts_with("✓ [Job] Success"), "{}", r[4]);
    assert!(r[4].contains(&format!("{id} [stopped]")), "{}", r[4]);
    assert!(r[5].starts_with("✗ [Job] Failed"), "{}", r[5]);
    assert!(r[5].contains("No accessible job"), "{}", r[5]);
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "port released"
    );
}

#[tokio::test]
async fn closing_or_dropping_an_agent_stops_its_processes() {
    let work = tempfile::tempdir().unwrap();
    let start = || {
        serve(vec![tool_call(
            "k",
            "Bash",
            json!({ "command": "sleep 300", "background": true }),
        )])
    };
    let pid_of = |a: &Agent| -> u32 {
        let r = tool_results(a);
        let started = r
            .iter()
            .find(|t| t.contains("pid "))
            .expect("a task result");
        regex::Regex::new(r"pid (\d+)")
            .unwrap()
            .captures(started)
            .unwrap()[1]
            .parse()
            .unwrap()
    };
    let url = start();
    let a = agent(&url, work.path()).build().unwrap();
    a.run("go").await.unwrap();
    let pid = pid_of(&a);
    a.close().await;
    assert!(procs::wait_gone(&[pid], Duration::from_secs(5)).await);

    let url = start();
    let b = agent(&url, work.path()).build().unwrap();
    b.run("go").await.unwrap();
    let pid = pid_of(&b);
    drop(b);
    assert!(procs::wait_gone(&[pid], Duration::from_secs(5)).await);
}

#[tokio::test]
async fn agents_do_not_share_shells() {
    let work = tempfile::tempdir().unwrap();
    let url_a = serve(vec![
        tool_call("a1", "Bash", json!({ "command": "export ONLY_A=1" })),
        text("ok"),
    ]);
    let url_b = serve(vec![
        tool_call("b1", "Bash", json!({ "command": "echo \"[$ONLY_A]\"" })),
        text("ok"),
    ]);
    let a = agent(&url_a, work.path()).build().unwrap();
    a.run("go").await.unwrap();
    // A second agent (as a sub-agent is: no shared session id).
    let b = agent(&url_b, work.path()).build().unwrap();
    b.run("go").await.unwrap();
    assert!(
        tool_results(&b)[0].contains("--- stdout ---\n[]"),
        "{}",
        tool_results(&b)[0]
    );
}

#[tokio::test]
async fn long_commands_report_progress() {
    let work = tempfile::tempdir().unwrap();
    let url = serve(vec![
        tool_call("p1", "Bash", json!({ "command": "sleep 0.8" })),
        text("ok"),
    ]);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let a = agent(&url, work.path())
        .extensions(shell_extensions(Duration::from_millis(200)))
        .on_event(move |e| {
            if let AgentEvent::ToolProgress { name, message } = e {
                s.lock().unwrap().push(format!("{name}: {message}"));
            }
        })
        .build()
        .unwrap();
    a.run("go").await.unwrap();
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 2, "{seen:?}");
    assert!(seen[0].starts_with("Bash: still running"), "{seen:?}");
}

struct Recording(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl PermissionPolicy for Recording {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        self.0.lock().unwrap().push(request.description.clone());
        if request.description.contains("rm -rf") {
            PermissionDecision::Deny("a session alias runs rm -rf".into())
        } else {
            PermissionDecision::Allow
        }
    }
}

#[tokio::test]
async fn aliases_cannot_hide_a_command_from_the_permission_policy() {
    let work = tempfile::tempdir().unwrap();
    let victim = work.path().join("victim");
    std::fs::create_dir(&victim).unwrap();
    let url = serve(vec![
        tool_call(
            "x1",
            "Bash",
            json!({ "command": format!("alias tidy='rm -rf {}'", victim.display()) }),
        ),
        tool_call("x2", "Bash", json!({ "command": "tidy" })),
        text("ok"),
    ]);
    let descriptions = Arc::new(Mutex::new(Vec::new()));
    let a = agent(&url, work.path())
        .permission_policy(Recording(descriptions.clone()))
        .build()
        .unwrap();
    a.run("go").await.unwrap();
    let d = descriptions.lock().unwrap();
    assert!(d[1].contains("alias tidy='rm -rf"), "{d:?}");
    assert!(victim.exists(), "the denied command did not run");
    assert!(tool_results(&a)[1].contains("Permission denied"));
}

#[tokio::test]
async fn a_reduced_bash_output_is_readable_after_restore() {
    use bricks_memory::{JsonlMemory, Memory};
    let work = tempfile::tempdir().unwrap();
    let mem = tempfile::tempdir().unwrap();
    let cmd = "for i in $(seq 1 3000); do echo \"   Compiling crate$i v0.1.0\"; [ $i = 1500 ] && echo 'error[E0425]: cannot find value `zz`' >&2; done; exit 101";
    let url = serve(vec![
        tool_call("r1", "Bash", json!({ "command": cmd })),
        text("seen"),
        text("seen"),
        text("restored"),
    ]);
    {
        let a = Agent::builder()
            .provider(common::provider(&url, "chat_completions", 100_000))
            .tools(bricks_tools::shell_with_jobs(JobRegistry::new(Default::default(), None)))
            .working_dir(work.path())
            .memory(JsonlMemory::new(mem.path()))
            .session_id("rs")
            .compression_level(CompressionLevel::Minimal)
            .build()
            .unwrap();
        a.run("build").await.unwrap();
    }
    let b = Agent::builder()
        .provider(common::provider(&url, "chat_completions", 100_000))
        .working_dir(work.path())
        .memory(JsonlMemory::new(mem.path()))
        .session_id("rs")
        .build()
        .unwrap();
    b.run("again").await.unwrap();
    let reduced = tool_results(&b).into_iter().next().unwrap();
    assert!(
        reduced.starts_with("✗ [Bash] Failed (") && reduced.contains("— code 101"),
        "{reduced}"
    );
    assert!(reduced.contains("error[E0425]"), "{reduced}");
    let path = reduced
        .split("Read file_path=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap();
    assert!(
        path.starts_with(
            JsonlMemory::new(mem.path())
                .session_files_dir("rs")
                .unwrap()
                .to_str()
                .unwrap()
        )
    );
    let raw = std::fs::read_to_string(path).unwrap();
    assert!(
        raw.contains("   Compiling crate3000 v0.1.0") && raw.contains("--- stderr ---"),
        "{}",
        raw.len()
    );
}

#[tokio::test]
async fn cancelling_a_turn_stops_the_running_command() {
    let work = tempfile::tempdir().unwrap();
    let pidfile = work.path().join("pid");
    let cmd = format!(
        "sh -c 'echo $$ > \"{}\"; exec sleep 300'",
        pidfile.display()
    );
    let url = serve(vec![
        tool_call("z1", "Bash", json!({ "command": cmd })),
        text("ok"),
    ]);
    let token = tokio_util::sync::CancellationToken::new();
    let a = Arc::new(
        agent(&url, work.path())
            .cancel_token(token.clone())
            .build()
            .unwrap(),
    );
    let runner = {
        let a = a.clone();
        tokio::spawn(async move { a.run("go").await })
    };
    let mut pid = None;
    for _ in 0..200 {
        if let Ok(t) = std::fs::read_to_string(&pidfile)
            && let Ok(p) = t.trim().parse::<u32>()
        {
            pid = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let pid = pid.expect("the command started");
    token.cancel();
    assert!(matches!(runner.await.unwrap(), Err(BricksError::Cancelled)));
    assert!(
        procs::wait_gone(&[pid], Duration::from_secs(5)).await,
        "pid {pid} survived the cancel"
    );
}

/// The bash analysis of `tool_primitives::bash_safety` is an SDK primitive:
/// a command it rates `Safe` still goes through the approval rules of the
/// real Bash tool (level `execute`), for `ask` as for `deny`.
#[tokio::test]
async fn a_command_rated_safe_still_needs_the_approval_rules() {
    use bricks_agent::control::{ApprovalBroker, BrokerApprover};
    use bricks_agent::permission::{PermissionEffect, PermissionProfile, SessionPolicy};
    use bricks_tools::tool_primitives::bash_safety::is_safe;

    let command = "ls > /dev/null";
    assert!(is_safe(command));
    // The session's policy says ask (nobody can answer here) or deny.
    let ask = SessionPolicy::chosen(PermissionProfile::Ask);
    let mut deny = SessionPolicy::chosen(PermissionProfile::Custom);
    deny.custom.execute = PermissionEffect::Deny;
    for (name, policy) in [("ask", ask), ("deny", deny)] {
        let work = tempfile::tempdir().unwrap();
        let url = serve(vec![
            tool_call("s1", "Bash", json!({ "command": command })),
            tool_call("s2", "Bash", json!({ "command": "touch ran" })),
            text("ok"),
        ]);
        // Non-interactive: an `ask` cannot be answered and is refused.
        let broker = ApprovalBroker::new(false);
        let a = agent(&url, work.path())
            .permission_profile(policy)
            .permission_policy(BrokerApprover::new(broker.clone()))
            .build()
            .unwrap();
        let _ = a.run("go").await;
        let results = tool_results(&a);
        assert!(!results.is_empty(), "{name}");
        assert!(
            results.iter().all(|r| r.contains("Permission denied")),
            "{name}: {results:?}"
        );
        assert!(!work.path().join("ran").exists(), "{name}: a command ran");
        if name == "ask" {
            let asked = broker.unsatisfied();
            assert_eq!(asked[0].tool, "Bash");
            assert_eq!(asked[0].level, "execute");
        }
    }
}
