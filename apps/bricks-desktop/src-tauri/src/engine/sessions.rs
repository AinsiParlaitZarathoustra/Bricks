//! Conversations: opening, closing, deleting, renaming, listing, searching
//! and commanding the sessions of the application.
//!
//! Lifecycle rules:
//! * a session has at most one controller; a second open attaches to it;
//! * a controller is retired by cancelling what runs, waiting for the run to
//!   save, then closing it; only then may the session be deleted, so a late
//!   save cannot recreate it;
//! * operations that create or retire controllers run one at a time.

use super::live::{self, OpenSession, files_dir, load_meta, session_item, spawn_consumer, stored_message_count};
use super::{AppState, error};
use crate::contract::{
    DesktopEvent, IpcErrorCode, IpcResult, OpenSessionRequest, OpenedSession, SearchMatch,
    SessionItem, SessionSearchHit,
};
use crate::services::conversations::{
    contains_folded, item_from_summary, message_text, path_text, snippet,
};
use crate::services::run_labels::normalize_label;
use crate::services::{invalid, io_error, not_found, now_ms};
use bricks_agent::control::{
    Activity, Command, Controller, OpenOptions, SessionChoice, SessionMeta, SessionScope,
    list_sessions, list_sessions_in,
};
use bricks_memory::{JsonlMemory, Memory};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// How long a retirement waits for the run to finish its save.
const SETTLE: Duration = Duration::from_secs(15);
const TITLE_MAX_CHARS: usize = 200;
/// Hits returned by one search.
const SEARCH_LIMIT: usize = 100;

impl AppState {
    /// Opens the session (new, or resumed from the store) in its project, or
    /// attaches to the controller already open for it.
    pub async fn open_session(&self, request: OpenSessionRequest) -> IpcResult<OpenedSession> {
        let _gate = self.lifecycle.lock().await;
        if let Some(id) = request.session_id.as_deref() {
            check_session_id(id)?;
            if let Some(entry) = self.open_entry(id) {
                if entry.project_id != request.project_id {
                    return Err(error(
                        IpcErrorCode::SessionBusy,
                        "cette conversation est ouverte dans un autre projet",
                    ));
                }
                return Ok(entry.opened().await);
            }
        }
        let project = self.projects.get(&request.project_id)?;
        if project.missing {
            return Err(error(
                IpcErrorCode::FolderMissing,
                format!("le dossier {} n'est plus accessible", project.path),
            ));
        }
        let registry = self.registry()?;
        // A conversation without a project works in its own folder: a new
        // one gets a fresh folder, a resumed one keeps its recorded folder.
        let working_dir = match (project.chats, request.session_id.as_deref()) {
            (false, _) => PathBuf::from(&project.path),
            (true, None) => self.projects.new_chat_dir()?,
            (true, Some(id)) => load_meta(self.sessions_dir(), id)
                .await
                .map(|m| m.working_dir)
                .unwrap_or_else(|| PathBuf::from(&project.path)),
        };
        let fresh_dir = (project.chats && request.session_id.is_none()).then(|| working_dir.clone());
        let mut cfg = bricks_config::engine_config(&working_dir, registry, true);
        // The catalog is the application's: a change reaches every session,
        // at its next run. New sessions start from this home's default policy.
        cfg.catalog = Arc::clone(&self.catalog) as Arc<dyn bricks_agent::control::ModelCatalog>;
        cfg.permissions = Some(self.permissions.clone());
        cfg.generated_titles = true;
        // Sessions are stored under this application state's home (tests use
        // a temporary one), never the global default.
        cfg.sessions_dir = self.sessions_dir().to_path_buf();
        let session = match request.session_id.clone() {
            Some(id) => SessionChoice::Resume(id),
            None => SessionChoice::New,
        };
        let options = OpenOptions {
            session,
            model: request.model.clone(),
            reasoning: request.reasoning.clone(),
        };
        let (controller, mut stream) = match Controller::open(cfg, options).await {
            Ok(opened) => opened,
            Err(e) => {
                if let Some(dir) = &fresh_dir {
                    let _ = std::fs::remove_dir(dir);
                }
                return Err(open_error(e));
            }
        };
        let session_id = controller.session_id();
        let message_count = stored_message_count(self.sessions_dir(), &session_id).await;
        let created_at = load_meta(self.sessions_dir(), &session_id)
            .await
            .map(|m| m.created_at)
            .unwrap_or_else(now_ms);
        let (entry, stop) = OpenSession::new(
            controller,
            project.id.clone(),
            self.sessions_dir().to_path_buf(),
            created_at,
            message_count,
            Arc::clone(&self.run_labels),
            Arc::clone(&self.run_records),
        );
        // What opening queued (the session's own first event) is taken now,
        // before any consumer runs, so the first view receives it.
        while let Some(env) = stream.try_next() {
            live::ingest(&self.hub, &entry, env);
        }
        self.sessions.lock().insert(session_id.clone(), entry.clone());
        spawn_consumer(self.hub.clone(), entry.clone(), stream, stop);
        self.projects.touch(&project.id);
        self.hub.publish(DesktopEvent::SessionState {
            session_id,
            open: true,
            running: false,
        });
        Ok(entry.opened().await)
    }

    /// Saves and closes the session's controller. Refused with `SessionBusy`
    /// while something runs, unless `cancel` stops it first.
    pub async fn close_session(&self, session_id: &str, cancel: bool) -> IpcResult<()> {
        check_session_id(session_id)?;
        let _gate = self.lifecycle.lock().await;
        let entry = self.require_open(session_id)?;
        if entry.is_busy() && !cancel {
            return Err(error(
                IpcErrorCode::SessionBusy,
                "une exécution est en cours : annulez-la ou attendez sa fin",
            ));
        }
        self.retire(&entry).await;
        Ok(())
    }

    /// Deletes a stored session. A controller open for it is cancelled and
    /// closed first, so the deletion is final.
    pub async fn delete_session(&self, session_id: &str) -> IpcResult<()> {
        check_session_id(session_id)?;
        let _gate = self.lifecycle.lock().await;
        if let Some(entry) = self.open_entry(session_id) {
            self.retire(&entry).await;
        }
        let transcript = self.sessions_dir().join(format!("{session_id}.jsonl"));
        let files = files_dir(self.sessions_dir(), session_id);
        let labels = Arc::clone(&self.run_labels);
        let records = Arc::clone(&self.run_records);
        let labelled = labels.exists(session_id) || records.exists(session_id);
        if !transcript.exists() && !files.exists() && !labelled {
            return Err(not_found("conversation inconnue"));
        }
        // The own folder of a conversation without a project goes with it.
        let chat_dir = load_meta(self.sessions_dir(), session_id)
            .await
            .map(|m| m.working_dir)
            .filter(|d| self.projects.is_chat_dir(d));
        JsonlMemory::new(self.sessions_dir())
            .delete(session_id)
            .await
            .map_err(|e| io_error("suppression de la conversation", e))?;
        let id = session_id.to_string();
        tauri::async_runtime::spawn_blocking(move || -> IpcResult<()> {
            labels.delete(&id)?;
            records.delete(&id)
        })
        .await
        .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))??;
        if let Some(dir) = chat_dir
            && let Err(e) = std::fs::remove_dir_all(&dir)
        {
            eprintln!("bricks-desktop: dossier de conversation {} : {e}", dir.display());
        }
        self.hub.publish(DesktopEvent::SessionDeleted {
            session_id: session_id.to_string(),
        });
        Ok(())
    }

    /// Renames a session. An open one keeps the title through its later
    /// saves; a closed one has its metadata rewritten.
    pub async fn rename_session(&self, session_id: &str, title: &str) -> IpcResult<SessionItem> {
        check_session_id(session_id)?;
        let title = title.trim();
        if title.is_empty() {
            return Err(invalid("le titre ne peut pas être vide"));
        }
        if title.chars().count() > TITLE_MAX_CHARS {
            return Err(invalid(format!(
                "le titre dépasse {TITLE_MAX_CHARS} caractères"
            )));
        }
        let _gate = self.lifecycle.lock().await;
        let item = match self.open_entry(session_id) {
            Some(entry) => {
                entry
                    .controller
                    .set_title(title)
                    .map_err(|e| io_error("renommage", e))?;
                session_item(&entry).await
            }
            None => {
                let dir = files_dir(self.sessions_dir(), session_id);
                let new_title = title.to_string();
                let meta = tauri::async_runtime::spawn_blocking(
                    move || -> Result<Option<SessionMeta>, String> {
                        match SessionMeta::load(&dir)? {
                            Some(mut meta) => {
                                meta.title = new_title;
                                meta.save(&dir)?;
                                Ok(Some(meta))
                            }
                            None => Ok(None),
                        }
                    },
                )
                .await
                .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))?
                .map_err(|e| io_error("renommage", e))?;
                let Some(meta) = meta else {
                    return Err(not_found("conversation inconnue"));
                };
                let count = stored_message_count(self.sessions_dir(), session_id).await;
                self.item_from_meta(&meta, count, false, false)
            }
        };
        self.hub.publish(DesktopEvent::SessionUpdated {
            session: item.clone(),
        });
        Ok(item)
    }

    /// The conversations of a project: its stored sessions (by working
    /// folder) merged with the ones open now, most recent first.
    pub async fn sessions_list(&self, project_id: &str) -> IpcResult<Vec<SessionItem>> {
        let project = self.projects.get(project_id)?;
        let stored = if project.chats {
            let mut all = list_sessions(self.sessions_dir())
                .await
                .map_err(|e| io_error("conversations", e))?;
            all.retain(|s| {
                s.working_dir
                    .as_deref()
                    .is_some_and(|d| self.projects.is_chat_dir(d))
            });
            all
        } else if project.missing {
            // A vanished folder resolves to nothing: its conversations are the
            // ones recorded under its exact path (listed, to be moved with it).
            let folder = PathBuf::from(&project.path);
            let mut all = list_sessions(self.sessions_dir())
                .await
                .map_err(|e| io_error("conversations", e))?;
            all.retain(|s| s.working_dir.as_deref() == Some(folder.as_path()));
            all
        } else {
            let scope = SessionScope::Workspace(PathBuf::from(&project.path));
            list_sessions_in(self.sessions_dir(), &scope)
                .await
                .map_err(|e| io_error("conversations", e))?
        };
        let open = self.entries_of_project(project_id);
        let mut items = Vec::new();
        for summary in &stored {
            match open.iter().find(|e| e.session_id == summary.id) {
                Some(entry) => items.push(session_item(entry).await),
                None => items.push(item_from_summary(
                    summary,
                    Some(project.id.clone()),
                    false,
                    false,
                )),
            }
        }
        for entry in &open {
            if !stored.iter().any(|s| s.id == entry.session_id) {
                items.push(session_item(entry).await);
            }
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
        Ok(items)
    }

    /// Conversations whose title or message text contains `query`, in one
    /// project or in all of them. A title match is reported as such; otherwise
    /// the first matching message, as a snippet.
    pub async fn sessions_search(
        &self,
        project_id: Option<&str>,
        query: &str,
    ) -> IpcResult<Vec<SessionSearchHit>> {
        let needle = query.trim();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(id) = project_id {
            self.projects.get(id)?;
        }
        let stored = list_sessions(self.sessions_dir())
            .await
            .map_err(|e| io_error("conversations", e))?;
        let open: Vec<Arc<OpenSession>> = match project_id {
            Some(id) => self.entries_of_project(id),
            None => self.sessions.lock().values().cloned().collect(),
        };
        // Candidates: stored sessions (by folder), and open ones not stored yet.
        let mut candidates: Vec<(SessionItem, bool)> = Vec::new();
        for summary in &stored {
            let owner = summary
                .working_dir
                .as_deref()
                .and_then(|d| self.project_id_of_dir(d));
            if project_id.is_some_and(|p| owner.as_deref() != Some(p)) {
                continue;
            }
            let (open_now, running) = match open.iter().find(|e| e.session_id == summary.id) {
                Some(e) => (true, e.is_running()),
                None => (false, false),
            };
            candidates.push((item_from_summary(summary, owner, open_now, running), true));
        }
        for entry in &open {
            if !stored.iter().any(|s| s.id == entry.session_id) {
                candidates.push((session_item(entry).await, false));
            }
        }
        candidates.sort_by_key(|(item, _)| std::cmp::Reverse(item.updated_at));

        let store = JsonlMemory::new(self.sessions_dir());
        let mut hits = Vec::new();
        for (item, is_stored) in candidates {
            if hits.len() >= SEARCH_LIMIT {
                break;
            }
            if contains_folded(&item.title, needle) {
                hits.push(SessionSearchHit {
                    session: item,
                    matched: SearchMatch::Title,
                    snippet: None,
                });
                continue;
            }
            if !is_stored {
                continue;
            }
            let messages = store.load(&item.id).await.unwrap_or_default();
            if let Some(text) = messages.iter().find_map(|m| snippet(&message_text(m), needle)) {
                hits.push(SessionSearchHit {
                    session: item,
                    matched: SearchMatch::Message,
                    snippet: Some(text),
                });
            }
        }
        Ok(hits)
    }

    /// Forgets a project: its open conversations are closed first (refused
    /// with `SessionBusy` while one runs). Files and stored sessions are
    /// untouched.
    pub async fn forget_project(&self, project_id: &str) -> IpcResult<()> {
        let _gate = self.lifecycle.lock().await;
        let open = self.entries_of_project(project_id);
        if open.iter().any(|e| e.is_busy()) {
            return Err(error(
                IpcErrorCode::SessionBusy,
                "une conversation de ce projet est en cours : annulez-la ou attendez sa fin",
            ));
        }
        for entry in &open {
            self.retire(entry).await;
        }
        self.projects.remove(project_id)
    }

    /// Sends an engine command (JSON as in `docs/cli.md`) to an open session.
    /// `display` is the command as typed (e.g. `/skill debug`) of a submit: it
    /// labels the run that submit starts, and never reaches the engine. Other
    /// commands ignore it.
    pub async fn send_command(
        &self,
        session_id: &str,
        command: serde_json::Value,
        display: Option<String>,
    ) -> IpcResult<()> {
        check_session_id(session_id)?;
        let entry = self.require_open(session_id)?;
        let command: Command = serde_json::from_value(command)
            .map_err(|e| invalid(format!("commande invalide : {e}")))?;
        if matches!(command, Command::Resume { .. }) {
            return Err(error(
                IpcErrorCode::Rejected,
                "pour changer de conversation, ouvrez-la : une session reste liée à son contrôleur",
            ));
        }
        // A submit's attachment copies are checked here (an image needs a model
        // with image input) and kept with the run it starts.
        let copies = match &command {
            Command::Submit { prompt } | Command::Revise { prompt, .. } => {
                self.submit_copies(&entry, prompt)?
            }
            _ => Vec::new(),
        };
        let label = display.as_deref().and_then(normalize_label);
        entry
            .send(command, label, copies)
            .await
            .map_err(|reason| error(IpcErrorCode::Rejected, reason))
    }

    /// The image a tool call showed the model (`ViewImage`), as a data URL,
    /// read from the open session's history: the bytes the model received,
    /// never the file as it is now. `None` when the call showed none.
    pub async fn tool_image(&self, session_id: &str, tool_call_id: &str) -> IpcResult<Option<String>> {
        check_session_id(session_id)?;
        let entry = self.require_open(session_id)?;
        let history = entry.controller.history().await;
        Ok(crate::services::conversations::tool_image(&history, tool_call_id))
    }

    /// The engine's snapshot of an open session.
    pub fn snapshot(&self, session_id: &str) -> IpcResult<serde_json::Value> {
        check_session_id(session_id)?;
        let entry = self.require_open(session_id)?;
        serde_json::to_value(entry.controller.snapshot())
            .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))
    }

    /// Quitting: cancels what runs, waits (within `budget`) for the saves,
    /// closes every controller.
    pub async fn shutdown(&self, budget: Duration) {
        let entries: Vec<Arc<OpenSession>> = self.sessions.lock().values().cloned().collect();
        let deadline = tokio::time::Instant::now() + budget;
        for entry in &entries {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            settle(&entry.controller, left).await;
        }
        for entry in &entries {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let _ = tokio::time::timeout(left, entry.controller.close()).await;
            entry.stop_consumer();
        }
        self.sessions.lock().clear();
    }

    fn require_open(&self, session_id: &str) -> IpcResult<Arc<OpenSession>> {
        self.open_entry(session_id).ok_or_else(|| {
            error(
                IpcErrorCode::NotFound,
                "cette conversation n'est pas ouverte",
            )
        })
    }

    /// Takes the session out of the registry, cancels its work, waits for
    /// its save, closes it and stops its consumer.
    pub(super) async fn retire(&self, entry: &Arc<OpenSession>) {
        self.sessions.lock().remove(&entry.session_id);
        if !settle(&entry.controller, SETTLE).await {
            eprintln!(
                "bricks-desktop: session {} still busy after {}s; closing it",
                entry.session_id,
                SETTLE.as_secs()
            );
        }
        entry.controller.close().await;
        entry.stop_consumer();
        self.hub.publish(DesktopEvent::SessionState {
            session_id: entry.session_id.clone(),
            open: false,
            running: false,
        });
    }

    /// A stored session's item, from its metadata.
    fn item_from_meta(
        &self,
        meta: &SessionMeta,
        message_count: usize,
        open: bool,
        running: bool,
    ) -> SessionItem {
        SessionItem {
            id: meta.id.clone(),
            project_id: self.project_id_of_dir(&meta.working_dir),
            title: meta.title.clone(),
            created_at: meta.created_at,
            updated_at: meta.updated_at,
            message_count: u32::try_from(message_count).unwrap_or(u32::MAX),
            model: Some(meta.model.clone()),
            reasoning: meta.reasoning.clone(),
            working_dir: Some(path_text(&meta.working_dir)),
            open,
            running,
        }
    }
}

/// Cancels what the controller runs and waits until it is idle with no
/// memory maintenance, within `budget`. Cancellation is sent again while
/// waiting: a cancel that arrives just after a submission, before the run
/// has started, is not kept by the engine, and a later one reaches it.
/// `false` when the budget ran out first.
async fn settle(controller: &Controller, budget: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let snapshot = controller.snapshot();
        if matches!(snapshot.activity, Activity::Idle) && !snapshot.maintenance_running {
            return true;
        }
        let _ = controller.send(Command::Cancel);
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        let step = Duration::from_millis(200)
            .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        let _ = tokio::time::timeout(step, controller.wait_idle()).await;
    }
}

/// A session id becomes a file name: letters, digits, `-`, `_` and `.`, not
/// starting with a dot.
fn check_session_id(id: &str) -> IpcResult<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(invalid("identifiant de conversation invalide"))
    }
}

/// Engine refusals of an open, by what the person can do about them.
fn open_error(message: String) -> crate::contract::IpcError {
    let code = if message.contains("no stored session") {
        IpcErrorCode::NotFound
    } else if message.contains("no longer exists") {
        IpcErrorCode::FolderMissing
    } else {
        IpcErrorCode::Config
    };
    error(code, message)
}
