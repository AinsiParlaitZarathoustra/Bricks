//! Permissions, end to end: the session's policy decides every call (ASK,
//! EXECUTE or CUSTOM; forbidden tools always refused), an answer covers one
//! call, a change of policy is the next decision (never an answer to a
//! waiting question), a hook's changed input is decided again, the policy is
//! stored with its session, and the obsolete `[permissions]` section of
//! `bricks.toml` is reported and never applied.
//!
//! Every controller test uses a temporary project and sessions folder; no
//! file of the user's `~/.bricks` is read or written.

#[allow(dead_code)]
mod common;

use async_trait::async_trait;
use bricks_agent::agents::ProfileSources;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog, ScriptedProvider};
use bricks_agent::control::*;
use bricks_agent::permission::{
    CustomRules, PermissionEffect, PermissionEngine, PermissionProfile, PolicyCell, SessionPolicy,
    load_session_policy,
};
use bricks_agent::{Agent, BricksConfig};
use bricks_hooks::{Hook, HookAction, HookContext, HookEvent};
use bricks_provider::Provider;
use bricks_tools::permissions::{
    InteractivePolicy, PermissionDecision, PermissionPolicy, PermissionRequest,
};
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use common::follow::{FollowHandles, Waited};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ─── The engine, without a session ───────────────────────────────────────────

/// The approver of the engine tests: it agrees, and an `AllowOnce` verdict
/// is what shows that a person was asked.
#[derive(Default)]
struct Asked(Mutex<Vec<String>>);

#[async_trait]
impl PermissionPolicy for Asked {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        self.0.lock().unwrap().push(request.tool_name.clone());
        PermissionDecision::Allow
    }
}

fn request(tool: &str, level: PermissionLevel, category: ToolCategory) -> PermissionRequest {
    PermissionRequest {
        tool_name: tool.into(),
        tool_input: json!({}),
        permission_level: level,
        category,
        description: format!("run {tool}"),
        id: format!("id-{tool}"),
        preview: None,
        agent_id: None,
        policy_revision: None,
    }
}

/// The verdict label of a call under `policy` at `revision`, and whether a
/// person was asked.
async fn decide(
    policy: SessionPolicy,
    revision: u64,
    tool: &str,
    level: PermissionLevel,
    category: ToolCategory,
) -> (String, bool) {
    let asked = Arc::new(Asked::default());
    let engine = PermissionEngine::new(PolicyCell::new(policy, revision), asked.clone());
    let label = match engine.check(&request(tool, level, category)).await {
        PermissionDecision::Allow => "allow".to_string(),
        PermissionDecision::AllowOnce => "allow once".to_string(),
        PermissionDecision::Deny(reason) => format!("deny: {reason}"),
    };
    let was_asked = !asked.0.lock().unwrap().is_empty();
    (label, was_asked)
}

/// The session's files folder, where its `permissions.json` is kept.
fn session_files(dir: &Path, id: &str) -> std::path::PathBuf {
    dir.join(".sessions").join(format!("{id}.files"))
}

fn ask() -> SessionPolicy {
    SessionPolicy::chosen(PermissionProfile::Ask)
}

fn execute() -> SessionPolicy {
    SessionPolicy::chosen(PermissionProfile::Execute)
}

fn custom(edit: impl FnOnce(&mut CustomRules)) -> SessionPolicy {
    common::custom(edit)
}

#[tokio::test]
async fn ask_lets_reads_run_and_asks_writes_commands_network_and_sub_agents() {
    let (label, asked) = decide(
        ask(),
        0,
        "Read",
        PermissionLevel::ReadOnly,
        ToolCategory::FileSystem,
    )
    .await;
    assert_eq!((label.as_str(), asked), ("allow", false));
    for (tool, level, category) in [
        ("Write", PermissionLevel::Write, ToolCategory::FileSystem),
        ("Bash", PermissionLevel::Execute, ToolCategory::Shell),
        ("WebFetch", PermissionLevel::ReadOnly, ToolCategory::Web),
        (
            "Agent",
            PermissionLevel::Execute,
            ToolCategory::Orchestration,
        ),
    ] {
        let (label, asked) = decide(ask(), 0, tool, level, category).await;
        assert_eq!((label.as_str(), asked), ("allow once", true), "{tool}");
    }
}

#[tokio::test]
async fn execute_runs_everything_but_the_forbidden() {
    for (tool, level, category) in [
        ("Write", PermissionLevel::Write, ToolCategory::FileSystem),
        ("Bash", PermissionLevel::Dangerous, ToolCategory::Shell),
        ("WebFetch", PermissionLevel::ReadOnly, ToolCategory::Web),
        (
            "Agent",
            PermissionLevel::Execute,
            ToolCategory::Orchestration,
        ),
    ] {
        let (label, asked) = decide(execute(), 0, tool, level, category).await;
        assert_eq!((label.as_str(), asked), ("allow", false), "{tool}");
    }
    let (label, asked) = decide(
        execute(),
        0,
        "Locked",
        PermissionLevel::Forbidden,
        ToolCategory::Shell,
    )
    .await;
    assert!(label.starts_with("deny") && !asked, "{label}");
}

#[tokio::test]
async fn custom_class_effects_and_a_tool_rule_that_wins_over_its_class() {
    let policy = custom(|c| {
        c.write = PermissionEffect::Deny;
        c.tools.insert("Write".into(), PermissionEffect::Allow);
    });
    let (label, asked) = decide(
        policy.clone(),
        0,
        "Edit",
        PermissionLevel::Write,
        ToolCategory::FileSystem,
    )
    .await;
    assert!(label.starts_with("deny") && !asked, "{label}");
    let (label, asked) = decide(
        policy.clone(),
        0,
        "Write",
        PermissionLevel::Write,
        ToolCategory::FileSystem,
    )
    .await;
    assert_eq!(
        (label.as_str(), asked),
        ("allow", false),
        "the tool rule wins"
    );
    // The other classes keep their ASK effects.
    let (label, asked) = decide(
        policy.clone(),
        0,
        "Read",
        PermissionLevel::ReadOnly,
        ToolCategory::FileSystem,
    )
    .await;
    assert_eq!((label.as_str(), asked), ("allow", false));
    let (_, asked) = decide(
        policy.clone(),
        0,
        "Bash",
        PermissionLevel::Execute,
        ToolCategory::Shell,
    )
    .await;
    assert!(asked, "commands are asked");
    let (_, asked) = decide(
        policy,
        0,
        "WebFetch",
        PermissionLevel::ReadOnly,
        ToolCategory::Web,
    )
    .await;
    assert!(asked, "network is asked");
}

#[tokio::test]
async fn a_custom_denial_is_refused_with_its_reason_and_the_revision() {
    let policy = custom(|c| {
        c.tools.insert("Bash".into(), PermissionEffect::Deny);
    });
    let (label, asked) = decide(
        policy,
        4,
        "Bash",
        PermissionLevel::Execute,
        ToolCategory::Shell,
    )
    .await;
    assert!(!asked, "a denial is never put to a person");
    assert!(label.contains("own rule"), "{label}");
    assert!(label.contains("policy revision 4"), "{label}");
}

#[tokio::test]
async fn forbidden_stays_denied_under_custom_even_with_an_allow_rule() {
    let policy = custom(|c| {
        c.tools.insert("Locked".into(), PermissionEffect::Allow);
    });
    let (label, asked) = decide(
        policy,
        0,
        "Locked",
        PermissionLevel::Forbidden,
        ToolCategory::Shell,
    )
    .await;
    assert!(label.starts_with("deny") && !asked, "{label}");
}

#[tokio::test]
async fn an_unchosen_policy_asks_even_if_its_profile_says_execute() {
    let mut policy = SessionPolicy::unchosen();
    policy.profile = PermissionProfile::Execute;
    let (_, asked) = decide(
        policy,
        0,
        "Bash",
        PermissionLevel::Execute,
        ToolCategory::Shell,
    )
    .await;
    assert!(asked, "nothing was chosen: the command is asked");
}

// ─── The obsolete [permissions] section ──────────────────────────────────────

#[tokio::test]
async fn a_permissions_section_is_reported_and_its_rules_are_not_applied() {
    let text = "[permissions]\nwrite = \"deny\"\nexecute = \"allow\"\n\n[permissions.tools]\nWrite = \"allow\"\n";
    let mut bricks = BricksConfig::from_texts(Some(text), &[]);
    bricks.agent.model = Some("test/a".into());
    assert!(
        bricks
            .diagnostics
            .iter()
            .any(|d| d.contains(bricks_agent::control::settings::OBSOLETE_PERMISSIONS)),
        "{:?}",
        bricks.diagnostics
    );

    // Its denial of writes is not applied: the write is asked (and runs once
    // a person agrees), where the old rule would have refused it.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("x.txt");
    let script = Script::new(vec![
        Reply::tool("w1", "Write", write_input(&target)),
        Reply::text("fait"),
    ]);
    let catalog = ScriptedCatalog::new(&["a"], script.clone());
    let mut env = start(dir, catalog, bricks, None).await;
    let (asked, evs) = run_answering(&mut env, "écris", Decision::Allow).await;
    assert_eq!(
        asked.len(),
        1,
        "the write is asked, not refused by the section"
    );
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    assert!(target.exists());
}

// ─── Through the controller ──────────────────────────────────────────────────

/// An open session, its scripted model, and its folders.
struct Env {
    ctl: Controller,
    events: EventStream,
    dir: tempfile::TempDir,
}

fn bricks() -> BricksConfig {
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.semantic.lsp.enabled = false;
    bricks
}

/// A new session in `dir`. `default` is the default policy of the store
/// (`None`: no store, the session starts unchosen).
async fn start(
    dir: tempfile::TempDir,
    catalog: Arc<dyn ModelCatalog>,
    bricks: BricksConfig,
    default: Option<SessionPolicy>,
) -> Env {
    let mut cfg = EngineConfig::new(dir.path(), catalog, bricks, dir.path().join(".sessions"));
    cfg.agent_profile_sources = Some(ProfileSources {
        project_dir: Some(dir.path().join(".bricks/agents")),
        user_dir: None,
    });
    cfg.permissions = default.map(|p| common::default_store(dir.path(), &p));
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
    next(&mut events).await; // session_opened
    Env { ctl, events, dir }
}

/// A new session with scripted replies.
async fn open(default: Option<SessionPolicy>, replies: Vec<Reply>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(replies);
    let catalog = ScriptedCatalog::new(&["a", "b"], script.clone());
    start(dir, catalog, bricks(), default).await
}

/// A new session whose sub-agents are awaited by the scripted models, as a
/// model would await them.
async fn open_with_sub_agents(default: Option<SessionPolicy>, replies: Vec<Reply>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(replies);
    let catalog = Arc::new(Follow {
        inner: ScriptedCatalog::new(&["a", "b"], script.clone()),
        waited: Arc::new(Waited::default()),
        enabled: Arc::new(AtomicBool::new(true)),
    });
    start(dir, catalog, bricks(), default).await
}

/// Opens the session `id` of `dir` again, with `default` as the store's
/// default (a stored policy of the session still wins).
async fn resume(
    dir: &Path,
    id: &str,
    default: Option<SessionPolicy>,
) -> Result<(Controller, EventStream), String> {
    let catalog = ScriptedCatalog::new(&["a"], Script::new(Vec::new()));
    let mut cfg = EngineConfig::new(dir, catalog, bricks(), dir.join(".sessions"));
    cfg.permissions = default.map(|p| common::default_store(dir, &p));
    Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::Resume(id.to_string()),
            model: None,
            reasoning: None,
        },
    )
    .await
}

/// The scripted models of [`open_with_sub_agents`]: each waits for the
/// sub-agents it was given while `enabled`.
struct Follow {
    inner: Arc<ScriptedCatalog>,
    waited: Arc<Waited>,
    enabled: Arc<AtomicBool>,
}

impl ModelCatalog for Follow {
    fn models(&self) -> Vec<ModelChoice> {
        self.inner.models()
    }
    fn build(&self, selection: &str, reasoning: Option<&str>) -> Result<Box<dyn Provider>, String> {
        Ok(Box::new(FollowHandles {
            inner: self.inner.build(selection, reasoning)?,
            selection: selection.to_string(),
            waited: Arc::clone(&self.waited),
            enabled: Arc::clone(&self.enabled),
        }))
    }
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(20), events.next())
        .await
        .expect("an event within 20 s")
        .expect("stream open")
}

/// The envelopes up to and including the first one matching `pred`.
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

/// Submits `prompt` and answers every approval with `answer`. Returns the
/// approvals asked, in order, and every envelope of the run.
async fn run_answering(
    env: &mut Env,
    prompt: &str,
    answer: Decision,
) -> (Vec<ApprovalRequest>, Vec<Envelope>) {
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text(prompt),
        })
        .unwrap();
    let mut asked = Vec::new();
    let mut evs = Vec::new();
    loop {
        let e = next(&mut env.events).await;
        if let Event::ApprovalRequested { approval } = &e.event {
            asked.push(approval.clone());
            env.ctl
                .send(Command::Approve {
                    approval_id: approval.approval_id.clone(),
                    decision: answer,
                    reason: None,
                })
                .unwrap();
        }
        let done = matches!(e.event, Event::RunFinished { .. });
        evs.push(e);
        if done {
            return (asked, evs);
        }
    }
}

/// `(is_error, output)` of every finished call of `name`, in order.
fn results(evs: &[Envelope], name: &str) -> Vec<(bool, String)> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::ToolFinished {
                name: n,
                is_error,
                output,
                ..
            } if n == name => Some((*is_error, output.clone())),
            _ => None,
        })
        .collect()
}

fn outcome(evs: &[Envelope]) -> RunOutcome {
    match &evs.last().unwrap().event {
        Event::RunFinished { outcome, .. } => *outcome,
        other => panic!("not a run_finished: {other:?}"),
    }
}

fn write_input(path: &Path) -> Value {
    json!({"file_path": path.to_string_lossy(), "content": "contenu\n"})
}

#[tokio::test]
async fn an_answer_covers_one_call_only() {
    let dir = tempfile::tempdir().unwrap();
    let f1 = dir.path().join("f1.txt");
    let f2 = dir.path().join("f2.txt");
    let script = Script::new(vec![
        Reply::tool("w1", "Write", write_input(&f1)),
        Reply::tool("w2", "Write", write_input(&f2)),
        Reply::text("fait"),
    ]);
    let catalog = ScriptedCatalog::new(&["a", "b"], script.clone());
    let mut env = start(dir, catalog, bricks(), None).await;
    let (asked, evs) = run_answering(&mut env, "deux écritures", Decision::Allow).await;
    assert_eq!(
        asked.len(),
        2,
        "each write is asked, the answer does not carry over"
    );
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    assert!(f1.exists() && f2.exists());
}

#[tokio::test]
async fn a_pending_request_is_not_answered_by_a_policy_change_and_later_calls_follow_it() {
    let dir = tempfile::tempdir().unwrap();
    let f1 = dir.path().join("f1.txt");
    let f2 = dir.path().join("f2.txt");
    let script = Script::new(vec![
        Reply::tool("w1", "Write", write_input(&f1)),
        Reply::tool("w2", "Write", write_input(&f2)),
        Reply::text("fini"),
    ]);
    let catalog = ScriptedCatalog::new(&["a", "b"], script.clone());
    let mut env = start(dir, catalog, bricks(), None).await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("écris"),
        })
        .unwrap();
    // The first write is asked under the unchosen (ASK) policy, revision 0.
    let before = until(&mut env.events, |e| {
        matches!(e, Event::ApprovalRequested { .. })
    })
    .await;
    let first = match &before.last().unwrap().event {
        Event::ApprovalRequested { approval } => approval.clone(),
        other => panic!("not an approval: {other:?}"),
    };
    assert_eq!(first.policy_revision, Some(0));

    env.ctl
        .send(Command::SetPermissionPolicy { policy: execute() })
        .unwrap();
    let changed = until(&mut env.events, |e| {
        matches!(e, Event::PermissionPolicyChanged { .. })
    })
    .await;
    assert!(
        !changed
            .iter()
            .any(|e| matches!(e.event, Event::ApprovalResolved { .. })),
        "the change does not resolve the waiting request"
    );
    assert_eq!(env.ctl.snapshot().pending_approvals.len(), 1);

    env.ctl
        .send(Command::Approve {
            approval_id: first.approval_id.clone(),
            decision: Decision::Deny,
            reason: Some("non".into()),
        })
        .unwrap();
    // The rest of the run follows the new revision: the second write runs
    // without a question.
    let mut rest = Vec::new();
    loop {
        let e = next(&mut env.events).await;
        if let Event::ApprovalRequested { .. } = e.event {
            panic!("no question after the change: {:?}", e.event);
        }
        let done = matches!(e.event, Event::RunFinished { .. });
        rest.push(e);
        if done {
            break;
        }
    }
    assert!(!f1.exists(), "the refused write wrote nothing");
    assert!(f2.exists(), "the write decided under EXECUTE ran");
    assert!(
        results(&rest, "Write")[0].0,
        "the first write is an error result"
    );
    assert!(!results(&rest, "Write")[1].0);
    assert_eq!(outcome(&rest), RunOutcome::Succeeded);
}

#[tokio::test]
async fn set_permission_policy_emits_increasing_revisions_and_the_snapshot_follows() {
    let mut env = open(None, vec![]).await;
    let id = env.ctl.session_id();
    assert_eq!(env.ctl.snapshot().permission_revision, 0);
    assert!(!env.ctl.snapshot().permission_policy.chosen);

    env.ctl
        .send(Command::SetPermissionPolicy { policy: execute() })
        .unwrap();
    let evs = until(&mut env.events, |e| {
        matches!(e, Event::PermissionPolicyChanged { .. })
    })
    .await;
    let Event::PermissionPolicyChanged { policy, revision } = &evs.last().unwrap().event else {
        unreachable!()
    };
    assert_eq!((policy.clone(), *revision), (execute(), 1));

    let custom_policy = custom(|c| {
        c.tools.insert("Bash".into(), PermissionEffect::Deny);
    });
    env.ctl
        .send(Command::SetPermissionPolicy {
            policy: custom_policy.clone(),
        })
        .unwrap();
    let evs = until(&mut env.events, |e| {
        matches!(e, Event::PermissionPolicyChanged { .. })
    })
    .await;
    let Event::PermissionPolicyChanged { revision, .. } = &evs.last().unwrap().event else {
        unreachable!()
    };
    assert_eq!(*revision, 2, "revisions grow at each change");

    let snap = env.ctl.snapshot();
    assert_eq!(snap.permission_revision, 2);
    assert_eq!(snap.permission_policy, custom_policy);
    // Stored with the session, under its revision.
    let stored = load_session_policy(&session_files(env.dir.path(), &id))
        .unwrap()
        .expect("the session's policy is stored");
    assert_eq!((stored.policy, stored.revision), (custom_policy, 2));
}

#[tokio::test]
async fn a_denial_says_which_revision_refused_the_call() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("n.txt");
    let script = Script::new(vec![
        Reply::tool("w1", "Write", write_input(&target)),
        Reply::text("refusé"),
    ]);
    let catalog = ScriptedCatalog::new(&["a"], script.clone());
    let mut env = start(dir, catalog, bricks(), None).await;
    env.ctl
        .send(Command::SetPermissionPolicy {
            policy: custom(|c| {
                c.tools.insert("Write".into(), PermissionEffect::Deny);
            }),
        })
        .unwrap();
    until(&mut env.events, |e| {
        matches!(e, Event::PermissionPolicyChanged { .. })
    })
    .await;
    let (asked, evs) = run_answering(&mut env, "écris", Decision::Allow).await;
    assert!(asked.is_empty(), "a denial is not asked");
    let writes = results(&evs, "Write");
    assert!(writes[0].0, "the write is an error result");
    assert!(
        writes[0].1.contains("Permission denied") && writes[0].1.contains("policy revision 1"),
        "{}",
        writes[0].1
    );
    assert!(!target.exists());
}

#[tokio::test]
async fn a_custom_tool_rule_wins_over_its_class_in_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let f1 = dir.path().join("f1.txt");
    let existing = dir.path().join("lu.txt");
    std::fs::write(&existing, "alpha\n").unwrap();
    let script = Script::new(vec![
        Reply::tool("w1", "Write", write_input(&f1)),
        Reply::tool(
            "e1",
            "Edit",
            json!({"file_path": existing.to_string_lossy(), "old_string": "alpha", "new_string": "beta"}),
        ),
        Reply::text("refusé"),
    ]);
    let catalog = ScriptedCatalog::new(&["a"], script.clone());
    let default = custom(|c| {
        c.write = PermissionEffect::Deny;
        c.tools.insert("Write".into(), PermissionEffect::Allow);
    });
    let mut env = start(dir, catalog, bricks(), Some(default)).await;
    let (asked, evs) = run_answering(&mut env, "écris", Decision::Deny).await;
    assert!(asked.is_empty(), "no call is asked under this policy");
    assert!(f1.exists(), "the tool rule lets the write run");
    assert!(!results(&evs, "Write")[0].0);
    let edits = results(&evs, "Edit");
    assert!(edits[0].0, "the write class refuses the edit");
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "alpha\n");
}

#[tokio::test]
async fn a_session_stores_its_policy_and_resumes_with_it_not_with_the_default() {
    let mut env = open(None, vec![Reply::text("bonjour")]).await;
    // A session is stored once it has an exchange.
    run_answering(&mut env, "salut", Decision::Deny).await;
    let id = env.ctl.session_id();
    let policy = custom(|c| {
        c.execute = PermissionEffect::Allow;
        c.tools.insert("Bash".into(), PermissionEffect::Deny);
    });
    env.ctl
        .send(Command::SetPermissionPolicy {
            policy: policy.clone(),
        })
        .unwrap();
    until(&mut env.events, |e| {
        matches!(e, Event::PermissionPolicyChanged { .. })
    })
    .await;
    let dir = env.dir.path().to_path_buf();
    env.ctl.close().await;

    // Resumed with another default (EXECUTE): the session keeps its own.
    let (ctl, mut events) = resume(&dir, &id, Some(execute())).await.unwrap();
    next(&mut events).await;
    let snap = ctl.snapshot();
    assert_eq!(snap.session_id, id);
    assert_eq!(snap.permission_policy, policy);
    assert_eq!(snap.permission_revision, 1);
}

#[tokio::test]
async fn a_new_session_copies_the_default_and_then_keeps_its_own_policy() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = ScriptedCatalog::new(&["a"], Script::new(vec![Reply::text("bonjour")]));
    let mut env = start(dir, catalog, bricks(), Some(execute())).await;
    run_answering(&mut env, "salut", Decision::Deny).await;
    let first_id = env.ctl.session_id();
    let snap = env.ctl.snapshot();
    assert_eq!(
        (snap.permission_policy, snap.permission_revision),
        (execute(), 0)
    );
    let stored = load_session_policy(&session_files(env.dir.path(), &first_id))
        .unwrap()
        .expect("the copy is stored with the session");
    assert_eq!(stored.policy, execute());
    let root = env.dir.path().to_path_buf();
    env.ctl.close().await;

    // The default changes; the session still has the policy it was given.
    let (ctl, mut events) = resume(&root, &first_id, Some(ask())).await.unwrap();
    next(&mut events).await;
    assert_eq!(ctl.snapshot().permission_policy, execute());
    ctl.close().await;

    // A new session in the same folder copies the new default.
    let (ctl, mut events) = resume_new(&root, Some(ask())).await;
    next(&mut events).await;
    assert_eq!(ctl.snapshot().permission_policy, ask());
    assert_ne!(ctl.session_id(), first_id);
}

async fn resume_new(dir: &Path, default: Option<SessionPolicy>) -> (Controller, EventStream) {
    let catalog = ScriptedCatalog::new(&["a"], Script::new(Vec::new()));
    let mut cfg = EngineConfig::new(dir, catalog, bricks(), dir.join(".sessions"));
    cfg.permissions = default.map(|p| common::default_store(dir, &p));
    Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn an_unreadable_session_policy_is_an_error_never_an_allow() {
    let mut env = open(None, vec![Reply::text("bonjour")]).await;
    run_answering(&mut env, "salut", Decision::Deny).await;
    let id = env.ctl.session_id();
    let root = env.dir.path().to_path_buf();
    env.ctl.close().await;
    std::fs::write(
        session_files(&root, &id).join("permissions.json"),
        "{ not json",
    )
    .unwrap();

    let err = match resume(&root, &id, Some(execute())).await {
        Err(e) => e,
        Ok(_) => panic!("a session whose policy cannot be read must not open"),
    };
    assert!(err.contains("permissions.json"), "{err}");
}

#[tokio::test]
async fn a_sub_agent_asks_under_its_sessions_policy() {
    let dir_default = custom(|c| {
        c.tools.insert("Agent".into(), PermissionEffect::Allow);
    });
    let mut env = open_with_sub_agents(
        Some(dir_default),
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({"task": "écrire un fichier", "isolation": "shared"}),
            ),
            Reply::tool(
                "cw1",
                "Write",
                json!({"file_path": "sous.txt", "content": "sous\n"}),
            ),
            Reply::text("sous-agent terminé"),
            Reply::text("ok"),
        ],
    )
    .await;
    let (asked, evs) = run_answering(&mut env, "délègue", Decision::Allow).await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    // The child's write is asked under the parent's policy, and says so.
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].tool, "Write");
    assert!(
        asked[0]
            .agent_id
            .as_deref()
            .is_some_and(|id| id.starts_with("agent_")),
        "the question comes from the sub-agent: {:?}",
        asked[0].agent_id
    );
    assert!(env.dir.path().join("sous.txt").exists());
}

#[tokio::test]
async fn a_running_sub_agent_takes_a_policy_change_at_its_next_call() {
    let default = custom(|c| {
        c.tools.insert("Agent".into(), PermissionEffect::Allow);
    });
    let mut env = open_with_sub_agents(
        Some(default),
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({"task": "écrire deux fichiers", "isolation": "shared"}),
            ),
            Reply::tool(
                "cw1",
                "Write",
                json!({"file_path": "sous-1.txt", "content": "un\n"}),
            ),
            Reply::tool(
                "cw2",
                "Write",
                json!({"file_path": "sous-2.txt", "content": "deux\n"}),
            ),
            Reply::text("sous-agent terminé"),
            Reply::text("ok"),
        ],
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("délègue"),
        })
        .unwrap();

    // The child's first write is asked, and waits.
    let mut buffered = until(&mut env.events, |e| {
        matches!(e, Event::ApprovalRequested { .. })
    })
    .await;
    let first = match &buffered.last().unwrap().event {
        Event::ApprovalRequested { approval } => approval.clone(),
        other => panic!("not an approval: {other:?}"),
    };
    assert_eq!(first.tool, "Write");

    // The policy changes while the question is open.
    env.ctl
        .send(Command::SetPermissionPolicy { policy: execute() })
        .unwrap();
    buffered.extend(
        until(&mut env.events, |e| {
            matches!(e, Event::PermissionPolicyChanged { .. })
        })
        .await,
    );
    assert!(
        !buffered
            .iter()
            .any(|e| matches!(e.event, Event::ApprovalResolved { .. })),
        "no question is resolved by a change of policy"
    );
    assert_eq!(env.ctl.snapshot().pending_approvals.len(), 1);

    env.ctl
        .send(Command::Approve {
            approval_id: first.approval_id.clone(),
            decision: Decision::Allow,
            reason: None,
        })
        .unwrap();

    // The second write is decided under EXECUTE: not asked.
    let mut later = Vec::new();
    loop {
        let e = next(&mut env.events).await;
        if let Event::ApprovalRequested { approval } = &e.event {
            later.push(approval.clone());
        }
        let done = matches!(e.event, Event::RunFinished { .. });
        buffered.push(e);
        if done {
            break;
        }
    }
    assert!(later.is_empty(), "the second write is not asked: {later:?}");
    assert!(env.dir.path().join("sous-1.txt").exists());
    assert!(env.dir.path().join("sous-2.txt").exists());
    assert_eq!(outcome(&buffered), RunOutcome::Succeeded);
}

// ─── A hook that changes the input ───────────────────────────────────────────

/// A tool whose level depends on its input: `mode: "read"` reads, anything
/// else writes.
struct Probe;

#[async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        "Probe"
    }

    fn description(&self) -> &str {
        "A probe whose level depends on its mode."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"mode": {"type": "string"}}})
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn permission_level_for(&self, input: &Value) -> PermissionLevel {
        if input["mode"] == "read" {
            PermissionLevel::ReadOnly
        } else {
            PermissionLevel::Write
        }
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::success(format!("ran {}", input["mode"]))
    }
}

/// A hook running a closure on every PreToolUse.
struct OnPreToolUse<F>(F);

#[async_trait]
impl<F> Hook for OnPreToolUse<F>
where
    F: Fn(&HookContext) -> HookAction + Send + Sync,
{
    fn events(&self) -> &[HookEvent] {
        &[HookEvent::PreToolUse]
    }

    async fn on_event(&self, ctx: &HookContext) -> HookAction {
        (self.0)(ctx)
    }
}

#[tokio::test]
async fn a_hook_that_changes_the_input_gets_the_new_input_decided_again() {
    let dir = tempfile::tempdir().unwrap();
    let asked: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    let approver = InteractivePolicy::new(move |req: &PermissionRequest| {
        seen.lock().unwrap().push(req.tool_input.clone());
        PermissionDecision::Deny("pas aujourd'hui".into())
    });
    let out = Agent::builder()
        .provider(ScriptedProvider::new(
            "a",
            Script::new(vec![
                Reply::tool("p1", "Probe", json!({"mode": "read"})),
                Reply::text("fini"),
            ]),
        ))
        .model("a")
        .working_dir(dir.path())
        .tool(Probe)
        .permission_profile(ask())
        .permission_policy(approver)
        .hook(OnPreToolUse(|ctx: &HookContext| {
            let input = ctx.tool_input.clone().unwrap_or_default();
            if input["mode"] == "read" {
                return HookAction::ModifyInput(json!({"mode": "write"}));
            }
            HookAction::Continue
        }))
        .build()
        .unwrap()
        .run("sonde")
        .await
        .unwrap();
    // The read needed no question on its own; the changed input was asked,
    // and refused, so the probe did not run.
    assert_eq!(*asked.lock().unwrap(), vec![json!({"mode": "write"})]);
    let call = out.tool_calls.iter().find(|c| c.id == "p1").unwrap();
    assert!(
        call.is_error && call.result.contains("Permission denied"),
        "{}",
        call.result
    );
}
