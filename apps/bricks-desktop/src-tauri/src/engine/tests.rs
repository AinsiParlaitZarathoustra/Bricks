//! The engine against the scripted catalog of `cersei_agent`: a real
//! controller, real stores, no provider.

use super::AppState;
use super::live::{OpenSession, files_dir, ingest, spawn_consumer};
use crate::contract::{DesktopEvent, IpcErrorCode, OpenedSession, RunLabel, SearchMatch};
use cersei_agent::BricksConfig;
use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::{
    ApprovalRequest, Command, Controller, EngineConfig, Envelope, Event, OpenOptions, Prompt,
    SCHEMA_VERSION, SessionChoice, SessionMeta, SkillsSetup, WrittenFile,
};
use cersei_tools::preview::{ChangeKind, ChangePreview, FileChange};
use cersei_types::{Message, Role};
use crate::services::conversations::message_text;
use std::path::Path;
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

/// As [`open_scripted_as`], with the personal skills of `home` usable when given.
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
    cfg.skills = home.map(|h| SkillsSetup::with_home(Some(h.to_path_buf())));
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
async fn an_applied_edit_is_journaled_once_with_the_patch_it_was_previewed_with() {
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
    let preview = ChangePreview {
        files: vec![FileChange {
            path: "a.txt".into(),
            absolute: wd.join("a.txt"),
            kind: ChangeKind::Modify,
            before_sha256: None,
            diff: "@@ -1 +1 @@\n-a\n+b\n".into(),
            added: 1,
            removed: 1,
        }],
        refusal: None,
    };
    let approval = ApprovalRequest {
        approval_id: "ap_1".into(),
        tool_call_id: "call_1".into(),
        tool: "Write".into(),
        level: "write".into(),
        description: "écrire a.txt".into(),
        input: serde_json::json!({ "file_path": "a.txt" }),
        preview: Some(preview),
        agent_id: None,
    };
    let applied = Event::EditApplied {
        tool_call_id: "call_1".into(),
        tool: "Write".into(),
        files: vec![WrittenFile {
            path: "a.txt".into(),
            kind: ChangeKind::Modify,
            added: 1,
            removed: 1,
            binary: false,
        }],
        agent_id: None,
        changeset_id: None,
    };
    ingest(&state.hub, &entry, envelope(1, Event::ApprovalRequested { approval }));
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
    use cersei_agent::control::{Activity, ModelCatalog, Prompt};

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

// ─── Sprint 14.4: labels of runs started from typed commands ───────────────

/// The command JSON of a submit of `text`, as the frontend sends it.
fn submit_json(text: &str) -> serde_json::Value {
    serde_json::to_value(Command::Submit { prompt: Prompt::text(text) }).unwrap()
}

/// A personal skill `name` under `home`, with its procedure as the body.
fn write_skill(home: &Path, name: &str, body: &str) {
    let dir = home.join(".agents").join("skills").join(name);
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


