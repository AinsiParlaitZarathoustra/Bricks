//! Attachment copies bound to the application's sessions: import, discard,
//! promotion of a draft into a session, the copies a submit sends, and their
//! preview. The copies and their rules are in `services::attachments`; this
//! file gives them the sessions' lifecycle (the lock every create and retire
//! takes) and the references a sent message keeps.

use super::AppState;
use super::live::{OpenSession, files_dir};
use crate::contract::{
    AttachmentPreview, AttachmentRef, AttachmentScope, ImportedAttachment, IpcErrorCode, IpcResult,
    ResolvedAttachment, SavedAttachment,
};
use crate::services::attachment_runs;
use crate::services::attachments as store;
use crate::services::join_error;
use crate::services::run_labels::check_session_id;
use crate::services::{error, invalid};
use bricks_agent::control::ModelCatalog;
use bricks_agent::control::ModelChoice;
use bricks_agent::control::protocol::{Prompt, PromptBlock};
use std::path::Path;

/// The refusal of an image sent to a model without image input. The composer
/// shows the same text.
pub const NO_IMAGE_INPUT: &str = "Ce modèle n’accepte pas les images : retirez cette pièce jointe.";

/// Whether the model `selection` takes images. A model the catalog does not
/// know is left to the engine, which refuses it on its own.
pub fn model_accepts_images(choices: &[ModelChoice], selection: &str) -> bool {
    choices
        .iter()
        .find(|m| m.selection == selection)
        .is_none_or(|m| m.input.iter().any(|input| input == "image"))
}

/// Runs blocking file work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> IpcResult<T> + Send + 'static,
) -> IpcResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(join_error)?
}

fn check_owner(id: &str, what: &str) -> IpcResult<()> {
    if store::valid_id(id) {
        Ok(())
    } else {
        Err(invalid(format!("identifiant de {what} invalide")))
    }
}

impl AppState {
    /// Imports pasted bytes into `draft_id` (a new draft when none is given).
    pub async fn import_attachment_bytes(
        &self,
        draft_id: Option<String>,
        name: String,
        data_base64: String,
    ) -> IpcResult<ImportedAttachment> {
        let drafts = self.drafts_dir().to_path_buf();
        blocking(move || store::import_bytes(&drafts, draft_id.as_deref(), &name, &data_base64))
            .await
    }

    /// Imports the file at `path` into `draft_id` (a new draft when none is given).
    pub async fn import_attachment_path(
        &self,
        draft_id: Option<String>,
        path: String,
    ) -> IpcResult<ImportedAttachment> {
        let drafts = self.drafts_dir().to_path_buf();
        blocking(move || store::import_path(&drafts, draft_id.as_deref(), &path)).await
    }

    /// Pasted bytes as the first version of the command returned them: a new
    /// draft's copy, by its absolute path.
    pub async fn save_pasted(
        &self,
        name: String,
        data_base64: String,
    ) -> IpcResult<SavedAttachment> {
        let imported = self
            .import_attachment_bytes(None, name, data_base64)
            .await?;
        let dir = store::draft_attachments(self.drafts_dir(), &imported.draft_id);
        let path = store::copy_path(&dir, &imported.attachment.id, &imported.attachment.name);
        Ok(SavedAttachment {
            path: path.display().to_string(),
            media_type: imported.attachment.mime,
            bytes: imported.attachment.size,
        })
    }

    /// Frees the copy of a draft. Idempotent.
    pub async fn discard_draft_attachment(
        &self,
        draft_id: String,
        attachment_id: String,
    ) -> IpcResult<()> {
        check_owner(&draft_id, "brouillon")?;
        check_owner(&attachment_id, "pièce jointe")?;
        let drafts = self.drafts_dir().to_path_buf();
        blocking(move || store::discard_draft_attachment(&drafts, &draft_id, &attachment_id)).await
    }

    /// Removes a copy of a session, unless a sent message refers to it.
    pub async fn discard_session_attachment(
        &self,
        session_id: String,
        attachment_id: String,
    ) -> IpcResult<()> {
        check_session_id(&session_id)?;
        check_owner(&attachment_id, "pièce jointe")?;
        let _gate = self.lifecycle.lock().await;
        let files = files_dir(self.sessions_dir(), &session_id);
        blocking(move || {
            if attachment_runs::referenced(&files).contains(&attachment_id) {
                return Err(error(
                    IpcErrorCode::Rejected,
                    "cette pièce jointe fait partie d'un message envoyé",
                ));
            }
            store::remove_copy(&store::session_attachments(&files), &attachment_id)
        })
        .await
    }

    /// Removes a draft and its copies. Idempotent.
    pub async fn discard_draft(&self, draft_id: String) -> IpcResult<()> {
        check_owner(&draft_id, "brouillon")?;
        let drafts = self.drafts_dir().to_path_buf();
        blocking(move || store::discard_draft(&drafts, &draft_id)).await
    }

    /// Moves a draft's copies into a session, before its submit. Returns the
    /// copies as the session now holds them. The session's folder is locked
    /// against the session's deletion while the copies move.
    pub async fn promote_draft(
        &self,
        draft_id: String,
        session_id: String,
    ) -> IpcResult<Vec<AttachmentRef>> {
        check_owner(&draft_id, "brouillon")?;
        check_session_id(&session_id)?;
        let _gate = self.lifecycle.lock().await;
        let drafts = self.drafts_dir().to_path_buf();
        let sessions = self.sessions_dir().to_path_buf();
        blocking(move || store::promote(&drafts, &sessions, &draft_id, &session_id)).await
    }

    /// The absolute paths of a session's copies, in the order given: what a
    /// submit passes to the engine.
    pub async fn session_attachment_paths(
        &self,
        session_id: String,
        attachment_ids: Vec<String>,
    ) -> IpcResult<Vec<ResolvedAttachment>> {
        check_session_id(&session_id)?;
        let files = files_dir(self.sessions_dir(), &session_id);
        blocking(move || store::resolve(&store::session_attachments(&files), &attachment_ids)).await
    }

    /// A copy as the interface shows it, with bounded reads.
    pub async fn attachment_preview(
        &self,
        scope: AttachmentScope,
        owner_id: String,
        attachment_id: String,
    ) -> IpcResult<AttachmentPreview> {
        let dir = match scope {
            AttachmentScope::Draft => {
                check_owner(&owner_id, "brouillon")?;
                store::draft_attachments(self.drafts_dir(), &owner_id)
            }
            AttachmentScope::Session => {
                check_session_id(&owner_id)?;
                store::session_attachments(&files_dir(self.sessions_dir(), &owner_id))
            }
        };
        blocking(move || store::preview(&dir, &attachment_id)).await
    }

    /// The copies a submit carries, which the desktop keeps with the run it
    /// starts, and the check of images against the session's model. An image
    /// sent to a model without image input is refused here, before anything
    /// is sent: no user turn is left in the history.
    pub(crate) fn submit_copies(
        &self,
        entry: &OpenSession,
        prompt: &Prompt,
    ) -> IpcResult<Vec<AttachmentRef>> {
        if prompt
            .blocks
            .iter()
            .any(|block| matches!(block, PromptBlock::Image { .. }))
        {
            let model = entry.controller.snapshot().model;
            let choices = ModelCatalog::models(self.catalog.as_ref());
            if !model_accepts_images(&choices, &model) {
                return Err(error(IpcErrorCode::Rejected, NO_IMAGE_INPUT));
            }
        }
        let files = files_dir(self.sessions_dir(), &entry.session_id);
        let dir = store::session_attachments(&files);
        let mut copies = Vec::new();
        for block in &prompt.blocks {
            let (PromptBlock::File { path } | PromptBlock::Image { path }) = block else {
                continue;
            };
            let path = Path::new(path);
            if path.parent() != Some(dir.as_path()) {
                continue;
            }
            let attachment = store::attachment_of_copy(path)
                .ok_or_else(|| crate::services::not_found("pièce jointe introuvable"))?;
            copies.push(attachment);
        }
        Ok(copies)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{AttachmentKind, IpcErrorCode};
    use bricks_agent::BricksConfig;
    use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
    use bricks_agent::control::{Controller, EngineConfig, OpenOptions, SessionChoice};
    use std::sync::Arc;
    use std::time::Duration;

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn state(home: &Path) -> AppState {
        AppState::assemble(
            home.to_path_buf(),
            Err("aucun registre dans ce test".into()),
        )
    }

    /// A session of `wd` on the scripted catalog, registered as open, as the
    /// desktop registers it.
    async fn open_scripted(state: &AppState, wd: &Path, script: Arc<Script>) -> Arc<OpenSession> {
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
            "project".into(),
            state.sessions_dir().to_path_buf(),
            0,
            0,
            Arc::clone(&state.run_labels),
            Arc::clone(&state.run_records),
        );
        while let Some(env) = stream.try_next() {
            super::super::live::ingest(&state.hub, &entry, env);
        }
        state
            .sessions
            .lock()
            .insert(entry.session_id.clone(), entry.clone());
        super::super::live::spawn_consumer(state.hub.clone(), entry.clone(), stream, stop);
        entry
    }

    #[test]
    fn a_model_without_image_input_refuses_images_and_an_unknown_one_is_left_to_the_engine() {
        let choice = |selection: &str, input: &[&str]| ModelChoice {
            selection: selection.into(),
            name: selection.into(),
            provider: "p".into(),
            profiles: Vec::new(),
            default_profile: None,
            max_input_tokens: 1000,
            input: input.iter().map(|s| s.to_string()).collect(),
            priced: false,
        };
        let choices = vec![
            choice("p/texte", &["text"]),
            choice("p/vision", &["text", "image"]),
        ];
        assert!(!model_accepts_images(&choices, "p/texte"));
        assert!(model_accepts_images(&choices, "p/vision"));
        assert!(model_accepts_images(&choices, "p/absent"));
    }

    #[tokio::test]
    async fn a_session_copy_is_sent_kept_with_its_run_and_shown_again_on_reopen() {
        let home = tempfile::tempdir().unwrap();
        let wd = std::fs::canonicalize(tempfile::tempdir().unwrap().path()).unwrap();
        let st = state(home.path());
        let entry = open_scripted(&st, &wd, Script::new(vec![Reply::text("reçu")])).await;
        let id = entry.session_id.clone();

        // A draft's copy, promoted into the session at the send.
        let imported = st
            .import_attachment_bytes(None, "notes.txt".into(), b64(b"contenu du fichier"))
            .await
            .unwrap();
        let promoted = st
            .promote_draft(imported.draft_id.clone(), id.clone())
            .await
            .unwrap();
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].kind, AttachmentKind::Text);
        let paths = st
            .session_attachment_paths(id.clone(), vec![imported.attachment.id.clone()])
            .await
            .unwrap();
        assert!(
            paths[0].path.contains(&format!("{id}.files")),
            "{}",
            paths[0].path
        );

        // The submit, as the interface sends it: the session's copy path.
        let command = serde_json::json!({
            "type": "submit",
            "prompt": { "blocks": [ { "kind": "file", "path": paths[0].path } ] }
        });
        st.send_command(&id, command, None).await.unwrap();
        entry.controller.wait_idle().await;

        // The reference is kept with the run, in the session's folder.
        let files = files_dir(st.sessions_dir(), &id);
        let mut records = Vec::new();
        for _ in 0..200 {
            records = attachment_runs::load(&files);
            if !records.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert_eq!(records.len(), 1, "one record for the run");
        assert_eq!(records[0].refs[0].id, imported.attachment.id);

        // Reopened: the user message carries the copy, and its raw wrapper is not shown.
        let opened = entry.opened().await;
        let user = opened
            .history
            .iter()
            .find(|m| matches!(m.role, crate::contract::HistoryRole::User))
            .expect("the user message");
        assert_eq!(user.attachments.len(), 1);
        assert_eq!(user.attachments[0].name, "notes.txt");
        assert!(
            !user.blocks.iter().any(|b| matches!(b, crate::contract::HistoryBlock::Text { text } if text.starts_with("<attached_file"))),
            "the wrapper of a copy is not a bubble of its own"
        );

        // A copy the sent message refers to cannot be discarded.
        let refused = st
            .discard_session_attachment(id.clone(), imported.attachment.id.clone())
            .await
            .unwrap_err();
        assert_eq!(refused.code, IpcErrorCode::Rejected);

        // Preview of the session copy (bounded).
        let shown = st
            .attachment_preview(
                AttachmentScope::Session,
                id.clone(),
                imported.attachment.id.clone(),
            )
            .await
            .unwrap();
        assert_eq!(shown.text.as_deref(), Some("contenu du fichier"));
        let _ = st.close_session(&id, false).await;
    }

    #[tokio::test]
    async fn a_session_copy_nobody_sent_is_discarded_and_deleting_the_session_removes_its_copies() {
        let home = tempfile::tempdir().unwrap();
        let st = state(home.path());
        let id = "20260101-120000-abcdef";
        let files = files_dir(st.sessions_dir(), id);
        std::fs::create_dir_all(&files).unwrap();
        let imported = st
            .import_attachment_bytes(None, "a.txt".into(), b64(b"un"))
            .await
            .unwrap();
        st.promote_draft(imported.draft_id.clone(), id.into())
            .await
            .unwrap();
        let copy = store::copy_path(
            &store::session_attachments(&files),
            &imported.attachment.id,
            "a.txt",
        );
        assert!(copy.is_file());

        // Nothing refers to it: it goes.
        st.discard_session_attachment(id.into(), imported.attachment.id.clone())
            .await
            .unwrap();
        assert!(!copy.exists());

        // Another copy, then the session's deletion.
        let second = st
            .import_attachment_bytes(None, "b.txt".into(), b64(b"deux"))
            .await
            .unwrap();
        st.promote_draft(second.draft_id.clone(), id.into())
            .await
            .unwrap();
        let kept = store::copy_path(
            &store::session_attachments(&files),
            &second.attachment.id,
            "b.txt",
        );
        assert!(kept.is_file());
        st.delete_session(id).await.unwrap();
        assert!(
            !files.exists(),
            "the session's folder, copies included, is gone"
        );
        assert!(!kept.exists());
    }

    #[tokio::test]
    async fn a_draft_is_discarded_with_its_copies_and_a_bad_owner_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let st = state(home.path());
        let first = st
            .import_attachment_bytes(None, "a.txt".into(), b64(b"1"))
            .await
            .unwrap();
        st.discard_draft_attachment(first.draft_id.clone(), first.attachment.id.clone())
            .await
            .unwrap();
        st.discard_draft(first.draft_id.clone()).await.unwrap();
        assert!(!st.drafts_dir().join(&first.draft_id).exists());
        // An owner that is not an opaque id never reaches the disk.
        let err = st
            .discard_draft_attachment("../../x".into(), first.attachment.id.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Invalid);
        let err = st
            .attachment_preview(
                AttachmentScope::Draft,
                "../x".into(),
                first.attachment.id.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Invalid);
    }

    #[tokio::test]
    async fn the_legacy_save_stores_a_draft_copy_by_its_path() {
        let home = tempfile::tempdir().unwrap();
        let st = state(home.path());
        let saved = st
            .save_pasted("capture.png".into(), b64(b"pas une image"))
            .await
            .unwrap();
        assert!(saved.path.ends_with("-capture.png"), "{}", saved.path);
        assert_eq!(
            saved.media_type, "text/plain",
            "the type comes from the content"
        );
        assert!(Path::new(&saved.path).is_file());
        assert!(Path::new(&saved.path).starts_with(st.drafts_dir()));
    }
}
