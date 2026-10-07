//! The 10.5 runtime through the real controller: concurrent fan-out,
//! bounded recursion, background agents, isolated ChangeSets, correlation,
//! skills, usage aggregation, permissions and jobs.
//!
//! The parent follows a script; children are driven by markers in their
//! task (`[barrier]`, `[delegate:…]`, `[write:file=content]`, `[fail]`,
//! `[hold]`), with fixed usage (7 in / 3 out per response).

use cersei_agent::agents::{InstanceState, ProfileSources};
use cersei_agent::control::scripted::{Reply, Script, ScriptedProvider};
use cersei_agent::control::*;
use cersei_agent::BricksConfig;
use cersei_provider::{CompletionRequest, CompletionStream, Provider};
use cersei_types::{Message, MessageContent, Role, Usage};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ─── Test model ──────────────────────────────────────────────────────────────

#[derive(Default)]
struct Shared {
    barrier: parking_lot::Mutex<Option<Arc<tokio::sync::Barrier>>>,
    hold: tokio::sync::Notify,
    released: std::sync::atomic::AtomicBool,
    builds: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
    child_requests: parking_lot::Mutex<Vec<CompletionRequest>>,
}

impl Shared {
    /// Let every `[hold]` child (present or future) go on.
    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.hold.notify_waiters();
    }
}

struct TestProvider {
    selection: String,
    parent: Arc<Script>,
    shared: Arc<Shared>,
}

fn is_child(r: &CompletionRequest) -> bool {
    r.system
        .as_deref()
        .is_some_and(|s| s.contains("You are a sub-agent"))
}

fn first_user_text(r: &CompletionRequest) -> String {
    r.messages
        .iter()
        .find(|m| m.role == Role::User)
        .and_then(|m| m.get_text().map(str::to_string))
        .unwrap_or_default()
}

fn has_tool_result(r: &CompletionRequest) -> bool {
    r.messages.iter().any(|m| match &m.content {
        MessageContent::Blocks(b) => b
            .iter()
            .any(|x| matches!(x, cersei_types::ContentBlock::ToolResult { .. })),
        _ => false,
    })
}

fn marker<'a>(task: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("[{name}:");
    let i = task.find(&open)? + open.len();
    let j = task[i..].find(']')? + i;
    Some(&task[i..j])
}

fn child_usage() -> Usage {
    Usage {
        input_tokens: 7,
        output_tokens: 3,
        ..Default::default()
    }
}

#[async_trait::async_trait]
impl Provider for TestProvider {
    fn name(&self) -> &str {
        "test"
    }
    fn context_window(&self, _: &str) -> u64 {
        100_000
    }
    fn model_info(&self) -> Option<cersei_provider::ModelInfo> {
        ScriptedProvider::new(&self.selection, self.parent.clone()).model_info()
    }
    async fn complete(&self, req: CompletionRequest) -> cersei_types::Result<CompletionStream> {
        if !is_child(&req) {
            return ScriptedProvider::new(&self.selection, self.parent.clone())
                .complete(req)
                .await;
        }
        self.shared.child_requests.lock().push(req.clone());
        let task = first_user_text(&req);
        let n = self.shared.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared.peak.fetch_max(n, Ordering::SeqCst);
        let _guard = scopeguard(Arc::clone(&self.shared));
        if task.contains("[fail]") {
            return Err(cersei_types::CerseiError::Provider(
                "scripted failure".into(),
            ));
        }
        if task.contains("[barrier]") {
            let b = self.shared.barrier.lock().clone();
            if let Some(b) = b {
                b.wait().await;
            }
        }
        let held = (task.contains("[hold]") && !has_tool_result(&req))
            || (task.contains("[hold-after]") && has_tool_result(&req));
        if held {
            loop {
                let woken = self.shared.hold.notified();
                tokio::pin!(woken);
                woken.as_mut().enable();
                if self.shared.released.load(Ordering::SeqCst) {
                    break;
                }
                woken.await;
            }
        }
        let mut reply = if has_tool_result(&req) {
            Reply::text(&format!("done: {}", task.lines().next().unwrap_or("")))
        } else if let Some(inner) = marker(&task, "delegate") {
            Reply::tool("d1", "Agent", json!({ "task": inner.replace('|', ":") }))
        } else if let Some(w) = marker(&task, "write") {
            let (file, content) = w.split_once('=').unwrap_or((w, "x"));
            Reply::tool(
                "w1",
                "Write",
                json!({ "file_path": file, "content": format!("{content}\n") }),
            )
        } else if task.contains("[glob]") {
            Reply::tool("g1", "Glob", json!({ "pattern": "*.txt" }))
        } else if let Some(cmd) = marker(&task, "bash") {
            Reply::tool("b1", "Bash", json!({ "command": cmd }))
        } else {
            Reply::text(&format!("result of {}", task.lines().next().unwrap_or("")))
        };
        reply.usage = Some(child_usage());
        ScriptedProvider::new(&self.selection, Script::new(vec![reply]))
            .complete(req)
            .await
    }
}

struct ActiveGuard(Arc<Shared>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
fn scopeguard(s: Arc<Shared>) -> ActiveGuard {
    ActiveGuard(s)
}

struct TestCatalog {
    inner: Arc<cersei_agent::control::scripted::ScriptedCatalog>,
    shared: Arc<Shared>,
}

impl ModelCatalog for TestCatalog {
    fn models(&self) -> Vec<ModelChoice> {
        self.inner.models()
    }
    fn build(&self, selection: &str, reasoning: Option<&str>) -> Result<Box<dyn Provider>, String> {
        self.inner.build(selection, reasoning)?;
        self.shared.builds.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(TestProvider {
            selection: selection.into(),
            parent: self.inner.script.clone(),
            shared: Arc::clone(&self.shared),
        }))
    }
}

// ─── Harness ─────────────────────────────────────────────────────────────────

struct Env {
    ctl: Controller,
    events: EventStream,
    dir: tempfile::TempDir,
    shared: Arc<Shared>,
}

fn git(dir: &Path, args: &[&str]) {
    let o = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

fn allow(tools: &[&str]) -> ApprovalRules {
    let mut r = ApprovalRules::default();
    for t in tools {
        r.tools.insert(t.to_string(), Action::Allow);
    }
    r
}

async fn open(
    replies: Vec<Reply>,
    rules: ApprovalRules,
    interactive: bool,
    tweak: impl FnOnce(&mut BricksConfig),
) -> Env {
    let dir = tempfile::tempdir().unwrap();
    // A repository with one commit (worktrees need one).
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("shared.txt"), "base\n").unwrap();
    std::fs::write(dir.path().join(".gitignore"), ".sessions/\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let script = Script::new(replies);
    let shared = Arc::new(Shared::default());
    let catalog = Arc::new(TestCatalog {
        inner: cersei_agent::control::scripted::ScriptedCatalog::new(&["a", "b"], script),
        shared: Arc::clone(&shared),
    });
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.permissions = rules;
    bricks.semantic.lsp.enabled = false;
    bricks.agents.admission_timeout_ms = 20_000;
    tweak(&mut bricks);
    let mut cfg = EngineConfig::new(dir.path(), catalog, bricks, dir.path().join(".sessions"));
    cfg.interactive = interactive;
    cfg.agent_profile_sources = Some(ProfileSources {
        project_dir: Some(dir.path().join(".bricks/agents")),
        user_dir: None,
    });
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    next(&mut events).await;
    Env {
        ctl,
        events,
        dir,
        shared,
    }
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(30), events.next())
        .await
        .expect("an event within 30 s")
        .expect("stream open")
}

async fn until_finished(events: &mut EventStream) -> Vec<Envelope> {
    let mut out = Vec::new();
    loop {
        let e = next(events).await;
        let done = matches!(e.event, Event::RunFinished { .. });
        out.push(e);
        if done {
            return out;
        }
    }
}

/// Events until `pred` holds for one of them (inclusive).
async fn until(events: &mut EventStream, pred: impl Fn(&Event) -> bool) -> Vec<Envelope> {
    let mut out = Vec::new();
    loop {
        let e = next(events).await;
        let done = pred(&e.event);
        out.push(e);
        if done {
            return out;
        }
    }
}

fn spawned(evs: &[Envelope]) -> Vec<cersei_agent::agents::SpawnInfo> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::AgentSpawned { agent } => Some((**agent).clone()),
            _ => None,
        })
        .collect()
}

fn results(evs: &[Envelope]) -> Vec<cersei_agent::agents::AgentResult> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::AgentFinished { result } => Some((**result).clone()),
            _ => None,
        })
        .collect()
}

fn tool_out(evs: &[Envelope], id: &str) -> (bool, String) {
    evs.iter()
        .find_map(|e| match &e.event {
            Event::ToolFinished {
                tool_call_id,
                is_error,
                output,
                ..
            } if tool_call_id == id => Some((*is_error, output.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no result for {id}"))
}

async fn submit(env: &mut Env, text: &str) -> Vec<Envelope> {
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text(text),
        })
        .unwrap();
    until_finished(&mut env.events).await
}

// ─── Fan-out and ordering ────────────────────────────────────────────────────

#[tokio::test]
async fn agents_run_children_at_the_same_time_and_answer_in_order() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agents",
                json!({ "agents": [
                    { "task": "T0 [barrier]", "profile": "inspecteur" },
                    { "task": "T1 [barrier]", "profile": "web_searcher" },
                    { "task": "T2 [barrier]", "profile": "testeur" }
                ]}),
            ),
            Reply::text("synthesis"),
        ],
        allow(&["Agents"]),
        true,
        |_| {},
    )
    .await;
    // Three children must reach the barrier together: sequential runs
    // would wait forever.
    *env.shared.barrier.lock() = Some(Arc::new(tokio::sync::Barrier::new(3)));
    let evs = submit(&mut env, "go").await;
    assert!(
        env.shared.peak.load(Ordering::SeqCst) >= 3,
        "overlap demonstrated"
    );
    let (err, out) = tool_out(&evs, "c1");
    assert!(!err, "{out}");
    let p0 = out.find("result of T0").unwrap();
    let p1 = out.find("result of T1").unwrap();
    let p2 = out.find("result of T2").unwrap();
    assert!(p0 < p1 && p1 < p2, "results in the order asked");
    // Correlation: one parent call, positions, parallel → worktrees.
    let sp = spawned(&evs);
    assert_eq!(sp.len(), 3);
    assert!(sp.iter().all(|s| s.tool_call_id.as_deref() == Some("c1")));
    let mut idx: Vec<usize> = sp.iter().filter_map(|s| s.batch_index).collect();
    idx.sort();
    assert_eq!(idx, vec![0, 1, 2]);
    assert!(sp.iter().all(|s| s.isolation == "worktree" && s.depth == 1));
    // Usage: parent 2 × (100/10), children 3 × (7/3), counted once.
    // Published just before `run_finished`.
    let n = evs.len();
    assert!(matches!(evs[n - 2].event, Event::RunUsage { .. }));
    let ru = evs
        .iter()
        .find_map(|e| match &e.event {
            Event::RunUsage {
                own,
                descendants,
                total,
                final_total,
                ..
            } => Some((
                own.clone(),
                descendants.clone(),
                total.clone(),
                *final_total,
            )),
            _ => None,
        })
        .unwrap();
    assert_eq!((ru.0.input_tokens, ru.0.output_tokens), (200, 20));
    assert_eq!((ru.1.input_tokens, ru.1.output_tokens), (21, 9));
    assert_eq!((ru.2.input_tokens, ru.2.output_tokens), (221, 29));
    assert!(ru.3, "nothing left running: final");
}

#[tokio::test]
async fn two_agent_calls_in_one_turn_carry_their_own_call_ids() {
    let mut env = open(
        vec![
            Reply::tools(vec![
                ("call_a", "Agent", json!({ "task": "AAA" })),
                ("call_b", "Agent", json!({ "task": "BBB" })),
            ]),
            Reply::text("ok"),
        ],
        allow(&["Agent"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let sp = spawned(&evs);
    assert_eq!(sp.len(), 2);
    for s in sp {
        let want = if s.task.starts_with("AAA") {
            "call_a"
        } else {
            "call_b"
        };
        assert_eq!(s.tool_call_id.as_deref(), Some(want), "{}", s.task);
    }
}

// ─── Recursion and limits ────────────────────────────────────────────────────

#[tokio::test]
async fn a_chain_runs_with_a_single_slot() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "level1 [delegate:level2 leaf]" }),
            ),
            Reply::text("chain done"),
        ],
        allow(&["Agent"]),
        true,
        |b| {
            b.agents.max_concurrent = 1;
        },
    )
    .await;
    let evs = tokio::time::timeout(Duration::from_secs(20), submit(&mut env, "go"))
        .await
        .expect("no circular wait with max_concurrent = 1");
    let sp = spawned(&evs);
    assert_eq!(sp.len(), 2);
    assert_eq!(sp[0].depth, 1);
    assert_eq!(sp[1].depth, 2);
    assert_eq!(sp[1].parent_id.as_deref(), Some(sp[0].agent_id.as_str()));
    let rs = results(&evs);
    assert!(rs.iter().all(|r| r.status == "completed"), "{rs:?}");
    assert_eq!(
        env.ctl
            .agent_runtime()
            .unwrap()
            .scheduler
            .stats()
            .peak_active,
        1
    );
}

#[tokio::test]
async fn depth_and_total_limits_refuse_before_any_provider() {
    // Depth: max_depth = 1, the child tries to delegate.
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "level1 [delegate:too deep]" }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agent"]),
        true,
        |b| b.agents.max_depth = 1,
    )
    .await;
    let builds_before = env.shared.builds.load(Ordering::SeqCst);
    let evs = submit(&mut env, "go").await;
    assert_eq!(spawned(&evs).len(), 1, "no grandchild");
    assert_eq!(
        env.shared.builds.load(Ordering::SeqCst),
        builds_before + 1,
        "only the child's provider"
    );
    let child_tool = evs.iter().find_map(|e| match &e.event {
        Event::AgentToolFinished { name, is_error, .. } if name == "Agent" => Some(*is_error),
        _ => None,
    });
    assert_eq!(child_tool, Some(true));
    let reqs = env.shared.child_requests.lock().clone();
    let fed = serde_json::to_string(&reqs.last().unwrap().messages).unwrap();
    assert!(fed.contains("AgentDepthExceeded"), "{fed}");

    // Total: 3 asked, 2 allowed per run: the whole batch is refused.
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agents",
                json!({ "agents": [{"task":"a"},{"task":"b"},{"task":"c"}] }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agents"]),
        true,
        |b| b.agents.max_total_per_run = 2,
    )
    .await;
    let builds_before = env.shared.builds.load(Ordering::SeqCst);
    let evs = submit(&mut env, "go").await;
    let (err, out) = tool_out(&evs, "c1");
    assert!(err && out.contains("AgentTotalExceeded"), "{out}");
    assert!(spawned(&evs).is_empty());
    assert_eq!(env.shared.builds.load(Ordering::SeqCst), builds_before);
    assert!(env.shared.child_requests.lock().is_empty());
}

#[tokio::test]
async fn fail_fast_cancels_the_siblings_and_keeps_their_states() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agents",
                json!({ "fail_fast": true, "agents": [
                    { "task": "bad [fail]" },
                    { "task": "slow one [hold]" },
                    { "task": "slow two [hold]" }
                ]}),
            ),
            Reply::text("handled"),
        ],
        allow(&["Agents"]),
        true,
        |_| {},
    )
    .await;
    let evs = tokio::time::timeout(Duration::from_secs(20), submit(&mut env, "go"))
        .await
        .expect("fail_fast does not wait for the held siblings");
    let rs = results(&evs);
    assert_eq!(rs.len(), 3);
    let by = |t: &str| {
        let id = spawned(&evs)
            .into_iter()
            .find(|s| s.task.starts_with(t))
            .unwrap()
            .agent_id;
        rs.iter().find(|r| r.agent_id == id).unwrap().status.clone()
    };
    assert_eq!(by("bad"), "failed");
    assert_eq!(by("slow one"), "cancelled");
    assert_eq!(by("slow two"), "cancelled");
    // Without fail_fast, one failure leaves the others' successes.
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agents",
                json!({ "agents": [{ "task": "bad [fail]" }, { "task": "good" }] }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agents"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let mut st: Vec<String> = results(&evs).into_iter().map(|r| r.status).collect();
    st.sort();
    assert_eq!(st, vec!["completed", "failed"]);
}

#[tokio::test]
async fn a_full_queue_is_an_explicit_individual_result() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agents", json!({ "agents": [{ "task": "one [hold]" }, { "task": "two [hold]" }, { "task": "three [hold]" }] })),
            Reply::text("ok"),
        ],
        allow(&["Agents"]),
        true,
        |b| {
            b.agents.max_concurrent = 1;
            b.agents.max_queued = 1;
        },
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    // Every child holds: one runs, one waits, the last is refused,
    // whatever the order they arrive in.
    let evs = until(
        &mut env.events,
        |e| matches!(e, Event::AgentFinished { result } if result.status == "failed"),
    )
    .await;
    let failed = results(&evs);
    assert!(
        failed[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("AgentQueueFull")),
        "{failed:?}"
    );
    env.shared.release();
    let rest = until_finished(&mut env.events).await;
    let all: Vec<_> = results(&evs).into_iter().chain(results(&rest)).collect();
    assert_eq!(all.len(), 3);
    assert_eq!(all.iter().filter(|r| r.status == "completed").count(), 2);
}

// ─── Background ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_background_agent_outlives_the_answer_and_totals_become_final() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "long work [hold]", "background": true }),
            ),
            Reply::text("started it"),
        ],
        allow(&["Agent"]),
        true,
        |_| {},
    )
    .await;
    let t = std::time::Instant::now();
    let evs = submit(&mut env, "go").await;
    assert!(
        t.elapsed() < Duration::from_secs(10),
        "the handle came back at once"
    );
    let (err, out) = tool_out(&evs, "c1");
    assert!(!err && out.contains("started in the background"), "{out}");
    let sp = spawned(&evs);
    assert!(sp[0].background && sp[0].isolation == "worktree");
    // The answer ended the run, not the child.
    let rt = env.ctl.agent_runtime().unwrap();
    assert_eq!(rt.pending(None).len(), 1);
    let p = evs.iter().find_map(|e| match &e.event {
        Event::RunUsage {
            final_total,
            pending_agents,
            ..
        } => Some((*final_total, *pending_agents)),
        _ => None,
    });
    assert_eq!(p, Some((false, 1)), "partial while the child runs");
    // Release it: it finishes without waking the parent's loop.
    let requests_before = env.shared.child_requests.lock().len();
    env.shared.release();
    let fin = until(&mut env.events, |e| {
        matches!(
            e,
            Event::RunUsage {
                final_total: true,
                ..
            }
        )
    })
    .await;
    assert_eq!(results(&fin).len(), 1);
    assert!(!fin.iter().any(|e| matches!(
        e.event,
        Event::RunStarted { .. } | Event::RunFinished { .. }
    )));
    let total = fin.iter().find_map(|e| match &e.event {
        Event::RunUsage { total, .. } => Some(total.input_tokens),
        _ => None,
    });
    assert_eq!(total, Some(200 + 7), "parent 2 × 100 + child 7, once");
    assert!(env.shared.child_requests.lock().len() > requests_before);
}

#[tokio::test]
async fn agent_control_waits_reads_and_cancels_in_scope() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "bg [hold]", "background": true }),
            ),
            Reply::text("started"),
        ],
        allow(&["Agent", "AgentControl"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    let rt = env.ctl.agent_runtime().unwrap();
    // A sub-agent sees only its descendants; the session sees all.
    assert!(rt.visible_to(&id, None));
    assert!(!rt.visible_to(&id, Some("agent_other")));
    // Cancel through the frontend command.
    env.ctl
        .send(Command::AgentControl {
            action: "cancel".into(),
            agent_id: Some(id.clone()),
            changeset_id: None,
            job_id: None,
        })
        .unwrap();
    let evs = until(&mut env.events, |e| {
        matches!(e, Event::AgentFinished { .. })
    })
    .await;
    assert_eq!(results(&evs)[0].status, "cancelled");
    let rec = rt.record(&id).unwrap();
    assert_eq!(rec.state, InstanceState::Cancelled);
    // `result` is idempotent and asks nothing of a model.
    let before = env.shared.child_requests.lock().len();
    for _ in 0..2 {
        env.ctl
            .send(Command::AgentControl {
                action: "result".into(),
                agent_id: Some(id.clone()),
                changeset_id: None,
                job_id: None,
            })
            .unwrap();
        let e = until(&mut env.events, |e| {
            matches!(e, Event::AgentControlResult { .. })
        })
        .await;
        assert!(matches!(
            &e.last().unwrap().event,
            Event::AgentControlResult { ok: true, .. }
        ));
    }
    assert_eq!(env.shared.child_requests.lock().len(), before);
}

#[tokio::test]
async fn a_cancelled_child_keeps_the_usage_it_spent() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "one round [glob] [hold-after]", "background": true }),
            ),
            Reply::text("started"),
        ],
        allow(&["Agent"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    // Its first response (a tool call) came back; it now holds.
    until(&mut env.events, |e| {
        matches!(e, Event::AgentToolFinished { .. })
    })
    .await;
    env.ctl
        .send(Command::AgentControl {
            action: "cancel".into(),
            agent_id: Some(id),
            changeset_id: None,
            job_id: None,
        })
        .unwrap();
    let fin = until(&mut env.events, |e| {
        matches!(
            e,
            Event::RunUsage {
                final_total: true,
                ..
            }
        )
    })
    .await;
    let r = &results(&fin)[0];
    assert_eq!(r.status, "cancelled");
    assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (7, 3));
    let d = fin.iter().find_map(|e| match &e.event {
        Event::RunUsage { descendants, .. } => Some(descendants.input_tokens),
        _ => None,
    });
    assert_eq!(d, Some(7), "the result and the total agree");
}

// ─── Isolated writers and ChangeSets ─────────────────────────────────────────

#[tokio::test]
async fn parallel_writers_get_worktrees_and_changesets_without_last_writer_wins() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agents",
                json!({ "agents": [
                    { "task": "edit A [bash:echo by A > shared.txt]" },
                    { "task": "edit B [bash:echo by B > shared.txt]" }
                ]}),
            ),
            Reply::text("done"),
        ],
        allow(&["Agents", "Bash"]),
        true,
        |_| {},
    )
    .await;
    // The parent has local, uncommitted work: the children must see it.
    std::fs::write(env.dir.path().join("parent_note.txt"), "dirty\n").unwrap();
    let evs = submit(&mut env, "go").await;
    let ready: Vec<_> = evs
        .iter()
        .filter_map(|e| match &e.event {
            Event::ChangesReady { changeset } => Some((**changeset).clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ready.len(), 2, "{:#?}", results(&evs));
    assert_ne!(ready[0].branch, ready[1].branch);
    for cs in &ready {
        assert_eq!(cs.files.len(), 1, "the inherited note is not theirs");
        assert_eq!(cs.files[0].path, "shared.txt");
    }
    // The parent's tree is untouched until an explicit apply.
    let shared = env.dir.path().join("shared.txt");
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), "base\n");
    let send = |action: &str, id: &str| Command::AgentControl {
        action: action.into(),
        agent_id: None,
        changeset_id: Some(id.into()),
        job_id: None,
    };
    env.ctl.send(send("apply_changes", &ready[0].id)).unwrap();
    let e = until(&mut env.events, |e| {
        matches!(e, Event::AgentControlResult { .. })
    })
    .await;
    assert!(matches!(
        &e.last().unwrap().event,
        Event::AgentControlResult { ok: true, .. }
    ));
    let first = std::fs::read_to_string(&shared).unwrap();
    env.ctl.send(send("apply_changes", &ready[1].id)).unwrap();
    let e = until(&mut env.events, |e| {
        matches!(e, Event::AgentControlResult { .. })
    })
    .await;
    match &e.last().unwrap().event {
        Event::AgentControlResult { ok, text, .. } => {
            assert!(!ok && text.contains("WorktreeConflict"), "{text}");
        }
        _ => unreachable!(),
    }
    assert_eq!(
        std::fs::read_to_string(&shared).unwrap(),
        first,
        "no last-writer-wins"
    );
    let conflict = |x: &Event| {
        matches!(
            x,
            Event::ChangesUpdated {
                state: cersei_agent::agents::ChangeSetState::Conflict,
                ..
            }
        )
    };
    if !e.iter().any(|x| conflict(&x.event)) {
        until(&mut env.events, conflict).await;
    }
}

// ─── Skills, permissions ─────────────────────────────────────────────────────

#[tokio::test]
async fn profile_skills_are_loaded_into_the_child() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "use the skill [bash:echo hi]", "profile": "skilled" }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agent", "Bash"]),
        true,
        |_| {},
    )
    .await;
    let root = env.dir.path();
    std::fs::create_dir_all(root.join(".bricks/agents")).unwrap();
    std::fs::write(
        root.join(".bricks/agents/skilled.md"),
        "---\nname: skilled\ndescription: Uses a skill.\nskills: [house-style, no-such-skill]\n---\nBe skilled.\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".claude/skills/house-style")).unwrap();
    std::fs::write(
        root.join(".claude/skills/house-style/SKILL.md"),
        "---\nname: house-style\ndescription: House style.\n---\nSKILL-MARKER-42: write short sentences.\n",
    )
    .unwrap();
    env.ctl.send(Command::ReloadAgentProfiles).unwrap();
    until(&mut env.events, |e| {
        matches!(e, Event::AgentProfiles { .. })
    })
    .await;
    let evs = submit(&mut env, "go").await;
    let rs = results(&evs);
    let states: Vec<(String, String)> = rs[0]
        .skills
        .iter()
        .map(|s| (s.name.clone(), s.state.clone()))
        .collect();
    assert_eq!(
        states,
        vec![
            ("house-style".to_string(), "loaded".to_string()),
            ("no-such-skill".to_string(), "missing".to_string())
        ]
    );
    assert!(rs[0].skills[0].revision.is_some() && rs[0].skills[0].source.is_some());
    // In the first request and the next one (it is not in the history).
    let reqs = env.shared.child_requests.lock().clone();
    assert!(reqs.len() >= 2);
    for r in &reqs {
        let sys = r.system.as_deref().unwrap();
        assert!(sys.contains("SKILL-MARKER-42"));
        assert!(sys.contains("no-such-skill"));
    }
    // No tool was taken away.
    let names: Vec<&str> = reqs[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"Bash") && names.contains(&"Agent"));
}

#[tokio::test]
async fn headless_defaults_refuse_cleanly_and_an_explicit_rule_does_not_allow_the_childs_tools() {
    // Default rules, nobody to ask: delegation is refused, the run says so.
    let mut env = open(
        vec![
            Reply::tool("c1", "Agents", json!({ "agents": [{ "task": "x" }] })),
            Reply::text("ok"),
        ],
        ApprovalRules::default(),
        false,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    assert!(spawned(&evs).is_empty());
    assert!(matches!(
        &evs.last().unwrap().event,
        Event::RunFinished {
            failure: Some(FailureKind::ApprovalRequired),
            ..
        }
    ));
    // `Agents` allowed explicitly: the child's Bash still needs its own rule.
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agents",
                json!({ "agents": [{ "task": "run [bash:echo hi]" }] }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agents"]),
        false,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    assert_eq!(spawned(&evs).len(), 1);
    let bash = evs.iter().find_map(|e| match &e.event {
        Event::AgentToolFinished { name, is_error, .. } if name == "Bash" => Some(*is_error),
        _ => None,
    });
    assert_eq!(bash, Some(true), "refused: no implicit allow");
}

// ─── Jobs ────────────────────────────────────────────────────────────────────

#[cfg(unix)]
#[tokio::test]
async fn background_jobs_are_owned_scoped_and_stopped_as_a_tree() {
    let mut env = open(
        vec![
            Reply::tool(
                "b1",
                "Bash",
                json!({ "command": "sleep 300 & sleep 300 & echo started; wait", "background": true }),
            ),
            Reply::text("started"),
        ],
        allow(&["Bash"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let job = evs
        .iter()
        .find_map(|e| match &e.event {
            Event::JobStarted {
                job_id, cwd, pid, ..
            } => Some((job_id.clone(), cwd.clone(), *pid)),
            _ => None,
        })
        .expect("job_started");
    assert!(job
        .1
        .ends_with(env.dir.path().file_name().unwrap().to_str().unwrap()));
    let (_, out) = tool_out(&evs, "b1");
    assert!(out.contains(&job.0));
    // Its children exist.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let kids = cersei_tools::shell::procs::descendants(job.2);
    assert!(kids.len() >= 2, "{kids:?}");
    // A person stops it: the whole tree goes.
    env.ctl
        .send(Command::AgentControl {
            action: "stop_job".into(),
            agent_id: None,
            changeset_id: None,
            job_id: Some(job.0.clone()),
        })
        .unwrap();
    let e = until(&mut env.events, |e| matches!(e, Event::JobFinished { .. })).await;
    match &e.last().unwrap().event {
        Event::JobFinished { state, .. } => assert_eq!(state, "stopped"),
        _ => unreachable!(),
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    for k in kids {
        assert!(!cersei_tools::shell::procs::is_running(k), "{k} survived");
    }
}

// Used by the harness for messages.
#[allow(dead_code)]
fn _msg(m: &Message) -> Option<&str> {
    m.get_text()
}
