//! The engine against the scripted catalog of `bricks_agent`: a real
//! controller, real stores, no provider.

use super::AppState;
use super::live::{OpenSession, files_dir, ingest, spawn_consumer};
use crate::contract::{DesktopEvent, IpcErrorCode, OpenedSession, RunLabel, SearchMatch};
use bricks_agent::BricksConfig;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::{
    Command, Controller, EngineConfig, Envelope, Event, OpenOptions, Prompt,
    SCHEMA_VERSION, SessionChoice, SessionMeta, SkillsSetup, WrittenFile,
};
use bricks_tools::preview::ChangeKind;
use bricks_types::{Message, Role};
use crate::services::conversations::message_text;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};

/// A state under `home`, with a registered project folder `wd`.
fn state_with_project(home: &Path, wd: &Path) -> (AppState, String) {
    let state = AppState::assemble(home.to_path_buf(), Err("aucun registre dans ce test".into()));
    let id = state
        .projects
        .add(wd.to_str().unwrap())
        .expect("project folder")
        .id;
    (state, id)
}

/// Opens a session of `wd` on the scripted catalog and registers it as an
/// open session, with its consumer task, as `open_session` does.
async fn open_scripted(state: &AppState, project_id: &str, wd: &Path, script: Arc<Script>) -> Arc<OpenSession> {
    open_scripted_as(state, project_id, wd, script, SessionChoice::New).await
}

/// As [`open_scripted`], for a new session or a stored one (`Resume`).
async fn open_scripted_as(
    state: &AppState,
    project_id: &str,
    wd: &Path,
    script: Arc<Script>,
    session: SessionChoice,
) -> Arc<OpenSession> {
    open_scripted_full(state, project_id, wd, script, session, None).await
}

/// As [`open_scripted_as`], with the skills installed in the store of `home`
/// (`home/skills`) usable when given.
async fn open_scripted_full(
    state: &AppState,
    project_id: &str,
    wd: &Path,
    script: Arc<Script>,
    session: SessionChoice,
    home: Option<&Path>,
) -> Arc<OpenSession> {
    let mut cfg = EngineConfig::new(
        wd,
        ScriptedCatalog::new(&["a"], script),
        BricksConfig::default(),
        state.sessions_dir().to_path_buf(),
    );
    cfg.interactive = false;
    cfg.skills = home.map(|h| SkillsSetup::with_store(Some(h.join("skills"))));
    let (controller, mut stream) = Controller::open(
        cfg,
        OpenOptions {
            session,
            model: Some("test/a".into()),
            reasoning: None,
        },
    )
    .await
    .expect("open");
    let (entry, stop) = OpenSession::new(
        controller,
        project_id.to_string(),
        state.sessions_dir().to_path_buf(),
        0,
        0,
        Arc::clone(&state.run_labels),
        Arc::clone(&state.run_records),
    );
    while let Some(env) = stream.try_next() {
        ingest(&state.hub, &entry, env);
    }
    state
        .sessions
        .lock()
        .insert(entry.session_id.clone(), entry.clone());
    spawn_consumer(state.hub.clone(), entry.clone(), stream, stop);
    entry
}

/// Waits until `check` holds (polling), or fails after a few seconds.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test]
async fn a_rename_is_kept_by_the_next_save_of_the_run() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text("ok")])).await;
    let id = entry.session_id.clone();

    let renamed = state.rename_session(&id, "  Mon titre  ").await.unwrap();
    assert_eq!(renamed.title, "Mon titre");

    entry
        .controller
        .send(Command::Submit {
            prompt: Prompt::text("un premier message assez long"),
        })
        .unwrap();
    entry.controller.wait_idle().await;

    // The run saved the session's metadata: the title is still the rename.
    let meta = SessionMeta::load(&files_dir(state.sessions_dir(), &id))
        .unwrap()
        .expect("metadata");
    assert_eq!(meta.title, "Mon titre");
    let _ = state.close_session(&id, false).await;
}

#[tokio::test]
async fn a_delete_cancels_the_run_and_nothing_is_saved_afterwards() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let script = Script::new(vec![Reply::text("ok"), Reply::hang()]);
    let entry = open_scripted(&state, &project, &wd, script).await;
    let id = entry.session_id.clone();

    // A finished run gives the session a stored transcript.
    entry
        .controller
        .send(Command::Submit { prompt: Prompt::text("premier") })
        .unwrap();
    entry.controller.wait_idle().await;
    let transcript = state.sessions_dir().join(format!("{id}.jsonl"));
    assert!(transcript.exists(), "a finished run is stored");

    // A second run that never ends: the delete must cancel it first.
    entry
        .controller
        .send(Command::Submit { prompt: Prompt::text("second, bloqué") })
        .unwrap();
    eventually("the second run to start", || entry.is_running()).await;

    state.delete_session(&id).await.expect("delete");
    assert!(state.open_entry(&id).is_none(), "retired");
    assert!(!transcript.exists());
    assert!(!files_dir(state.sessions_dir(), &id).exists());

    // A late save from the cancelled run would recreate the files: none does.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!transcript.exists(), "not recreated");
    assert!(!files_dir(state.sessions_dir(), &id).exists(), "not recreated");
}

#[tokio::test]
async fn closing_a_busy_session_is_refused_until_cancelled() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::hang()])).await;
    let id = entry.session_id.clone();
    entry
        .controller
        .send(Command::Submit { prompt: Prompt::text("bloqué") })
        .unwrap();
    eventually("the run to start", || entry.is_running()).await;

    let refused = state.close_session(&id, false).await.unwrap_err();
    assert_eq!(refused.code, IpcErrorCode::SessionBusy);
    assert!(state.open_entry(&id).is_some(), "still open");

    state.close_session(&id, true).await.expect("cancel, then close");
    assert!(state.open_entry(&id).is_none());
}

#[tokio::test]
async fn search_finds_titles_and_message_text_of_stored_sessions() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);

    // The title is the first prompt; the word searched for is in the answer.
    let answer = "Il faut migrer vers Postgres bientôt";
    let first = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text(answer)])).await;
    first
        .controller
        .send(Command::Submit { prompt: Prompt::text("Plan Atlas") })
        .unwrap();
    first.controller.wait_idle().await;
    let first_id = first.session_id.clone();
    state.close_session(&first_id, false).await.unwrap();

    let by_message = state.sessions_search(Some(&project), "postgres").await.unwrap();
    assert_eq!(by_message.len(), 1);
    assert!(matches!(by_message[0].matched, SearchMatch::Message));
    let snippet = by_message[0].snippet.clone().unwrap();
    assert!(snippet.contains("Postgres"), "{snippet}");

    let by_title = state.sessions_search(Some(&project), "ATLAS").await.unwrap();
    assert_eq!(by_title.len(), 1);
    assert!(matches!(by_title[0].matched, SearchMatch::Title));
    assert!(by_title[0].snippet.is_none());

    assert!(state.sessions_search(Some(&project), "introuvable").await.unwrap().is_empty());
    assert!(state.sessions_search(None, "  ").await.unwrap().is_empty());
    // A session of another project is not in this project's results.
    let other_dir = tempfile::tempdir().unwrap();
    let other = std::fs::canonicalize(other_dir.path()).unwrap();
    let other_id = state.projects.add(other.to_str().unwrap()).unwrap().id;
    assert!(state.sessions_search(Some(&other_id), "postgres").await.unwrap().is_empty());
    assert_eq!(state.sessions_search(None, "postgres").await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_applied_edit_is_journaled_once_with_the_patch_the_engine_wrote() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![])).await;
    let session = entry.session_id.clone();

    let envelope = |seq: u64, event: Event| Envelope {
        schema: SCHEMA_VERSION,
        session_id: session.clone(),
        run_id: Some("run_1".into()),
        seq,
        at: 1_000 + seq as i64,
        event,
    };
    // No approval was shown for this edit (an auto-approved tool): the patch
    // the engine wrote is the only one there is.
    let applied = Event::EditApplied {
        tool_call_id: "call_1".into(),
        tool: "Write".into(),
        files: vec![WrittenFile {
            path: "a.txt".into(),
            kind: ChangeKind::Modify,
            added: 1,
            removed: 1,
            binary: false,
            absolute: Some(wd.join("a.txt").display().to_string()),
            patch: Some("@@ -1 +1 @@\n-a\n+b\n".into()),
            patch_truncated: false,
        }],
        agent_id: None,
        changeset_id: None,
    };
    ingest(&state.hub, &entry, envelope(2, applied.clone()));
    // The same change reported again (a replayed event) is not a second entry.
    ingest(&state.hub, &entry, envelope(3, applied));

    let journal = Arc::clone(&state.hub.journal);
    let project_id = project.clone();
    eventually("the journal entry", || {
        journal.list(&project_id, None, 10).map(|r| !r.is_empty()).unwrap_or(false)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let records = journal.list(&project, None, 10).unwrap();
    assert_eq!(records.len(), 1, "deduplicated");
    let record = &records[0];
    assert_eq!(record.session_id, session);
    assert_eq!(record.files[0].kind, "modified");
    assert!(record.files[0].has_patch);
    let diff = journal.diff(&record.id, "a.txt").unwrap();
    assert!(diff.patch.contains("+b"));
    assert!(!diff.truncated);
    assert_eq!(journal.info(&project).unwrap().since, Some(record.at));
}

#[tokio::test]
async fn the_bridge_keeps_the_last_run_and_its_sequence_number() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![])).await;
    let session = entry.session_id.clone();
    let event = |seq: u64, e: Event| Envelope {
        schema: SCHEMA_VERSION,
        session_id: session.clone(),
        run_id: None,
        seq,
        at: seq as i64,
        event: e,
    };
    let started = |seq| event(seq, Event::RunStarted {
        prompt: "p".into(),
        attachments: vec![],
        model: "test/a".into(),
        reasoning: None,
    });
    ingest(&state.hub, &entry, started(1));
    ingest(&state.hub, &entry, event(2, Event::Notice { message: "ancien".into() }));
    ingest(&state.hub, &entry, started(3));
    ingest(&state.hub, &entry, event(4, Event::Notice { message: "dernier".into() }));

    let opened = entry.opened().await;
    assert_eq!(opened.last_seq, 4);
    assert_eq!(opened.recent_events.len(), 2, "only the last run is kept");
    assert_eq!(opened.recent_events[0]["seq"], 3);
    assert_eq!(opened.recent_events[1]["message"], "dernier");
    assert_eq!(opened.recent_events[1]["session_id"], session.as_str());
}

/// A session opened on a model whose catalog then loses it: its next run is
/// refused with the reason, nothing starts, and nothing is substituted.
#[tokio::test]
async fn a_run_after_its_model_was_removed_is_refused_with_the_reason() {
    use crate::commands::providers::{apply_edit, current_file};
    use crate::engine::catalog::DesktopCatalog;
    use crate::services::providers_toml::{Edit, fixtures::PROVIDERS};
    use bricks_agent::control::{Activity, ModelCatalog, Prompt};

    let dir = tempfile::tempdir().unwrap();
    let providers = dir.path().join("providers.toml");
    std::fs::write(&providers, PROVIDERS).unwrap();
    let wd = dir.path().join("projet");
    std::fs::create_dir_all(&wd).unwrap();
    let catalog = DesktopCatalog::new(bricks_config::load_registry(Some(&providers)).map(Arc::new));
    let mut cfg = EngineConfig::new(
        &wd,
        Arc::clone(&catalog) as Arc<dyn ModelCatalog>,
        BricksConfig::default(),
        dir.path().join("sessions"),
    );
    cfg.interactive = false;
    let (controller, mut stream) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: Some("local/small".into()),
            reasoning: None,
        },
    )
    .await
    .expect("the model is configured when the session opens");

    let revision = current_file(&providers).revision;
    apply_edit(
        &providers,
        &parking_lot::Mutex::new(()),
        &catalog,
        &revision,
        &Edit::ModelDelete {
            provider_id: "local".into(),
            model_id: "small".into(),
        },
    )
    .expect("the model is removed from the file");

    let refused = controller
        .send(Command::Submit {
            prompt: Prompt::text("bonjour"),
        })
        .expect_err("the run is refused");
    assert!(refused.contains("`small`"), "names the model: {refused}");
    assert!(refused.contains("choose a configured model"), "asks for a model: {refused}");
    // The same refusal is announced on the event stream.
    let announced = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(env) = stream.next().await {
            if let Event::CommandRejected { reason, .. } = env.event {
                return reason;
            }
        }
        panic!("the event stream ended without a refusal");
    })
    .await
    .expect("the refusal arrives");
    assert_eq!(announced, refused);
    assert_eq!(controller.activity(), Activity::Idle, "nothing started");
    controller.close().await;
}

// ─── labels of runs started from typed commands ───────────────

/// The command JSON of a submit of `text`, as the frontend sends it.
fn submit_json(text: &str) -> serde_json::Value {
    serde_json::to_value(Command::Submit { prompt: Prompt::text(text) }).unwrap()
}

/// A skill `name` installed in the store of `home`, with its procedure as the body.
fn write_skill(home: &Path, name: &str, body: &str) {
    let dir = home.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Procédure {name}.\n---\n{body}\n"),
    )
    .unwrap();
}

/// The `run_started` of the session's last run, as the view holds it.
fn last_run_started(opened: &OpenedSession) -> serde_json::Value {
    opened
        .recent_events
        .iter()
        .find(|e| e["type"] == "run_started")
        .cloned()
        .expect("the last run started")
}

/// Waits until the consumer has taken the events of the run after `previous`
/// (the run id of the run before it, `None` for the first) up to its end, so
/// every decision about its label has been made. Then lets the label store
/// finish.
async fn run_consumed(entry: &OpenSession, previous: Option<&str>) -> OpenedSession {
    for _ in 0..200 {
        let opened = entry.opened().await;
        let started = opened
            .recent_events
            .iter()
            .find(|e| e["type"] == "run_started")
            .and_then(|e| e["run_id"].as_str().map(String::from));
        let finished = opened.recent_events.iter().any(|e| e["type"] == "run_finished");
        if finished && started.is_some() && started.as_deref() != previous {
            tokio::time::sleep(Duration::from_millis(50)).await;
            return opened;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    panic!("the run was not consumed");
}

/// The run id of the session's last run, as its `run_started` names it.
async fn last_run_id(entry: &OpenSession) -> String {
    last_run_started(&entry.opened().await)["run_id"]
        .as_str()
        .expect("a run id")
        .to_string()
}

#[tokio::test]
async fn a_typed_command_labels_the_run_its_submit_started_and_persists_it() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let script = Script::new(vec![Reply::text("un"), Reply::text("deux")]);
    let entry = open_scripted(&state, &project, &wd, script).await;
    let id = entry.session_id.clone();

    // A first run, typed as plain text: no label.
    state.send_command(&id, submit_json("premier"), None).await.expect("first submit");
    run_consumed(&entry, None).await;
    let first = last_run_id(&entry).await;

    // The second run, typed as `/skill debug …`: its submit carries the typed
    // command, the engine receives only the request.
    state
        .send_command(&id, submit_json("Corrige ce problème"), Some("/skill debug".into())).await
        .expect("second submit");
    let opened = run_consumed(&entry, Some(&first)).await;
    let started = last_run_started(&opened);
    assert_ne!(started["run_id"].as_str().unwrap(), first, "a different run");

    eventually("the label of the typed run", || {
        state.run_labels.list(&id).map(|l| l.len() == 1).unwrap_or(false)
    })
    .await;
    let stored = state.run_labels.list(&id).unwrap();
    assert_eq!(stored.len(), 1, "only the typed run is labelled");
    assert_eq!(stored[0].run_id, started["run_id"].as_str().unwrap());
    assert_eq!(stored[0].label, "/skill debug");
    assert_eq!(stored[0].prompt, "Corrige ce problème", "the request the engine received");
    assert_eq!(stored[0].user_index, Some(1), "the second user message of the session");
    assert_eq!(stored[0].at, started["at"].as_i64().unwrap());
    // The view of the open session carries the same labels, oldest first.
    assert_eq!(entry.opened().await.run_labels, stored);
    let _ = state.close_session(&id, false).await;
}

#[tokio::test]
async fn a_labelled_run_is_announced_to_the_windows_once_stored() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text("ok")])).await;
    let id = entry.session_id.clone();

    let (seen_tx, seen_rx) = std::sync::mpsc::channel::<String>();
    let channel: Channel<DesktopEvent> = Channel::new(move |body| {
        if let InvokeResponseBody::Json(text) = body {
            let _ = seen_tx.send(text);
        }
        Ok(())
    });
    let subscription = state.hub.subscribe(channel);

    state
        .send_command(&id, submit_json("bonjour"), Some("/skill debug".into())).await
        .expect("submit");
    run_consumed(&entry, None).await;

    let mut announced = None;
    for _ in 0..200 {
        while let Ok(text) = seen_rx.try_recv() {
            if text.contains("\"kind\":\"run_labeled\"") {
                announced = Some(text);
            }
        }
        if announced.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    let text = announced.expect("the label is announced");
    assert!(text.contains("\"label\":\"/skill debug\""), "{text}");
    assert!(text.contains(&format!("\"session_id\":\"{id}\"")), "{text}");
    assert_eq!(state.run_labels.list(&id).unwrap().len(), 1);
    assert!(state.hub.unsubscribe(&subscription));
    let _ = state.close_session(&id, false).await;
}

#[tokio::test]
async fn a_label_is_kept_when_the_session_is_reopened() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let script = Script::new(vec![Reply::text("un"), Reply::text("deux")]);
    let entry = open_scripted(&state, &project, &wd, Arc::clone(&script)).await;
    let id = entry.session_id.clone();
    state.send_command(&id, submit_json("un"), None).await.expect("plain run");
    run_consumed(&entry, None).await;
    let first = last_run_id(&entry).await;
    state
        .send_command(&id, submit_json("deux"), Some("/skill review".into())).await
        .expect("typed run");
    run_consumed(&entry, Some(&first)).await;
    eventually("the label", || {
        state.run_labels.list(&id).map(|l| l.len() == 1).unwrap_or(false)
    })
    .await;
    state.close_session(&id, false).await.expect("close");

    // Reopened from the store: the same labels, oldest first, in the view.
    let reopened = open_scripted_as(&state, &project, &wd, script, SessionChoice::Resume(id.clone())).await;
    let opened = reopened.opened().await;
    assert_eq!(opened.run_labels.len(), 1);
    assert_eq!(opened.run_labels[0].label, "/skill review");
    assert_eq!(opened.run_labels, state.run_labels.list(&id).unwrap());
    let _ = state.close_session(&id, false).await;
}

#[tokio::test]
async fn a_refused_submit_leaves_no_label_for_the_next_run() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let script = Script::new(vec![Reply::hang(), Reply::text("ok")]);
    let entry = open_scripted(&state, &project, &wd, script).await;
    let id = entry.session_id.clone();

    // A run that hangs: a second submit is refused while it runs.
    state.send_command(&id, submit_json("bloqué"), None).await.expect("first submit");
    eventually("the first run to start", || entry.is_running()).await;
    let refused = state
        .send_command(&id, submit_json("pendant"), Some("/skill debug".into())).await
        .expect_err("refused while a run is active");
    assert_eq!(refused.code, IpcErrorCode::Rejected);

    // A cancel that arrives before the run has begun is not kept by the
    // engine: it is sent again until the run is over.
    for _ in 0..200 {
        if !entry.is_busy() {
            break;
        }
        // Refused once the run is over: that is the end of the loop.
        let _ = state.send_command(&id, serde_json::to_value(Command::Cancel).unwrap(), None).await;
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    let cancelled = run_consumed(&entry, None).await;
    let hung = last_run_started(&cancelled)["run_id"].as_str().unwrap().to_string();

    // The next submit is labelled with its own command, not the refused one.
    state
        .send_command(&id, submit_json("après"), Some("/skill review".into())).await
        .expect("submit after the cancel");
    run_consumed(&entry, Some(&hung)).await;
    eventually("the label of the run", || {
        state.run_labels.list(&id).map(|l| l.len() == 1).unwrap_or(false)
    })
    .await;
    let stored = state.run_labels.list(&id).unwrap();
    assert_eq!(stored[0].label, "/skill review");
    assert_eq!(stored[0].user_index, Some(1), "after the cancelled run's message");
    assert_eq!(stored[0].run_id, last_run_id(&entry).await);
    assert_ne!(stored[0].run_id, hung, "the cancelled run is not labelled");
    let _ = state.close_session(&id, false).await;
}

#[tokio::test]
async fn a_command_that_is_not_a_submit_ignores_the_display() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text("ok")])).await;
    let id = entry.session_id.clone();

    let list_profiles = serde_json::to_value(Command::ListAgentProfiles {
        query: String::new(),
        page: 0,
    })
    .unwrap();
    state
        .send_command(&id, list_profiles, Some("/skill debug".into())).await
        .expect("accepted, the display is not for it");

    // The run that follows was typed as plain text: nothing is attached to it.
    state.send_command(&id, submit_json("bonjour"), None).await.expect("submit");
    run_consumed(&entry, None).await;
    assert!(state.run_labels.list(&id).unwrap().is_empty(), "no label");
    let _ = state.close_session(&id, false).await;
}

#[tokio::test]
async fn deleting_a_session_removes_its_labels() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text("ok")])).await;
    let id = entry.session_id.clone();
    state
        .send_command(&id, submit_json("bonjour"), Some("/skill debug".into())).await
        .expect("submit");
    run_consumed(&entry, None).await;
    eventually("the label", || state.run_labels.exists(&id)).await;
    let file = home.path().join("desktop").join("run-labels").join(format!("{id}.jsonl"));
    assert!(file.exists(), "stored where the desktop keeps it");

    state.delete_session(&id).await.expect("delete");
    assert!(!file.exists(), "the labels file is removed with the session");
    assert!(state.run_labels.list(&id).unwrap().is_empty());
    let late = RunLabel {
        run_id: "run_late".into(),
        label: "/skill debug".into(),
        prompt: "tardif".into(),
        at: 1,
        user_index: None,
    };
    assert!(!state.run_labels.append(&id, &late).unwrap(), "a late label is not kept");
    assert!(!file.exists(), "and does not recreate the file");
}

/// A finished run is kept with its answer and the place of its message; it is
/// listed with the session when it is opened again, and removed with it.
#[tokio::test]
async fn a_finished_run_keeps_its_answer_and_goes_with_its_session() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text("Réponse finale")])).await;
    let id = entry.session_id.clone();
    state.send_command(&id, submit_json("bonjour"), None).await.expect("submit");
    run_consumed(&entry, None).await;
    let records = Arc::clone(&state.run_records);
    let key = id.clone();
    eventually("the run record", || records.list(&key).map(|l| l.len() == 1).unwrap_or(false)).await;

    let stored = state.run_records.list(&id).unwrap();
    assert_eq!(stored[0].assistant_text, "Réponse finale");
    assert_eq!(stored[0].user_index, Some(0), "the run added the first user message");
    assert_eq!(stored[0].outcome.as_deref(), Some("succeeded"));
    assert!(stored[0].finished_at.is_some());
    assert_eq!(entry.opened().await.run_records, stored, "the view of the session carries it");

    let file = home.path().join("desktop").join("run-records").join(format!("{id}.jsonl"));
    assert!(file.exists(), "stored where the desktop keeps it");
    state.delete_session(&id).await.expect("delete");
    assert!(!file.exists(), "removed with the session");
    let late = state.run_records.append(&id, &stored[0]).unwrap();
    assert!(!late, "a late record is not kept");
    assert!(!file.exists(), "and does not recreate the file");
}

/// Four runs of the same text, the second and the fourth typed as commands.
/// After a reopen, each label names its own message, not an identical one.
#[tokio::test]
async fn identical_prompts_keep_their_own_places_after_a_reopen() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let script = Script::new(vec![Reply::text("un"), Reply::text("deux"), Reply::text("trois"), Reply::text("quatre")]);
    let entry = open_scripted(&state, &project, &wd, Arc::clone(&script)).await;
    let id = entry.session_id.clone();

    let mut previous: Option<String> = None;
    for display in [None, Some("/skill review"), None, Some("/skill debug")] {
        state
            .send_command(&id, submit_json("même texte"), display.map(String::from))
            .await
            .expect("submit");
        run_consumed(&entry, previous.as_deref()).await;
        previous = Some(last_run_id(&entry).await);
    }
    eventually("both labels", || {
        state.run_labels.list(&id).map(|l| l.len() == 2).unwrap_or(false)
    })
    .await;
    state.close_session(&id, false).await.expect("close");

    let reopened = open_scripted_as(&state, &project, &wd, script, SessionChoice::Resume(id.clone())).await;
    let labels: Vec<(String, Option<u32>)> = reopened
        .opened()
        .await
        .run_labels
        .into_iter()
        .map(|l| (l.label, l.user_index))
        .collect();
    assert_eq!(
        labels,
        vec![("/skill review".to_string(), Some(1)), ("/skill debug".to_string(), Some(3))],
        "the second and fourth user messages, in order"
    );
    let _ = state.close_session(&id, false).await;
}

/// A skill activated by the typed command is a user message of its own, placed
/// before the prompt: the label names the prompt, not the activation.
#[tokio::test]
async fn a_skill_activation_before_the_prompt_is_not_taken_for_it() {
    let home = tempfile::tempdir().unwrap();
    write_skill(home.path(), "debug", "Corps de la procédure de debug.");
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let script = Script::new(vec![Reply::text("un"), Reply::text("deux")]);
    let entry = open_scripted_full(&state, &project, &wd, script, SessionChoice::New, Some(home.path())).await;
    let id = entry.session_id.clone();

    state.send_command(&id, submit_json("premier"), None).await.expect("plain run");
    let first = {
        run_consumed(&entry, None).await;
        last_run_id(&entry).await
    };

    let typed = serde_json::to_value(Command::Submit {
        prompt: Prompt::text("Corrige ce problème").with_skills(vec!["debug".into()]),
    })
    .unwrap();
    state
        .send_command(&id, typed, Some("/skill debug".into()))
        .await
        .expect("the typed run, with its skill");
    run_consumed(&entry, Some(&first)).await;
    eventually("the label", || {
        state.run_labels.list(&id).map(|l| l.len() == 1).unwrap_or(false)
    })
    .await;

    // The order the engine keeps: the first message, the activation, the prompt.
    let users: Vec<String> = entry
        .controller
        .history()
        .await
        .iter()
        .filter(|m: &&Message| m.role == Role::User)
        .map(message_text)
        .collect();
    assert_eq!(users.len(), 3, "{users:?}");
    assert_eq!(users[0], "premier");
    assert!(users[1].contains("Corps de la procédure de debug"), "{users:?}");
    assert_eq!(users[2], "Corrige ce problème");

    let stored = state.run_labels.list(&id).unwrap();
    assert_eq!(stored[0].user_index, Some(2), "the prompt, after the activation");
    let _ = state.close_session(&id, false).await;
}



#[tokio::test]
async fn revealing_a_file_of_an_unknown_session_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let state = AppState::assemble(home.path().to_path_buf(), Err("aucun registre dans ce test".into()));
    let result = crate::commands::agent_file::agent_root_of(&state, "ses_absente", "agent_x");
    assert!(matches!(
        result,
        Err(crate::contract::IpcError { code: IpcErrorCode::NotFound, .. })
    ));
}

#[tokio::test]
async fn revealing_a_file_of_an_unknown_agent_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text("ok")])).await;
    let id = entry.session_id.clone();

    let result = crate::commands::agent_file::agent_root_of(&state, &id, "agent_absent");
    assert!(matches!(
        result,
        Err(crate::contract::IpcError { code: IpcErrorCode::NotFound, .. })
    ));
    let _ = state.close_session(&id, false).await;
}

// ─── Missing folders, relocation and its journal ─────────────────────────────

/// A stored conversation of `wd` with one answered exchange, closed: only its files remain.
async fn stored_session(state: &AppState, project_id: &str, wd: &Path) -> String {
    let entry = open_scripted(state, project_id, wd, Script::new(vec![Reply::text("ok")])).await;
    let id = entry.session_id.clone();
    entry
        .controller
        .send(Command::Submit {
            prompt: Prompt::text("un premier message assez long"),
        })
        .unwrap();
    entry.controller.wait_idle().await;
    state.close_session(&id, false).await.unwrap();
    id
}

/// A folder under `root`, created and canonical.
fn folder(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(&dir).unwrap()
}

fn journal_file(home: &Path, project_id: &str) -> PathBuf {
    home.join("desktop").join("relocations").join(format!("{project_id}.json"))
}

#[tokio::test]
async fn a_project_whose_folder_is_deleted_still_lists_its_sessions() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let wd = folder(root.path(), "projet");
    let (state, project) = state_with_project(home.path(), &wd);
    let id = stored_session(&state, &project, &wd).await;

    std::fs::remove_dir_all(&wd).unwrap();
    assert!(state.projects.get(&project).unwrap().missing);

    let listed = state.sessions_list(&project).await.unwrap();
    let ids: Vec<&str> = listed.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, [id.as_str()]);
    assert_eq!(listed[0].project_id.as_deref(), Some(project.as_str()));
}

#[tokio::test]
async fn a_folder_that_is_gone_still_belongs_to_its_project_by_its_exact_path() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let wd = folder(root.path(), "projet");
    let (state, project) = state_with_project(home.path(), &wd);
    std::fs::remove_dir_all(&wd).unwrap();

    assert_eq!(state.project_id_of_dir(&wd).as_deref(), Some(project.as_str()));
    assert_eq!(state.project_id_of_dir(&wd.join("sub")), None);
}

#[tokio::test]
async fn relocating_moves_the_stored_sessions_and_keeps_the_project_id() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let wd = folder(root.path(), "ancien");
    let (state, project) = state_with_project(home.path(), &wd);
    let id = stored_session(&state, &project, &wd).await;
    std::fs::remove_dir_all(&wd).unwrap();
    let moved = folder(root.path(), "nouveau");

    let relocated = state
        .relocate_project(&project, moved.to_str().unwrap())
        .await
        .unwrap();

    assert_eq!(relocated.id, project);
    assert_eq!(relocated.path, moved.display().to_string());
    assert!(!relocated.missing);
    let meta = SessionMeta::load(&files_dir(state.sessions_dir(), &id))
        .unwrap()
        .expect("the session is still stored");
    assert_eq!(meta.working_dir, moved);
    let listed = state.sessions_list(&project).await.unwrap();
    assert_eq!(listed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
    assert!(!journal_file(home.path(), &project).exists(), "the journal is removed once done");
}

#[tokio::test]
async fn relocating_onto_the_folder_of_another_project_is_a_conflict() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let wd_a = folder(root.path(), "a");
    let wd_b = folder(root.path(), "b");
    let (state, project_a) = state_with_project(home.path(), &wd_a);
    state.projects.add(wd_b.to_str().unwrap()).unwrap();

    let err = state
        .relocate_project(&project_a, wd_b.to_str().unwrap())
        .await
        .unwrap_err();

    assert_eq!(err.code, IpcErrorCode::Conflict);
    assert_eq!(state.projects.get(&project_a).unwrap().path, wd_a.display().to_string());
    assert!(!journal_file(home.path(), &project_a).exists());
}

#[tokio::test]
async fn a_leftover_journal_is_finished_when_the_store_already_has_the_new_path() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let from = folder(root.path(), "ancien");
    let (state, project) = state_with_project(home.path(), &from);
    let id = stored_session(&state, &project, &from).await;
    let to = folder(root.path(), "nouveau");
    // The store moved; the sessions and the journal did not.
    state.projects.relocate(&project, &to).unwrap();
    let journal = serde_json::json!({
        "schema": 1,
        "project_id": project,
        "from": from,
        "to": to,
        "sessions": [id],
    });
    std::fs::create_dir_all(journal_file(home.path(), &project).parent().unwrap()).unwrap();
    std::fs::write(journal_file(home.path(), &project), journal.to_string()).unwrap();

    state.recover_relocations();

    let meta = SessionMeta::load(&files_dir(state.sessions_dir(), &id)).unwrap().unwrap();
    assert_eq!(meta.working_dir, to);
    assert!(!journal_file(home.path(), &project).exists());
}

#[tokio::test]
async fn a_leftover_journal_is_undone_when_the_store_still_has_the_old_path() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let from = folder(root.path(), "ancien");
    let (state, project) = state_with_project(home.path(), &from);
    let id = stored_session(&state, &project, &from).await;
    let to = folder(root.path(), "nouveau");
    // The session was already rewritten to the new folder when the run stopped.
    let dir = files_dir(state.sessions_dir(), &id);
    let mut meta = SessionMeta::load(&dir).unwrap().unwrap();
    meta.working_dir = to.clone();
    meta.save(&dir).unwrap();
    let journal = serde_json::json!({
        "schema": 1,
        "project_id": project,
        "from": from,
        "to": to,
        "sessions": [id],
    });
    std::fs::create_dir_all(journal_file(home.path(), &project).parent().unwrap()).unwrap();
    std::fs::write(journal_file(home.path(), &project), journal.to_string()).unwrap();

    state.recover_relocations();

    let meta = SessionMeta::load(&dir).unwrap().unwrap();
    assert_eq!(meta.working_dir, from, "back in the folder the project still has");
    assert_eq!(state.projects.get(&project).unwrap().path, from.display().to_string());
    assert!(!journal_file(home.path(), &project).exists());
}

// ─── Conversations writing to each other, forks, moves ──────────────────────

const SENDER: &str = "20990101-000000-aaaaaa";

/// A stored conversation for [`SENDER`]: only existing conversations send.
fn register_sender(state: &AppState, wd: &Path) {
    SessionMeta::new(SENDER, wd, "test/a", None)
        .save(&files_dir(state.sessions_dir(), SENDER))
        .unwrap();
}

#[tokio::test]
async fn a_message_to_a_busy_conversation_waits_then_gets_its_own_answer() {
    use crate::services::mail::MailState;
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    register_sender(&state, &wd);
    let script = Script::new(vec![Reply::hang(), Reply::text("réponse de B")]);
    let b = open_scripted(&state, &project, &wd, script).await;
    let b_id = b.session_id.clone();

    // B works on something of its own.
    b.controller
        .send(Command::Submit { prompt: Prompt::text("travail en cours") })
        .unwrap();
    eventually("B to work", || b.is_running()).await;

    // The send returns at once; the message waits for B.
    let mail = state.mail_send(SENDER, &b_id, "bonjour B").await.expect("sent");
    assert_eq!(mail.state, MailState::Sent);
    let early = state.mail.wait(&mail.id, Duration::from_millis(100)).await.unwrap();
    assert_eq!(early.state, MailState::Sent, "still waiting while B works");

    // B's own run ends; the message is delivered, and only its run answers.
    b.controller.send(Command::Cancel).unwrap();
    let done = state.mail.wait(&mail.id, Duration::from_secs(10)).await.unwrap();
    assert_eq!(done.state, MailState::Answered, "{done:?}");
    assert_eq!(done.answer.as_deref(), Some("réponse de B"));
    let history = b.controller.history().await;
    assert!(
        history.iter().any(|m| message_text(m).contains("bonjour B")),
        "the message is in B's conversation"
    );
    let _ = state.close_session(&b_id, true).await;
}

#[tokio::test]
async fn deleting_the_conversation_a_message_waits_for_fails_it_at_once() {
    use crate::services::mail::MailState;
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    register_sender(&state, &wd);
    let b = open_scripted(&state, &project, &wd, Script::new(vec![Reply::hang()])).await;
    let b_id = b.session_id.clone();
    b.controller
        .send(Command::Submit { prompt: Prompt::text("travail en cours") })
        .unwrap();
    eventually("B to work", || b.is_running()).await;
    let mail = state.mail_send(SENDER, &b_id, "bonjour").await.expect("sent");

    // The sender waits; meanwhile B is deleted.
    let mailbox = Arc::clone(&state.mail);
    let id = mail.id.clone();
    let waiting = tokio::spawn(async move { mailbox.wait(&id, Duration::from_secs(20)).await });
    state.delete_session(&b_id).await.expect("delete");
    let failed = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the wait ends with the deletion, not at its timeout")
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, MailState::Failed);
    assert!(failed.reason.unwrap().contains("closed or deleted"));
}

#[tokio::test]
async fn a_conversation_answering_a_message_cannot_write_back_during_that_turn() {
    use crate::services::mail::MailState;
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    register_sender(&state, &wd);
    let b = open_scripted(&state, &project, &wd, Script::new(vec![Reply::hang()])).await;
    let b_id = b.session_id.clone();
    let mail = state.mail_send(SENDER, &b_id, "réponds-moi").await.expect("sent");
    let mailbox = Arc::clone(&state.mail);
    let id = mail.id.clone();
    eventually("the message's run to start", || {
        mailbox.get(&id).is_some_and(|m| m.state == MailState::InProgress)
    })
    .await;
    let back = state.mail_send(&b_id, SENDER, "et toi ?").await.unwrap_err();
    assert_eq!(back.code, IpcErrorCode::Rejected);
    assert!(back.message.contains("answering a message"), "{}", back.message);
    // Writing to itself is refused too.
    assert_eq!(state.mail_send(&b_id, &b_id, "x").await.unwrap_err().code, IpcErrorCode::Invalid);
    // Closing B cancels the message's run: it gets no answer, and says so.
    let _ = state.close_session(&b_id, true).await;
    let ended = state.mail.wait(&mail.id, Duration::from_secs(10)).await.unwrap();
    assert_eq!(ended.state, MailState::Failed, "{ended:?}");
    assert!(ended.answer.is_none());
}

#[tokio::test]
async fn a_fork_copies_the_history_and_files_and_outlives_its_parent() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text("ok")])).await;
    let id = entry.session_id.clone();
    entry
        .controller
        .send(Command::Submit { prompt: Prompt::text("une question à forker") })
        .unwrap();
    entry.controller.wait_idle().await;
    let _ = state.rename_session(&id, "Salut").await;
    // A file of the conversation, named in its history.
    let files = files_dir(state.sessions_dir(), &id);
    std::fs::create_dir_all(files.join("attachments")).unwrap();
    std::fs::write(files.join("attachments/a.png"), b"png").unwrap();
    state.close_session(&id, false).await.unwrap();

    let fork = state.fork_session(&id).await.expect("fork");
    assert_eq!(fork.title, "Salut (fork)");
    assert_eq!(fork.parent_id.as_deref(), Some(id.as_str()));
    let fork_files = files_dir(state.sessions_dir(), &fork.id);
    assert!(fork_files.join("attachments/a.png").exists(), "files copied");
    let store = bricks_memory::JsonlMemory::new(state.sessions_dir());
    use bricks_memory::Memory as _;
    let copied = store.load(&fork.id).await.unwrap();
    assert!(copied.iter().any(|m| message_text(m).contains("une question à forker")));

    // The parent goes; the fork stays whole and lists its parent.
    state.delete_session(&id).await.unwrap();
    assert!(fork_files.join("attachments/a.png").exists());
    assert_eq!(store.load(&fork.id).await.unwrap().len(), copied.len());
    let listed = state.sessions_list(&project).await.unwrap();
    assert!(listed.iter().any(|s| s.id == fork.id && s.parent_id.as_deref() == Some(id.as_str())));
}

#[tokio::test]
async fn moving_a_conversation_goes_to_another_project_only_and_never_while_it_works() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let other_dir = tempfile::tempdir().unwrap();
    let other = std::fs::canonicalize(other_dir.path()).unwrap();
    let stray_dir = tempfile::tempdir().unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let other_project = state.projects.add(other.to_str().unwrap()).unwrap().id;
    let script = Script::new(vec![Reply::text("ok"), Reply::hang()]);
    let entry = open_scripted(&state, &project, &wd, script).await;
    let id = entry.session_id.clone();
    entry
        .controller
        .send(Command::Submit { prompt: Prompt::text("premier") })
        .unwrap();
    entry.controller.wait_idle().await;

    // A folder that is no project is refused, and nothing changes.
    let refused = state
        .move_session(&id, stray_dir.path().to_str().unwrap())
        .await
        .unwrap_err();
    assert_eq!(refused.code, IpcErrorCode::Rejected);
    // A working conversation is not moved.
    entry
        .controller
        .send(Command::Submit { prompt: Prompt::text("bloqué") })
        .unwrap();
    eventually("the run to start", || entry.is_running()).await;
    let busy = state.move_session(&id, other.to_str().unwrap()).await.unwrap_err();
    assert_eq!(busy.code, IpcErrorCode::SessionBusy);
    let meta = SessionMeta::load(&files_dir(state.sessions_dir(), &id)).unwrap().unwrap();
    assert_eq!(meta.working_dir, wd, "unchanged");

    entry.controller.send(Command::Cancel).unwrap();
    entry.controller.wait_idle().await;
    let moved = state.move_session(&id, other.to_str().unwrap()).await.expect("moved");
    assert_eq!(moved.project_id.as_deref(), Some(other_project.as_str()));
    assert!(state.open_entry(&id).is_none(), "closed to move");
    let meta = SessionMeta::load(&files_dir(state.sessions_dir(), &id)).unwrap().unwrap();
    assert_eq!(meta.working_dir, other);
}

/// The history of the stored conversation `id`, as its messages' texts.
async fn stored_texts(state: &AppState, id: &str) -> Vec<String> {
    let store = bricks_memory::JsonlMemory::new(state.sessions_dir());
    use bricks_memory::Memory as _;
    store.load(id).await.unwrap().iter().map(message_text).collect()
}

#[tokio::test]
async fn two_messages_to_a_busy_conversation_each_get_their_own_answer() {
    use crate::services::mail::MailState;
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    register_sender(&state, &wd);
    let script = Script::new(vec![Reply::hang(), Reply::text("réponse 1"), Reply::text("réponse 2")]);
    let b = open_scripted(&state, &project, &wd, script).await;
    let b_id = b.session_id.clone();
    b.controller
        .send(Command::Submit { prompt: Prompt::text("travail en cours") })
        .unwrap();
    eventually("B to work", || b.is_running()).await;

    let one = state.mail_send(SENDER, &b_id, "message un").await.expect("first sent");
    let two = state.mail_send(SENDER, &b_id, "message deux").await.expect("second sent");
    assert_ne!(one.id, two.id);
    b.controller.send(Command::Cancel).unwrap();

    let one = state.mail.wait(&one.id, Duration::from_secs(20)).await.unwrap();
    let two = state.mail.wait(&two.id, Duration::from_secs(20)).await.unwrap();
    assert_eq!(one.state, MailState::Answered, "{one:?}");
    assert_eq!(two.state, MailState::Answered, "{two:?}");
    // Which message is delivered first is a race between the two waits; what
    // holds is that each run's answer goes to the message whose prompt it
    // answered: the run of the earlier message in the history got the first reply.
    let history = b.controller.history().await;
    let at = |needle: &str| {
        history
            .iter()
            .position(|m| message_text(m).contains(needle))
            .unwrap_or_else(|| panic!("«{needle}» is in the history"))
    };
    let (earlier, later) = if at("message un") < at("message deux") {
        (&one, &two)
    } else {
        (&two, &one)
    };
    assert_eq!(earlier.answer.as_deref(), Some("réponse 1"), "{earlier:?}");
    assert_eq!(later.answer.as_deref(), Some("réponse 2"), "{later:?}");
    assert_eq!(earlier.outcome.as_deref(), Some("succeeded"));
    assert_eq!(later.outcome.as_deref(), Some("succeeded"));
    // Each message is logged under its sender, with its own answer.
    let log = std::fs::read_to_string(crate::services::mail::log_path(&home.path().join("desktop").join("messages"), SENDER))
        .unwrap();
    assert!(log.contains(&one.id) && log.contains(&two.id), "{log}");
    let _ = state.close_session(&b_id, true).await;
}

#[tokio::test]
async fn a_message_to_a_stored_conversation_that_is_not_open_opens_it_and_is_delivered() {
    use crate::contract::OpenSessionRequest;
    use crate::services::mail::MailState;
    let home = tempfile::tempdir().unwrap();
    // The real registry (the local provider of the fixtures, refusing its
    // connections at once), so opening the conversation goes through the
    // same path as the application's: no scripted catalog.
    let providers = home.path().join("providers.toml");
    std::fs::write(&providers, crate::services::providers_toml::fixtures::PROVIDERS).unwrap();
    let registry = bricks_config::load_registry(Some(&providers)).map(Arc::new);
    let state = AppState::assemble(home.path().to_path_buf(), registry);
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let project = state.projects.add(wd.to_str().unwrap()).expect("project").id;
    register_sender(&state, &wd);

    let opened = state
        .open_session(OpenSessionRequest {
            project_id: project.clone(),
            session_id: None,
            model: Some("local/small".into()),
            reasoning: None,
        })
        .await
        .expect("opened");
    let id = opened.session.id.clone();
    // A conversation is stored once a run has been made in it: this one fails
    // at the provider, which is all the stored conversation needs.
    state.send_command(&id, submit_json("première"), None).await.expect("first run");
    state.open_entry(&id).expect("open").controller.wait_idle().await;
    state.close_session(&id, false).await.expect("closed: only its stored files remain");
    assert!(state.open_entry(&id).is_none());

    let mail = state.mail_send(SENDER, &id, "bonjour").await.expect("sent");
    assert!(state.open_entry(&id).is_some(), "the target is opened to receive the message");
    let done = state
        .mail
        .wait(&mail.id, Duration::from_secs(60))
        .await
        .expect("the message is known");
    assert!(done.state.is_final(), "the message ends: {done:?}");
    assert_ne!(done.state, MailState::Sent, "it was submitted to the opened conversation");
    assert!(
        stored_texts(&state, &id).await.iter().any(|t| t.contains("bonjour")),
        "the message is in the conversation's history"
    );
    let _ = state.close_session(&id, true).await;
}

#[tokio::test]
async fn a_fork_of_a_fork_names_its_parent_and_the_last_fork_outlives_the_middle_one() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let entry = open_scripted(&state, &project, &wd, Script::new(vec![Reply::text("ok")])).await;
    let root = entry.session_id.clone();
    entry
        .controller
        .send(Command::Submit { prompt: Prompt::text("une question") })
        .unwrap();
    entry.controller.wait_idle().await;
    state.close_session(&root, false).await.unwrap();

    let middle = state.fork_session(&root).await.expect("first fork");
    let last = state.fork_session(&middle.id).await.expect("fork of the fork");
    assert_eq!(middle.parent_id.as_deref(), Some(root.as_str()));
    assert_eq!(last.parent_id.as_deref(), Some(middle.id.as_str()), "the parent is the fork, not the root");
    assert_eq!(last.title, format!("{} (fork)", middle.title), "the title of the parent, forked again");
    let expected = stored_texts(&state, &root).await;
    assert_eq!(stored_texts(&state, &last.id).await, expected, "the whole history is copied");

    // The middle conversation goes: the last fork stays whole, and still names it.
    state.delete_session(&middle.id).await.expect("delete the middle");
    assert_eq!(stored_texts(&state, &last.id).await, expected, "still readable after its parent's deletion");
    let page = state.session_page(&last.id, 0, 100).await.expect("readable");
    assert_eq!(page.total, expected.len());
    let listed = state.sessions_list(&project).await.unwrap();
    let kept = listed.iter().find(|s| s.id == last.id).expect("still listed");
    assert_eq!(kept.parent_id.as_deref(), Some(middle.id.as_str()), "the parent is kept as recorded");
    assert!(listed.iter().any(|s| s.id == root), "the root stays");
    assert!(!listed.iter().any(|s| s.id == middle.id), "the middle one is gone");
}

#[tokio::test]
async fn session_page_pages_the_messages_and_refuses_an_unknown_conversation() {
    let home = tempfile::tempdir().unwrap();
    let wd_dir = tempfile::tempdir().unwrap();
    let wd = std::fs::canonicalize(wd_dir.path()).unwrap();
    let (state, project) = state_with_project(home.path(), &wd);
    let script = Script::new(vec![Reply::text("r1"), Reply::text("r2"), Reply::text("r3")]);
    let entry = open_scripted(&state, &project, &wd, script).await;
    let id = entry.session_id.clone();
    for prompt in ["p1", "p2", "p3"] {
        entry.controller.send(Command::Submit { prompt: Prompt::text(prompt) }).unwrap();
        entry.controller.wait_idle().await;
    }
    // Three exchanges: six messages, user then assistant.
    let open = state.session_page(&id, 0, 2).await.expect("an open conversation is read");
    assert_eq!(open.total, 6);
    assert_eq!(open.lines.len(), 2);
    assert!(open.lines[0].starts_with("[0] user: p1"), "{:?}", open.lines);
    assert!(open.lines[1].starts_with("[1] assistant: r1"), "{:?}", open.lines);
    state.close_session(&id, false).await.unwrap();

    let page = state.session_page(&id, 2, 2).await.expect("a stored conversation is read");
    assert_eq!((page.offset, page.total, page.lines.len()), (2, 6, 2));
    assert!(page.lines[0].starts_with("[2] user: p2"), "{:?}", page.lines);
    let tail = state.session_page(&id, 4, 10).await.unwrap();
    assert_eq!(tail.lines.len(), 2, "what is left after the offset");
    assert!(tail.lines[1].starts_with("[5] assistant: r3"), "{:?}", tail.lines);
    let past = state.session_page(&id, 6, 2).await.unwrap();
    assert!(past.lines.is_empty());
    assert_eq!(past.total, 6, "the total is kept past the end");
    // A limit is kept between 1 and 100.
    assert_eq!(state.session_page(&id, 0, 0).await.unwrap().lines.len(), 1);
    assert_eq!(state.session_page(&id, 0, 10_000).await.unwrap().lines.len(), 6);

    let unknown = state.session_page("20990101-000000-zzzzzz", 0, 5).await.err().expect("refused");
    assert_eq!(unknown.code, IpcErrorCode::NotFound);
    let malformed = state.session_page("../secrets", 0, 5).await.err().expect("refused");
    assert_eq!(malformed.code, IpcErrorCode::Invalid);
}

#[tokio::test]
async fn moving_to_a_vanished_folder_or_to_the_chats_folder_is_refused_and_changes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let wd = folder(root.path(), "projet");
    let gone = folder(root.path(), "disparu");
    let (state, project) = state_with_project(home.path(), &wd);
    let vanished = state.projects.add(gone.to_str().unwrap()).unwrap().id;
    let id = stored_session(&state, &project, &wd).await;
    std::fs::remove_dir_all(&gone).unwrap();
    assert!(state.projects.get(&vanished).unwrap().missing);

    let to_vanished = state.move_session(&id, gone.to_str().unwrap()).await.err().expect("refused");
    assert_eq!(to_vanished.code, IpcErrorCode::Rejected, "{}", to_vanished.message);
    let chats = state.projects.chats().expect("the chats project");
    let to_chats = state.move_session(&id, &chats.path).await.err().expect("refused");
    assert_eq!(to_chats.code, IpcErrorCode::Rejected, "{}", to_chats.message);

    let meta = SessionMeta::load(&files_dir(state.sessions_dir(), &id)).unwrap().unwrap();
    assert_eq!(meta.working_dir, wd, "still in its project's folder");
    let listed = state.sessions_list(&project).await.unwrap();
    assert!(listed.iter().any(|s| s.id == id), "still listed in its project");
    assert!(state.sessions_list(&vanished).await.unwrap().iter().all(|s| s.id != id));
}
