//! Jobs through a real agent: how the agent assembles its job registry, what
//! a sub-agent sharing the session's registry can see, and the removed task
//! tool names (scripted model, real processes).

#![cfg(unix)]

mod common;

use bricks_agent::Agent;
use bricks_tools::Extensions;
use bricks_tools::jobs::{JobRegistry, JobSettings, JobTool, JobsHandle, NO_ACCESSIBLE_JOB};
use bricks_types::*;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

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
    serve_with(move |_request| it.next().unwrap_or_else(|| text("done")))
}

/// Serves what `next` returns for each request, given the request body so
/// far (a reply can name an id that an earlier result gave).
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

/// The job ids named in `text`, each once, in order of appearance.
fn job_ids_in(text: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for m in regex::Regex::new(r"job_[0-9a-f]{12}").unwrap().find_iter(text) {
        if !ids.iter().any(|i| i == m.as_str()) {
            ids.push(m.as_str().to_string());
        }
    }
    ids
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

/// A provider that is never called: these tests only build agents, or run
/// scripted ones on `serve`.
const UNUSED: &str = "http://127.0.0.1:9/v1";

fn builder(url: &str, work: &std::path::Path) -> bricks_agent::AgentBuilder {
    Agent::builder()
        .provider(common::provider(url, "chat_completions", 100_000))
        .working_dir(work)
        .raw_output_dir(work.join(".raw"))
}

fn registry() -> Arc<JobRegistry> {
    JobRegistry::new(
        JobSettings {
            stop_grace_ms: 300,
            ..Default::default()
        },
        None,
    )
}

// ─── Assembly ────────────────────────────────────────────────────────────────

#[test]
fn tools_on_two_registries_without_a_handle_are_refused_at_build() {
    let work = tempfile::tempdir().unwrap();
    let mut tools = bricks_tools::shell_with_jobs(registry());
    tools.push(Box::new(JobTool::new(registry())));
    match builder(UNUSED, work.path()).tools(tools).build() {
        Err(BricksError::Config(msg)) => {
            assert!(msg.contains("`Job`"), "{msg}");
            assert!(msg.contains("another job registry"), "{msg}");
        }
        Err(e) => panic!("wrong error: {e:?}"),
        Ok(_) => panic!("two registries were assembled"),
    }
}

#[test]
fn a_handle_in_the_extensions_takes_every_job_tool() {
    let work = tempfile::tempdir().unwrap();
    let session = registry();
    let ext = Extensions::default();
    ext.insert(JobsHandle(Arc::clone(&session)));
    // Tools on another registry: the session's one wins.
    let agent = builder(UNUSED, work.path())
        .tools(bricks_tools::shell_with_jobs(registry()))
        .extensions(ext.clone())
        .build();
    assert!(agent.is_ok(), "a handle assembles the tools");
    let handle = ext.get::<JobsHandle>().expect("the handle");
    assert!(Arc::ptr_eq(&handle.0, &session), "the handle is the session's");
}

#[tokio::test]
async fn tools_without_a_handle_bring_their_registry_and_the_agent_installs_one() {
    let work = tempfile::tempdir().unwrap();
    let url = serve(vec![
        tool_call("c1", "Bash", json!({ "command": "sleep 30", "background": true })),
        text("ok"),
    ]);
    let ext = Extensions::default();
    let agent = builder(&url, work.path())
        .tools(bricks_tools::coding())
        .extensions(ext.clone())
        .build()
        .unwrap();
    let handle = ext.get::<JobsHandle>().expect("a handle is installed for coding()");
    agent.run("go").await.unwrap();
    // The background job went to the installed registry, and the agent's
    // Bash is the one on it.
    let views = handle.0.views(None);
    assert_eq!(views.len(), 1, "{:?}", tool_results(&agent));
    assert!(tool_results(&agent)[0].contains(&views[0].id));
    handle.0.stop_all().await;
}

#[test]
fn the_plain_shell_gets_no_registry_and_no_handle() {
    let work = tempfile::tempdir().unwrap();
    let ext = Extensions::default();
    let agent = builder(UNUSED, work.path())
        .tools(bricks_tools::shell())
        .extensions(ext.clone())
        .build();
    assert!(agent.is_ok());
    assert!(ext.get::<JobsHandle>().is_none());
}

// ─── Sub-agents on the session's registry ────────────────────────────────────

/// Two agents on one session registry, as a sub-agent is: the parent starts
/// a background and a handed-over job; the sub-agent sees neither, and an
/// unknown id gives the same answer.
#[tokio::test]
async fn jobs_of_both_kinds_are_followed_by_their_owner_and_not_by_another_agent() {
    let work = tempfile::tempdir().unwrap();
    let session = registry();
    let ext = Extensions::default();
    ext.insert(JobsHandle(Arc::clone(&session)));

    let mut step = 0;
    let url_a = serve_with(move |request| {
        let ids = job_ids_in(request);
        let reply = match step {
            0 => tool_call("a1", "Bash", json!({ "command": "sleep 30", "background": true })),
            1 => tool_call(
                "a2",
                "Bash",
                json!({ "command": "sleep 30", "yield_time_ms": 100 }),
            ),
            2 => tool_call("a3", "Job", json!({})),
            3 => tool_call("a4", "Job", json!({ "action": "stop", "job_id": ids[0] })),
            4 => tool_call("a5", "Job", json!({ "action": "stop", "job_id": ids[1] })),
            _ => text("ok"),
        };
        step += 1;
        reply
    });
    let a = builder(&url_a, work.path())
        .tools(bricks_tools::shell())
        .extensions(ext.clone())
        .build()
        .unwrap();
    a.run("start").await.unwrap();
    let r = tool_results(&a);
    let ids: Vec<String> = session.views(None).iter().map(|v| v.id.clone()).collect();
    assert_eq!(ids.len(), 2, "{r:?}");
    assert!(r[0].contains(&ids[0]), "{}", r[0]);
    assert!(r[1].starts_with("… [Bash] Running"), "{}", r[1]);
    assert!(r[1].contains(&ids[1]), "{}", r[1]);
    assert!(r[2].contains(&format!("{} [running]", ids[0])), "{}", r[2]);
    assert!(r[2].contains(&format!("{} [running]", ids[1])), "{}", r[2]);
    assert!(r[3].contains(&format!("{} [stopped]", ids[0])), "{}", r[3]);
    assert!(r[4].contains(&format!("{} [stopped]", ids[1])), "{}", r[4]);
    session.stop_all().await;

    // The sub-agent, on the same session registry, looks at the parent's ids.
    let parent_ids = ids.clone();
    // Its own map, with the session's registry only: a shared map would hand
    // it the parent's identity (the runner reads the last identity inserted).
    let ext_b = Extensions::default();
    ext_b.insert(JobsHandle(Arc::clone(&session)));
    let mut step_b = 0;
    let url_b = serve_with(move |_request| {
        let reply = match step_b {
            0 => tool_call("b1", "Job", json!({ "action": "status", "job_id": parent_ids[0] })),
            1 => tool_call("b2", "Job", json!({ "action": "output", "job_id": parent_ids[1] })),
            2 => tool_call("b3", "Job", json!({ "action": "wait", "job_id": parent_ids[0], "timeout_ms": 50 })),
            3 => tool_call("b4", "Job", json!({ "action": "stop", "job_id": "job_000000000000" })),
            4 => tool_call("b5", "Job", json!({})),
            _ => text("ok"),
        };
        step_b += 1;
        reply
    });
    let b = builder(&url_b, work.path())
        .tools(bricks_tools::shell())
        .extensions(ext_b)
        .build()
        .unwrap();
    b.run("look").await.unwrap();
    let rb = tool_results(&b);
    for (i, r) in rb.iter().take(4).enumerate() {
        assert!(r.starts_with("✗ [Job] Failed"), "{i}: {r}");
        assert!(r.contains(NO_ACCESSIBLE_JOB), "{i}: {r}");
    }
    assert!(rb[4].contains("You have no jobs"), "{}", rb[4]);
}

// ─── Removed task tools ──────────────────────────────────────────────────────

#[tokio::test]
async fn the_removed_task_tools_are_unknown_to_an_agent() {
    let work = tempfile::tempdir().unwrap();
    let url = serve(vec![
        tool_call("u1", "BashTaskOutput", json!({ "task_id": "bg-1" })),
        tool_call("u2", "BashTaskStatus", json!({})),
        tool_call("u3", "BashTaskStop", json!({ "task_id": "bg-1" })),
        text("ok"),
    ]);
    let agent = builder(&url, work.path())
        .tools(bricks_tools::coding())
        .build()
        .unwrap();
    agent.run("go").await.unwrap();
    let r = tool_results(&agent);
    assert_eq!(r.len(), 3, "{r:?}");
    for (name, result) in ["BashTaskOutput", "BashTaskStatus", "BashTaskStop"].iter().zip(&r) {
        assert!(
            result.starts_with(&format!("✗ [{name}]")),
            "{name}: {result}"
        );
        assert!(
            result.contains(&format!("Tool '{name}' not found.")),
            "{name}: {result}"
        );
        assert!(!result.contains("bg-1 ["), "{name} ran nothing: {result}");
    }
    // The listed alternatives do not offer the removed names.
    let listed = r[0]
        .lines()
        .find(|l| l.starts_with("Available tools"))
        .expect("the list of tools");
    assert!(!listed.contains("BashTask"), "{}", r[0]);
}
