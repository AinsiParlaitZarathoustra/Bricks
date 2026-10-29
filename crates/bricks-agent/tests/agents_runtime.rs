//! The 10.5 runtime through the real controller: concurrent fan-out,
//! bounded recursion, background agents, isolated ChangeSets, correlation,
//! skills, usage aggregation, permissions and jobs.
//!
//! The parent follows a script; children are driven by markers in their
//! task (`[barrier]`, `[spawn-child:…]`, `[write:file=content]`, `[fail]`,
//! `[hold]`), with fixed usage (7 in / 3 out per response).

#[allow(dead_code)]
mod common;

use bricks_agent::BricksConfig;
use bricks_agent::agents::runtime::MessageOutcome;
use bricks_agent::agents::{InstanceState, ProfileSources};
use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
use bricks_agent::control::*;
use bricks_agent::events::InjectRefusal;
use bricks_agent::permission::SessionPolicy;
use bricks_provider::{CompletionRequest, CompletionStream, Provider};
use bricks_types::{MessageContent, Role, Usage};
use common::follow::{Waited, next_wait, started_ids};
use serde_json::json;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    /// The parent's replies, in order (see `with_ids` for placeholders).
    queue: parking_lot::Mutex<VecDeque<Reply>>,
    /// Whether the parent and the children wait for the sub-agents they
    /// were given, one `AgentControl wait` at a time (see `common::follow`).
    auto_wait: AtomicBool,
    waited: Waited,
}

impl Shared {
    fn push_parent(&self, replies: Vec<Reply>) {
        self.queue.lock().extend(replies);
    }

    /// The wait a model would make next for the handles it was given.
    fn follow(&self, req: &CompletionRequest) -> Option<Reply> {
        if !self.auto_wait.load(Ordering::SeqCst) {
            return None;
        }
        next_wait(req, &self.waited)
    }

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

/// `$ID<n>` in a scripted tool input stands for the n-th sub-agent started
/// so far (in the parent's messages, in order), whatever its random id.
fn with_ids(mut reply: Reply, started: &[String]) -> Reply {
    for (_, _, input) in reply.tools.iter_mut() {
        let mut s = input.to_string();
        for (n, id) in started.iter().enumerate() {
            s = s.replace(&format!("$ID{n}"), id);
        }
        *input = serde_json::from_str(&s).expect("a scripted input with ids is JSON");
    }
    reply
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
            .any(|x| matches!(x, bricks_types::ContentBlock::ToolResult { .. })),
        _ => false,
    })
}

fn tool_results(r: &CompletionRequest) -> usize {
    r.messages
        .iter()
        .map(|m| match &m.content {
            MessageContent::Blocks(b) => b
                .iter()
                .filter(|x| matches!(x, bricks_types::ContentBlock::ToolResult { .. }))
                .count(),
            _ => 0,
        })
        .sum()
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
    fn model_info(&self) -> Option<bricks_provider::ModelInfo> {
        ScriptedProvider::new(&self.selection, self.parent.clone()).model_info()
    }
    async fn complete(&self, req: CompletionRequest) -> bricks_types::Result<CompletionStream> {
        if !is_child(&req) {
            if let Some(wait) = self.shared.follow(&req) {
                return ScriptedProvider::new(&self.selection, Script::new(vec![wait]))
                    .complete(req)
                    .await;
            }
            let reply = self
                .shared
                .queue
                .lock()
                .pop_front()
                .unwrap_or_else(|| Reply::text("(no more scripted replies)"));
            let reply = with_ids(reply, &started_ids(&req));
            return ScriptedProvider::new(&self.selection, Script::new(vec![reply]))
                .complete(req)
                .await;
        }
        self.shared.child_requests.lock().push(req.clone());
        if let Some(mut wait) = self.shared.follow(&req) {
            wait.usage = Some(child_usage());
            return ScriptedProvider::new(&self.selection, Script::new(vec![wait]))
                .complete(req)
                .await;
        }
        let task = first_user_text(&req);
        let n = self.shared.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared.peak.fetch_max(n, Ordering::SeqCst);
        let _guard = scopeguard(Arc::clone(&self.shared));
        if task.contains("[fail]") {
            return Err(bricks_types::BricksError::Provider(
                "scripted failure".into(),
            ));
        }
        if task.contains("[barrier]") {
            let b = self.shared.barrier.lock().clone();
            if let Some(b) = b {
                b.wait().await;
            }
        }
        // `[hold-first]`: the first request only (a message sent meanwhile is
        // read by the next one).
        let held = (task.contains("[hold]") && !has_tool_result(&req))
            || (task.contains("[hold-after]") && has_tool_result(&req))
            || (task.contains("[hold-first]") && req.messages.len() == 1);
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
        let looped = marker(&task, "loop").and_then(|n| n.parse::<usize>().ok());
        let done = tool_results(&req);
        let mut reply = if looped.is_some_and(|n| done < n) {
            // `[loop:N]`: N turns of distinct work, then the answer.
            Reply::tool(
                &format!("l{done}"),
                "Glob",
                json!({ "pattern": format!("*.z{done}") }),
            )
        } else if has_tool_result(&req) {
            Reply::text(&format!("done: {}", task.lines().next().unwrap_or("")))
        } else if let Some(inner) = marker(&task, "spawn-child-skilled") {
            Reply::tool(
                "d1",
                "Agent",
                json!({ "task": inner.replace('|', ":"), "profile": "skilled" }),
            )
        } else if let Some(inner) = marker(&task, "spawn-child") {
            Reply::tool(
                "d1",
                "Agent",
                json!({ "task": inner.replace('|', ":").replace('{', "[").replace('}', "]") }),
            )
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
    inner: Arc<bricks_agent::control::scripted::ScriptedCatalog>,
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
    catalog: Arc<TestCatalog>,
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

fn allow(tools: &[&str]) -> Option<SessionPolicy> {
    Some(common::allow_tools(tools))
}

async fn open(
    replies: Vec<Reply>,
    rules: Option<SessionPolicy>,
    interactive: bool,
    tweak: impl FnOnce(&mut BricksConfig),
) -> Env {
    open_full(replies, rules, interactive, tweak, |_, _| {}).await
}

async fn open_full(
    replies: Vec<Reply>,
    rules: Option<SessionPolicy>,
    interactive: bool,
    tweak: impl FnOnce(&mut BricksConfig),
    engine: impl FnOnce(&mut EngineConfig, &Path),
) -> Env {
    let dir = tempfile::tempdir().unwrap();
    // A repository with one commit (worktrees need one).
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("shared.txt"), "base\n").unwrap();
    std::fs::write(dir.path().join(".gitignore"), ".sessions/\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let shared = Arc::new(Shared::default());
    shared.queue.lock().extend(replies);
    // By default the parent and the children wait for every sub-agent they
    // were given (a test that must not wait turns it off).
    shared.auto_wait.store(true, Ordering::SeqCst);
    let catalog = Arc::new(TestCatalog {
        inner: bricks_agent::control::scripted::ScriptedCatalog::new(
            &["a", "b"],
            Script::new(Vec::new()),
        ),
        shared: Arc::clone(&shared),
    });
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.semantic.lsp.enabled = false;
    bricks.agents.admission_timeout_ms = 20_000;
    tweak(&mut bricks);
    let mut cfg = EngineConfig::new(
        dir.path(),
        catalog.clone(),
        bricks,
        dir.path().join(".sessions"),
    );
    cfg.interactive = interactive;
    cfg.agent_profile_sources = Some(ProfileSources {
        project_dir: Some(dir.path().join(".bricks/agents")),
        user_dir: None,
    });
    cfg.permissions = rules.map(|p| common::default_store(dir.path(), &p));
    engine(&mut cfg, dir.path());
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
        catalog,
    }
}

/// Closes the session and opens it again, as a restart of the process would:
/// the stored conversations and the manifest are read back.
async fn restart(env: Env) -> Env {
    let Env {
        ctl,
        events,
        dir,
        shared,
        catalog,
    } = env;
    let session = ctl.snapshot().session_id.clone();
    drop(events);
    drop(ctl);
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.semantic.lsp.enabled = false;
    bricks.agents.admission_timeout_ms = 20_000;
    let mut cfg = EngineConfig::new(
        dir.path(),
        catalog.clone(),
        bricks,
        dir.path().join(".sessions"),
    );
    cfg.interactive = true;
    cfg.agent_profile_sources = Some(ProfileSources {
        project_dir: Some(dir.path().join(".bricks/agents")),
        user_dir: None,
    });
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::Resume(session),
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
        catalog,
    }
}

/// The first file called `name` under `root`.
fn find_file(root: &Path, name: &str) -> Option<std::path::PathBuf> {
    for e in std::fs::read_dir(root).ok()? {
        let p = e.ok()?.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name)
            && p.parent().is_some_and(|d| d.ends_with("agents"))
        {
            return Some(p);
        }
    }
    None
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

fn spawned(evs: &[Envelope]) -> Vec<bricks_agent::agents::SpawnInfo> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::AgentSpawned { agent } => Some((**agent).clone()),
            _ => None,
        })
        .collect()
}

fn results(evs: &[Envelope]) -> Vec<bricks_agent::agents::AgentResult> {
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
    // `Agents` answers with the three handles at once.
    let sp = spawned(&evs);
    assert_eq!(sp.len(), 3);
    let (err, handles) = tool_out(&evs, "c1");
    assert!(!err, "{handles}");
    assert!(
        sp.iter().all(|s| handles.contains(&s.agent_id)),
        "{handles}"
    );
    // The parent waits for them in the order asked: the results come in
    // that order (one `wait` each).
    let waited: Vec<(usize, String)> = (0..3)
        .map(|i| {
            let id = sp
                .iter()
                .find(|s| s.batch_index == Some(i))
                .unwrap()
                .agent_id
                .clone();
            let pos = evs
                .iter()
                .position(|e| {
                    matches!(&e.event, Event::ToolFinished { tool_call_id, .. } if *tool_call_id == format!("wait_{id}"))
                })
                .unwrap();
            let (err, out) = tool_out(&evs, &format!("wait_{id}"));
            assert!(!err, "{out}");
            (pos, out)
        })
        .collect();
    assert!(waited[0].0 < waited[1].0 && waited[1].0 < waited[2].0);
    for (i, (_, out)) in waited.iter().enumerate() {
        assert!(out.contains(&format!("result of T{i}")), "{out}");
    }
    // Correlation: one parent call, positions, parallel → worktrees.
    assert!(sp.iter().all(|s| s.tool_call_id.as_deref() == Some("c1")));
    let mut idx: Vec<usize> = sp.iter().filter_map(|s| s.batch_index).collect();
    idx.sort();
    assert_eq!(idx, vec![0, 1, 2]);
    assert!(sp.iter().all(|s| s.isolation == "worktree" && s.depth == 1));
    // Usage: parent 5 × (100/10) (the call, three waits, the answer),
    // children 3 × (7/3), counted once. Published just before `run_finished`.
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
    assert_eq!((ru.0.input_tokens, ru.0.output_tokens), (500, 50));
    assert_eq!((ru.1.input_tokens, ru.1.output_tokens), (21, 9));
    assert_eq!((ru.2.input_tokens, ru.2.output_tokens), (521, 59));
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
                json!({ "task": "level1 [spawn-child:level2 leaf]" }),
            ),
            Reply::text("chain done"),
        ],
        allow(&["Agent"]),
        true,
        |b| {
            b.agents.max_concurrent = 1;
            b.agents.max_depth = 2;
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
async fn the_depth_limit_refuses_before_any_provider() {
    // Depth: the default max_depth = 1, the child tries to start a sub-agent.
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "level1 [spawn-child:too deep]" }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agent"]),
        true,
        |_| {},
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
    // The holding children are not waited for: the answer comes at once.
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    // Every child holds: one runs, one waits, the last is refused,
    // whatever the order they arrive in.
    let mut evs = until(
        &mut env.events,
        |e| matches!(e, Event::AgentFinished { result } if result.status == "failed"),
    )
    .await;
    let failed = results(&evs);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(
        failed[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("AgentQueueFull")),
        "{failed:?}"
    );
    env.shared.release();
    while results(&evs).len() < 3 {
        evs.extend(
            until(&mut env.events, |e| {
                matches!(e, Event::AgentFinished { .. })
            })
            .await,
        );
    }
    let all = results(&evs);
    assert_eq!(all.len(), 3);
    assert_eq!(all.iter().filter(|r| r.status == "completed").count(), 2);
    assert_eq!(all.iter().filter(|r| r.status == "failed").count(), 1);
}

// ─── No turn limit on any path ───────────────────────────────────────────────

/// Fan-out, background, worktree isolation and a grandchild: every child
/// works past the former limits (30 by default, 100 at most) and answers.
#[tokio::test]
async fn every_path_lets_a_child_go_on_until_its_answer() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agents",
                json!({ "agents": [
                    { "task": "first [loop:105]" },
                    { "task": "second [spawn-child:grandchild {loop|40}]" }
                ]}),
            ),
            Reply::text("all done"),
        ],
        allow(&["Agents", "Agent"]),
        true,
        |b| b.agents.max_depth = 2,
    )
    .await;
    let evs = tokio::time::timeout(Duration::from_secs(120), submit(&mut env, "go"))
        .await
        .expect("the test harness's own timeout");
    let sp = spawned(&evs);
    assert!(
        sp.iter()
            .filter(|s| s.depth == 1)
            .all(|s| s.isolation == "worktree")
    );
    // The parent waited for both; the final total is in the run's events.
    assert!(evs.iter().any(|e| matches!(
        e.event,
        Event::RunUsage {
            final_total: true,
            ..
        }
    )));
    let rs = results(&evs);
    assert_eq!(rs.len(), 3, "two children and a grandchild");
    for r in &rs {
        assert_eq!(r.status, "completed", "{r:?}");
        assert_eq!(r.termination, Some(bricks_agent::Termination::Completed));
    }
    let mut turns: Vec<u32> = rs.iter().map(|r| r.turns).collect();
    turns.sort();
    // The grandchild (depth 2) ran 40 turns of work, the first child 105;
    // the second child: its `Agent` call, its wait for the grandchild, its
    // answer.
    assert_eq!(turns, vec![3, 41, 106]);
}

// ─── Background ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_background_agent_outlives_the_answer_and_totals_become_final() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "long work [hold]" })),
            Reply::text("started it"),
        ],
        allow(&["Agent"]),
        true,
        |_| {},
    )
    .await;
    // The answer ends the run: the child is not waited for.
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    let t = std::time::Instant::now();
    let evs = submit(&mut env, "go").await;
    assert!(
        t.elapsed() < Duration::from_secs(10),
        "the handle came back at once"
    );
    let (err, out) = tool_out(&evs, "c1");
    assert!(!err && out.contains("sub-agent(s) started"), "{out}");
    let sp = spawned(&evs);
    assert_eq!(sp.len(), 1);
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
    assert_eq!(results(&fin)[0].status, "completed");
    assert!(!fin.iter().any(|e| matches!(
        e.event,
        Event::RunStarted { .. } | Event::RunFinished { .. }
    )));
    let total = fin.iter().find_map(|e| match &e.event {
        Event::RunUsage { total, .. } => Some(total.input_tokens),
        _ => None,
    });
    assert_eq!(total, Some(200 + 7), "parent 2 × 100 + child 7, once");
}

#[tokio::test]
async fn agent_control_waits_reads_and_cancels_in_scope() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "bg [hold]" })),
            Reply::text("started"),
        ],
        allow(&["Agent", "AgentControl"]),
        true,
        |_| {},
    )
    .await;
    env.shared.auto_wait.store(false, Ordering::SeqCst);
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
            message: None,
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
                message: None,
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
                json!({ "task": "one round [glob] [hold-after]" }),
            ),
            Reply::text("started"),
        ],
        allow(&["Agent"]),
        true,
        |_| {},
    )
    .await;
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    let evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    // Its first response (a tool call) came back; it now holds.
    if !evs
        .iter()
        .any(|e| matches!(e.event, Event::AgentToolFinished { .. }))
    {
        until(&mut env.events, |e| {
            matches!(e, Event::AgentToolFinished { .. })
        })
        .await;
    }
    env.ctl
        .send(Command::AgentControl {
            action: "cancel".into(),
            agent_id: Some(id),
            changeset_id: None,
            job_id: None,
            message: None,
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
        message: None,
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
                state: bricks_agent::agents::ChangeSetState::Conflict,
                ..
            }
        )
    };
    if !e.iter().any(|x| conflict(&x.event)) {
        until(&mut env.events, conflict).await;
    }
}

// ─── Skills, permissions ─────────────────────────────────────────────────────

/// Skills on (the store in a sub-folder, nothing of the machine), the
/// project's `.agents/skills` given as an explicit source, approved by the
/// caller when `trust`.
fn with_skills(trust: bool) -> impl FnOnce(&mut EngineConfig, &Path) {
    move |cfg, dir| {
        let project = dir.canonicalize().unwrap();
        let mut setup = SkillsSetup::with_store(Some(dir.join(".home/skills")));
        setup.extra_roots = vec![bricks_skills::SkillRoot {
            id: "project:.agents".into(),
            scope: bricks_skills::Scope::Project,
            path: Some(project.join(".agents/skills")),
            trusted: false,
        }];
        setup.trust_project = trust;
        cfg.skills = Some(setup);
    }
}

fn write_skill(root: &Path, name: &str, body: &str) {
    let d = root.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Procedure {name}.\n---\n{body}\n"),
    )
    .unwrap();
}

async fn skilled_profile(env: &mut Env, skills: &str) {
    let root = env.dir.path();
    std::fs::create_dir_all(root.join(".bricks/agents")).unwrap();
    std::fs::write(
        root.join(".bricks/agents/skilled.md"),
        format!(
            "---\nname: skilled\ndescription: Uses skills.\nskills: [{skills}]\n---\nBe skilled.\n"
        ),
    )
    .unwrap();
    // Not committed: the skill is found in place, not through Git.
    write_skill(
        &root.join(".agents/skills"),
        "house-style",
        "SKILL-MARKER-42: write short sentences.",
    );
    env.ctl.send(Command::ReloadAgentProfiles).unwrap();
    until(&mut env.events, |e| {
        matches!(e, Event::AgentProfiles { .. })
    })
    .await;
    // Sources are read on demand, not per turn.
    env.ctl.send(Command::ReloadSkills).unwrap();
    until(&mut env.events, |e| matches!(e, Event::Skills { .. })).await;
}

fn skill_events(evs: &[Envelope]) -> Vec<(Option<String>, String, String)> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::SkillActivated {
                agent_id,
                name,
                origin,
                ..
            } => Some((agent_id.clone(), name.clone(), origin.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn profile_skills_are_preloaded_whole_before_the_childs_first_call() {
    let mut env = open_full(
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
        with_skills(true),
    )
    .await;
    skilled_profile(&mut env, "house-style").await;
    let evs = submit(&mut env, "go").await;
    let rs = results(&evs);
    let states: Vec<(String, String)> = rs[0]
        .skills
        .iter()
        .map(|s| (s.name.clone(), s.state.clone()))
        .collect();
    assert_eq!(
        states,
        vec![("house-style".to_string(), "loaded".to_string())]
    );
    assert!(rs[0].skills[0].revision.is_some() && rs[0].skills[0].source.is_some());
    let child = rs[0].agent_id.clone();
    assert!(skill_events(&evs).contains(&(Some(child), "house-style".into(), "profile".into())));
    // In the child's first request, whole, and in the next one.
    let reqs = env.shared.child_requests.lock().clone();
    assert!(reqs.len() >= 2);
    for r in &reqs {
        let sys = r.system.as_deref().unwrap();
        assert!(sys.contains("SKILL-MARKER-42: write short sentences."));
        assert!(!sys.contains("truncated"));
    }
    let names: Vec<&str> = reqs[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"Bash") && names.contains(&"Agent") && names.contains(&"Skill"));
}

#[tokio::test]
async fn a_background_child_gets_its_profile_skills_too() {
    let mut env = open_full(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "later [glob]", "profile": "skilled" }),
            ),
            Reply::text("started"),
        ],
        allow(&["Agent", "Glob"]),
        true,
        |_| {},
        with_skills(true),
    )
    .await;
    skilled_profile(&mut env, "house-style").await;
    let mut evs = submit(&mut env, "go").await;
    if results(&evs).is_empty() {
        evs.extend(
            until(&mut env.events, |e| {
                matches!(e, Event::AgentFinished { .. })
            })
            .await,
        );
    }
    let rs = results(&evs);
    assert_eq!(rs[0].skills[0].state, "loaded");
    let reqs = env.shared.child_requests.lock().clone();
    assert!(
        reqs[0]
            .system
            .as_deref()
            .unwrap()
            .contains("SKILL-MARKER-42")
    );
}

#[tokio::test]
async fn a_profile_skill_that_cannot_be_used_refuses_the_launch_before_any_call() {
    for (skills, trust, want) in [
        ("house-style, no-such-skill", true, "no-such-skill"),
        // Project skills not approved for the session: refused too.
        ("house-style", false, "not approved"),
    ] {
        let mut env = open_full(
            vec![
                Reply::tool("c1", "Agent", json!({ "task": "x", "profile": "skilled" })),
                Reply::text("ok"),
            ],
            allow(&["Agent"]),
            true,
            |_| {},
            with_skills(trust),
        )
        .await;
        skilled_profile(&mut env, skills).await;
        let evs = submit(&mut env, "go").await;
        let (is_error, out) = tool_out(&evs, "c1");
        assert!(is_error && out.contains(want), "{out}");
        assert!(spawned(&evs).is_empty());
        assert!(env.shared.child_requests.lock().is_empty(), "no child call");
    }
}

#[tokio::test]
async fn a_profile_over_the_skill_budget_is_refused_not_cut() {
    let mut env = open_full(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "x", "profile": "skilled" })),
            Reply::text("ok"),
        ],
        allow(&["Agent"]),
        true,
        |b| b.agents.skills_max_bytes = 100,
        with_skills(true),
    )
    .await;
    skilled_profile(&mut env, "house-style").await;
    write_skill(
        &env.dir.path().join(".agents/skills"),
        "house-style",
        &"long line\n".repeat(50),
    );
    env.ctl.send(Command::ReloadSkills).unwrap();
    until(&mut env.events, |e| matches!(e, Event::Skills { .. })).await;
    let evs = submit(&mut env, "go").await;
    let (is_error, out) = tool_out(&evs, "c1");
    assert!(
        is_error && out.contains("skills_max_bytes") && out.contains("nothing is cut"),
        "{out}"
    );
    assert!(env.shared.child_requests.lock().is_empty());
}

#[tokio::test]
async fn a_worktree_child_reads_its_origin_projects_skill_in_place() {
    let mut env = open_full(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "work [glob]", "profile": "skilled", "isolation": "worktree" }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agent", "Glob"]),
        true,
        |_| {},
        with_skills(true),
    )
    .await;
    skilled_profile(&mut env, "house-style").await;
    let evs = submit(&mut env, "go").await;
    let sp = spawned(&evs);
    assert_eq!(sp[0].isolation, "worktree");
    let origin = env.dir.path().canonicalize().unwrap();
    let sys = env.shared.child_requests.lock()[0].system.clone().unwrap();
    let skill_dir = origin.join(".agents/skills/house-style");
    assert!(
        sys.contains(&format!("Skill directory: {}", skill_dir.display())),
        "{sys}"
    );
    // The child works in its worktree, kept in the session's files; the
    // `Spawned` event names the parent's directory (the worktree comes later).
    let wd = sys
        .split_once("Your working directory is unchanged: ")
        .and_then(|(_, rest)| rest.split_once(".\n"))
        .map(|(wd, _)| wd.to_string())
        .unwrap_or_else(|| panic!("no working directory in: {sys}"));
    assert_ne!(wd, origin.display().to_string(), "{sys}");
    assert!(
        wd.contains("workspaces"),
        "worktree {wd} outside the session's files"
    );
}

#[tokio::test]
async fn nested_delegation_resolves_skills_the_same_way_and_the_parent_keeps_its_own() {
    let mut env = open_full(
        vec![
            Reply::tool("p1", "Skill", json!({ "skill": "parent-only" })),
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "relay [spawn-child-skilled:inner [glob]]" }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agent", "Glob"]),
        true,
        |b| b.agents.max_depth = 2,
        with_skills(true),
    )
    .await;
    skilled_profile(&mut env, "house-style").await;
    write_skill(
        &env.dir.path().join(".home/skills"),
        "parent-only",
        "PARENT-ONLY-BODY",
    );
    env.ctl.send(Command::ReloadSkills).unwrap();
    until(&mut env.events, |e| matches!(e, Event::Skills { .. })).await;
    let evs = submit(&mut env, "go").await;
    assert!(skill_events(&evs).contains(&(None, "parent-only".into(), "model".into())));
    let reqs = env.shared.child_requests.lock().clone();
    let grandchild: Vec<_> = reqs
        .iter()
        .filter(|r| {
            r.system
                .as_deref()
                .is_some_and(|s| s.contains("SKILL-MARKER-42"))
        })
        .collect();
    assert!(
        !grandchild.is_empty(),
        "the grandchild got its profile's skill"
    );
    for r in &reqs {
        let mut all = r.system.clone().unwrap_or_default();
        for m in &r.messages {
            all.push_str(&m.get_all_text());
        }
        assert!(
            !all.contains("PARENT-ONLY-BODY"),
            "a parent's activation is its own"
        );
    }
    // The children's own ledgers: only the grandchild has `house-style`.
    let profile_events: Vec<_> = skill_events(&evs)
        .into_iter()
        .filter(|(_, n, o)| n == "house-style" && o == "profile")
        .collect();
    assert_eq!(profile_events.len(), 1);
}

#[tokio::test]
async fn headless_defaults_refuse_cleanly_and_an_explicit_rule_does_not_allow_the_childs_tools() {
    // Default rules, nobody to ask: delegation is refused, the run says so.
    let mut env = open(
        vec![
            Reply::tool("c1", "Agents", json!({ "agents": [{ "task": "x" }] })),
            Reply::text("ok"),
        ],
        None,
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
    assert!(
        job.1
            .ends_with(env.dir.path().file_name().unwrap().to_str().unwrap())
    );
    let (_, out) = tool_out(&evs, "b1");
    assert!(out.contains(&job.0));
    // Its children exist.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let kids = bricks_tools::shell::procs::descendants(job.2);
    assert!(kids.len() >= 2, "{kids:?}");
    // A person stops it: the whole tree goes.
    env.ctl
        .send(Command::AgentControl {
            action: "stop_job".into(),
            agent_id: None,
            changeset_id: None,
            job_id: Some(job.0.clone()),
            message: None,
        })
        .unwrap();
    let e = until(&mut env.events, |e| matches!(e, Event::JobFinished { .. })).await;
    match &e.last().unwrap().event {
        Event::JobFinished { state, .. } => assert_eq!(state, "stopped"),
        _ => unreachable!(),
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    for k in kids {
        assert!(!bricks_tools::shell::procs::is_running(k), "{k} survived");
    }
}

// ─── Applied changes (`edit_applied`) ────────────────────────────────────────

/// `(agent_id, changeset_id, first path, added, removed)`.
type Edit = (Option<String>, Option<String>, String, usize, usize);

fn edits(evs: &[Envelope]) -> Vec<Edit> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::EditApplied {
                agent_id,
                changeset_id,
                files,
                ..
            } => Some((
                agent_id.clone(),
                changeset_id.clone(),
                files[0].path.clone(),
                files[0].added,
                files[0].removed,
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn shared_children_report_their_writes_and_isolated_ones_only_when_applied() {
    let mut env = open(
        vec![
            Reply::tools(vec![(
                "c1",
                "Agent",
                json!({ "task": "fg [write:fg.txt=a]", "isolation": "shared" }),
            )]),
            Reply::tool(
                "c2",
                "Agent",
                json!({ "task": "bg [write:bg.txt=b]", "isolation": "shared", "background": true }),
            ),
            Reply::tool(
                "c3",
                "Agents",
                json!({ "agents": [
                    { "task": "iso one [write:same.txt=one]" },
                    { "task": "iso two [write:same.txt=two]" }
                ]}),
            ),
            Reply::text("done"),
        ],
        allow(&["Agent", "Agents", "Write", "AgentControl"]),
        true,
        |_| {},
    )
    .await;
    let mut evs = submit(&mut env, "go").await;
    // The background child may finish after the answer.
    if !edits(&evs).iter().any(|e| e.2 == "bg.txt") {
        evs.extend(
            until(
                &mut env.events,
                |e| matches!(e, Event::EditApplied { files, .. } if files[0].path == "bg.txt"),
            )
            .await,
        );
    }
    let sp = spawned(&evs);
    let id_of = |t: &str| {
        sp.iter()
            .find(|s| s.task.starts_with(t))
            .unwrap()
            .agent_id
            .clone()
    };
    let e = edits(&evs);
    // Shared children: their writes, once each, with their identity.
    assert_eq!(
        e,
        vec![
            (Some(id_of("fg")), None, "fg.txt".into(), 1, 0),
            (Some(id_of("bg")), None, "bg.txt".into(), 1, 0),
        ],
        "worktree writes are not changes of the session's workspace yet"
    );
    let ready: Vec<_> = evs
        .iter()
        .filter_map(|e| match &e.event {
            Event::ChangesReady { changeset } => Some((**changeset).clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ready.len(), 2);

    // Applied by a person: one change, the ChangeSet's lines.
    let control = |action: &str, id: &str| Command::AgentControl {
        action: action.into(),
        agent_id: None,
        changeset_id: Some(id.into()),
        job_id: None,
        message: None,
    };
    env.ctl
        .send(control("apply_changes", &ready[0].id))
        .unwrap();
    // The result and the change arrive by two routes, in either order.
    let mut got = Vec::new();
    while !(got
        .iter()
        .any(|e: &Envelope| matches!(e.event, Event::EditApplied { .. }))
        && got
            .iter()
            .any(|e: &Envelope| matches!(e.event, Event::AgentControlResult { .. })))
    {
        got.push(next(&mut env.events).await);
    }
    assert_eq!(
        edits(&got),
        vec![(
            Some(ready[0].agent_id.clone()),
            Some(ready[0].id.clone()),
            "same.txt".into(),
            1,
            0
        )]
    );
    // Again: refused, nothing more.
    env.ctl
        .send(control("apply_changes", &ready[0].id))
        .unwrap();
    let again = until(&mut env.events, |e| {
        matches!(e, Event::AgentControlResult { .. })
    })
    .await;
    assert!(edits(&again).is_empty());
    assert!(matches!(
        &again.last().unwrap().event,
        Event::AgentControlResult { ok: false, .. }
    ));
    // The other one conflicts (same file): nothing.
    env.ctl
        .send(control("apply_changes", &ready[1].id))
        .unwrap();
    let conflict = until(&mut env.events, |e| {
        matches!(e, Event::AgentControlResult { .. })
    })
    .await;
    assert!(matches!(
        &conflict.last().unwrap().event,
        Event::AgentControlResult { ok: false, .. }
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    while let Some(e) = env.events.try_next() {
        assert!(
            !matches!(e.event, Event::EditApplied { .. }),
            "{:?}",
            e.event
        );
    }
}

#[tokio::test]
async fn a_changeset_applied_by_the_agent_counts_once_too() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "iso [write:tool.txt=x]", "isolation": "worktree" }),
            ),
            Reply::text("ready"),
        ],
        allow(&["Agent", "Write", "AgentControl"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let cs = evs
        .iter()
        .find_map(|e| match &e.event {
            Event::ChangesReady { changeset } => Some((**changeset).clone()),
            _ => None,
        })
        .unwrap();
    assert!(edits(&evs).is_empty());
    // The session agent applies it with AgentControl.
    env.shared.push_parent(vec![
        Reply::tool(
            "a1",
            "AgentControl",
            json!({ "action": "apply_changes", "changeset_id": cs.id }),
        ),
        Reply::tool(
            "a2",
            "AgentControl",
            json!({ "action": "apply_changes", "changeset_id": cs.id }),
        ),
        Reply::text("applied"),
    ]);
    let evs = submit(&mut env, "apply it").await;
    let mut e = edits(&evs);
    if e.is_empty() {
        e = edits(&until(&mut env.events, |e| matches!(e, Event::EditApplied { .. })).await);
    }
    assert_eq!(
        e,
        vec![(
            Some(cs.agent_id.clone()),
            Some(cs.id.clone()),
            "tool.txt".into(),
            1,
            0
        )]
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    while let Some(x) = env.events.try_next() {
        assert!(
            !matches!(x.event, Event::EditApplied { .. }),
            "applied once"
        );
    }
    assert_eq!(
        std::fs::read_to_string(env.dir.path().join("tool.txt")).unwrap(),
        "x\n"
    );
}

// ─── Messages to a running sub-agent ─────────────────────────────────────────

/// Reads events until a sub-agent starts (`AgentSpawned`), keeping them.
async fn until_spawned(env: &mut Env, evs: &mut Vec<Envelope>) -> String {
    loop {
        let e = next(&mut env.events).await;
        let id = match &e.event {
            Event::AgentSpawned { agent } => Some(agent.agent_id.clone()),
            _ => None,
        };
        evs.push(e);
        if let Some(id) = id {
            return id;
        }
    }
}

/// Waits (bounded) until the sub-agent's model has been asked `n` times.
async fn until_requests(shared: &Shared, n: usize) {
    for _ in 0..1000 {
        if shared.child_requests.lock().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the sub-agent's model was never asked {n} time(s)");
}

/// The requests of the sub-agents whose task starts with `task`, in order.
fn requests_of(shared: &Shared, task: &str) -> Vec<CompletionRequest> {
    shared
        .child_requests
        .lock()
        .iter()
        .filter(|r| first_user_text(r).starts_with(task))
        .cloned()
        .collect()
}

/// The user's text messages of a request, in order.
fn user_texts(r: &CompletionRequest) -> Vec<String> {
    r.messages
        .iter()
        .filter(|m| m.role == Role::User)
        .filter_map(|m| m.get_text().map(str::to_string))
        .collect()
}

fn is_tool_result_message(m: &bricks_types::Message) -> bool {
    matches!(&m.content, MessageContent::Blocks(b)
        if b.iter().any(|x| matches!(x, bricks_types::ContentBlock::ToolResult { .. })))
}

/// A message sent while the sub-agent runs its tool is read after the tool
/// result, at its next request, in the order sent; never between a call and
/// its result.
#[tokio::test]
async fn a_message_is_read_after_the_pending_tool_results_and_before_the_next_request() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "bash work [bash:sleep 1; echo x]" }),
            ),
            Reply::text("all done"),
        ],
        allow(&["Agent", "Bash"]),
        true,
        |_| {},
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let mut evs = Vec::new();
    let child = loop {
        let e = next(&mut env.events).await;
        let hit = match &e.event {
            Event::AgentToolStarted { agent_id, name, .. } if name == "Bash" => {
                Some(agent_id.clone())
            }
            _ => None,
        };
        evs.push(e);
        if let Some(id) = hit {
            break id;
        }
    };
    let rt = env.ctl.agent_runtime().unwrap();
    assert!(matches!(
        rt.send_message(&child, None, "MSG-A".into()),
        MessageOutcome::Accepted { .. }
    ));
    assert!(matches!(
        rt.send_message(&child, None, "MSG-B".into()),
        MessageOutcome::Accepted { .. }
    ));
    evs.extend(until_finished(&mut env.events).await);
    let reqs = requests_of(&env.shared, "bash work");
    assert_eq!(reqs.len(), 2, "the call, then the request after its result");
    let last = &reqs[1];
    let n = last.messages.len();
    let tool_at = last
        .messages
        .iter()
        .position(is_tool_result_message)
        .expect("the tool result");
    let a = last
        .messages
        .iter()
        .position(|m| m.get_text() == Some("MSG-A"))
        .expect("MSG-A");
    let b = last
        .messages
        .iter()
        .position(|m| m.get_text() == Some("MSG-B"))
        .expect("MSG-B");
    assert!(tool_at < a && a < b, "{tool_at} {a} {b}");
    assert_eq!((a, b), (n - 2, n - 1), "both are the newest messages");
}

/// A message arriving while the final answer is generated is read by one
/// more request: the run does not end with it unanswered.
#[tokio::test]
async fn a_message_during_the_final_answer_gets_one_more_request() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "final work [hold-first]" })),
            Reply::text("all done"),
        ],
        allow(&["Agent"]),
        true,
        |_| {},
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let mut evs = Vec::new();
    let child = until_spawned(&mut env, &mut evs).await;
    // Its first request is held: the answer is being generated.
    until_requests(&env.shared, 1).await;
    let rt = env.ctl.agent_runtime().unwrap();
    assert!(matches!(
        rt.send_message(&child, None, "MSG-F".into()),
        MessageOutcome::Accepted { .. }
    ));
    env.shared.release();
    evs.extend(until_finished(&mut env.events).await);
    let reqs = requests_of(&env.shared, "final work");
    assert_eq!(reqs.len(), 2, "one more request for the message");
    assert_eq!(reqs[1].messages.last().unwrap().get_text(), Some("MSG-F"));
    let rs = results(&evs);
    assert_eq!(rs[0].status, "completed");
    assert_eq!(rs[0].turns, 2);
}

/// A message to a sub-agent that has ended is refused, and says so: the
/// answer is never « sent ».
#[tokio::test]
async fn a_message_to_an_ended_agent_is_refused_and_never_reported_as_sent() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "quick" })),
            Reply::text("all done"),
        ],
        allow(&["Agent", "AgentControl"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    env.ctl
        .send(Command::AgentControl {
            action: "message".into(),
            agent_id: Some(id.clone()),
            changeset_id: None,
            job_id: None,
            message: Some("late".into()),
        })
        .unwrap();
    let e = until(&mut env.events, |e| {
        matches!(e, Event::AgentControlResult { .. })
    })
    .await;
    match &e.last().unwrap().event {
        Event::AgentControlResult { ok, text, .. } => {
            assert!(!ok, "{text}");
            assert!(text.contains("nothing was sent"), "{text}");
            assert!(!text.contains("queued"), "{text}");
        }
        _ => unreachable!(),
    }
    let rt = env.ctl.agent_runtime().unwrap();
    assert!(matches!(
        rt.send_message(&id, None, "late".into()),
        MessageOutcome::NotRunning(_)
    ));
}

/// An unknown id, and a sibling's agent (out of its scope), are refused; a
/// session-level message reaches a running sibling.
#[tokio::test]
async fn a_message_to_an_unknown_or_out_of_scope_agent_is_refused() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agents",
                json!({ "agents": [{ "task": "a [hold-first]" }, { "task": "b [hold-first]" }] }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agents"]),
        true,
        |_| {},
    )
    .await;
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    let evs = submit(&mut env, "go").await;
    let sp = spawned(&evs);
    let (a, b) = (sp[0].agent_id.clone(), sp[1].agent_id.clone());
    // Both children are running (their first request is held).
    until_requests(&env.shared, 2).await;
    let rt = env.ctl.agent_runtime().unwrap();
    assert_eq!(
        rt.send_message(&b, Some(&a), "from a sibling".into()),
        MessageOutcome::OutOfScope
    );
    assert_eq!(
        rt.send_message("agent_nope", None, "x".into()),
        MessageOutcome::Unknown
    );
    let o = rt.send_message(&b, None, "from the session".into());
    assert!(matches!(o, MessageOutcome::Accepted { .. }), "{o:?}");
    env.ctl
        .send(Command::AgentControl {
            action: "message".into(),
            agent_id: Some("agent_nope".into()),
            changeset_id: None,
            job_id: None,
            message: Some("x".into()),
        })
        .unwrap();
    let e = until(&mut env.events, |e| {
        matches!(e, Event::AgentControlResult { .. })
    })
    .await;
    match &e.last().unwrap().event {
        // The controller's own wording for an unknown id (the tool says
        // « nothing was sent »): refused, never reported as sent.
        Event::AgentControlResult { ok, text, .. } => {
            assert!(!ok && text.contains("unknown sub-agent"), "{text}")
        }
        _ => unreachable!(),
    }
    env.shared.release();
    let mut all = evs;
    while results(&all).len() < 2 {
        all.extend(
            until(&mut env.events, |e| {
                matches!(e, Event::AgentFinished { .. })
            })
            .await,
        );
    }
    assert!(results(&all).iter().all(|r| r.status == "completed"));
}

/// The queue of messages is bounded (32 waiting, 16 KiB each): what it
/// cannot take is refused, the empty and the too large too; what it takes is
/// read in order.
#[tokio::test]
async fn the_message_queue_is_bounded_and_refuses_what_it_cannot_take() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "queue work [hold-first]" })),
            Reply::text("all done"),
        ],
        allow(&["Agent"]),
        true,
        |_| {},
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let mut evs = Vec::new();
    let child = until_spawned(&mut env, &mut evs).await;
    until_requests(&env.shared, 1).await;
    let rt = env.ctl.agent_runtime().unwrap();
    assert_eq!(
        rt.send_message(&child, None, "  \n".into()),
        MessageOutcome::Refused(InjectRefusal::Empty)
    );
    assert_eq!(
        rt.send_message(&child, None, "x".repeat(16 * 1024 + 1)),
        MessageOutcome::Refused(InjectRefusal::TooLarge)
    );
    let mut texts = vec!["x".repeat(16 * 1024)];
    texts.extend((0..31).map(|n| format!("M-{n}")));
    for (n, t) in texts.iter().enumerate() {
        match rt.send_message(&child, None, t.clone()) {
            MessageOutcome::Accepted { pending, .. } => assert_eq!(pending, n + 1),
            other => panic!("message {n}: {other:?}"),
        }
    }
    assert_eq!(
        rt.send_message(&child, None, "M-33".into()),
        MessageOutcome::Refused(InjectRefusal::QueueFull)
    );
    env.shared.release();
    evs.extend(until_finished(&mut env.events).await);
    let reqs = requests_of(&env.shared, "queue work");
    let last = reqs.last().unwrap();
    let got = user_texts(last);
    assert_eq!(got[0], "queue work [hold-first]");
    assert_eq!(&got[1..], texts.as_slice(), "read in the order sent");
    assert_eq!(results(&evs)[0].status, "completed");
}

// ─── Resume ──────────────────────────────────────────────────────────────────

/// The count of the events of a tool, by name, of any agent.
fn tool_started(evs: &[Envelope], name: &str) -> usize {
    evs.iter()
        .filter(|e| matches!(&e.event, Event::AgentToolStarted { name: n, .. } if n == name))
        .count()
}

/// A resumed sub-agent continues its stored conversation: its next request
/// carries what it did before and the new prompt; no earlier tool runs again.
#[tokio::test]
async fn a_resumed_agent_continues_its_stored_conversation() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "first step [glob]" })),
            Reply::text("first answered"),
        ],
        allow(&["Agent", "AgentControl", "Glob"]),
        true,
        |_| {},
    )
    .await;
    let mut evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    env.shared.push_parent(vec![
        Reply::tool(
            "r1",
            "AgentControl",
            json!({ "action": "resume", "agent_id": id, "prompt": "second step" }),
        ),
        Reply::tool(
            "w1",
            "AgentControl",
            json!({ "action": "wait", "agent_id": id }),
        ),
        Reply::text("resumed and answered"),
    ]);
    evs.extend(submit(&mut env, "again").await);
    let (err, out) = tool_out(&evs, "r1");
    assert!(!err, "{out}");
    assert!(out.contains("execution 2"), "{out}");
    let rt = env.ctl.agent_runtime().unwrap();
    assert_eq!(rt.execution(&id), Some(2));
    let reqs = requests_of(&env.shared, "first step");
    assert_eq!(
        reqs.len(),
        3,
        "the call, the answer after its tool, the resumed one"
    );
    let resumed = serde_json::to_string(&reqs[2].messages).unwrap();
    assert!(resumed.contains("\"g1\""), "its earlier tool call is kept");
    assert!(
        resumed.contains("done: first step [glob]"),
        "its earlier answer is kept"
    );
    assert!(resumed.contains("second step"), "the new prompt is there");
    assert_eq!(user_texts(&reqs[2]).last().unwrap(), "second step");
    assert_eq!(tool_started(&evs, "Glob"), 1, "the earlier call ran once");
    let ends: Vec<u32> = results(&evs).iter().map(|r| r.execution).collect();
    assert_eq!(ends, vec![1, 2]);
}

/// After a restart, the stored conversation is read back and resumes.
#[tokio::test]
async fn a_resume_works_after_a_restart() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "kept [glob]" })),
            Reply::text("first answered"),
        ],
        allow(&["Agent", "AgentControl", "Glob"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    let mut env = restart(env).await;
    env.shared.push_parent(vec![
        Reply::tool(
            "r1",
            "AgentControl",
            json!({ "action": "resume", "agent_id": id, "prompt": "after restart" }),
        ),
        Reply::tool(
            "w1",
            "AgentControl",
            json!({ "action": "wait", "agent_id": id }),
        ),
        Reply::text("resumed"),
    ]);
    let evs = submit(&mut env, "again").await;
    let (err, out) = tool_out(&evs, "r1");
    assert!(!err, "{out}");
    let reqs = requests_of(&env.shared, "kept");
    let resumed = serde_json::to_string(&reqs.last().unwrap().messages).unwrap();
    assert!(resumed.contains("\"g1\"") && resumed.contains("after restart"));
    assert_eq!(results(&evs)[0].execution, 2);
}

/// Without its launch record (an older manifest), an ended sub-agent can be
/// read but not resumed: refused, nothing starts.
#[tokio::test]
async fn a_resume_without_a_launch_record_is_refused() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "old one" })),
            Reply::text("first answered"),
        ],
        allow(&["Agent", "AgentControl"]),
        true,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    let manifest = find_file(&env.dir.path().join(".sessions"), "manifest.json").unwrap();
    let mut items: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    for it in items.iter_mut() {
        if it["info"]["agent_id"] == id.as_str() {
            it.as_object_mut().unwrap().remove("launch");
        }
    }
    std::fs::write(&manifest, serde_json::to_string(&items).unwrap()).unwrap();
    let mut env = restart(env).await;
    let builds = env.shared.builds.load(Ordering::SeqCst);
    env.shared.push_parent(vec![
        Reply::tool(
            "r1",
            "AgentControl",
            json!({ "action": "resume", "agent_id": id, "prompt": "again" }),
        ),
        Reply::text("refused"),
    ]);
    let evs = submit(&mut env, "again").await;
    let (err, out) = tool_out(&evs, "r1");
    assert!(err && out.contains("faithfully"), "{out}");
    assert_eq!(
        env.shared.builds.load(Ordering::SeqCst),
        builds,
        "nothing built"
    );
    assert_eq!(results(&evs).len(), 0);
}

/// A profile changed since the launch: the resume is refused.
#[tokio::test]
async fn a_resume_after_a_profile_change_is_refused() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "p work", "profile": "mine" }),
            ),
            Reply::text("done"),
        ],
        allow(&["Agent", "AgentControl"]),
        true,
        |_| {},
    )
    .await;
    let root = env.dir.path().join(".bricks/agents");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("mine.md"),
        "---\nname: mine\ndescription: Version one.\n---\nReview v1.\n",
    )
    .unwrap();
    env.ctl.send(Command::ReloadAgentProfiles).unwrap();
    until(&mut env.events, |e| {
        matches!(e, Event::AgentProfiles { .. })
    })
    .await;
    let evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    std::fs::write(
        root.join("mine.md"),
        "---\nname: mine\ndescription: Version two.\n---\nReview v2.\n",
    )
    .unwrap();
    env.ctl.send(Command::ReloadAgentProfiles).unwrap();
    until(&mut env.events, |e| {
        matches!(e, Event::AgentProfiles { .. })
    })
    .await;
    env.shared.push_parent(vec![
        Reply::tool(
            "r1",
            "AgentControl",
            json!({ "action": "resume", "agent_id": id, "prompt": "again" }),
        ),
        Reply::text("refused"),
    ]);
    let evs = submit(&mut env, "again").await;
    let (err, out) = tool_out(&evs, "r1");
    assert!(err && out.contains("changed since its launch"), "{out}");
}

/// Two resumes of one ended agent at once: one starts, the other is refused.
#[tokio::test]
async fn two_resumes_at_once_start_one_execution() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "once [glob]" })),
            Reply::text("first answered"),
            Reply::tools(vec![
                (
                    "r1",
                    "AgentControl",
                    json!({ "action": "resume", "agent_id": "$ID0", "prompt": "again" }),
                ),
                (
                    "r2",
                    "AgentControl",
                    json!({ "action": "resume", "agent_id": "$ID0", "prompt": "again" }),
                ),
            ]),
            Reply::text("both asked"),
        ],
        allow(&["Agent", "AgentControl", "Glob"]),
        true,
        |_| {},
    )
    .await;
    let mut evs = submit(&mut env, "go").await;
    let id = spawned(&evs)[0].agent_id.clone();
    evs.extend(submit(&mut env, "resume twice").await);
    let refused = [tool_out(&evs, "r1"), tool_out(&evs, "r2")]
        .into_iter()
        .filter(|(err, _)| *err)
        .count();
    assert_eq!(refused, 1, "exactly one of the two is refused");
    while results(&evs).len() < 2 {
        evs.extend(
            until(&mut env.events, |e| {
                matches!(e, Event::AgentFinished { .. })
            })
            .await,
        );
    }
    let rs: Vec<_> = results(&evs)
        .into_iter()
        .filter(|r| r.agent_id == id)
        .collect();
    assert_eq!(
        rs.iter().map(|r| r.execution).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(env.ctl.agent_runtime().unwrap().execution(&id), Some(2));
}

/// Resume of an agent that still runs: refused, its message path instead.
#[tokio::test]
async fn a_resume_of_a_running_agent_is_refused() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "long [hold]" })),
            Reply::tool(
                "r1",
                "AgentControl",
                json!({ "action": "resume", "agent_id": "$ID0", "prompt": "again" }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agent", "AgentControl"]),
        true,
        |_| {},
    )
    .await;
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    let mut evs = submit(&mut env, "go").await;
    let (err, out) = tool_out(&evs, "r1");
    assert!(
        err && out.contains("still") && out.contains("send it a message"),
        "{out}"
    );
    env.shared.release();
    evs.extend(
        until(&mut env.events, |e| {
            matches!(e, Event::AgentFinished { .. })
        })
        .await,
    );
    let id = spawned(&evs)[0].agent_id.clone();
    assert_eq!(env.ctl.agent_runtime().unwrap().execution(&id), Some(1));
}

/// A resumed agent waits for a slot like a launch: with one slot taken by
/// another agent, it is queued, and runs when the slot is free.
#[tokio::test]
async fn a_resume_waits_for_a_slot_like_a_launch() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({ "task": "a1 [glob]" })),
            Reply::tool(
                "w1",
                "AgentControl",
                json!({ "action": "wait", "agent_id": "$ID0" }),
            ),
            Reply::tool("h1", "Agent", json!({ "task": "b1 [hold]" })),
            Reply::tool(
                "r1",
                "AgentControl",
                json!({ "action": "resume", "agent_id": "$ID0", "prompt": "again" }),
            ),
            Reply::text("parent done"),
        ],
        allow(&["Agent", "AgentControl", "Glob"]),
        true,
        |b| b.agents.max_concurrent = 1,
    )
    .await;
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    let evs = submit(&mut env, "go").await;
    let sp = spawned(&evs);
    let (a, b) = (sp[0].agent_id.clone(), sp[1].agent_id.clone());
    let (err, out) = tool_out(&evs, "r1");
    assert!(!err, "{out}");
    let queued = evs.iter().any(|e| {
        matches!(&e.event, Event::AgentState { agent_id, state: InstanceState::Queued, execution: 2, .. } if *agent_id == a)
    });
    assert!(queued, "the resumed agent waits for the free slot");
    assert_eq!(
        env.ctl.agent_runtime().unwrap().scheduler.stats().active,
        1,
        "{b} holds it"
    );
    env.shared.release();
    let mut all = evs;
    loop {
        if results(&all)
            .iter()
            .any(|r| r.agent_id == a && r.execution == 2)
        {
            break;
        }
        all.extend(
            until(&mut env.events, |e| {
                matches!(e, Event::AgentFinished { .. })
            })
            .await,
        );
    }
    let r2 = results(&all)
        .into_iter()
        .find(|r| r.agent_id == a && r.execution == 2)
        .unwrap();
    assert_eq!(r2.status, "completed");
}

// ─── Admission ───────────────────────────────────────────────────────────────

/// Sixteen run at once with the default slots; the seventeenth waits in the
/// queue, and runs when one ends.
#[tokio::test]
async fn sixteen_run_at_once_and_the_seventeenth_waits() {
    let agents: Vec<serde_json::Value> = (0..17)
        .map(|n| json!({ "task": format!("h{n} [hold]") }))
        .collect();
    let mut env = open(
        vec![
            Reply::tool("c1", "Agents", json!({ "agents": agents })),
            Reply::text("all started"),
        ],
        allow(&["Agents"]),
        true,
        |_| {},
    )
    .await;
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    let evs = submit(&mut env, "go").await;
    let rt = env.ctl.agent_runtime().unwrap();
    for _ in 0..1000 {
        if rt.scheduler.stats().active == 16 && rt.scheduler.stats().queued == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let st = rt.scheduler.stats();
    assert_eq!((st.active, st.queued, st.max_concurrent), (16, 1, 16));
    env.shared.release();
    let mut all = evs;
    while results(&all).len() < 17 {
        all.extend(
            until(&mut env.events, |e| {
                matches!(e, Event::AgentFinished { .. })
            })
            .await,
        );
    }
    assert!(results(&all).iter().all(|r| r.status == "completed"));
}

/// More than 32 tasks one after the other: there is no cumulative cap.
#[tokio::test]
async fn more_than_thirty_two_successive_tasks_are_admitted() {
    let mut replies: Vec<Reply> = (0..40)
        .map(|n| {
            Reply::tool(
                &format!("c{n}"),
                "Agent",
                json!({ "task": format!("step {n}") }),
            )
        })
        .collect();
    replies.push(Reply::text("all forty"));
    let mut env = open(replies, allow(&["Agent"]), true, |_| {}).await;
    let evs = tokio::time::timeout(Duration::from_secs(60), submit(&mut env, "go"))
        .await
        .expect("the harness's own timeout");
    assert_eq!(spawned(&evs).len(), 40);
    let rs = results(&evs);
    assert_eq!(rs.len(), 40);
    assert!(rs.iter().all(|r| r.status == "completed"));
}

/// A batch of twenty is accepted whole: sixteen run, four wait.
#[tokio::test]
async fn a_batch_of_twenty_is_accepted_and_the_extra_four_queue() {
    let agents: Vec<serde_json::Value> = (0..20)
        .map(|n| json!({ "task": format!("b{n} [hold]") }))
        .collect();
    let mut env = open(
        vec![
            Reply::tool("c1", "Agents", json!({ "agents": agents })),
            Reply::text("all started"),
        ],
        allow(&["Agents"]),
        true,
        |_| {},
    )
    .await;
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    let evs = submit(&mut env, "go").await;
    assert_eq!(spawned(&evs).len(), 20, "none refused as a batch");
    let rt = env.ctl.agent_runtime().unwrap();
    for _ in 0..1000 {
        if rt.scheduler.stats().queued == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let st = rt.scheduler.stats();
    assert_eq!((st.active, st.queued), (16, 4));
    env.shared.release();
    let mut all = evs;
    while results(&all).len() < 20 {
        all.extend(
            until(&mut env.events, |e| {
                matches!(e, Event::AgentFinished { .. })
            })
            .await,
        );
    }
    assert!(results(&all).iter().all(|r| r.status == "completed"));
}

/// An admission that expires is refused with nothing left reserved: the next
/// delegation gets the slot once the holder is done.
#[tokio::test]
async fn an_expired_admission_releases_its_reservation() {
    // The holder is launched first and alone, in the shared workspace (no
    // worktree to prepare), so it is the one holding the only slot.
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({ "task": "h [hold]", "isolation": "shared" }),
            ),
            Reply::tool(
                "c0",
                "Agent",
                json!({ "task": "s", "isolation": "shared" }),
            ),
            Reply::text("ok"),
        ],
        allow(&["Agents", "Agent"]),
        true,
        |b| {
            b.agents.max_concurrent = 1;
            b.agents.admission_timeout_ms = 300;
        },
    )
    .await;
    env.shared.auto_wait.store(false, Ordering::SeqCst);
    let evs = submit(&mut env, "go").await;
    let sp = spawned(&evs);
    let (holder, expired) = (sp[0].agent_id.clone(), sp[1].agent_id.clone());
    let mut all = evs;
    all.extend(
        until(
            &mut env.events,
            |e| matches!(e, Event::AgentFinished { result } if result.agent_id == expired),
        )
        .await,
    );
    let r = results(&all)
        .into_iter()
        .find(|r| r.agent_id == expired)
        .unwrap();
    assert_eq!(r.status, "failed");
    assert!(
        r.error
            .as_deref()
            .is_some_and(|e| e.contains("AgentAdmissionTimeout")),
        "{r:?}"
    );
    env.shared.release();
    all.extend(
        until(
            &mut env.events,
            |e| matches!(e, Event::AgentFinished { result } if result.agent_id == holder),
        )
        .await,
    );
    let rt = env.ctl.agent_runtime().unwrap();
    let st = rt.scheduler.stats();
    assert_eq!((st.active, st.queued), (0, 0), "nothing left reserved");
    env.shared.auto_wait.store(true, Ordering::SeqCst);
    env.shared.push_parent(vec![
        Reply::tool("c2", "Agent", json!({ "task": "after" })),
        Reply::text("again done"),
    ]);
    let evs = submit(&mut env, "again").await;
    assert_eq!(results(&evs).len(), 1);
    assert_eq!(results(&evs)[0].status, "completed");
}
