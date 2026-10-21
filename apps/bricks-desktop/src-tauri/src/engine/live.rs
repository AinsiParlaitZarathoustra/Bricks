//! One open session: its controller, the task that consumes its events, and
//! what the bridge keeps to redisplay it (a bounded ring of envelopes, the
//! last sequence number, the running state, the change journal feed).

use super::hub::Hub;
use crate::contract::{
    AttachmentRef, ChangeFile, ChangeRecord, DesktopEvent, HistoryMessage, OpenedSession,
    RunLabel, RunRecord, SessionItem,
};
use crate::services::attachment_runs::{self, RunAttachments};
use crate::services::changes::{NewChange, StoredPatch, new_record_id};
use crate::services::conversations::{history_message, path_text};
use crate::services::now_ms;
use crate::services::run_labels::{self, RunLabelStore, user_index_of};
use crate::services::run_records::{Recorder, RunRecordStore};
use bricks_agent::control::{
    Activity, Command, Controller, Envelope, Event, EventStream, SessionMeta, WrittenFile,
};
use bricks_memory::{JsonlMemory, Memory};
use bricks_tools::preview::ChangeKind;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;

/// Envelopes kept for redisplay of a finished run, at most this many.
pub const RING_CAPACITY: usize = 4000;
/// Envelopes kept while a run is running: all of it is needed to rebuild the
/// run on screen, so only a run this long loses its oldest envelopes (its
/// start is kept whatever happens).
pub const RUNNING_RING_CAPACITY: usize = 50_000;

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
    /// The presentation labels of the session's runs.
    labels: Arc<RunLabelStore>,
    /// What the desktop keeps of each finished run.
    run_records: Arc<RunRecordStore>,
    /// Completion of the last run record append: the next one waits for it,
    /// so the versions of a run are written in the order they were made.
    records_tail: parking_lot::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Held while a submit is recorded and sent, so two submits never
    /// interleave their records (and the history is counted once per submit).
    submit_gate: tokio::sync::Mutex<()>,
    /// Completion of the last journal append of this session: the next one
    /// waits for it, so the appends keep the order of the edits.
    journal_tail: parking_lot::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

struct Live {
    ring: VecDeque<serde_json::Value>,
    /// The `run_started` envelope the ring lost (only ever the current run's).
    ring_head: Option<serde_json::Value>,
    last_seq: u64,
    running: bool,
    /// Messages in the stored session, refreshed when a run ends.
    message_count: usize,
    /// The accepted submits whose `run_started` has not come yet, oldest
    /// first.
    expected: VecDeque<Expected>,
    /// The label of the run in progress, kept until its end: its message
    /// index is only known once the run has added its messages.
    running_label: Option<PendingLabel>,
    /// Builds the records of the runs (answer, commands, files).
    recorder: Recorder,
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
    /// yet run (`None` when the submit was not made while idle).
    base: Option<u32>,
    /// The copies of attachments the submit carries: kept with the run it starts.
    attachments: Vec<AttachmentRef>,
}

/// A labelled run, from its start to its end.
struct PendingLabel {
    run_id: String,
    label: String,
    prompt: String,
    at: i64,
    base: u32,
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
        run_records: Arc<RunRecordStore>,
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
                ring_head: None,
                last_seq: 0,
                running: false,
                message_count,
                expected: VecDeque::new(),
                running_label: None,
                recorder: Recorder::default(),
            }),
            labels,
            run_records,
            records_tail: parking_lot::Mutex::new(None),
            submit_gate: tokio::sync::Mutex::new(()),
            journal_tail: parking_lot::Mutex::new(None),
        };
        (Arc::new(entry), stop_rx)
    }

    /// Sends a command to the controller. A submit is recorded first, with
    /// its label (the one typed for it, if any), so the `run_started` it
    /// starts takes that label; a refused submit leaves nothing behind. Other
    /// commands ignore the label.
    pub async fn send(
        &self,
        command: Command,
        label: Option<String>,
        attachments: Vec<AttachmentRef>,
    ) -> Result<(), String> {
        if !matches!(command, Command::Submit { .. }) {
            return self.controller.send(command);
        }
        let _gate = self.submit_gate.lock().await;
        // A submit is accepted only while idle, and the run appends its
        // messages after the ones there are now: count them while nothing
        // changes them.
        let base = if self.is_busy() {
            None
        } else {
            Some(run_labels::user_count(&self.controller.history().await))
        };
        self.live
            .lock()
            .expected
            .push_back(Expected { label, base, attachments });
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
        let messages = self.controller.history().await;
        let mut history: Vec<HistoryMessage> = messages.iter().map(history_message).collect();
        // The copies each user message was sent with, to show them again.
        let files = files_dir(&self.sessions_dir, &self.session_id);
        let runs = {
            let files = files.clone();
            tauri::async_runtime::spawn_blocking(move || attachment_runs::load(&files))
                .await
                .unwrap_or_default()
        };
        attachment_runs::link_history(&messages, &mut history, &runs, &files);
        let (recent_events, last_seq) = {
            let live = self.live.lock();
            let mut recent: Vec<serde_json::Value> = live.ring_head.iter().cloned().collect();
            recent.extend(live.ring.iter().cloned());
            (recent, live.last_seq)
        };
        let snapshot = serde_json::to_value(self.controller.snapshot()).unwrap_or(serde_json::Value::Null);
        let run_labels = self.load_run_labels().await;
        let run_records = self.load_run_records().await;
        OpenedSession {
            session: session_item(self).await,
            snapshot,
            history,
            recent_events,
            last_seq,
            run_labels,
            run_records,
        }
    }

    /// The records of the session's finished runs, oldest first. A file that
    /// cannot be read is reported and gives none: the conversation still opens.
    async fn load_run_records(&self) -> Vec<RunRecord> {
        let records = Arc::clone(&self.run_records);
        let session_id = self.session_id.clone();
        match tauri::async_runtime::spawn_blocking(move || records.list(&session_id)).await {
            Ok(Ok(list)) => list,
            Ok(Err(e)) => {
                eprintln!(
                    "bricks-desktop: run records of {} : {}",
                    self.session_id, e.message
                );
                Vec::new()
            }
            Err(e) => {
                eprintln!("bricks-desktop: run records of {} : {e}", self.session_id);
                Vec::new()
            }
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
    let mut attachment_record = None;
    let mut record = None;
    {
        let mut live = entry.live.lock();
        live.last_seq = env.seq;
        match &env.event {
            Event::RunStarted { prompt, .. } => {
                live.ring.clear();
                live.ring_head = None;
                live.running = true;
                running_changed = Some(true);
                refresh = true;
                // The run of the oldest accepted submit still unstarted; a
                // typed command labels only the run its own submit started.
                live.running_label = None;
                let expected = live.expected.pop_front();
                let base = expected.as_ref().and_then(|e| e.base);
                // The copies the submit carried are kept with the run it started.
                let copies = expected
                    .as_ref()
                    .map(|e| e.attachments.clone())
                    .unwrap_or_default();
                if let (Some(base), Some(run_id), false) =
                    (base, env.run_id.clone(), copies.is_empty())
                {
                    attachment_record = Some(RunAttachments {
                        run_id,
                        base,
                        prompt: prompt.clone(),
                        refs: copies,
                    });
                }
                if let Some(run_id) = env.run_id.as_deref() {
                    live.recorder.start(run_id, base, prompt, env.at);
                }
                if let Some(expected) = expected
                    && let (Some(label), Some(base), Some(run_id)) =
                        (expected.label, base, env.run_id.clone())
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
        // A finished run's record is written again when a late event (a
        // sub-agent or a job ending after the run) changes it.
        if let Some(run_id) = live.recorder.observe(&env)
            && let Some(run) = live.recorder.run(&run_id)
        {
            record = Some((run.to_record(None), run.base(), run.prompt().to_string()));
        }
        push_ring(&mut live, value.clone());
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
    if let Some((record, base, prompt)) = record {
        spawn_record(entry, record, base, prompt);
    }
    if let Some(pending) = run_label {
        spawn_label(hub.clone(), entry.clone(), pending);
    }
    if let Some(attached) = attachment_record {
        spawn_attachment_record(entry, attached);
    }
}

/// Appends the copies a run started with to the session's record, off the
/// event task. A session deleted meanwhile keeps no record: nothing is created.
fn spawn_attachment_record(entry: &OpenSession, record: RunAttachments) {
    let files = files_dir(&entry.sessions_dir, &entry.session_id);
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = attachment_runs::append(&files, &record) {
            eprintln!("bricks-desktop: attachments of a message : {}", e.message);
        }
    });
}

/// Keeps an envelope for redisplay. A finished run keeps `RING_CAPACITY`
/// envelopes, a running one `RUNNING_RING_CAPACITY`. The run's own start is
/// never lost: when it leaves the ring it is kept apart as the head.
fn push_ring(live: &mut Live, value: serde_json::Value) {
    live.ring.push_back(value);
    let cap = if live.running {
        RUNNING_RING_CAPACITY
    } else {
        RING_CAPACITY
    };
    while live.ring.len() > cap {
        let Some(front) = live.ring.pop_front() else {
            break;
        };
        if front.get("type").and_then(serde_json::Value::as_str) == Some("run_started") {
            live.ring_head = Some(front);
        }
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

/// Builds the journal entry of an applied edit. Its patches are the ones the
/// engine wrote (`WrittenFile::patch`): a file without one (binary) has none.
fn journal_entry(
    entry: &OpenSession,
    env: &Envelope,
    tool: &str,
    tool_call_id: &str,
    agent_id: Option<&str>,
    changeset_id: Option<&str>,
    files: &[WrittenFile],
) -> NewChange {
    let patches: Vec<StoredPatch> = files
        .iter()
        .filter_map(|w| {
            let patch = w.patch.clone()?;
            Some(StoredPatch {
                path: w.path.clone(),
                patch,
                binary: w.binary,
                truncated: w.patch_truncated,
            })
        })
        .collect();
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
                absolute: w.absolute.clone(),
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

/// Writes a version of a run's record off the event task, once the history
/// tells which user message the run added (`user_index`). Versions are
/// appended in the order they were made.
fn spawn_record(
    entry: &Arc<OpenSession>,
    mut record: RunRecord,
    base: Option<u32>,
    prompt: String,
) {
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let before = entry.records_tail.lock().replace(done_rx);
    let entry = Arc::clone(entry);
    tauri::async_runtime::spawn(async move {
        if let Some(before) = before {
            // A dropped sender (a failed append) also ends the wait.
            let _ = before.await;
        }
        if let Some(base) = base {
            let history = entry.controller.history().await;
            record.user_index = user_index_of(&history, base, &prompt);
        }
        let store = Arc::clone(&entry.run_records);
        let session_id = entry.session_id.clone();
        let result =
            tauri::async_runtime::spawn_blocking(move || store.append(&session_id, &record)).await;
        let _ = done_tx.send(());
        match result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => eprintln!("bricks-desktop: run records : {}", e.message),
            Err(e) => eprintln!("bricks-desktop: run records : {e}"),
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

    fn live_with(running: bool) -> Live {
        Live {
            ring: VecDeque::new(),
            ring_head: None,
            last_seq: 0,
            running,
            message_count: 0,
            expected: VecDeque::new(),
            running_label: None,
            recorder: Recorder::default(),
        }
    }

    fn event(kind: &str) -> serde_json::Value {
        serde_json::json!({ "type": kind })
    }

    #[test]
    fn a_running_run_keeps_its_start_when_its_oldest_envelopes_leave_the_ring() {
        let mut live = live_with(true);
        push_ring(&mut live, event("run_started"));
        for _ in 0..RUNNING_RING_CAPACITY {
            push_ring(&mut live, event("text_delta"));
        }
        assert_eq!(live.ring.len(), RUNNING_RING_CAPACITY);
        assert_eq!(live.ring_head, Some(event("run_started")), "the start is kept apart");
        assert_eq!(live.ring.front(), Some(&event("text_delta")));
    }

    #[test]
    fn a_finished_run_keeps_the_shorter_tail_and_no_start_when_it_is_not_there() {
        let mut live = live_with(false);
        for _ in 0..(RING_CAPACITY + 5) {
            push_ring(&mut live, event("text_delta"));
        }
        assert_eq!(live.ring.len(), RING_CAPACITY);
        assert!(live.ring_head.is_none(), "only a run start is kept apart");
    }

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
