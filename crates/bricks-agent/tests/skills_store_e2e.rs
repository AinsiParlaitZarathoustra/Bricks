//! The skill store through the real controller and the real `Skill` tool, a
//! scripted model whose requests are captured: what the catalog holds (the
//! store and the built-in skill only), installation by the model without a
//! restart, withdrawal of an activation whose skill was removed (before a
//! run, while the session was closed, after a compaction), and a reinstalled
//! skill that does not come back by itself. No network; temporary folders
//! only.

#[allow(dead_code)]
mod common;

use bricks_agent::BricksConfig;
use bricks_agent::permission::SessionPolicy;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::*;
use bricks_provider::CompletionRequest;
use bricks_skills::store::{InstallOrigin, SkillStore};
use bricks_types::{ContentBlock, ToolResultContent};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

struct Env {
    ctl: Controller,
    events: EventStream,
    script: Arc<Script>,
}

fn config(
    root: &Path,
    script: &Arc<Script>,
    rules: Option<SessionPolicy>,
    tweak: impl FnOnce(&mut BricksConfig),
) -> EngineConfig {
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.semantic.lsp.enabled = false;
    tweak(&mut bricks);
    let mut cfg = EngineConfig::new(
        root.join("project"),
        ScriptedCatalog::new(&["a"], script.clone()),
        bricks,
        root.join("sessions"),
    );
    cfg.interactive = true;
    cfg.permissions = rules.map(|p| common::default_store(root, &p));
    cfg.skills = Some(SkillsSetup::with_store(Some(root.join("store"))));
    cfg
}

/// A new session on `root` (project, store and sessions folders inside it).
async fn open(
    root: &Path,
    replies: Vec<Reply>,
    rules: Option<SessionPolicy>,
    tweak: impl FnOnce(&mut BricksConfig),
) -> Env {
    std::fs::create_dir_all(root.join("project")).unwrap();
    let script = Script::new(replies);
    let cfg = config(root, &script, rules, tweak);
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
        script,
    }
}

/// A source folder with one valid skill, outside the store.
fn write_source(root: &Path, name: &str, body: &str) -> PathBuf {
    let dir = root.join("sources").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Procedure {name}.\n---\n\n{body}\n"),
    )
    .unwrap();
    dir
}

fn allow(tools: &[&str]) -> Option<SessionPolicy> {
    Some(common::allow_tools(tools))
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

/// Everything a request carries as text, system prompt included.
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

/// The same, without the system prompt.
fn messages_text(r: &CompletionRequest) -> String {
    let mut r = r.clone();
    r.system = None;
    text_of(&r)
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

/// Every file under `dir`, read as text where it is text.
fn files_text(dir: &Path) -> String {
    let mut out = String::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.push_str(&files_text(&p));
        } else if let Ok(t) = std::fs::read_to_string(&p) {
            out.push_str(&t);
        }
    }
    out
}

/// The `SKILL.md` text the model writes for a new skill.
fn write_draft(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: Greets the user.\n---\n\n{body}\n")
}

/// Activation, then removal of the skill, then one more run. Returns the
/// environment with the second run done; the scripted replies of both runs
/// were consumed. `marker` is in the skill's body.
async fn activated_then_removed(root: &Path, src: &Path, marker: &str) -> Env {
    SkillStore::new(root.join("store"))
        .install(src, InstallOrigin::Imported)
        .unwrap();
    let mut env = open(
        root,
        vec![
            Reply::tool(
                "s1",
                "Skill",
                json!({"action": "activate", "skill": "wd-skill"}),
            ),
            Reply::text("first done"),
        ],
        allow(&["Skill"]),
        |_| {},
    )
    .await;
    let evs = submit(
        &mut env,
        "Please use the wd-skill. My note mentions ORCHID once.",
    )
    .await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    assert_eq!(
        activated(&evs),
        [("wd-skill".to_string(), "model".to_string())]
    );
    let reqs = env.script.requests();
    assert!(
        text_of(&reqs[1].1).contains(marker),
        "the activation reached the model"
    );

    SkillStore::new(root.join("store"))
        .remove("wd-skill")
        .unwrap();
    env.script.push(Reply::text("second done"));
    submit(&mut env, "Second question, nothing else.").await;
    env
}

// 10. The catalog of a store-only agent: exactly the built-in skill.
#[tokio::test]
async fn store_only_catalog_lists_just_the_builtin_skill() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    write_source(
        &home.join(".claude/skills"),
        "home-claude-only",
        "Body one.",
    );
    write_source(
        &home.join(".agents/skills"),
        "home-agents-only",
        "Body two.",
    );
    write_source(
        &root.path().join("project/.claude/skills"),
        "project-claude-only",
        "Body three.",
    );
    let mut env = open(
        root.path(),
        vec![Reply::text("ok")],
        None,
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "hello").await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);

    let reqs = env.script.requests();
    let sys = reqs[0].1.system.clone().unwrap();
    let start = sys.find("<available_skills>").expect("catalog present");
    let end = sys[start..].find("</available_skills>").unwrap() + start;
    let block = &sys[start..end];
    assert_eq!(block.matches("<name>").count(), 1, "{block}");
    assert!(block.contains("<name>create-skills</name>"), "{block}");
    for hidden in [
        "home-claude-only",
        "home-agents-only",
        "project-claude-only",
    ] {
        assert!(!sys.contains(hidden), "`{hidden}` leaked into the request");
    }
}

// 11. The full flow: create-skills, write a draft, validate, install, activate.
#[tokio::test]
async fn model_writes_validates_installs_and_uses_a_skill_without_restart() {
    let root = tempfile::tempdir().unwrap();
    let draft = root.path().join("project/draft/hello-world");
    let skill_path = draft.join("SKILL.md");
    let text = write_draft("hello-world", "HELLO-BODY-11: greet the user.");
    let mut env = open(
        root.path(),
        vec![
            Reply::tool(
                "s1",
                "Skill",
                json!({"action": "activate", "skill": "create-skills"}),
            ),
            Reply::tool(
                "s2",
                "Write",
                json!({"file_path": skill_path.to_string_lossy(), "content": text}),
            ),
            Reply::tool(
                "s3",
                "Skill",
                json!({"action": "validate", "path": draft.to_string_lossy()}),
            ),
            Reply::tool(
                "s4",
                "Skill",
                json!({"action": "install", "path": draft.to_string_lossy()}),
            ),
            Reply::tool(
                "s5",
                "Skill",
                json!({"action": "activate", "skill": "hello-world"}),
            ),
            Reply::text("done"),
        ],
        allow(&["Skill", "Write"]),
        |_| {},
    )
    .await;
    let evs = submit(&mut env, "Make a hello-world skill and use it.").await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);

    let (write_err, write_out) = tool_output(&evs, "s2");
    assert!(!write_err, "{write_out}");
    let (validate_err, validate_out) = tool_output(&evs, "s3");
    assert!(!validate_err, "{validate_out}");
    assert!(
        validate_out.contains("Valid: skill `hello-world`"),
        "{validate_out}"
    );
    let (install_err, install_out) = tool_output(&evs, "s4");
    assert!(!install_err, "{install_out}");
    assert!(
        install_out.contains("Installed skill `hello-world`"),
        "{install_out}"
    );
    let (activate_err, activate_out) = tool_output(&evs, "s5");
    assert!(!activate_err, "{activate_out}");
    assert!(activate_out.contains("HELLO-BODY-11"), "{activate_out}");

    assert_eq!(
        activated(&evs),
        [
            ("create-skills".to_string(), "model".to_string()),
            ("hello-world".to_string(), "model".to_string()),
        ]
    );
    let reqs = env.script.requests();
    assert_eq!(reqs.len(), 6);
    assert!(
        !text_of(&reqs[3].1).contains("<name>hello-world</name>"),
        "listed before install"
    );
    let after_install = text_of(&reqs[4].1);
    assert!(
        after_install.contains("<name>hello-world</name>"),
        "not listed after install"
    );
    let after_activation = text_of(&reqs[5].1);
    assert!(
        after_activation.contains("<skill_activation"),
        "no activation in the next request"
    );
    assert!(after_activation.contains("HELLO-BODY-11"));
    assert!(root.path().join("store/hello-world/SKILL.md").is_file());
}

// 12. A removed skill's instructions leave the next request; the user's words stay.
#[tokio::test]
async fn removed_skill_is_withdrawn_from_the_next_request_and_user_text_stays() {
    let root = tempfile::tempdir().unwrap();
    let src = write_source(
        root.path(),
        "wd-skill",
        "Always answer with the code ORCHID-7749.",
    );
    let env = activated_then_removed(root.path(), &src, "ORCHID-7749").await;

    let reqs = env.script.requests();
    let second_run = &reqs[2].1;
    assert!(
        !text_of(second_run).contains("ORCHID-7749"),
        "instructions still sent"
    );
    assert!(
        messages_text(second_run).contains("is no longer available"),
        "no note about the withdrawn skill"
    );
    assert_eq!(
        messages_text(second_run).matches("ORCHID").count(),
        1,
        "the user's own mention must stay exactly once"
    );
}

// 13. A reinstalled skill is listed again, but its old activation stays withdrawn.
#[tokio::test]
async fn reinstalled_skill_does_not_bring_back_the_old_activation() {
    let root = tempfile::tempdir().unwrap();
    let src = write_source(
        root.path(),
        "wd-skill",
        "Always answer with the code ORCHID-7749.",
    );
    let mut env = activated_then_removed(root.path(), &src, "ORCHID-7749").await;

    SkillStore::new(root.path().join("store"))
        .install(&src, InstallOrigin::Imported)
        .unwrap();
    env.script.push(Reply::text("third done"));
    submit(&mut env, "Third question.").await;
    let reqs = env.script.requests();
    let third_run = &reqs[3].1;
    assert!(
        text_of(third_run).contains("<name>wd-skill</name>"),
        "reinstalled skill not listed"
    );
    assert!(
        !text_of(third_run).contains("ORCHID-7749"),
        "old activation came back"
    );

    // Only the model's own new activation brings the instructions back.
    env.script.push(Reply::tool(
        "s2",
        "Skill",
        json!({"action": "activate", "skill": "wd-skill"}),
    ));
    env.script.push(Reply::text("fourth done"));
    submit(&mut env, "Use it again.").await;
    let reqs = env.script.requests();
    assert!(
        messages_text(&reqs[5].1).contains("ORCHID-7749"),
        "re-activation not sent"
    );
}

// 14. A skill removed while the session is closed: the first request after
// the resume carries no trace of its instructions.
#[tokio::test]
async fn skill_removed_while_the_session_is_closed_is_withdrawn_on_resume() {
    let root = tempfile::tempdir().unwrap();
    let src = write_source(
        root.path(),
        "wd-skill",
        "Always answer with the code ORCHID-7749.",
    );
    SkillStore::new(root.path().join("store"))
        .install(&src, InstallOrigin::Imported)
        .unwrap();
    let mut env = open(
        root.path(),
        vec![
            Reply::tool(
                "s1",
                "Skill",
                json!({"action": "activate", "skill": "wd-skill"}),
            ),
            Reply::text("first done"),
        ],
        allow(&["Skill"]),
        |_| {},
    )
    .await;
    submit(
        &mut env,
        "Please use the wd-skill. My note mentions ORCHID once.",
    )
    .await;
    let id = env.ctl.session_id();
    env.ctl.close().await;

    SkillStore::new(root.path().join("store"))
        .remove("wd-skill")
        .unwrap();

    let script = Script::new(vec![Reply::text("resumed")]);
    let cfg = config(root.path(), &script, None, |_| {});
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
        prompt: Prompt::text("Second question, nothing else."),
    })
    .unwrap();
    let evs = until_finished(&mut events).await;
    ctl.wait_idle().await;
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);

    let reqs = script.requests();
    let first_after_resume = &reqs[0].1;
    assert!(
        !text_of(first_after_resume).contains("ORCHID-7749"),
        "instructions after resume"
    );
    assert!(messages_text(first_after_resume).contains("is no longer available"));
    assert_eq!(
        messages_text(first_after_resume).matches("ORCHID").count(),
        1
    );
    ctl.close().await;
}

// 15. A compaction written from the activation is not kept once the skill is
// removed: neither the instructions nor the summary reach the next request;
// the session record on disk still holds the original activation.
#[tokio::test]
async fn compaction_written_from_a_removed_skill_is_not_sent_again() {
    let root = tempfile::tempdir().unwrap();
    let src = write_source(
        root.path(),
        "cp-skill",
        "CMP-MARKER-5531 check the lock first.",
    );
    SkillStore::new(root.path().join("store"))
        .install(&src, InstallOrigin::Imported)
        .unwrap();
    let mut env = open(
        root.path(),
        vec![
            Reply::tool(
                "s1",
                "Skill",
                json!({"action": "activate", "skill": "cp-skill"}),
            ),
            Reply::text("one"),
            Reply::text("two"),
            Reply::text("SUMMARY-PARAPHRASE: the session reviewed the cp procedure."),
            Reply::text("after"),
        ],
        allow(&["Skill"]),
        |b| {
            b.context.keep_recent_messages = 2;
            b.context.min_compaction_gain = 0.0;
        },
    )
    .await;
    submit(&mut env, "start").await;
    submit(&mut env, "more").await;
    env.ctl.send(Command::Compact).unwrap();
    let Event::Compaction {
        compacted, outcome, ..
    } = until(&mut env.events, "compaction").await.event
    else {
        unreachable!()
    };
    assert!(compacted, "{outcome}");

    SkillStore::new(root.path().join("store"))
        .remove("cp-skill")
        .unwrap();
    submit(&mut env, "next").await;

    let reqs = env.script.requests();
    let next_request = text_of(&reqs.last().unwrap().1);
    assert!(
        !next_request.contains("CMP-MARKER-5531"),
        "instructions sent again"
    );
    assert!(
        !next_request.contains("SUMMARY-PARAPHRASE"),
        "summary written from them sent again"
    );

    let raw = files_text(&root.path().join("sessions"));
    assert!(
        raw.contains("CMP-MARKER-5531"),
        "the session record lost the original activation"
    );
}

// 16. A profile's preloaded store skill reaches a sub-agent started while it
// is installed, and not one started after its removal. (The sub-agent shares
// the scripted provider, so its requests are captured in the same list.)
// Not covered: a removal while a sub-agent is running between its requests;
// the scripted provider cannot pause a sub-agent for the test to act.
#[tokio::test]
async fn sub_agent_profile_does_not_preload_a_store_skill_removed_before_it_starts() {
    let root = tempfile::tempdir().unwrap();
    let src = write_source(
        root.path(),
        "house-style",
        "PRELOAD-MARKER-88: write short sentences.",
    );
    SkillStore::new(root.path().join("store"))
        .install(&src, InstallOrigin::Imported)
        .unwrap();
    let profiles = root.path().join("project/.bricks/agents");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("skilled.md"),
        "---\nname: skilled\ndescription: Uses skills.\nskills: [house-style]\n---\nBe skilled.\n",
    )
    .unwrap();
    let mut env = open(
        root.path(),
        vec![
            Reply::tool(
                "c1",
                "Agent",
                json!({"task": "use the skill", "profile": "skilled", "isolation": "shared"}),
            ),
            Reply::text("child one"),
            Reply::text("parent one"),
            Reply::tool(
                "c2",
                "Agent",
                json!({"task": "use the skill again", "profile": "skilled", "isolation": "shared"}),
            ),
            Reply::text("child two"),
            Reply::text("parent two"),
        ],
        allow(&["Agent", "Skill"]),
        |_| {},
    )
    .await;

    // The sub-agent runs alongside its parent: its requests are told apart
    // by its profile's text in the system prompt, not by their position.
    let child_requests = |env: &Env| -> Vec<CompletionRequest> {
        env.script
            .requests()
            .into_iter()
            .map(|(_, r)| r)
            .filter(|r| {
                r.system
                    .as_deref()
                    .is_some_and(|s| s.contains("Be skilled."))
            })
            .collect()
    };
    let evs = submit(&mut env, "go").await;
    let first = child_requests(&env);
    assert!(
        first.len() == 1 && text_of(&first[0]).contains("PRELOAD-MARKER-88"),
        "the first sub-agent did not get its preloaded skill: agent result {:?}, {} sub-agent requests",
        tool_output(&evs, "c1"),
        first.len()
    );

    SkillStore::new(root.path().join("store"))
        .remove("house-style")
        .unwrap();
    // Started after the removal: refused before any request, so no sub-agent
    // ever runs without the skill its profile names.
    let evs = submit(&mut env, "go again").await;
    let (is_error, out) = tool_output(&evs, "c2");
    assert!(
        is_error
            && out.contains("no skill named `house-style`")
            && out.contains("Nothing was started"),
        "{out}"
    );
    assert_eq!(child_requests(&env).len(), 1, "a second sub-agent ran");
}
