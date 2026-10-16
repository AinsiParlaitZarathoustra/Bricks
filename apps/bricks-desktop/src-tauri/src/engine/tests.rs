//! The engine against the scripted catalog of `cersei_agent`: a real
//! controller, real stores, no provider.

use super::AppState;
use super::live::{OpenSession, files_dir, ingest, spawn_consumer};
use crate::contract::{IpcErrorCode, SearchMatch};
use cersei_agent::BricksConfig;
use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::{
    ApprovalRequest, Command, Controller, EngineConfig, Envelope, Event, OpenOptions, Prompt,
    SCHEMA_VERSION, SessionChoice, SessionMeta, WrittenFile,
};
use cersei_tools::preview::{ChangeKind, ChangePreview, FileChange};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

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
    let mut cfg = EngineConfig::new(
        wd,
        ScriptedCatalog::new(&["a"], script),
        BricksConfig::default(),
        state.sessions_dir().to_path_buf(),
    );
    cfg.interactive = false;
    let (controller, mut stream) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
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

