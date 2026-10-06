//! The shell tools through a real agent (scripted model, real processes).

#![cfg(unix)]

mod common;

use async_trait::async_trait;
use cersei_agent::events::AgentEvent;
use cersei_agent::Agent;
use cersei_compression::CompressionLevel;
use cersei_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use cersei_tools::shell::{procs, ShellConfig};
use cersei_types::*;
use serde_json::{json, Value};
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut it = replies.into_iter();
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
            let body = it.next().unwrap_or_else(|| text("done"));
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

fn shell_extensions(progress_every: Duration) -> cersei_tools::Extensions {
    let ext = cersei_tools::Extensions::default();
    ext.insert(ShellConfig {
        progress_every,
        term_grace: Duration::from_millis(500),
        ..ShellConfig::default()
    });
    ext
}

fn agent(url: &str, work: &std::path::Path) -> cersei_agent::AgentBuilder {
    Agent::builder()
        .provider(common::provider(url, "chat_completions", 100_000))
        .tools(cersei_tools::shell())
        .working_dir(work)
        .raw_output_dir(work.join(".raw"))
        .max_turns(8)
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

    assert!(r[0].starts_with("✓ [Bash] Succès ("), "{}", r[0]);
    assert!(
        r[0].ends_with(") — code 0\n(Commande exécutée avec succès sans sortie)"),
        "{}",
        r[0]
    );
    assert!(
        r[1].contains("--- stdout ---\nBricks42\nsub dir"),
        "{}",
        r[1]
    );
    let fail = &r[2];
    assert!(fail.starts_with("✗ [Bash] Échec ("), "{fail}");
    assert!(fail.contains(") — code 127\n--- stderr ---\n"), "{fail}");
    assert!(fail.contains("command not found"), "{fail}");
    assert!(
        fail.contains("--- suggestion ---\nVérifiez que l'outil est installé"),
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
        t.starts_with("⏱ [Bash] Interrompu après timeout de 0.6s ("),
        "{t}"
    );
    assert!(t.contains("aucun code de sortie"), "{t}");
    assert!(!t.contains("code 0") && !t.contains("code -1"), "{t}");
    assert!(
        t.contains("[Interrompu après timeout de 0.6s — sortie partielle ci-dessous]"),
        "{t}"
    );
    assert!(
        t.contains("--- stdout ---\npartial") && t.contains("--- stderr ---\nerr-part"),
        "{t}"
    );
    assert!(r[1].contains("alive"), "the session stays usable: {}", r[1]);
}

#[tokio::test]
async fn background_tasks_through_the_agent() {
    let work = tempfile::tempdir().unwrap();
    let url = serve(vec![
        tool_call(
            "b1",
            "Bash",
            json!({
                "command": "python3 -u -m http.server 0 --bind 127.0.0.1",
                "background": true,
                "ready_pattern": "port \\d+",
                "ready_timeout": 15000
            }),
        ),
        tool_call("b2", "BashTaskOutput", json!({ "task_id": "bg-1" })),
        tool_call("b3", "BashTaskStatus", json!({})),
        tool_call("b4", "BashTaskStop", json!({ "task_id": "bg-1" })),
        tool_call("b5", "BashTaskStop", json!({ "task_id": "bg-1" })),
        tool_call("b6", "BashTaskOutput", json!({ "task_id": "bg-99" })),
        text("ok"),
    ]);
    let a = agent(&url, work.path()).build().unwrap();
    a.run("serve").await.unwrap();
    let r = tool_results(&a);
    assert!(r[0].starts_with("… [Bash] En cours ("), "{}", r[0]);
    assert!(
        r[0].contains("Tâche bg-1 démarrée") && r[0].contains("prête après"),
        "{}",
        r[0]
    );
    let port: u16 = regex::Regex::new(r"port (\d+)")
        .unwrap()
        .captures(&r[1])
        .expect(&r[1])[1]
        .parse()
        .unwrap();
    assert!(
        r[1].contains("     1 | Serving HTTP") || r[1].contains("1 | Serving HTTP"),
        "{}",
        r[1]
    );
    assert!(r[2].contains("bg-1 [running]"), "{}", r[2]);
    assert!(r[3].contains("Arrêtée : bg-1 [stopped]"), "{}", r[3]);
    assert!(r[4].contains("Déjà terminée"), "{}", r[4]);
    assert!(r[5].starts_with("✗ [BashTaskOutput]"), "{}", r[5]);
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
    use cersei_memory::{JsonlMemory, Memory};
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
            .tools(cersei_tools::shell())
            .working_dir(work.path())
            .memory(JsonlMemory::new(mem.path()))
            .session_id("rs")
            .compression_level(CompressionLevel::Minimal)
            .max_turns(3)
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
        reduced.starts_with("✗ [Bash] Échec (") && reduced.contains("— code 101"),
        "{reduced}"
    );
    assert!(reduced.contains("error[E0425]"), "{reduced}");
    let path = reduced
        .split("Read file_path=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap();
    assert!(path.starts_with(
        JsonlMemory::new(mem.path())
            .session_files_dir("rs")
            .unwrap()
            .to_str()
            .unwrap()
    ));
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
        if let Ok(t) = std::fs::read_to_string(&pidfile) {
            if let Ok(p) = t.trim().parse::<u32>() {
                pid = Some(p);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let pid = pid.expect("the command started");
    token.cancel();
    assert!(matches!(runner.await.unwrap(), Err(CerseiError::Cancelled)));
    assert!(
        procs::wait_gone(&[pid], Duration::from_secs(5)).await,
        "pid {pid} survived the cancel"
    );
}
