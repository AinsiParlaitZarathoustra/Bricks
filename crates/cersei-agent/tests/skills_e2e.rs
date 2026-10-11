//! Agent Skills through the real controller, the real `Skill` tool and a
//! scripted model whose requests are captured: catalog without bodies,
//! activation by the model and by a person, trust of project skills,
//! permissions, deduplication, compaction, restoration, reload, change of
//! project, cancellation. Project and home folders are temporary; no skill
//! of the machine is read.

use cersei_agent::BricksConfig;
use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::*;
use cersei_provider::CompletionRequest;
use cersei_types::{ContentBlock, ToolResultContent};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

struct Env {
    ctl: Controller,
    events: EventStream,
    project: tempfile::TempDir,
    home: tempfile::TempDir,
    script: Arc<Script>,
    sessions: PathBuf,
}

fn skill(root: &Path, name: &str, extra: &str, body: &str) {
    let d = root.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Procedure {name}.\n{extra}---\n{body}\n"),
    )
    .unwrap();
}

fn personal(env_home: &Path) -> PathBuf {
    env_home.join(".agents/skills")
}

fn config(
    project: &Path,
    home: &Path,
    sessions: &Path,
    script: &Arc<Script>,
    rules: ApprovalRules,
    interactive: bool,
    tweak: impl FnOnce(&mut BricksConfig),
) -> EngineConfig {
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.permissions = rules;
    bricks.semantic.lsp.enabled = false;
    tweak(&mut bricks);
    let mut cfg = EngineConfig::new(
        project,
        ScriptedCatalog::new(&["a"], script.clone()),
        bricks,
        sessions,
    );
    cfg.interactive = interactive;
    cfg.skills = Some(SkillsSetup::with_home(Some(home.to_path_buf())));
    cfg
}

async fn open_env(
    replies: Vec<Reply>,
    rules: ApprovalRules,
    interactive: bool,
    prepare: impl FnOnce(&Path, &Path),
    tweak: impl FnOnce(&mut BricksConfig),
) -> Env {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    prepare(project.path(), home.path());
    let script = Script::new(replies);
    let sessions = project.path().join(".sessions");
    let cfg = config(
        project.path(),
        home.path(),
        &sessions,
        &script,
        rules,
        interactive,
        tweak,
    );
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
    assert_eq!(next(&mut events).await.event.kind(), "session_opened");
    Env {
        ctl,
        events,
        project,
        home,
        script,
        sessions,
    }
}

fn allow(tools: &[&str]) -> ApprovalRules {
    let mut r = ApprovalRules::default();
    for t in tools {
        r.tools.insert(t.to_string(), Action::Allow);
    }
    r
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

async fn until(events: &mut EventStream, kind: &str) -> Envelope {
    loop {
        let e = next(events).await;
        if e.event.kind() == kind {
            return e;
        }
    }
}

async fn submit(env: &mut Env, text: &str) -> Vec<Envelope> {
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text(text),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    env.ctl.wait_idle().await;
    evs
}

/// Everything a request carries as text.
fn text_of(r: &CompletionRequest) -> String {
    let mut s = r.system.clone().unwrap_or_default();
    for m in &r.messages {
        for b in m.content_blocks() {
            match b {
                ContentBlock::Text { text } => s.push_str(&text),
                ContentBlock::ToolResult {
                    content: ToolResultContent::Text(t),
                    ..
                } => s.push_str(&t),
                ContentBlock::ToolUse { input, .. } => s.push_str(&input.to_string()),
                _ => {}
            }
            s.push('\n');
        }
    }
    s
}

fn messages_text(r: &CompletionRequest) -> String {
    let mut r = r.clone();
    r.system = None;
    text_of(&r)
}

fn kinds(evs: &[Envelope]) -> Vec<&'static str> {
    evs.iter().map(|e| e.event.kind()).collect()
}

fn activated(evs: &[Envelope]) -> Vec<(String, String)> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::SkillActivated { name, origin, .. } => Some((name.clone(), origin.clone())),
            _ => None,
        })
        .collect()
}

fn outcome(evs: &[Envelope]) -> RunOutcome {
    match &evs.last().unwrap().event {
        Event::RunFinished { outcome, .. } => *outcome,
        _ => unreachable!(),
    }
}

fn tool_output(evs: &[Envelope], id: &str) -> (bool, String) {
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

// ─── Catalog and model activation ────────────────────────────────────────────

#[tokio::test]
async fn the_model_activates_through_the_real_tool_and_the_catalog_has_no_body() {
    let mut env = open_env(
        vec![
            Reply::tool(
                "s1",
                "Skill",
                json!({"action": "activate", "skill": "deploy"}),
            ),
            Reply::text("done"),
        ],
        ApprovalRules::default(),
        true,
        |_, h| {
            skill(
                &personal(h),
                "deploy",
                "",
                "DEPLOY-BODY-1: check the tag first.",
            )
        },
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "ship it").await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    assert_eq!(
        activated(&evs),
        [("deploy".to_string(), "model".to_string())]
    );
    let reqs = env.script.requests();
    let first = &reqs[0].1;
    let sys = first.system.clone().unwrap();
    assert!(sys.contains("<name>deploy</name>") && sys.contains("Procedure deploy."));
    assert!(!text_of(first).contains("DEPLOY-BODY-1"), "catalog only");
    assert!(first.tools.iter().any(|t| t.name == "Skill"));
    // The instructions reach the next request whole, through the result.
    let second = messages_text(&reqs[1].1);
    assert!(second.contains("DEPLOY-BODY-1: check the tag first."));
    assert!(second.contains("grant no permission"));
    let active = env.ctl.agent().active_skills();
    assert_eq!(active.len(), 1);
    assert_eq!((active[0].0.as_str(), active[0].3), ("deploy", "model"));
}

#[tokio::test]
async fn a_repeated_activation_is_not_duplicated_and_keeps_the_new_request() {
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "deploy"})),
            Reply::tool(
                "s2",
                "Skill",
                json!({"skill": "deploy", "args": "SECOND-REQUEST"}),
            ),
            Reply::text("done"),
        ],
        ApprovalRules::default(),
        true,
        |_, h| skill(&personal(h), "deploy", "", "DEPLOY-BODY-2"),
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let (_, second) = tool_output(&evs, "s2");
    assert!(second.contains("already active") && second.contains("SECOND-REQUEST"));
    let last = messages_text(&env.script.requests().last().unwrap().1);
    assert_eq!(last.matches("DEPLOY-BODY-2").count(), 1, "{last}");
    assert_eq!(activated(&evs).len(), 1);
}

// ─── Trust of project skills ─────────────────────────────────────────────────

#[tokio::test]
async fn project_skills_stay_out_of_requests_until_a_person_allows_them() {
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "local-proc"})),
            Reply::text("first"),
            Reply::text("second"),
            Reply::text("third"),
        ],
        ApprovalRules::default(),
        true,
        |p, h| {
            skill(&p.join(".agents/skills"), "local-proc", "", "PROJECT-BODY");
            skill(&personal(h), "mine", "", "PERSONAL-BODY");
        },
        |_| {},
    )
    .await;
    // Asked once, after the session opened.
    let asked = until(&mut env.events, "skills_trust_requested").await;
    let Event::SkillsTrustRequested { sources, .. } = asked.event else {
        unreachable!()
    };
    assert_eq!(sources[0].skills, 1);
    let evs = submit(&mut env, "go").await;
    let (is_error, out) = tool_output(&evs, "s1");
    assert!(is_error && out.contains("not approved"), "{out}");
    for (_, r) in env.script.requests() {
        let t = text_of(&r);
        assert!(!t.contains("Procedure local-proc.") && !t.contains("PROJECT-BODY"));
        assert!(
            t.contains("<name>mine</name>"),
            "personal skills are usable"
        );
    }
    // The person can inspect them before deciding (not the model).
    env.ctl
        .send(Command::ListSkills {
            query: String::new(),
            page: 0,
        })
        .unwrap();
    let Event::Skills { listing } = until(&mut env.events, "skills").await.event else {
        unreachable!()
    };
    let local = listing
        .entries
        .iter()
        .find(|e| e.name == "local-proc")
        .unwrap();
    assert_eq!(local.state, "pending_approval");
    assert_eq!(local.description.as_deref(), Some("Procedure local-proc."));
    // Refused: the session goes on with the other sources.
    env.ctl
        .send(Command::TrustProjectSkills { allow: false })
        .unwrap();
    let Event::SkillsTrustResolved { allowed, by, .. } =
        until(&mut env.events, "skills_trust_resolved").await.event
    else {
        unreachable!()
    };
    assert!(!allowed && by == "user");
    let evs = submit(&mut env, "still working?").await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    // Allowed by the person: the catalog has them from the next request.
    env.ctl
        .send(Command::TrustProjectSkills { allow: true })
        .unwrap();
    until(&mut env.events, "skills_trust_resolved").await;
    submit(&mut env, "now").await;
    let last = env.script.requests().last().unwrap().1.clone();
    assert!(last.system.unwrap().contains("<name>local-proc</name>"));
}

#[tokio::test]
async fn headless_sessions_announce_project_skills_without_waiting() {
    let mut env = open_env(
        vec![Reply::text("ok")],
        ApprovalRules::default(),
        false,
        |p, _| skill(&p.join(".claude/skills"), "local-proc", "", "PROJECT-BODY"),
        |_| {},
    )
    .await;
    let n = until(&mut env.events, "notice").await;
    let Event::Notice { message } = n.event else {
        unreachable!()
    };
    assert!(message.contains("not used") && message.contains("--trust-project-skills"));
    let evs = submit(&mut env, "go").await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    assert!(!text_of(&env.script.requests()[0].1).contains("local-proc"));
}

#[tokio::test]
async fn the_callers_explicit_trust_applies_to_this_session_only() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    skill(
        &project.path().join(".agents/skills"),
        "local-proc",
        "",
        "PROJECT-BODY",
    );
    let script = Script::new(vec![Reply::text("ok")]);
    let sessions = project.path().join(".sessions");
    let mut cfg = config(
        project.path(),
        home.path(),
        &sessions,
        &script,
        ApprovalRules::default(),
        false,
        |_| {},
    );
    cfg.skills.as_mut().unwrap().trust_project = true;
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
    let Event::SkillsTrustResolved { allowed, by, .. } =
        until(&mut events, "skills_trust_resolved").await.event
    else {
        unreachable!()
    };
    assert!(allowed && by == "caller");
    ctl.send(Command::Submit {
        prompt: Prompt::text("go").with_skills(vec!["local-proc".into()]),
    })
    .unwrap();
    until_finished(&mut events).await;
    ctl.wait_idle().await;
    assert!(text_of(&script.requests()[0].1).contains("PROJECT-BODY"));
    // Another session of the same project (resumed) starts undecided.
    let id = ctl.session_id();
    ctl.close().await;
    let script2 = Script::new(vec![Reply::text("ok")]);
    let cfg = config(
        project.path(),
        home.path(),
        &sessions,
        &script2,
        ApprovalRules::default(),
        false,
        |_| {},
    );
    let (ctl2, _events2) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::Resume(id),
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        ctl2.skills().unwrap().trust().project,
        bricks_skills::ProjectDecision::Pending
    );
}

// ─── Permissions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_refused_read_activates_nothing() {
    let mut rules = ApprovalRules::default();
    rules.tools.insert("Skill".into(), Action::Deny);
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "deploy"})),
            Reply::text("ok"),
        ],
        rules,
        true,
        |_, h| skill(&personal(h), "deploy", "", "DEPLOY-BODY-3"),
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let (is_error, out) = tool_output(&evs, "s1");
    assert!(is_error && out.contains("Permission denied"), "{out}");
    assert!(activated(&evs).is_empty());
    assert!(env.ctl.agent().active_skills().is_empty());
    assert!(!messages_text(&env.script.requests()[1].1).contains("DEPLOY-BODY-3"));
}

#[tokio::test]
async fn an_active_skill_and_its_allowed_tools_grant_nothing() {
    // The skill says it may use Bash; the rules refuse Bash.
    let mut rules = ApprovalRules::default();
    rules.tools.insert("Bash".into(), Action::Deny);
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "cleanup"})),
            Reply::tool("b1", "Bash", json!({"command": "touch ran.txt"})),
            Reply::text("ok"),
        ],
        rules,
        true,
        |_, h| {
            skill(
                &personal(h),
                "cleanup",
                "allowed-tools: Bash(touch:*)\n",
                "Run `touch ran.txt`.",
            )
        },
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "go").await;
    assert!(!tool_output(&evs, "s1").0);
    let (is_error, out) = tool_output(&evs, "b1");
    assert!(is_error && out.contains("Permission denied"), "{out}");
    assert!(!env.project.path().join("ran.txt").exists());
    let snapshot = env.ctl.snapshot();
    assert!(snapshot.allowed_for_session.is_empty());
}

// ─── A person's activation ───────────────────────────────────────────────────

#[tokio::test]
async fn a_person_activates_before_the_request_and_a_failure_sends_nothing() {
    let mut env = open_env(
        vec![Reply::text("first"), Reply::text("second")],
        ApprovalRules::default(),
        true,
        |_, h| skill(&personal(h), "review", "", "REVIEW-BODY"),
        |_| {},
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("check src/lib.rs").with_skills(vec!["review".into()]),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    env.ctl.wait_idle().await;
    assert_eq!(
        activated(&evs),
        [("review".to_string(), "user".to_string())]
    );
    let i = kinds(&evs)
        .iter()
        .position(|k| *k == "skill_activated")
        .unwrap();
    let j = kinds(&evs)
        .iter()
        .position(|k| *k == "run_finished")
        .unwrap();
    assert!(i < j);
    let first = env.script.requests()[0].1.clone();
    let n = first.messages.len();
    assert!(first.messages[n - 2].get_all_text().contains("REVIEW-BODY"));
    assert_eq!(first.messages[n - 1].get_all_text(), "check src/lib.rs");
    // Again: not duplicated, the new request is sent.
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("and tests/").with_skills(vec!["review".into()]),
        })
        .unwrap();
    until_finished(&mut env.events).await;
    env.ctl.wait_idle().await;
    let last = env.script.requests().last().unwrap().1.clone();
    assert_eq!(messages_text(&last).matches("REVIEW-BODY").count(), 1);
    assert!(messages_text(&last).contains("and tests/"));
    // A skill that cannot be used: refused, nothing sent.
    let before = env.script.requests().len();
    let e = env
        .ctl
        .send(Command::Submit {
            prompt: Prompt::text("x").with_skills(vec!["no-such".into()]),
        })
        .unwrap_err();
    assert!(e.contains("no-such"), "{e}");
    until(&mut env.events, "command_rejected").await;
    assert_eq!(env.script.requests().len(), before);
}

#[tokio::test]
async fn an_activation_that_does_not_fit_is_refused_whole() {
    // 90 KiB of instructions: more than the scripted model's budget once
    // reduced... checked with a tight context policy.
    let big = "BIG-LINE with words to count\n".repeat(3_000);
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "huge"})),
            Reply::text("ok"),
        ],
        ApprovalRules::default(),
        true,
        |_, h| skill(&personal(h), "huge", "", &big),
        |b| {
            // Keep almost nothing free: the margin takes most of the window.
            b.context.safety_margin_ratio = 0.9;
        },
    )
    .await;
    let evs = submit(&mut env, "go").await;
    let (is_error, out) = tool_output(&evs, "s1");
    assert!(is_error && out.contains("was not activated"), "{out}");
    assert!(out.contains("not shortened"));
    assert!(
        evs.iter()
            .any(|e| e.event.kind() == "skill_activation_failed")
    );
    assert!(env.ctl.agent().active_skills().is_empty());
    let last = messages_text(&env.script.requests().last().unwrap().1);
    assert!(!last.contains("BIG-LINE"), "no fragment of the body");
}

// ─── Compaction, reload, restoration ─────────────────────────────────────────

#[tokio::test]
async fn compaction_keeps_the_instructions_once() {
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "style"})),
            Reply::text("one"),
            Reply::text("two"),
            Reply::text("three"),
            Reply::text("SUMMARY OF THE SESSION"),
            Reply::text("after"),
        ],
        ApprovalRules::default(),
        true,
        |_, h| skill(&personal(h), "style", "", "STYLE-BODY: short sentences."),
        |b| {
            b.context.keep_recent_messages = 2;
            b.context.min_compaction_gain = 0.0;
        },
    )
    .await;
    submit(&mut env, "start").await;
    submit(&mut env, "more").await;
    submit(&mut env, "again").await;
    env.ctl.send(Command::Compact).unwrap();
    let Event::Compaction {
        compacted, outcome, ..
    } = until(&mut env.events, "compaction").await.event
    else {
        unreachable!()
    };
    assert!(compacted, "{outcome}");
    let history = env.ctl.history().await;
    let all: String = history.iter().map(|m| m.get_all_text()).collect();
    assert_eq!(
        all.matches("STYLE-BODY: short sentences.").count(),
        1,
        "{all}"
    );
    assert!(all.contains("SUMMARY OF THE SESSION"));
    submit(&mut env, "next").await;
    let last = messages_text(&env.script.requests().last().unwrap().1);
    assert_eq!(last.matches("STYLE-BODY").count(), 1);
}

#[tokio::test]
async fn reload_does_not_change_an_active_skill() {
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "style"})),
            Reply::text("one"),
            Reply::text("two"),
        ],
        ApprovalRules::default(),
        true,
        |_, h| skill(&personal(h), "style", "", "STYLE-V1"),
        |_| {},
    )
    .await;
    submit(&mut env, "start").await;
    let before = env.ctl.agent().active_skills()[0].2.clone();
    skill(&personal(env.home.path()), "style", "", "STYLE-V2");
    env.ctl.send(Command::ReloadSkills).unwrap();
    let Event::Skills { listing } = until(&mut env.events, "skills").await.event else {
        unreachable!()
    };
    assert_eq!(listing.generation, 2);
    assert_eq!(env.ctl.agent().active_skills()[0].2, before);
    submit(&mut env, "more").await;
    let last = messages_text(&env.script.requests().last().unwrap().1);
    assert!(last.contains("STYLE-V1") && !last.contains("STYLE-V2"));
}

#[tokio::test]
async fn a_restored_session_checks_its_skills_against_their_source() {
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "style"})),
            Reply::text("one"),
        ],
        ApprovalRules::default(),
        true,
        |_, h| skill(&personal(h), "style", "", "STYLE-R1"),
        |_| {},
    )
    .await;
    submit(&mut env, "start").await;
    let id = env.ctl.session_id();
    env.ctl.close().await;
    let reopen = |script: &Arc<Script>| {
        config(
            env.project.path(),
            env.home.path(),
            &env.sessions,
            script,
            ApprovalRules::default(),
            true,
            |_| {},
        )
    };
    let resume = OpenOptions {
        session: SessionChoice::Resume(id.clone()),
        model: None,
        reasoning: None,
    };
    // Same revision: active again.
    let s2 = Script::new(vec![Reply::text("again")]);
    let (ctl, mut events) = Controller::open(reopen(&s2), resume.clone()).await.unwrap();
    ctl.send(Command::Submit {
        prompt: Prompt::text("continue"),
    })
    .unwrap();
    let evs = until_finished(&mut events).await;
    ctl.wait_idle().await;
    assert_eq!(
        activated(&evs),
        [("style".to_string(), "restored".to_string())]
    );
    assert_eq!(ctl.agent().active_skills().len(), 1);
    ctl.close().await;
    // The file changed: suspended, said so, not replaced silently.
    skill(&personal(env.home.path()), "style", "", "STYLE-R2");
    let s3 = Script::new(vec![Reply::text("again")]);
    let (ctl, mut events) = Controller::open(reopen(&s3), resume).await.unwrap();
    ctl.send(Command::Submit {
        prompt: Prompt::text("continue"),
    })
    .unwrap();
    let evs = until_finished(&mut events).await;
    let failed: Vec<String> = evs
        .iter()
        .filter_map(|e| match &e.event {
            Event::SkillActivationFailed { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(
        failed[0].contains("suspended") && failed[0].contains("changed"),
        "{failed:?}"
    );
    assert!(ctl.agent().active_skills().is_empty());
    assert!(!text_of(&s3.requests()[0].1).contains("STYLE-R2"));
}

#[tokio::test]
async fn a_change_of_project_leaves_nothing_of_the_previous_one() {
    // Session B in another project, stored first.
    let other = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let sessions = tempfile::tempdir().unwrap();
    let sb = Script::new(vec![Reply::text("b")]);
    let cfg = config(
        other.path(),
        home.path(),
        sessions.path(),
        &sb,
        ApprovalRules::default(),
        true,
        |_| {},
    );
    let (ctl_b, mut ev_b) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    ctl_b
        .send(Command::Submit {
            prompt: Prompt::text("b"),
        })
        .unwrap();
    until_finished(&mut ev_b).await;
    ctl_b.wait_idle().await;
    let id_b = ctl_b.session_id();
    ctl_b.close().await;
    // Session A, project skills allowed and one activated.
    let project = tempfile::tempdir().unwrap();
    skill(
        &project.path().join(".agents/skills"),
        "a-proc",
        "",
        "A-BODY",
    );
    let sa = Script::new(vec![
        Reply::tool("s1", "Skill", json!({"skill": "a-proc"})),
        Reply::text("a"),
        Reply::text("in b"),
    ]);
    let cfg = config(
        project.path(),
        home.path(),
        sessions.path(),
        &sa,
        ApprovalRules::default(),
        true,
        |_| {},
    );
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
    ctl.send(Command::TrustProjectSkills { allow: true })
        .unwrap();
    until(&mut events, "skills_trust_resolved").await;
    ctl.send(Command::Submit {
        prompt: Prompt::text("a"),
    })
    .unwrap();
    until_finished(&mut events).await;
    ctl.wait_idle().await;
    assert_eq!(ctl.agent().active_skills().len(), 1);
    // Resume B: its project, an undecided trust, no activation of A.
    ctl.send(Command::Resume { session_id: id_b }).unwrap();
    until(&mut events, "session_opened").await;
    assert_eq!(
        ctl.skills().unwrap().trust().project,
        bricks_skills::ProjectDecision::Pending
    );
    assert!(ctl.agent().active_skills().is_empty());
    ctl.send(Command::Submit {
        prompt: Prompt::text("in b"),
    })
    .unwrap();
    until_finished(&mut events).await;
    let last = text_of(&sa.requests().last().unwrap().1);
    assert!(!last.contains("a-proc") && !last.contains("A-BODY"));
}

#[tokio::test]
async fn cancelling_during_an_activation_leaves_nothing_active() {
    let mut env = open_env(
        vec![
            Reply::tools(vec![
                ("s1", "Skill", json!({"skill": "style"})),
                ("b1", "Bash", json!({"command": "sleep 5"})),
            ]),
            Reply::tool("s2", "Skill", json!({"skill": "style"})),
            Reply::text("ok"),
        ],
        allow(&["Bash"]),
        true,
        |_, h| skill(&personal(h), "style", "", "STYLE-C"),
        |_| {},
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("go"),
        })
        .unwrap();
    loop {
        let e = next(&mut env.events).await;
        if matches!(&e.event, Event::ToolStarted { name, .. } if name == "Bash") {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    env.ctl.send(Command::Cancel).unwrap();
    let evs = until_finished(&mut env.events).await;
    env.ctl.wait_idle().await;
    assert_eq!(outcome(&evs), RunOutcome::Cancelled);
    assert!(env.ctl.agent().active_skills().is_empty());
    assert!(activated(&evs).is_empty());
    // The next activation is a normal, complete one.
    let evs = submit(&mut env, "again").await;
    let (_, out) = tool_output(&evs, "s2");
    assert!(out.contains("STYLE-C") && !out.contains("already active"));
}

#[tokio::test]
async fn no_skills_configured_means_no_tool_and_a_clear_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(vec![Reply::text("ok")]);
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    let cfg = EngineConfig::new(
        dir.path(),
        ScriptedCatalog::new(&["a"], script.clone()),
        bricks,
        dir.path().join(".sessions"),
    );
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
    assert!(ctl.agent().tool_by_name("Skill").is_none());
    assert!(ctl.send(Command::ReloadSkills).is_err());
    assert!(
        ctl.send(Command::Submit {
            prompt: Prompt::text("x").with_skills(vec!["debug".into()]),
        })
        .is_err()
    );
    ctl.send(Command::Submit {
        prompt: Prompt::text("plain"),
    })
    .unwrap();
    let evs = until_finished(&mut events).await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    assert!(
        !script.requests()[0]
            .1
            .system
            .clone()
            .unwrap_or_default()
            .contains("available_skills")
    );
}

#[tokio::test]
async fn a_restored_project_skill_waits_for_a_new_decision() {
    let mut env = open_env(
        vec![
            Reply::tool("s1", "Skill", json!({"skill": "local-proc"})),
            Reply::text("one"),
        ],
        ApprovalRules::default(),
        true,
        |p, _| skill(&p.join(".agents/skills"), "local-proc", "", "LOCAL-BODY"),
        |_| {},
    )
    .await;
    env.ctl
        .send(Command::TrustProjectSkills { allow: true })
        .unwrap();
    until(&mut env.events, "skills_trust_resolved").await;
    submit(&mut env, "start").await;
    assert_eq!(env.ctl.agent().active_skills().len(), 1);
    let id = env.ctl.session_id();
    env.ctl.close().await;
    // Reopened: the old approval does not carry over.
    let s2 = Script::new(vec![Reply::text("again")]);
    let cfg = config(
        env.project.path(),
        env.home.path(),
        &env.sessions,
        &s2,
        ApprovalRules::default(),
        true,
        |_| {},
    );
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::Resume(id),
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    ctl.send(Command::Submit {
        prompt: Prompt::text("continue"),
    })
    .unwrap();
    let evs = until_finished(&mut events).await;
    ctl.wait_idle().await;
    assert!(evs.iter().any(|e| matches!(&e.event,
        Event::SkillActivationFailed { reason, .. } if reason.contains("suspended") && reason.contains("not approved"))));
    assert!(ctl.agent().active_skills().is_empty());
    // A new decision by the person: checked again, active again.
    ctl.send(Command::TrustProjectSkills { allow: true })
        .unwrap();
    let e = until(&mut events, "skill_activated").await;
    let Event::SkillActivated { name, origin, .. } = e.event else {
        unreachable!()
    };
    assert_eq!((name.as_str(), origin.as_str()), ("local-proc", "restored"));
    assert_eq!(ctl.agent().active_skills().len(), 1);
}
