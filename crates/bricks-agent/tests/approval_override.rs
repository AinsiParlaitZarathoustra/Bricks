//! The runtime approval override (`ApprovalMode`), end to end: the priority
//! table at the gate, and through the real controller and a scripted model
//! (an open session, its running sub-agent, a pending request)..
//!
//! Every test uses a temporary project and sessions folder, and a
//! `ProfileSources` that points at the temporary folder: nothing is read
//! from or written to the user's `~/.bricks`.

use bricks_agent::BricksConfig;
use bricks_agent::agents::ProfileSources;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::*;
use bricks_tools::PermissionLevel;
use bricks_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

struct Env {
    ctl: Controller,
    events: EventStream,
    dir: tempfile::TempDir,
    script: Arc<Script>,
    /// The override shared with the engine: a test changes the mode through it,
    /// as the desktop does when the preference is saved.
    mode: Arc<ApprovalOverride>,
}

fn allow(tools: &[&str]) -> ApprovalRules {
    let mut rules = ApprovalRules::default();
    for t in tools {
        rules.tools.insert(t.to_string(), Action::Allow);
    }
    rules
}

/// A session with a scripted model. Replies are pushed by each test, in the
/// order the model will be asked.
async fn open(rules: ApprovalRules, mode: ApprovalMode) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(Vec::new());
    let catalog = ScriptedCatalog::new(&["a", "b"], script.clone());
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.permissions = rules;
    bricks.semantic.lsp.enabled = false;
    let mut cfg = EngineConfig::new(dir.path(), catalog, bricks, dir.path().join(".sessions"));
    cfg.agent_profile_sources = Some(ProfileSources {
        project_dir: Some(dir.path().join(".bricks/agents")),
        user_dir: None,
    });
    let mode = ApprovalOverride::new(mode);
    cfg.approval_override = Arc::clone(&mode);
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
    Env {
        ctl,
        events,
        dir,
        script,
        mode,
    }
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(20), events.next())
        .await
        .expect("an event within 20 s")
        .expect("stream open")
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

fn write_input(path: &std::path::Path) -> serde_json::Value {
    json!({"file_path": path.to_string_lossy(), "content": "contenu\n"})
}

fn request(tool: &str, level: PermissionLevel, id: &str) -> PermissionRequest {
    PermissionRequest {
        tool_name: tool.into(),
        tool_input: json!({}),
        permission_level: level,
        description: format!("run {tool}"),
        id: id.into(),
        preview: None,
        agent_id: None,
    }
}

// ---------------------------------------------------------------- the gate

/// Forbidden tools and explicit `deny` rules are refused whatever the mode,
/// and they never reach a person (the broker here is not interactive, so a
/// question would have been refused too: the test asserts the refusal reason
/// comes from the policy, not from the absence of a person).
#[tokio::test]
async fn forbidden_and_explicit_denials_win_over_every_mode() {
    let mut rules = ApprovalRules::default();
    rules.tools.insert("Bash".into(), Action::Deny);
    rules.dangerous = Action::Deny;
    for mode in [
        ApprovalMode::Standard,
        ApprovalMode::AlwaysAsk,
        ApprovalMode::NoConfirmation,
    ] {
        let gate = ApprovalGate::new(rules.clone(), ApprovalBroker::new(false))
            .with_override(ApprovalOverride::new(mode));
        let cases = [
            ("Locked", PermissionLevel::Forbidden),
            ("Bash", PermissionLevel::Execute),
            ("Shell", PermissionLevel::Dangerous),
        ];
        for (tool, level) in cases {
            let decision = gate.check(&request(tool, level, "x")).await;
            match decision {
                PermissionDecision::Deny(reason) => {
                    assert!(
                        reason.contains(tool),
                        "{mode:?} {tool}: the reason names the tool: {reason}"
                    );
                }
                other => panic!("{mode:?} {tool}: expected a refusal, got {other:?}"),
            }
        }
    }
}

/// `NoConfirmation` allows what the rules would ask; the gate answers without
/// a question, so a non-interactive broker is enough to show it.
#[tokio::test]
async fn no_confirmation_allows_without_asking_at_the_gate() {
    let gate = ApprovalGate::new(ApprovalRules::default(), ApprovalBroker::new(false))
        .with_override(ApprovalOverride::new(ApprovalMode::NoConfirmation));
    for (tool, level) in [
        ("Write", PermissionLevel::Write),
        ("Edit", PermissionLevel::None),
        ("Read", PermissionLevel::ReadOnly),
    ] {
        let decision = gate.check(&request(tool, level, "y")).await;
        assert!(
            matches!(decision, PermissionDecision::Allow),
            "{tool}: {decision:?}"
        );
    }
}

// ------------------------------------------------- through the controller

/// Standard is the project's rules and session grants, unchanged: a read is
/// free, a write is asked once, and the grant answers the next write.
#[tokio::test]
async fn standard_keeps_the_rules_and_the_session_grant() {
    let mut env = open(ApprovalRules::default(), ApprovalMode::Standard).await;
    let f1 = env.dir.path().join("f1.txt");
    let f2 = env.dir.path().join("f2.txt");
    env.script
        .push(Reply::tool("w1", "Write", write_input(&f1)));
    env.script.push(Reply::text("fait"));
    env.script
        .push(Reply::tool("w2", "Write", write_input(&f2)));
    env.script.push(Reply::text("fait"));

    let (asked, evs) = run_answering(&mut env, "écris f1", Decision::AllowForSession).await;
    assert_eq!(asked.len(), 1, "a write is asked under standard");
    assert_eq!(asked[0].tool, "Write");
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    assert!(f1.exists());

    let (asked, evs) = run_answering(&mut env, "écris f2", Decision::Deny).await;
    assert!(asked.is_empty(), "the session grant answers the next write");
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    assert!(f2.exists());
    assert_eq!(env.ctl.snapshot().approval_mode, ApprovalMode::Standard);
}

/// A session grant does not survive `AlwaysAsk`: the same tool is asked again,
/// and a read the rules allow is asked as well.
#[tokio::test]
async fn always_ask_ignores_a_session_grant_and_asks_reads_too() {
    let mut env = open(ApprovalRules::default(), ApprovalMode::Standard).await;
    let f1 = env.dir.path().join("f1.txt");
    let f2 = env.dir.path().join("f2.txt");
    let existing = env.dir.path().join("lu.txt");
    std::fs::write(&existing, "lu\n").unwrap();
    env.script
        .push(Reply::tool("w1", "Write", write_input(&f1)));
    env.script.push(Reply::text("fait"));
    env.script
        .push(Reply::tool("w2", "Write", write_input(&f2)));
    env.script.push(Reply::text("refusé"));
    env.script.push(Reply::tool(
        "r1",
        "Read",
        json!({"file_path": existing.to_string_lossy()}),
    ));
    env.script.push(Reply::text("lu"));

    let (asked, _) = run_answering(&mut env, "écris f1", Decision::AllowForSession).await;
    assert_eq!(asked.len(), 1);

    env.mode.set(ApprovalMode::AlwaysAsk);
    assert_eq!(env.ctl.snapshot().approval_mode, ApprovalMode::AlwaysAsk);

    let (asked, evs) = run_answering(&mut env, "écris f2", Decision::Deny).await;
    assert_eq!(asked.len(), 1, "the grant no longer answers the write");
    assert_eq!(asked[0].tool, "Write");
    let writes = results(&evs, "Write");
    assert_eq!(writes.len(), 1);
    assert!(writes[0].0, "the refused write is an error result");
    assert!(!f2.exists(), "a refused write writes nothing");

    let (asked, evs) = run_answering(&mut env, "lis", Decision::Allow).await;
    assert_eq!(asked.len(), 1, "a read the rules allow is asked");
    assert_eq!(asked[0].tool, "Read");
    let reads = results(&evs, "Read");
    assert_eq!(reads.len(), 1);
    assert!(!reads[0].0, "the approved read runs");
}

/// "Allow for this session" given under AlwaysAsk is a one-time allow: it is
/// not recorded, so the same tool is asked again once the mode is standard.
#[tokio::test]
async fn an_always_ask_session_grant_does_not_survive_a_return_to_standard() {
    let mut env = open(ApprovalRules::default(), ApprovalMode::AlwaysAsk).await;
    let f1 = env.dir.path().join("f1.txt");
    let f2 = env.dir.path().join("f2.txt");
    env.script
        .push(Reply::tool("w1", "Write", write_input(&f1)));
    env.script.push(Reply::text("fait"));
    env.script
        .push(Reply::tool("w2", "Write", write_input(&f2)));
    env.script.push(Reply::text("refusé"));

    let (asked, _) = run_answering(&mut env, "écris f1", Decision::AllowForSession).await;
    assert_eq!(asked.len(), 1, "the first write is asked under always-ask");
    assert!(f1.exists(), "the one-time allow ran the write");
    assert!(
        env.ctl.snapshot().allowed_for_session.is_empty(),
        "the grant is not recorded"
    );

    env.mode.set(ApprovalMode::Standard);
    let (asked, evs) = run_answering(&mut env, "écris f2", Decision::Deny).await;
    assert_eq!(asked.len(), 1, "back to standard, the write is asked again");
    assert_eq!(asked[0].tool, "Write");
    let writes = results(&evs, "Write");
    assert!(writes.len() == 1 && writes[0].0, "the refused write is an error result");
    assert!(!f2.exists(), "a refused write writes nothing");
}

/// `NoConfirmation` runs the calls the rules would ask; an explicit deny rule
/// still refuses the call, with nothing asked.
#[tokio::test]
async fn no_confirmation_runs_writes_but_an_explicit_denial_still_refuses() {
    let mut rules = ApprovalRules::default();
    rules.tools.insert("Bash".into(), Action::Deny);
    let mut env = open(rules, ApprovalMode::NoConfirmation).await;
    let f1 = env.dir.path().join("f1.txt");
    env.script
        .push(Reply::tool("w1", "Write", write_input(&f1)));
    env.script.push(Reply::text("fait"));
    env.script
        .push(Reply::tool("b1", "Bash", json!({"command": "echo hi"})));
    env.script.push(Reply::text("refusé"));

    let (asked, evs) = run_answering(&mut env, "écris", Decision::Deny).await;
    assert!(asked.is_empty(), "nothing is asked under no confirmation");
    assert!(f1.exists(), "the write ran without a question");
    assert!(!results(&evs, "Write")[0].0);

    let (asked, evs) = run_answering(&mut env, "lance", Decision::Allow).await;
    assert!(asked.is_empty(), "an explicit deny is not asked either");
    let bash = results(&evs, "Bash");
    assert_eq!(bash.len(), 1);
    assert!(bash[0].0, "the denied call is an error result");
    assert!(
        bash[0].1.contains("not allowed"),
        "the refusal comes from the policy: {}",
        bash[0].1
    );
}

/// Each change of mode is the next decision of the same open controller:
/// asked under standard, free under no confirmation, asked again under
/// always ask, then back to the project's rules.
#[tokio::test]
async fn each_change_of_mode_is_the_next_decision_of_the_open_controller() {
    let mut env = open(ApprovalRules::default(), ApprovalMode::Standard).await;
    let names = ["a.txt", "b.txt", "c.txt"];
    let paths: Vec<_> = names.iter().map(|n| env.dir.path().join(n)).collect();
    for (i, path) in paths.iter().enumerate() {
        env.script
            .push(Reply::tool(&format!("w{i}"), "Write", write_input(path)));
        env.script.push(Reply::text("fait"));
    }

    let (asked, _) = run_answering(&mut env, "un", Decision::Allow).await;
    assert_eq!(asked.len(), 1, "standard asks the write");

    env.mode.set(ApprovalMode::NoConfirmation);
    let (asked, _) = run_answering(&mut env, "deux", Decision::Allow).await;
    assert!(asked.is_empty(), "no confirmation: not asked");
    assert!(paths[1].exists());

    env.mode.set(ApprovalMode::AlwaysAsk);
    let (asked, _) = run_answering(&mut env, "trois", Decision::Allow).await;
    assert_eq!(asked.len(), 1, "always ask: asked again");
    assert!(paths[2].exists());
}

/// A running sub-agent takes the change at its next decision. Its first write
/// waits for a person; the mode changes meanwhile. The waiting request is
/// not answered by the change, and the sub-agent's second write runs without
/// a question under no confirmation.
#[tokio::test]
async fn a_running_sub_agent_takes_the_change_at_its_next_decision() {
    let mut env = open(allow(&["Agent"]), ApprovalMode::Standard).await;
    let f1 = env.dir.path().join("sous-1.txt");
    let f2 = env.dir.path().join("sous-2.txt");
    env.script.push(Reply::tool(
        "c1",
        "Agent",
        json!({"task": "écrire deux fichiers"}),
    ));
    env.script
        .push(Reply::tool("cw1", "Write", write_input(&f1)));
    env.script
        .push(Reply::tool("cw2", "Write", write_input(&f2)));
    env.script.push(Reply::text("sous-agent terminé"));
    env.script.push(Reply::text("ok"));

    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("délègue"),
        })
        .unwrap();

    // The sub-agent's first write is asked, and waits.
    let mut buffered = Vec::new();
    let first = loop {
        let e = next(&mut env.events).await;
        let found = match &e.event {
            Event::ApprovalRequested { approval } => Some(approval.clone()),
            _ => None,
        };
        buffered.push(e);
        if let Some(a) = found {
            break a;
        }
    };
    assert_eq!(first.tool, "Write");
    assert!(
        first
            .agent_id
            .as_deref()
            .is_some_and(|id| id.starts_with("agent_")),
        "the question comes from the sub-agent: {:?}",
        first.agent_id
    );

    // The change lands while the question is open.
    env.mode.set(ApprovalMode::NoConfirmation);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        env.ctl.snapshot().pending_approvals.len(),
        1,
        "the change does not answer the open question"
    );
    while let Some(e) = env.events.try_next() {
        assert!(
            !matches!(e.event, Event::ApprovalResolved { .. }),
            "no question is resolved by a change of mode: {:?}",
            e.event.kind()
        );
        buffered.push(e);
    }

    env.ctl
        .send(Command::Approve {
            approval_id: first.approval_id.clone(),
            decision: Decision::Allow,
            reason: None,
        })
        .unwrap();

    // The rest of the run: any further question would be answered here, and
    // counted.
    let mut later = Vec::new();
    loop {
        let e = next(&mut env.events).await;
        if let Event::ApprovalRequested { approval } = &e.event {
            later.push(approval.clone());
            env.ctl
                .send(Command::Approve {
                    approval_id: approval.approval_id.clone(),
                    decision: Decision::Allow,
                    reason: None,
                })
                .unwrap();
        }
        let done = matches!(e.event, Event::RunFinished { .. });
        buffered.push(e);
        if done {
            break;
        }
    }

    assert!(later.is_empty(), "the second write is not asked: {later:?}");
    assert!(f1.exists() && f2.exists(), "both writes ran");
    assert_eq!(outcome(&buffered), RunOutcome::Succeeded);
}
