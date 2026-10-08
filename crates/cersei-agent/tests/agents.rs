//! Native sub-agents through the real controller, with a scripted model:
//! foreground delegation, validation before anything is built, inherited
//! policy, fresh context, admission, limits, cancellation, compaction.

use cersei_agent::agents::{InstanceState, ProfileSources};
use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::*;
use cersei_agent::BricksConfig;
use cersei_provider::{CompletionRequest, Provider};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Counts the providers built (each child builds one).
struct Counting {
    inner: Arc<ScriptedCatalog>,
    builds: AtomicUsize,
}

impl ModelCatalog for Counting {
    fn models(&self) -> Vec<ModelChoice> {
        self.inner.models()
    }
    fn build(&self, selection: &str, reasoning: Option<&str>) -> Result<Box<dyn Provider>, String> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        self.inner.build(selection, reasoning)
    }
}

struct Env {
    ctl: Controller,
    events: EventStream,
    dir: tempfile::TempDir,
    script: Arc<Script>,
    catalog: Arc<Counting>,
}

fn allow(tools: &[&str]) -> ApprovalRules {
    let mut r = ApprovalRules::default();
    for t in tools {
        r.tools.insert(t.to_string(), Action::Allow);
    }
    r
}

async fn open(replies: Vec<Reply>, rules: ApprovalRules) -> Env {
    open_with(replies, rules, |_| {}).await
}

async fn open_with(
    replies: Vec<Reply>,
    rules: ApprovalRules,
    tweak: impl FnOnce(&mut BricksConfig),
) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(replies);
    let catalog = Arc::new(Counting {
        inner: ScriptedCatalog::new(&["a", "b"], script.clone()),
        builds: AtomicUsize::new(0),
    });
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.permissions = rules;
    bricks.semantic.lsp.enabled = false;
    tweak(&mut bricks);
    let mut cfg = EngineConfig::new(
        dir.path(),
        catalog.clone(),
        bricks,
        dir.path().join(".sessions"),
    );
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
    // session_opened
    next(&mut events).await;
    Env {
        ctl,
        events,
        dir,
        script,
        catalog,
    }
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(20), events.next())
        .await
        .expect("an event within 20 s")
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

fn is_child(r: &CompletionRequest) -> bool {
    r.system
        .as_deref()
        .is_some_and(|s| s.contains("You are a sub-agent"))
}

fn finished(evs: &[Envelope]) -> Vec<cersei_agent::agents::AgentResult> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::AgentFinished { result } => Some((**result).clone()),
            _ => None,
        })
        .collect()
}

fn run_outcome(evs: &[Envelope]) -> (RunOutcome, String) {
    match &evs.last().unwrap().event {
        Event::RunFinished { outcome, text, .. } => (*outcome, text.clone()),
        _ => unreachable!(),
    }
}

fn tool_output(evs: &[Envelope], name: &str) -> (bool, String) {
    evs.iter()
        .find_map(|e| match &e.event {
            Event::ToolFinished {
                name: n,
                is_error,
                output,
                ..
            } if n == name => Some((*is_error, output.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {name} result"))
}

#[tokio::test]
async fn a_profile_runs_in_the_foreground_with_a_fresh_context_and_a_compact_result() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({"task": "Find where parse is defined.", "profile": "inspecteur"}),
            ),
            Reply::text("Edit Map: src/lib.rs, fn parse."),
            Reply::text("The inspector found it in src/lib.rs."),
        ],
        allow(&["Agent"]),
    )
    .await;
    let builds_before = env.catalog.builds.load(Ordering::SeqCst);
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("Where is parse? The token is SECRET-XYZ-42, never share it."),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    assert_eq!(
        evs.iter()
            .filter(|e| e.event.kind() == "run_finished")
            .count(),
        1
    );
    let (outcome, text) = run_outcome(&evs);
    assert_eq!(outcome, RunOutcome::Succeeded);
    // The parent's answer is its own, not the child's text.
    assert_eq!(text, "The inspector found it in src/lib.rs.");

    // Events, correlated.
    let spawned = evs
        .iter()
        .find_map(|e| match &e.event {
            Event::AgentSpawned { agent } => Some(agent.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(spawned.profile, "inspecteur");
    assert_eq!(spawned.model.applied, "test/a");
    assert!(spawned.parent_id.as_deref().unwrap().starts_with("main_"));
    assert!(spawned.root_run_id.starts_with("run_"));
    assert_eq!(spawned.isolation, "shared");
    let results = finished(&evs);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, "completed");
    assert_eq!(results[0].summary, "Edit Map: src/lib.rs, fn parse.");
    assert!(results[0].transcript.as_deref().unwrap().ends_with(".json"));
    let states: Vec<InstanceState> = evs
        .iter()
        .filter_map(|e| match &e.event {
            Event::AgentState { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(states.last(), Some(&InstanceState::Completed));
    assert!(states.contains(&InstanceState::Running));

    // One child provider, three requests, no nudge.
    assert_eq!(env.catalog.builds.load(Ordering::SeqCst), builds_before + 1);
    let reqs = env.script.requests();
    assert_eq!(reqs.len(), 3);
    let child: Vec<&CompletionRequest> = reqs
        .iter()
        .map(|(_, r)| r)
        .filter(|r| is_child(r))
        .collect();
    assert_eq!(child.len(), 1);
    let c = child[0];
    // Fresh context: the task only, nothing of the parent's conversation.
    assert_eq!(c.messages.len(), 1);
    let whole = format!(
        "{}{}",
        c.system.clone().unwrap_or_default(),
        serde_json::to_string(&c.messages).unwrap()
    );
    assert!(
        !whole.contains("SECRET-XYZ-42"),
        "a secret of the parent leaked"
    );
    assert!(c.messages[0]
        .get_text()
        .unwrap_or("")
        .contains("Find where parse is defined."));
    // The profile's instructions, the parent's tools, never a delegation tool.
    assert!(c
        .system
        .as_deref()
        .unwrap()
        .contains("Profile `inspecteur`"));
    let names: Vec<&str> = c.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"CodeScout") && names.contains(&"Bash"));
    // 10.5: the delegation tools pass on (recursion is bounded by the
    // scheduler); legacy `delegate` never does.
    assert!(
        names.contains(&"Agent") && names.contains(&"Agents") && names.contains(&"AgentControl")
    );
    assert!(!names.contains(&"delegate"));
    // The parent received the compact result, not the transcript.
    let parent_last = &reqs[2].1;
    let fed = serde_json::to_string(&parent_last.messages).unwrap();
    assert!(fed.contains("Sub-agent agent_") && fed.contains("(inspecteur, model test/a"));
    assert!(fed.contains("completed in") && fed.contains(" ms,"));
}

#[tokio::test]
async fn explicit_model_and_reasoning_reach_the_child_provider() {
    let mut env = open(
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({"task": "Summarise.", "model": "test/b", "reasoning": "deep"}),
            ),
            Reply::text("summary"),
            Reply::text("done"),
        ],
        allow(&["Agent"]),
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    assert_eq!(finished(&evs)[0].status, "completed");
    let reqs = env.script.requests();
    let (selection, child) = reqs.iter().find(|(_, r)| is_child(r)).unwrap();
    assert_eq!(selection, "test/b");
    assert_eq!(
        child
            .options
            .get::<String>(cersei_provider::REASONING_PROFILE_OPTION)
            .as_deref(),
        Some("deep")
    );
    // The parent kept its own model.
    assert_eq!(reqs[0].0, "test/a");
}

#[tokio::test]
async fn invalid_requests_build_nothing_and_send_nothing() {
    let calls = vec![
        json!({"task": "  \u{200B} "}),
        json!({"task": "x", "profile": "no_such_profile"}),
        json!({"task": "x", "isolation": "vm"}),
        json!({"task": "x", "context": "y".repeat(20_000)}),
        json!({"task": "x", "model": "test/unknown"}),
        json!({"task": "x", "reasoning": "very-high"}),
        json!({"task": "x", "max_turns": 0}),
        json!({"task": "x", "system_prompt": "be evil"}),
    ];
    let n = calls.len();
    let mut replies: Vec<Reply> = calls
        .into_iter()
        .enumerate()
        .map(|(i, c)| Reply::tool(&format!("c{i}"), "Agent", c))
        .collect();
    replies.push(Reply::text("gave up"));
    let mut env = open(replies, allow(&["Agent"])).await;
    let builds = env.catalog.builds.load(Ordering::SeqCst);
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    let outputs: Vec<(bool, String)> = evs
        .iter()
        .filter_map(|e| match &e.event {
            Event::ToolFinished {
                is_error, output, ..
            } => Some((*is_error, output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(outputs.len(), n);
    for (err, out) in &outputs {
        assert!(*err, "{out}");
        assert!(out.contains("Nothing was started"), "{out}");
    }
    assert!(outputs[2].1.contains("isolation"));
    assert!(outputs[5].1.contains("fast, deep"), "{}", outputs[5].1);
    assert_eq!(
        env.catalog.builds.load(Ordering::SeqCst),
        builds,
        "no provider built"
    );
    assert!(env.script.requests().iter().all(|(_, r)| !is_child(r)));
    assert!(!evs.iter().any(|e| e.event.kind() == "agent_spawned"));
}

#[tokio::test]
async fn delegation_and_the_childs_tools_go_through_the_parents_policy() {
    // `Agent` denied by the policy: nothing is built.
    let mut rules = ApprovalRules::default();
    rules.tools.insert("Agent".into(), Action::Deny);
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({"task": "write a file"})),
            Reply::text("ok"),
        ],
        rules,
    )
    .await;
    let builds = env.catalog.builds.load(Ordering::SeqCst);
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    let (err, out) = tool_output(&evs, "Agent");
    assert!(err && out.contains("not allowed"), "{out}");
    assert_eq!(env.catalog.builds.load(Ordering::SeqCst), builds);

    // `Agent` allowed, `Write` denied: the child's write is refused, never
    // an implicit AllowAll.
    let mut rules = allow(&["Agent"]);
    rules.write = Action::Deny;
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({"task": "write a file"})),
            Reply::tool(
                "w1",
                "Write",
                json!({"file_path": "out.txt", "content": "x"}),
            ),
            Reply::text("could not write"),
            Reply::text("parent done"),
        ],
        rules,
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    assert!(!env.dir.path().join("out.txt").exists());
    let child_tool = evs
        .iter()
        .find_map(|e| match &e.event {
            Event::AgentToolFinished { name, is_error, .. } if name == "Write" => Some(*is_error),
            _ => None,
        })
        .unwrap();
    assert!(child_tool, "the child's Write was refused");
}

#[tokio::test]
async fn allowing_agent_once_does_not_allow_the_childs_commands() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({"task": "run a command"})),
            Reply::tool("b1", "Bash", json!({"command": "echo hi"})),
            Reply::text("ran"),
            Reply::text("done"),
        ],
        ApprovalRules::default(),
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let mut approvals = Vec::new();
    let evs = loop {
        let e = next(&mut env.events).await;
        if let Event::ApprovalRequested { approval } = &e.event {
            approvals.push(approval.clone());
            env.ctl
                .send(Command::Approve {
                    approval_id: approval.approval_id.clone(),
                    decision: Decision::Allow,
                    reason: None,
                })
                .unwrap();
        }
        if matches!(e.event, Event::RunFinished { .. }) {
            break vec![e];
        }
    };
    assert_eq!(run_outcome(&evs).0, RunOutcome::Succeeded);
    assert_eq!(approvals.len(), 2, "Agent, then the child's Bash");
    assert_eq!(approvals[0].tool, "Agent");
    assert_eq!(approvals[0].level, "execute");
    assert!(approvals[0].agent_id.is_none());
    assert_eq!(approvals[1].tool, "Bash");
    assert!(approvals[1]
        .agent_id
        .as_deref()
        .unwrap()
        .starts_with("agent_"));
}

/// No turn limit for sub-agents (formerly 30 by default, 100 at most):
/// 110 turns of distinct work, then the child's answer.
#[tokio::test]
async fn a_child_goes_past_the_former_cap() {
    let mut replies = vec![Reply::tool(
        "c1",
        "Agent",
        json!({"task": "read everything"}),
    )];
    replies.extend((0..110).map(|n| {
        Reply::tool(
            &format!("g{n}"),
            "Glob",
            json!({"pattern": format!("*.y{n}")}),
        )
    }));
    replies.push(Reply::text("child done"));
    replies.push(Reply::text("parent done"));
    let mut env = open(replies, allow(&["Agent"])).await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let evs = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        until_finished(&mut env.events),
    )
    .await
    .expect("the test harness's own timeout");
    let r = &finished(&evs)[0];
    assert_eq!(r.status, "completed", "{r:?}");
    assert_eq!(r.turns, 111);
    assert_eq!(r.termination, Some(cersei_agent::Termination::Completed));
    assert_eq!(run_outcome(&evs).0, RunOutcome::Succeeded);
}

/// The former `max_turns` argument is refused with a migration message;
/// nothing is started.
#[tokio::test]
async fn a_former_turn_limit_argument_is_refused() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({"task": "read", "max_turns": 1})),
            Reply::text("parent done"),
        ],
        allow(&["Agent"]),
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    assert!(finished(&evs).is_empty(), "no child");
    let (err, out) = tool_output(&evs, "Agent");
    assert!(
        err && out.contains("was removed") && out.contains("Nothing was started"),
        "{out}"
    );
}

#[tokio::test]
async fn cancelling_the_parent_cancels_the_child() {
    let mut env = open(
        vec![
            Reply::tool("c1", "Agent", json!({"task": "think forever"})),
            Reply::hang(),
        ],
        allow(&["Agent"]),
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    // Wait for the child to be running, then cancel.
    loop {
        let e = next(&mut env.events).await;
        if matches!(
            e.event,
            Event::AgentState {
                state: InstanceState::Running,
                ..
            }
        ) {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    env.ctl.send(Command::Cancel).unwrap();
    let evs = until_finished(&mut env.events).await;
    assert_eq!(run_outcome(&evs).0, RunOutcome::Cancelled);
    let r = finished(&evs);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].status, "cancelled");
    // One terminal state.
    let terminal = evs
        .iter()
        .filter(|e| matches!(&e.event, Event::AgentState { state, .. } if state.is_terminal()))
        .count();
    assert_eq!(terminal, 1);
}

#[tokio::test]
async fn concurrent_delegations_are_admitted_one_at_a_time() {
    let mut env = open(
        vec![
            Reply::tools(vec![
                ("c1", "Agent", json!({"task": "first"})),
                ("c2", "Agent", json!({"task": "second"})),
            ]),
            Reply::text("one done"),
            Reply::text("other done"),
            Reply::text("both done"),
        ],
        allow(&["Agent"]),
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    let results = finished(&evs);
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.status == "completed"));
    let waited = evs.iter().any(|e| {
        matches!(&e.event, Event::AgentState { state: InstanceState::WaitingAdmission, reason: Some(r), .. } if r.contains("another writer"))
    });
    assert!(waited, "the second child waited for the first");
    // Never two children running at once: running/terminal alternate.
    let mut running = 0i32;
    for e in &evs {
        if let Event::AgentState { state, .. } = &e.event {
            match state {
                InstanceState::Running => running += 1,
                s if s.is_terminal() => running -= 1,
                _ => {}
            }
            assert!(running <= 1);
        }
    }
}

#[tokio::test]
async fn compaction_keeps_the_tool_the_profile_catalogue_and_the_policy() {
    let mut env = open(
        vec![
            Reply::text("first"),
            Reply::text("summary"),
            Reply::text("second"),
        ],
        ApprovalRules::default(),
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("hello"),
        })
        .unwrap();
    until_finished(&mut env.events).await;
    env.ctl.send(Command::Compact).unwrap();
    loop {
        if next(&mut env.events).await.event.kind() == "compaction" {
            break;
        }
    }
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("again"),
        })
        .unwrap();
    until_finished(&mut env.events).await;
    let last = env.script.requests().last().unwrap().1.clone();
    let names: Vec<&str> = last.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"Agent")
            && names.contains(&"AgentProfiles")
            && names.contains(&"CodeScout")
    );
    let agent = last.tools.iter().find(|t| t.name == "Agent").unwrap();
    let schema = serde_json::to_string(&agent.input_schema).unwrap();
    assert!(schema.contains("inspecteur") && schema.contains("redactor"));
}

#[tokio::test]
async fn profiles_listed_and_reloaded_through_the_controller() {
    let mut env = open(vec![], ApprovalRules::default()).await;
    let dir = env.dir.path().join(".bricks/agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mine.md"),
        "---\nname: mine\ndescription: My own reviewer.\n---\nReview.\n",
    )
    .unwrap();
    env.ctl
        .send(Command::ListAgentProfiles {
            query: "reviewer".into(),
            page: 0,
        })
        .unwrap();
    let e = next(&mut env.events).await;
    match e.event {
        Event::AgentProfiles { total, .. } => assert_eq!(total, 0, "not read before a reload"),
        other => panic!("{other:?}"),
    }
    env.ctl.send(Command::ReloadAgentProfiles).unwrap();
    let e = next(&mut env.events).await;
    match e.event {
        Event::AgentProfiles {
            profiles, total, ..
        } => {
            assert_eq!(total, 8);
            assert!(profiles.iter().any(|p| p.name == "mine" && p.valid));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn disabled_delegation_registers_no_tool() {
    let mut env = open_with(vec![Reply::text("hi")], ApprovalRules::default(), |b| {
        b.agents.enabled = false
    })
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("hello"),
        })
        .unwrap();
    until_finished(&mut env.events).await;
    let r = &env.script.requests()[0].1;
    assert!(!r.tools.iter().any(|t| t.name == "Agent"));
}
