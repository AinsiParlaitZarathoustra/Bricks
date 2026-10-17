//! One open session: its controller, the task that consumes its events, and
//! what the bridge keeps to redisplay it (a bounded ring of envelopes, the
//! last sequence number, the running state, the change journal feed).

use super::hub::Hub;
use crate::contract::{
    ChangeFile, ChangeRecord, DesktopEvent, HistoryMessage, OpenedSession, RunLabel, SessionItem,
};
use crate::services::changes::{NewChange, StoredPatch, cap_patch, new_record_id};
use crate::services::conversations::{history_message, path_text};
use crate::services::now_ms;
use crate::services::run_labels::{self, RunLabelStore, user_index_of};
use cersei_agent::control::{
    Activity, Command, Controller, Envelope, Event, EventStream, SessionMeta, WrittenFile,
};
use cersei_memory::{JsonlMemory, Memory};
use cersei_tools::preview::ChangeKind;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;

/// Envelopes kept for redisplay: the current or last run, at most this many.
pub const RING_CAPACITY: usize = 4000;
/// Approval previews kept until their change is applied.
const PREVIEWS_KEPT: usize = 64;

pub struct OpenSession {
    /// The registry key. Fixed for the controller's life: a `resume` command
    /// is refused, so the session never changes under its key.
    pub session_id: String,
    pub project_id: String,
    pub controller: Controller,
    sessions_dir: PathBuf,
    created_at: i64,
    stop: watch::Sender<bool>,
    live: parking_lot::Mutex<Live>,
    /// The presentation labels of the session's runs (Sprint 14.4).
    labels: Arc<RunLabelStore>,
    /// Held while a submit is recorded and sent, so two submits never
    /// interleave their records (and the history is counted once per submit).
    submit_gate: tokio::sync::Mutex<()>,
    /// Completion of the last journal append of this session: the next one
    /// waits for it, so the appends keep the order of the edits.
    journal_tail: parking_lot::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

struct Live {
    ring: VecDeque<serde_json::Value>,
    last_seq: u64,
    running: bool,
    /// Messages in the stored session, refreshed when a run ends.
    message_count: usize,
    previews: VecDeque<Preview>,
    /// The accepted submits whose `run_started` has not come yet, oldest
    /// first (Sprint 14.4).
    expected: VecDeque<Expected>,
    /// The label of the run in progress, kept until its end: its message
    /// index is only known once the run has added its messages.
    running_label: Option<PendingLabel>,
}

/// A submit accepted by the controller, waiting for the run it starts. The
/// engine starts the runs in the order of their submits, one at a time, so
/// the next `run_started` belongs to the oldest record. The consumer can lag
/// behind the engine (its queue is bounded), so the match is by order of
/// submits, not by timing.
struct Expected {
    /// The label as typed (`None` for a submit typed as plain text).
    label: Option<String>,
    /// The user-role messages before the run, counted while it could not
    /// yet run (only for a labelled submit).
    base: Option<u32>,
}

/// A labelled run, from its start to its end.
struct PendingLabel {
    run_id: String,
    label: String,
    prompt: String,
    at: i64,
    base: u32,
}

/// The diffs an approval showed for one tool call, kept until it is applied.
struct Preview {
    agent_id: Option<String>,
    tool_call_id: String,
    /// Path and unified diff of each file.
    files: Vec<(String, String)>,
}

impl OpenSession {
    /// The open session and the receiver its consumer watches for the stop.
    pub fn new(
        controller: Controller,
        project_id: String,
        sessions_dir: PathBuf,
        created_at: i64,
        message_count: usize,
        labels: Arc<RunLabelStore>,
    ) -> (Arc<Self>, watch::Receiver<bool>) {
        let (stop, stop_rx) = watch::channel(false);
        let entry = Self {
            session_id: controller.session_id(),
            project_id,
            controller,
            sessions_dir,
            created_at,
            stop,
            live: parking_lot::Mutex::new(Live {
                ring: VecDeque::new(),
                last_seq: 0,
                running: false,
                message_count,
                previews: VecDeque::new(),
                expected: VecDeque::new(),
                running_label: None,
            }),
            labels,
            submit_gate: tokio::sync::Mutex::new(()),
            journal_tail: parking_lot::Mutex::new(None),
        };
        (Arc::new(entry), stop_rx)
    }

    /// Sends a command to the controller. A submit is recorded first, with
    /// its label (the one typed for it, if any), so the `run_started` it
    /// starts takes that label; a refused submit leaves nothing behind. Other
    /// commands ignore the label.
    pub async fn send(&self, command: Command, label: Option<String>) -> Result<(), String> {
        if !matches!(command, Command::Submit { .. }) {
            return self.controller.send(command);
        }
        let _gate = self.submit_gate.lock().await;
        // A submit is accepted only while idle, and the run appends its
        // messages after the ones there are now: count them while nothing
        // changes them.
        let base = match label {
            Some(_) if !self.is_busy() => {
                Some(run_labels::user_count(&self.controller.history().await))
            }
            _ => None,
        };
        self.live.lock().expected.push_back(Expected { label, base });
        let sent = self.controller.send(command);
        if sent.is_err() {
            // Refused: nothing started. The record just pushed is the newest.
            self.live.lock().expected.pop_back();
        }
        sent
    }

    /// A run or a sub-agent is active (the controller's own view).
    pub fn is_running(&self) -> bool {
        matches!(self.controller.activity(), Activity::Running { .. })
    }

    /// Anything is active: a run, a compaction, a resume.
    pub fn is_busy(&self) -> bool {
        self.controller.activity() != Activity::Idle
    }

    /// Asks the consumer task to finish after the events already queued.
    pub fn stop_consumer(&self) {
        let _ = self.stop.send(true);
    }

    /// The session as the first view receives it. `recent_events` and
    /// `last_seq` are read together, so the view can drop the channel events
    /// it already has.
    pub async fn opened(&self) -> OpenedSession {
        let history: Vec<HistoryMessage> = self
            .controller
            .history()
            .await
            .iter()
            .map(history_message)
            .collect();
        let (recent_events, last_seq) = {
            let live = self.live.lock();
            (live.ring.iter().cloned().collect(), live.last_seq)
        };
        let snapshot = serde_json::to_value(self.controller.snapshot()).unwrap_or(serde_json::Value::Null);
        let run_labels = self.load_run_labels().await;
        OpenedSession {
            session: session_item(self).await,
            snapshot,
            history,
            recent_events,
            last_seq,
            run_labels,
        }
    }

    /// The labels of the session's runs, oldest first. A file that cannot be
    /// read is reported and gives none: the conversation still opens.
    async fn load_run_labels(&self) -> Vec<RunLabel> {
        let labels = Arc::clone(&self.labels);
        let session_id = self.session_id.clone();
        match tauri::async_runtime::spawn_blocking(move || labels.list(&session_id)).await {
            Ok(Ok(list)) => list,
            Ok(Err(e)) => {
                eprintln!("bricks-desktop: run labels of {} : {}", self.session_id, e.message);
                Vec::new()
            }
            Err(e) => {
                eprintln!("bricks-desktop: run labels of {} : {e}", self.session_id);
                Vec::new()
            }
        }
    }

    /// The session's counted messages, as read from the store.
    pub(crate) fn set_message_count(&self, count: usize) {
        self.live.lock().message_count = count;
    }
}

/// The session as the interface lists it, with its stored metadata.
pub async fn session_item(entry: &OpenSession) -> SessionItem {
    let snap = entry.controller.snapshot();
    let meta = load_meta(&entry.sessions_dir, &entry.session_id).await;
    let (created_at, updated_at) = match meta {
        Some(m) => (m.created_at, m.updated_at),
        None => (entry.created_at, now_ms()),
    };
    let message_count = entry.live.lock().message_count;
    SessionItem {
        id: entry.session_id.clone(),
        project_id: Some(entry.project_id.clone()),
        title: snap.title,
        created_at,
        updated_at,
        message_count: u32::try_from(message_count).unwrap_or(u32::MAX),
        model: Some(snap.model),
        reasoning: snap.reasoning,
        working_dir: Some(path_text(&snap.working_dir)),
        open: true,
        running: matches!(snap.activity, Activity::Running { .. }),
    }
}

/// The metadata file of a stored session (`None` for an older session).
pub async fn load_meta(sessions_dir: &Path, session_id: &str) -> Option<SessionMeta> {
    let dir = files_dir(sessions_dir, session_id);
    tauri::async_runtime::spawn_blocking(move || SessionMeta::load(&dir).ok().flatten())
        .await
        .ok()
        .flatten()
}

/// The directory of a session's files, as the engine lays it out.
pub fn files_dir(sessions_dir: &Path, session_id: &str) -> PathBuf {
    JsonlMemory::new(sessions_dir)
        .session_files_dir(session_id)
        .unwrap_or_else(|| sessions_dir.join(format!("{session_id}.files")))
}

/// Messages in the stored session.
pub async fn stored_message_count(sessions_dir: &Path, session_id: &str) -> usize {
    let store = JsonlMemory::new(sessions_dir);
    let id = session_id.to_string();
    store.load(&id).await.map(|m| m.len()).unwrap_or(0)
}

/// Starts the task that consumes one controller's events. It stops when
/// `stop` is set (after delivering what was already queued) or when the
/// stream ends.
pub fn spawn_consumer(
    hub: Arc<Hub>,
    entry: Arc<OpenSession>,
    mut stream: EventStream,
    mut stop: watch::Receiver<bool>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = stop.changed() => break,
                next = stream.next() => match next {
                    Some(env) => ingest(&hub, &entry, env),
                    None => break,
                },
            }
        }
        while let Some(env) = stream.try_next() {
            ingest(&hub, &entry, env);
        }
    });
}

/// Handles one envelope: keeps it for redisplay, forwards it unchanged to
/// the windows, and reacts to what it announces (running state, title,
/// applied edits).
pub fn ingest(hub: &Arc<Hub>, entry: &Arc<OpenSession>, env: Envelope) {
    let Ok(value) = serde_json::to_value(&env) else {
        return;
    };
    let mut running_changed = None;
    let mut refresh = false;
    let mut count_messages = false;
    let mut change = None;
    let mut run_label = None;
    {
        let mut live = entry.live.lock();
        live.last_seq = env.seq;
        match &env.event {
            Event::RunStarted { prompt, .. } => {
                live.ring.clear();
                live.running = true;
                running_changed = Some(true);
                refresh = true;
                // The run of the oldest accepted submit still unstarted; a
                // typed command labels only the run its own submit started.
                live.running_label = None;
                if let Some(expected) = live.expected.pop_front()
                    && let (Some(label), Some(base), Some(run_id)) =
                        (expected.label, expected.base, env.run_id.clone())
                {
                    live.running_label = Some(PendingLabel {
                        run_id,
                        label,
                        prompt: prompt.clone(),
                        at: env.at,
                        base,
                    });
                }
            }
            Event::RunFinished { .. } => {
                live.running = false;
                running_changed = Some(false);
                refresh = true;
                count_messages = true;
                // The run's messages are all in the history now: its label
                // is placed there, off this task.
                run_label = live.running_label.take();
            }
            Event::ModelChanged { .. } | Event::ReasoningChanged { .. } => refresh = true,
            Event::ApprovalRequested { approval } => {
                if let Some(preview) = &approval.preview
                    && !preview.files.is_empty()
                {
                    live.remember_preview(Preview {
                        agent_id: approval.agent_id.clone(),
                        tool_call_id: approval.tool_call_id.clone(),
                        files: preview
                            .files
                            .iter()
                            .map(|f| (f.path.clone(), f.diff.clone()))
                            .collect(),
                    });
                }
            }
            Event::EditApplied {
                tool_call_id,
                tool,
                files,
                agent_id,
                changeset_id,
            } if !files.is_empty() => {
                // The `@` file index of the project is stale after a write.
                crate::commands::files::invalidate(&entry.project_id);
                change = Some(journal_entry(
                    &mut live,
                    entry,
                    &env,
                    tool,
                    tool_call_id,
                    agent_id.as_deref(),
                    changeset_id.as_deref(),
                    files,
                ));
            }
            _ => {}
        }
        live.ring.push_back(value.clone());
        while live.ring.len() > RING_CAPACITY {
            live.ring.pop_front();
        }
        hub.publish(DesktopEvent::Engine {
            project_id: Some(entry.project_id.clone()),
            envelope: value,
        });
    }
    if let Some(running) = running_changed {
        hub.publish(DesktopEvent::SessionState {
            session_id: entry.session_id.clone(),
            open: true,
            running,
        });
    }
    if refresh || count_messages {
        spawn_refresh(hub.clone(), entry.clone(), count_messages);
    }
    if let Some(new_change) = change {
        spawn_journal(hub.clone(), entry, new_change);
    }
    if let Some(pending) = run_label {
        spawn_label(hub.clone(), entry.clone(), pending);
    }
}

/// Places the label of a run that has ended, then stores it off the event
/// task and announces it when it is new. The message is the first one from
/// the run's base whose text is the prompt; a label whose message is not
/// there is not kept, rather than placed on a message it does not belong to.
fn spawn_label(hub: Arc<Hub>, entry: Arc<OpenSession>, pending: PendingLabel) {
    tauri::async_runtime::spawn(async move {
        let history = entry.controller.history().await;
        let Some(user_index) = user_index_of(&history, pending.base, &pending.prompt) else {
            eprintln!(
                "bricks-desktop: run labels : the message of run {} is not in the history; its label is not kept",
                pending.run_id
            );
            return;
        };
        let session_id = entry.session_id.clone();
        let label = RunLabel {
            run_id: pending.run_id,
            label: pending.label,
            prompt: pending.prompt,
            at: pending.at,
            user_index: Some(user_index),
        };
        let announced = DesktopEvent::RunLabeled {
            session_id: session_id.clone(),
            label: label.clone(),
        };
        let labels = Arc::clone(&entry.labels);
        let result = tauri::async_runtime::spawn_blocking(move || labels.append(&session_id, &label)).await;
        match result {
            Ok(Ok(true)) => hub.publish(announced),
            Ok(Ok(false)) => {}
            Ok(Err(e)) => eprintln!("bricks-desktop: run labels : {}", e.message),
            Err(e) => eprintln!("bricks-desktop: run labels : {e}"),
        }
    });
}

/// Builds the journal entry of an applied edit, with the patches an earlier
/// approval previewed for the same tool call.
#[allow(clippy::too_many_arguments)]
fn journal_entry(
    live: &mut Live,
    entry: &OpenSession,
    env: &Envelope,
    tool: &str,
    tool_call_id: &str,
    agent_id: Option<&str>,
    changeset_id: Option<&str>,
    files: &[WrittenFile],
) -> NewChange {
    let previewed = live
        .previews
        .iter()
        .position(|p| p.tool_call_id == tool_call_id && p.agent_id.as_deref() == agent_id)
        .and_then(|i| live.previews.remove(i));
    let patches: Vec<StoredPatch> = match &previewed {
        Some(p) => files
            .iter()
            .filter_map(|w| {
                let (_, diff) = p.files.iter().find(|(path, _)| *path == w.path)?;
                let (patch, truncated) = cap_patch(diff);
                Some(StoredPatch {
                    path: w.path.clone(),
                    patch,
                    binary: w.binary,
                    truncated,
                })
            })
            .collect(),
        None => Vec::new(),
    };
    let record = ChangeRecord {
        id: new_record_id(&entry.project_id),
        project_id: entry.project_id.clone(),
        session_id: env.session_id.clone(),
        run_id: env.run_id.clone(),
        agent_id: agent_id.map(String::from),
        changeset_id: changeset_id.map(String::from),
        tool: tool.to_string(),
        tool_call_id: tool_call_id.to_string(),
        at: env.at,
        files: files
            .iter()
            .map(|w| ChangeFile {
                path: w.path.clone(),
                kind: change_kind(w.kind).to_string(),
                added: u32::try_from(w.added).unwrap_or(u32::MAX),
                removed: u32::try_from(w.removed).unwrap_or(u32::MAX),
                binary: w.binary,
                has_patch: patches.iter().any(|p| p.path == w.path),
            })
            .collect(),
    };
    NewChange { record, patches }
}

fn change_kind(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Create => "created",
        ChangeKind::Modify => "modified",
        ChangeKind::Delete => "deleted",
    }
}

impl Live {
    fn remember_preview(&mut self, preview: Preview) {
        self.previews.push_back(preview);
        while self.previews.len() > PREVIEWS_KEPT {
            self.previews.pop_front();
        }
    }
}

/// Appends the entry off the event task; announces it when it is new. The
/// appends of one session run in the order of their edits: a replayed edit
/// is appended after the one it repeats, so the first one keeps its patch.
fn spawn_journal(hub: Arc<Hub>, entry: &OpenSession, change: NewChange) {
    let record = change.record.clone();
    let journal = Arc::clone(&hub.journal);
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let before = entry.journal_tail.lock().replace(done_rx);
    tauri::async_runtime::spawn(async move {
        if let Some(before) = before {
            // A dropped sender (a failed append) also ends the wait.
            let _ = before.await;
        }
        let result = tauri::async_runtime::spawn_blocking(move || journal.append(change)).await;
        let _ = done_tx.send(());
        match result {
            Ok(Ok(true)) => hub.record_added(record),
            Ok(Ok(false)) => {}
            Ok(Err(e)) => eprintln!("bricks-desktop: change journal: {}", e.message),
            Err(e) => eprintln!("bricks-desktop: change journal: {e}"),
        }
    });
}

/// Refreshes the session's item for the windows (and its message count after
/// a run).
fn spawn_refresh(hub: Arc<Hub>, entry: Arc<OpenSession>, count_messages: bool) {
    tauri::async_runtime::spawn(async move {
        if count_messages {
            let n = stored_message_count(&entry.sessions_dir, &entry.session_id).await;
            entry.set_message_count(n);
        }
        let session = session_item(&entry).await;
        hub.publish(DesktopEvent::SessionUpdated { session });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_change_kinds_use_the_contract_names() {
        assert_eq!(change_kind(ChangeKind::Create), "created");
        assert_eq!(change_kind(ChangeKind::Modify), "modified");
        assert_eq!(change_kind(ChangeKind::Delete), "deleted");
    }

    #[test]
    fn project_ids_match_the_record_convention() {
        // A record id is `<project id>_<random>`: the project part never
        // contains an underscore.
        assert!(!crate::services::projects::project_id("/x").contains('_'));
    }
}
