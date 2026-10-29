//! `revise`: a sent message is replaced and the run goes on from it, with a scripted model.
//! The active context is cut before the message (kept as a snapshot, the raw history keeps
//! everything); a revision that does not match what was shown, that points past the history,
//! or that comes during a run is refused with nothing changed.

#[allow(dead_code)]
mod common;

use bricks_agent::BricksConfig;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::*;
use bricks_provider::CompletionRequest;
use bricks_types::{ContentBlock, Message};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

struct Env {
    ctl: Controller,
    events: EventStream,
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    script: Arc<Script>,
}

async fn open(replies: Vec<Reply>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(replies);
    let catalog = ScriptedCatalog::new(&["a", "b"], script.clone());
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    let mut cfg = EngineConfig::new(dir.path(), catalog, bricks, dir.path().join(".sessions"));
    cfg.interactive = true;
    cfg.queue_capacity = 64;
    cfg.permissions = Some(common::default_store(
        dir.path(),
        &bricks_agent::permission::SessionPolicy::chosen(
            bricks_agent::permission::PermissionProfile::Execute,
        ),
    ));
    let (ctl, events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    Env {
        ctl,
        events,
        dir,
        script,
    }
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .expect("an event within 10 s")
        .expect("stream open")
}

/// Every envelope up to and including `run_finished`.
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

fn kinds(evs: &[Envelope]) -> Vec<&'static str> {
    evs.iter().map(|e| e.event.kind()).collect()
}

fn outcome(evs: &[Envelope]) -> RunOutcome {
    match &evs.last().unwrap().event {
        Event::RunFinished { outcome, .. } => *outcome,
        other => panic!("not a run_finished: {other:?}"),
    }
}

/// The texts of a message list, in order (tool results included).
fn texts(messages: &[Message]) -> Vec<String> {
    messages.iter().map(|m| m.get_all_text()).collect()
}

/// Every text a request carries, the system prompt left out.
fn request_text(r: &CompletionRequest) -> String {
    let mut s = String::new();
    for m in &r.messages {
        for b in m.content_blocks() {
            if let ContentBlock::Text { text } = b {
                s.push_str(&text);
                s.push('\n');
            }
        }
    }
    s
}

/// Two answered exchanges: « Premier » then « Second ».
async fn two_exchanges(env: &mut Env) {
    until(&mut env.events, "session_opened").await;
    for (prompt, answer) in [("Premier", "réponse 1"), ("Second", "réponse 2")] {
        env.ctl
            .send(Command::Submit {
                prompt: Prompt::text(prompt),
            })
            .unwrap();
        let evs = until_finished(&mut env.events).await;
        assert_eq!(outcome(&evs), RunOutcome::Succeeded, "{answer}");
    }
    env.ctl.wait_idle().await;
}

#[tokio::test]
async fn a_revision_cuts_the_history_before_the_message_and_runs_again_from_it() {
    let mut env = open(vec![
        Reply::text("réponse 1"),
        Reply::text("réponse 2"),
        Reply::text("réponse 3"),
    ])
    .await;
    two_exchanges(&mut env).await;
    assert_eq!(
        texts(&env.ctl.history().await),
        ["Premier", "réponse 1", "Second", "réponse 2"]
    );

    env.ctl
        .send(Command::Revise {
            user_index: 0,
            original: "Premier".into(),
            prompt: Prompt::text("Premier corrigé"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;

    // The cut is announced first, then the run that replaces the message.
    let k = kinds(&evs);
    assert_eq!(k.first(), Some(&"history_revised"), "{k:?}");
    assert!(k.contains(&"run_started"), "{k:?}");
    let Event::HistoryRevised {
        user_index,
        messages_removed,
    } = evs[0].event
    else {
        unreachable!()
    };
    assert_eq!(user_index, 0);
    assert_eq!(messages_removed, 4, "the message and everything after it");
    assert_eq!(outcome(&evs), RunOutcome::Succeeded);
    env.ctl.wait_idle().await;

    assert_eq!(
        texts(&env.ctl.history().await),
        ["Premier corrigé", "réponse 3"]
    );
    // The new request carries the revised message only: nothing that came after it.
    let reqs = env.script.requests();
    assert_eq!(reqs.len(), 3, "two answers, then the revised run");
    let revised = request_text(&reqs[2].1);
    assert!(revised.contains("Premier corrigé"), "{revised}");
    assert!(!revised.contains("Second") && !revised.contains("réponse 1"), "{revised}");
}

#[tokio::test]
async fn the_cut_context_is_kept_as_a_snapshot_and_the_raw_history_keeps_it() {
    let mut env = open(vec![
        Reply::text("réponse 1"),
        Reply::text("réponse 2"),
        Reply::text("réponse 3"),
    ])
    .await;
    two_exchanges(&mut env).await;
    assert!(env.ctl.agent().compaction_snapshots().is_empty());

    env.ctl
        .send(Command::Revise {
            user_index: 1,
            original: "Second".into(),
            prompt: Prompt::text("Second bis"),
        })
        .unwrap();
    until_finished(&mut env.events).await;
    env.ctl.wait_idle().await;

    let snapshots = env.ctl.agent().compaction_snapshots();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        texts(&snapshots[0].messages),
        ["Premier", "réponse 1", "Second", "réponse 2"],
        "the context as it was before the revision"
    );
    assert_eq!(
        texts(&env.ctl.history().await),
        ["Premier", "réponse 1", "Second bis", "réponse 3"]
    );
    // The raw history keeps every message ever sent, the cut ones included.
    let raw = texts(&env.ctl.agent().raw_history());
    assert!(raw.contains(&"Second".to_string()), "{raw:?}");
    assert!(raw.contains(&"Second bis".to_string()), "{raw:?}");
}

#[tokio::test]
async fn a_revision_whose_original_text_differs_is_refused_and_nothing_changes() {
    let mut env = open(vec![Reply::text("réponse 1"), Reply::text("réponse 2")]).await;
    two_exchanges(&mut env).await;
    let before = texts(&env.ctl.history().await);

    let err = env
        .ctl
        .send(Command::Revise {
            user_index: 0,
            original: "Une autre question".into(),
            prompt: Prompt::text("Nouveau"),
        })
        .unwrap_err();
    assert!(err.contains("changed since it was shown"), "{err}");

    let rejected = until(&mut env.events, "command_rejected").await;
    let Event::CommandRejected { command, reason } = rejected.event else {
        unreachable!()
    };
    assert_eq!(command, "revise");
    assert!(reason.contains("changed since it was shown"), "{reason}");
    env.ctl.wait_idle().await;

    assert_eq!(texts(&env.ctl.history().await), before);
    assert!(env.ctl.agent().compaction_snapshots().is_empty());
    assert_eq!(env.script.requests().len(), 2, "no request was sent");
    assert_eq!(env.ctl.activity(), Activity::Idle);
}

#[tokio::test]
async fn a_revision_with_an_index_past_the_history_is_refused() {
    let mut env = open(vec![Reply::text("réponse 1")]).await;
    until(&mut env.events, "session_opened").await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("Premier"),
        })
        .unwrap();
    until_finished(&mut env.events).await;
    env.ctl.wait_idle().await;

    let err = env
        .ctl
        .send(Command::Revise {
            user_index: 7,
            original: "Premier".into(),
            prompt: Prompt::text("Nouveau"),
        })
        .unwrap_err();
    assert!(err.contains("no longer in the active context"), "{err}");
    let rejected = until(&mut env.events, "command_rejected").await;
    assert!(matches!(rejected.event, Event::CommandRejected { ref command, .. } if command == "revise"));
    env.ctl.wait_idle().await;

    assert_eq!(texts(&env.ctl.history().await), ["Premier", "réponse 1"]);
    assert_eq!(env.script.requests().len(), 1);
}

#[tokio::test]
async fn a_revision_during_a_run_is_refused_and_the_run_goes_on() {
    let mut env = open(vec![Reply::hang(), Reply::text("après")]).await;
    until(&mut env.events, "session_opened").await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("long"),
        })
        .unwrap();
    until(&mut env.events, "run_started").await;

    let err = env
        .ctl
        .send(Command::Revise {
            user_index: 0,
            original: "long".into(),
            prompt: Prompt::text("court"),
        })
        .unwrap_err();
    assert!(err.contains("in progress"), "{err}");
    let rejected = until(&mut env.events, "command_rejected").await;
    assert!(matches!(rejected.event, Event::CommandRejected { ref command, .. } if command == "revise"));
    // Still the same run: it is running, and no history was cut.
    assert!(matches!(env.ctl.activity(), Activity::Running { .. }));
    assert!(env.ctl.agent().compaction_snapshots().is_empty());

    env.ctl.send(Command::Cancel).unwrap();
    let fin = until(&mut env.events, "run_finished").await;
    assert!(matches!(fin.event, Event::RunFinished { outcome: RunOutcome::Cancelled, .. }));
    env.ctl.wait_idle().await;
    assert_eq!(texts(&env.ctl.history().await).first().map(String::as_str), Some("long"));
}

#[tokio::test]
async fn a_tool_result_is_not_a_message_to_revise() {
    let mut env = open(vec![
        Reply::tool("c1", "Read", json!({"file_path": "a.txt"})),
        Reply::text("fini"),
    ])
    .await;
    std::fs::write(env.dir.path().join("a.txt"), "A").unwrap();
    until(&mut env.events, "session_opened").await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("Lis a"),
        })
        .unwrap();
    until_finished(&mut env.events).await;
    env.ctl.wait_idle().await;

    // User-role message 1 is the tool result, which the person never wrote.
    let err = env
        .ctl
        .send(Command::Revise {
            user_index: 1,
            original: "A".into(),
            prompt: Prompt::text("Nouveau"),
        })
        .unwrap_err();
    assert!(err.contains("changed since it was shown"), "{err}");
    env.ctl.wait_idle().await;
    assert_eq!(env.ctl.history().await.len(), 4);
}

#[tokio::test]
async fn the_revise_command_the_desktop_sends_is_read_by_the_engine() {
    let wire = json!({
        "type": "revise",
        "user_index": 2,
        "original": "Bonjour",
        "prompt": { "blocks": [ { "kind": "text", "text": "Salut" } ] }
    });
    let command: Command = serde_json::from_value(wire).unwrap();
    assert_eq!(command.name(), "revise");
    let Command::Revise {
        user_index,
        original,
        prompt,
    } = command
    else {
        unreachable!()
    };
    assert_eq!(user_index, 2);
    assert_eq!(original, "Bonjour");
    assert_eq!(prompt, Prompt::text("Salut"));
}
