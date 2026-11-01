//! The session's agent runtime: admission (depth, active slots, bounded
//! queue), the instances and their results, and the usage ledger of each
//! root run.
//!
//! Definitions:
//! * depth: the session's agent is 0, its children 1, their children 2 (a
//!   protection against runaway recursion, not a count of tasks);
//! * no cumulative cap: any number of tasks may run one after the other;
//! * `max_concurrent`: active descendants, every entry included (launch,
//!   batch, resume, at any depth). A slot is held while a child
//!   generates or runs tools; it is **released while the child waits for
//!   its own children** (and taken back before it continues), so a chain
//!   root → child → grandchild works with `max_concurrent = 1`. The
//!   session's own agent holds no slot.
//! * a full queue, an admission timeout or a hard limit is an explicit
//!   error; the tools stay in the catalogue.

use super::admission::{WriterGuard, writers_for};
use super::spawn::{AgentResult, InstanceState, SpawnInfo, SubAgentEvent};
use crate::events::AgentEvent;
use bricks_types::Usage;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

// ─── Limits ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_concurrent: usize,
    pub max_depth: u32,
    pub max_queued: usize,
    pub admission_timeout: Duration,
    /// Headless: how long to wait for background children after the answer.
    pub background_drain: Duration,
}

/// Why admission refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    DepthExceeded { depth: u32, max: u32 },
    QueueFull { max: usize },
    Timeout { ms: u64 },
    Cancelled,
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DepthExceeded { depth, max } => write!(
                f,
                "AgentDepthExceeded: a sub-agent at depth {depth} is beyond [agents] max_depth = {max}"
            ),
            Self::QueueFull { max } => write!(
                f,
                "AgentQueueFull: {max} sub-agent(s) already wait for a slot ([agents] max_queued)"
            ),
            Self::Timeout { ms } => write!(
                f,
                "AgentAdmissionTimeout: no slot within {ms} ms ([agents] admission_timeout_ms)"
            ),
            Self::Cancelled => write!(f, "cancelled while waiting for a slot"),
        }
    }
}

// ─── Scheduler ───────────────────────────────────────────────────────────────

pub struct Scheduler {
    pub limits: Limits,
    slots: Arc<Semaphore>,
    queued: AtomicUsize,
    peak_active: AtomicUsize,
}

/// Admission figures, for metrics and inspectors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerStats {
    pub active: usize,
    pub queued: usize,
    pub peak_active: usize,
    pub max_concurrent: usize,
}

impl Scheduler {
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(limits.max_concurrent.max(1))),
            limits,
            queued: AtomicUsize::new(0),
            peak_active: AtomicUsize::new(0),
        })
    }

    pub fn stats(&self) -> SchedulerStats {
        let max = self.limits.max_concurrent.max(1);
        SchedulerStats {
            active: max - self.slots.available_permits(),
            queued: self.queued.load(Ordering::SeqCst),
            peak_active: self.peak_active.load(Ordering::SeqCst),
            max_concurrent: max,
        }
    }

    /// Check the depth of `n` new children of an agent at `parent_depth`
    /// (a batch fits whole or is refused whole). There is no cumulative cap:
    /// the slots and the bounded queue pace them.
    pub fn reserve(&self, _root: &str, parent_depth: u32, _n: u32) -> Result<(), AdmissionError> {
        let depth = parent_depth + 1;
        if depth > self.limits.max_depth {
            return Err(AdmissionError::DepthExceeded {
                depth,
                max: self.limits.max_depth,
            });
        }
        Ok(())
    }

    /// An activity slot: at once if free, else in the bounded queue (fair,
    /// first come first served), cancellable, with a deadline.
    pub async fn acquire(
        &self,
        cancel: &CancellationToken,
        on_queued: impl FnOnce(),
    ) -> Result<OwnedSemaphorePermit, AdmissionError> {
        if let Ok(p) = Arc::clone(&self.slots).try_acquire_owned() {
            self.note_active();
            return Ok(p);
        }
        let q = self.queued.fetch_add(1, Ordering::SeqCst);
        if q >= self.limits.max_queued {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            return Err(AdmissionError::QueueFull {
                max: self.limits.max_queued,
            });
        }
        on_queued();
        let r = tokio::select! {
            p = Arc::clone(&self.slots).acquire_owned() => p.map_err(|_| AdmissionError::Cancelled),
            _ = cancel.cancelled() => Err(AdmissionError::Cancelled),
            _ = tokio::time::sleep(self.limits.admission_timeout) => Err(AdmissionError::Timeout {
                ms: self.limits.admission_timeout.as_millis() as u64,
            }),
        };
        self.queued.fetch_sub(1, Ordering::SeqCst);
        if r.is_ok() {
            self.note_active();
        }
        r
    }

    /// A slot again after a wait (no queue limit: the agent already held
    /// one; cancellable).
    pub async fn reacquire(&self, cancel: &CancellationToken) -> Option<OwnedSemaphorePermit> {
        let p = tokio::select! {
            p = Arc::clone(&self.slots).acquire_owned() => p.ok(),
            _ = cancel.cancelled() => None,
        };
        if p.is_some() {
            self.note_active();
        }
        p
    }

    fn note_active(&self) {
        let active = self.limits.max_concurrent.max(1) - self.slots.available_permits();
        self.peak_active.fetch_max(active, Ordering::SeqCst);
    }
}

// ─── Activity lease ──────────────────────────────────────────────────────────

/// How long a writing call of a shared-workspace child waits for another
/// writer to finish before it is refused (the child goes on, reading).
pub const WRITER_CLAIM_TIMEOUT: Duration = Duration::from_secs(60);

/// What an active child holds: its slot and, in a shared workspace, the
/// writer lease from its first writing call on (a child that only reads
/// never takes it). Both are let go while it waits for its own children.
pub struct ActivityLease {
    scheduler: Arc<Scheduler>,
    holder: String,
    writer_dir: Option<PathBuf>,
    slot: tokio::sync::Mutex<Option<OwnedSemaphorePermit>>,
    writer: tokio::sync::Mutex<Option<WriterGuard>>,
    /// The writer lease was claimed: it is taken back after a wait.
    claimed: std::sync::atomic::AtomicBool,
    /// Number of waits in progress. Its lock also orders the transitions:
    /// the first wait lets the slot and lease go, the last one takes them
    /// back before another wait can start, so two waits of one child never
    /// both take a slot back (with one slot, the second would wait forever
    /// for the one its own lease holds).
    waits: tokio::sync::Mutex<usize>,
    cancel: CancellationToken,
}

impl ActivityLease {
    pub fn new(
        scheduler: Arc<Scheduler>,
        holder: String,
        slot: OwnedSemaphorePermit,
        writer: Option<(PathBuf, WriterGuard)>,
        cancel: CancellationToken,
    ) -> Arc<Self> {
        let (writer_dir, guard) = match writer {
            Some((d, g)) => (Some(d), Some(g)),
            None => (None, None),
        };
        let claimed = std::sync::atomic::AtomicBool::new(guard.is_some());
        Arc::new(Self {
            scheduler,
            holder,
            writer_dir,
            slot: tokio::sync::Mutex::new(Some(slot)),
            writer: tokio::sync::Mutex::new(guard),
            claimed,
            waits: tokio::sync::Mutex::new(0),
            cancel,
        })
    }

    /// A child of the shared workspace `dir` that takes its writer lease at
    /// its first writing call ([`Self::claim_writer`]), not before.
    pub fn shared(
        scheduler: Arc<Scheduler>,
        holder: String,
        slot: OwnedSemaphorePermit,
        dir: PathBuf,
        cancel: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            scheduler,
            holder,
            writer_dir: Some(dir),
            slot: tokio::sync::Mutex::new(Some(slot)),
            writer: tokio::sync::Mutex::new(None),
            claimed: std::sync::atomic::AtomicBool::new(false),
            waits: tokio::sync::Mutex::new(0),
            cancel,
        })
    }

    /// The workspace this lease writes in, when it is a shared one.
    pub fn writer_dir(&self) -> Option<&Path> {
        self.writer_dir.as_deref()
    }

    /// Take the shared workspace's writer lease for the rest of the run, at
    /// the first writing call (once held, at once). `Err` with the holder
    /// when another writer keeps it past `timeout`; `Ok(false)` if cancelled.
    pub async fn claim_writer(&self, timeout: Duration) -> Result<bool, String> {
        let Some(dir) = &self.writer_dir else {
            return Ok(true);
        };
        let mut writer = self.writer.lock().await;
        if writer.is_some() {
            return Ok(true);
        }
        let gate = writers_for(dir);
        match tokio::time::timeout(timeout, gate.acquire(&self.holder, &self.cancel, |_| {})).await
        {
            Ok(Some(g)) => {
                *writer = Some(g);
                self.claimed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(_) => Err(gate.holder().unwrap_or_else(|| "another agent".into())),
        }
    }

    /// Run `fut` (waiting for children) without holding the slot or the
    /// writer lease; take them back afterwards (writer first, then slot).
    pub async fn waiting<F: std::future::Future>(&self, fut: F) -> F::Output {
        {
            let mut waits = self.waits.lock().await;
            *waits += 1;
            if *waits == 1 {
                self.slot.lock().await.take();
                self.writer.lock().await.take();
            }
        }
        let out = fut.await;
        let mut waits = self.waits.lock().await;
        *waits -= 1;
        if *waits == 0 {
            if let Some(dir) = &self.writer_dir
                && self.claimed.load(std::sync::atomic::Ordering::SeqCst)
            {
                let g = writers_for(dir)
                    .acquire(&self.holder, &self.cancel, |_| {})
                    .await;
                *self.writer.lock().await = g;
            }
            let p = self.scheduler.reacquire(&self.cancel).await;
            *self.slot.lock().await = p;
        }
        out
    }

    /// Let everything go (end of the child).
    pub async fn release(&self) {
        self.slot.lock().await.take();
        self.writer.lock().await.take();
    }
}

/// The lease of the agent whose tool context this is (children only).
#[derive(Clone)]
pub struct LeaseHandle(pub Arc<ActivityLease>);

/// Wait for `fut` with the caller's lease released, if it has one.
pub async fn while_waiting<F: std::future::Future>(
    ext: &bricks_tools::Extensions,
    fut: F,
) -> F::Output {
    match ext.get::<LeaseHandle>() {
        Some(l) => l.0.waiting(fut).await,
        None => fut.await,
    }
}

// ─── Instances ───────────────────────────────────────────────────────────────

/// One instance as the registry keeps it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceRecord {
    pub info: SpawnInfo,
    pub state: InstanceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AgentResult>,
    pub background: bool,
    /// Durable metadata: milliseconds since the Unix epoch.
    pub updated_at_ms: u64,
    /// The current execution of this identity: 1 at launch, one more for
    /// each resume. Events, results and waits belong to one execution.
    #[serde(default = "first_execution")]
    pub execution: u32,
    /// What the launch actually used, to resume it with the same profile,
    /// model, tools and workspace (never a secret). Absent for instances
    /// launched before it existed: those cannot be resumed faithfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<super::spawn::LaunchRecord>,
    /// Stopped by an explicit cancel: a later message is refused, a resume
    /// must be asked for explicitly.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
}

fn first_execution() -> u32 {
    1
}

struct Live {
    cancel: CancellationToken,
    done: tokio::sync::watch::Receiver<bool>,
    done_tx: tokio::sync::watch::Sender<bool>,
    handle: Option<tokio::task::JoinHandle<()>>,
    /// The running execution's message queue, while it runs.
    controls: Option<Arc<crate::events::StreamControls>>,
}

/// Why a resume was refused before anything started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeRefusal {
    Unknown,
    OutOfScope,
    /// Still running (or queued): send it a message instead.
    NotEnded(InstanceState),
    /// Another resume of the same identity already started.
    Busy,
}

/// Events of the runtime that are not tied to a run (background children
/// finishing after their parent answered, final usage totals).
pub type SessionSink = Arc<dyn Fn(AgentEvent) + Send + Sync>;

/// Instances, results, the usage of each root run, and the durable
/// manifest of the session.
pub struct AgentRuntime {
    pub scheduler: Arc<Scheduler>,
    records: Mutex<Vec<InstanceRecord>>,
    live: Mutex<HashMap<String, Live>>,
    usage: Mutex<HashMap<String, RunUsage>>,
    /// Cancelled when the session closes.
    pub session_token: CancellationToken,
    session_sink: Mutex<Option<SessionSink>>,
    manifest: Option<PathBuf>,
    pub workspaces: Arc<super::workspace::WorkspaceManager>,
    /// Messagent: the team thread and the orchestrator's queue.
    pub(crate) mail: Mutex<super::messagent::Mail>,
    /// Grows at each delivery or end: waits wake up and look again.
    wake: tokio::sync::watch::Sender<u64>,
    /// The number of the next sub-agent (`sub_0000`…), never reused.
    next_sub: Mutex<u32>,
}

/// Sub-agents of one session, at most (`sub_0000` to `sub_9999`).
pub const MAX_SUB_AGENTS: u32 = 10_000;

/// Why a wait for events ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    /// Deliveries wait for the waiter (results, messages): read next turn.
    Delivered(usize),
    /// One of the awaited sub-agents ended.
    Ended(String),
    TimedOut,
    Cancelled,
}

/// Usage of one root run: the session agent's own, and its descendants'
/// own usage, each response counted once.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunUsage {
    pub own: Usage,
    pub descendants: Usage,
    /// The session agent's answer is done.
    pub root_finished: bool,
}

impl RunUsage {
    pub fn total(&self) -> Usage {
        let mut t = self.own.clone();
        t.merge(&self.descendants);
        t
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl AgentRuntime {
    pub fn new(
        limits: Limits,
        workspaces: Arc<super::workspace::WorkspaceManager>,
        manifest: Option<PathBuf>,
    ) -> Arc<Self> {
        let rt = Arc::new(Self {
            scheduler: Scheduler::new(limits),
            records: Mutex::new(Vec::new()),
            live: Mutex::new(HashMap::new()),
            usage: Mutex::new(HashMap::new()),
            session_token: CancellationToken::new(),
            session_sink: Mutex::new(None),
            manifest,
            workspaces,
            mail: Mutex::new(Default::default()),
            wake: tokio::sync::watch::channel(0).0,
            next_sub: Mutex::new(0),
        });
        rt.recover();
        rt
    }

    /// Where events not tied to a run go (set by the controller).
    pub fn set_session_sink(&self, sink: SessionSink) {
        *self.session_sink.lock() = Some(sink);
    }

    pub fn has_session_sink(&self) -> bool {
        self.session_sink.lock().is_some()
    }

    /// A ChangeSet was applied to the session's workspace: say so, and
    /// report its lines as one applied change (its id identifies it, so a
    /// repeat never counts twice). Called only after a successful apply.
    pub fn changeset_applied(&self, done: &super::workspace::ChangeSet) {
        use super::spawn::SubAgentEvent;
        use bricks_tools::preview::ChangeKind;
        self.emit(AgentEvent::SubAgent(SubAgentEvent::ChangesUpdated {
            changeset_id: done.id.clone(),
            state: super::workspace::ChangeSetState::Applied,
            files: done.files.iter().map(|f| f.path.clone()).collect(),
            detail: None,
        }));
        self.emit(AgentEvent::SubAgent(SubAgentEvent::EditApplied {
            agent_id: done.agent_id.clone(),
            tool_call_id: done.id.clone(),
            tool: "apply_changes".into(),
            changeset_id: Some(done.id.clone()),
            files: done
                .files
                .iter()
                .map(|f| {
                    // The part of the ChangeSet's patch that concerns the file
                    // (none for a binary file: its patch has no text).
                    let patch = (!f.binary)
                        .then(|| patch_section(&done.patch, &f.path))
                        .flatten();
                    let (patch, patch_truncated) = match patch {
                        Some(p) => {
                            let (p, cut) =
                                crate::control::cap_text(&p, crate::control::MAX_PATCH_BYTES);
                            (Some(p), cut)
                        }
                        None => (None, false),
                    };
                    crate::control::WrittenFile {
                        path: f.path.clone(),
                        kind: match f.status.chars().next() {
                            Some('A') => ChangeKind::Create,
                            Some('D') => ChangeKind::Delete,
                            _ => ChangeKind::Modify,
                        },
                        added: f.added.unwrap_or(0) as usize,
                        removed: f.removed.unwrap_or(0) as usize,
                        binary: f.binary,
                        absolute: None,
                        patch,
                        patch_truncated,
                    }
                })
                .collect(),
        }));
    }

    pub fn emit(&self, ev: AgentEvent) {
        if let Some(s) = self.session_sink.lock().clone() {
            s(ev);
        }
    }

    /// A previous process's instances that never ended are marked
    /// interrupted; nothing is restarted, no old pid is touched. A record
    /// that cannot be read is reported and left out, and the original file is
    /// kept as a `.bak` before the next save replaces it.
    fn recover(&self) {
        let Some(path) = &self.manifest else { return };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let mut records = Vec::new();
        let mut unreadable = false;
        match serde_json::from_str::<Vec<serde_json::Value>>(&text) {
            Ok(items) => {
                for item in items {
                    match serde_json::from_value::<InstanceRecord>(item) {
                        Ok(r) => records.push(r),
                        Err(e) => {
                            unreadable = true;
                            tracing::warn!(error = %e, "an agent record cannot be read and is left out");
                        }
                    }
                }
            }
            Err(e) => {
                unreadable = true;
                tracing::warn!(error = %e, "the agent manifest cannot be read");
            }
        }
        if unreadable {
            let backup = path.with_extension(format!("json.{}.bak", now_ms()));
            match std::fs::copy(path, &backup) {
                Ok(_) => {
                    tracing::warn!(backup = %backup.display(), "the original manifest is kept there")
                }
                Err(e) => tracing::warn!(error = %e, "the original manifest could not be kept"),
            }
        }
        for r in records.iter_mut() {
            if !r.state.is_terminal() {
                r.state = InstanceState::Interrupted;
                r.reason = Some("the process that ran it ended before it finished".into());
                r.updated_at_ms = now_ms();
            }
        }
        // Every id the manifest names counts, a record that cannot be read
        // included: an id is never given twice.
        let next = text
            .match_indices("sub_")
            .filter_map(|(i, _)| {
                let digits = text.get(i + 4..i + 8)?;
                digits
                    .bytes()
                    .all(|b| b.is_ascii_digit())
                    .then(|| digits.parse::<u32>().ok())
                    .flatten()
            })
            .chain(
                records
                    .iter()
                    .filter_map(|r| r.info.agent_id.strip_prefix("sub_")?.parse::<u32>().ok()),
            )
            .max()
            .map_or(0, |n| n + 1);
        *self.next_sub.lock() = next;
        *self.records.lock() = records;
        self.save();
    }

    /// The next sub-agent id, `sub_0000` to `sub_9999`, never reused in
    /// the session.
    pub fn allocate_id(&self) -> Result<String, String> {
        let mut next = self.next_sub.lock();
        if *next >= MAX_SUB_AGENTS {
            return Err(format!(
                "this session already started {MAX_SUB_AGENTS} sub-agents (sub_0000 to sub_9999): \
                 no id is left; resume an ended sub-agent instead"
            ));
        }
        let id = format!("sub_{:04}", *next);
        *next += 1;
        Ok(id)
    }

    /// Something was delivered or ended: waits look again.
    pub(crate) fn bump(&self) {
        self.wake.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// The message queue of a running sub-agent.
    pub(crate) fn controls_of(&self, id: &str) -> Option<Arc<crate::events::StreamControls>> {
        self.live.lock().get(id).and_then(|l| l.controls.clone())
    }

    /// Wait until something is delivered to `waiter`, one of `ids` ends,
    /// the wait is cancelled, or `timeout` passes. Deliveries already
    /// waiting end it at once; nothing is consumed (they are read next turn).
    pub async fn wait_event(
        &self,
        waiter: &str,
        ids: &[String],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> WaitOutcome {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut rx = self.wake.subscribe();
        loop {
            let waiting = self.waiting_for(waiter);
            if waiting > 0 {
                return WaitOutcome::Delivered(waiting);
            }
            if let Some(id) = ids.iter().find(|id| {
                self.record(id)
                    .is_some_and(|r| r.state.is_terminal() && r.result.is_some())
            }) {
                return WaitOutcome::Ended(id.clone());
            }
            tokio::select! {
                changed = rx.changed() => if changed.is_err() { return WaitOutcome::Cancelled },
                _ = tokio::time::sleep_until(deadline) => return WaitOutcome::TimedOut,
                _ = cancel.cancelled() => return WaitOutcome::Cancelled,
            }
        }
    }

    fn save(&self) {
        let Some(path) = &self.manifest else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let records = self.records.lock().clone();
        if let Ok(body) = serde_json::to_vec_pretty(&records) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    pub fn register(&self, info: SpawnInfo, background: bool, cancel: CancellationToken) {
        let (done_tx, done) = tokio::sync::watch::channel(false);
        self.live.lock().insert(
            info.agent_id.clone(),
            Live {
                cancel,
                done,
                done_tx,
                handle: None,
                controls: None,
            },
        );
        self.records.lock().push(InstanceRecord {
            info,
            state: InstanceState::Created,
            reason: None,
            result: None,
            background,
            updated_at_ms: now_ms(),
            execution: 1,
            launch: None,
            stopped: false,
        });
        self.save();
    }

    /// The identity's details once known (its worktree, branch, skills)
    /// and what its execution uses: `status` and the manifest stay true.
    pub fn update_launch(
        &self,
        info: &SpawnInfo,
        launch: Option<super::spawn::LaunchRecord>,
    ) {
        {
            let mut recs = self.records.lock();
            if let Some(r) = recs.iter_mut().find(|r| r.info.agent_id == info.agent_id) {
                r.info = info.clone();
                if launch.is_some() {
                    r.launch = launch;
                }
                r.updated_at_ms = now_ms();
            }
        }
        self.save();
    }

    /// The current execution of an identity.
    pub fn execution(&self, id: &str) -> Option<u32> {
        self.records
            .lock()
            .iter()
            .find(|r| r.info.agent_id == id)
            .map(|r| r.execution)
    }

    /// The message queue of the running execution `execution` (or none).
    pub(crate) fn set_controls(
        &self,
        id: &str,
        execution: u32,
        controls: Option<Arc<crate::events::StreamControls>>,
    ) {
        if self.execution(id) != Some(execution) {
            return;
        }
        if let Some(l) = self.live.lock().get_mut(id) {
            l.controls = controls;
        }
    }

    /// Start a new execution of an ended identity, atomically: of two
    /// concurrent resumes, one starts. The record is queued again with a
    /// fresh cancellation token (from the session) and a fresh end signal;
    /// its previous result stays in its stored transcript.
    pub fn begin_resume(
        &self,
        id: &str,
        viewer: Option<&str>,
    ) -> Result<(u32, CancellationToken), ResumeRefusal> {
        if !self.visible_to(id, viewer) {
            return Err(if self.record(id).is_some() {
                ResumeRefusal::OutOfScope
            } else {
                ResumeRefusal::Unknown
            });
        }
        let cancel = self.session_token.child_token();
        let (done_tx, done) = tokio::sync::watch::channel(false);
        let execution = {
            let mut recs = self.records.lock();
            let mut live = self.live.lock();
            let Some(r) = recs.iter_mut().find(|r| r.info.agent_id == id) else {
                return Err(ResumeRefusal::Unknown);
            };
            if !r.state.is_terminal() {
                return Err(ResumeRefusal::NotEnded(r.state));
            }
            // The previous execution must be over, not only marked so.
            if live.get(id).is_some_and(|l| !*l.done.borrow()) {
                return Err(ResumeRefusal::Busy);
            }
            r.execution += 1;
            r.state = InstanceState::Queued;
            r.reason = None;
            r.result = None;
            r.stopped = false;
            r.updated_at_ms = now_ms();
            live.insert(
                id.to_string(),
                Live {
                    cancel: cancel.clone(),
                    done,
                    done_tx,
                    handle: None,
                    controls: None,
                },
            );
            r.execution
        };
        self.save();
        Ok((execution, cancel))
    }

    pub fn attach_task(&self, id: &str, handle: tokio::task::JoinHandle<()>) {
        if let Some(l) = self.live.lock().get_mut(id) {
            l.handle = Some(handle);
        }
    }

    /// Set a state; a terminal state is set once and never changed.
    /// Returns whether it changed.
    pub fn set_state(&self, id: &str, state: InstanceState, reason: Option<String>) -> bool {
        let changed = {
            let mut recs = self.records.lock();
            match recs.iter_mut().find(|r| r.info.agent_id == id) {
                Some(r) if !r.state.is_terminal() => {
                    r.state = state;
                    r.reason = reason;
                    r.updated_at_ms = now_ms();
                    true
                }
                _ => false,
            }
        };
        if changed && state.is_terminal() {
            self.save();
        }
        changed
    }

    /// The end of one execution. A late end of an older execution (its
    /// identity was resumed since) changes nothing.
    pub fn finish(&self, id: &str, result: AgentResult) {
        let current = {
            let mut recs = self.records.lock();
            match recs.iter_mut().find(|r| r.info.agent_id == id) {
                Some(r) if r.execution == result.execution => {
                    if r.result.is_none() {
                        r.result = Some(result);
                        r.updated_at_ms = now_ms();
                    }
                    true
                }
                _ => false,
            }
        };
        if !current {
            return;
        }
        self.save();
        if let Some(l) = self.live.lock().get_mut(id) {
            l.controls = None;
            let _ = l.done_tx.send(true);
        }
        self.notify_parent(id);
        self.bump();
        self.maybe_final_usage(id);
    }

    /// The end of a sub-agent, delivered once to the agent that launched
    /// it (its report, shortened to fit one delivery; `AgentControl result`
    /// gives it whole).
    fn notify_parent(&self, id: &str) {
        let Some(r) = self.record(id) else { return };
        let Some(result) = &r.result else { return };
        // The session's agent has an identity of its own (`main_…`): for the
        // team it is `main`, the only parent that is no sub-agent.
        let parent = r
            .info
            .parent_id
            .clone()
            .filter(|p| self.record(p).is_some())
            .unwrap_or_else(|| super::messagent::MAIN.to_string());
        let report = super::spawn::render_result(result);
        const ROOM: usize = crate::events::MAX_INJECTED_BYTES - 512;
        let body = if report.len() > ROOM {
            let mut cut = ROOM;
            while !report.is_char_boundary(cut) {
                cut -= 1;
            }
            format!(
                "{}\n[report shortened here; AgentControl result {id} gives it whole]",
                &report[..cut]
            )
        } else {
            report
        };
        let _ = self.deliver(
            &parent,
            format!(
                "[{id} · {} ended: {}]\n{body}",
                super::messagent::member_name(&r.info),
                result.status
            ),
        );
    }

    /// An explicit stop: a later message is refused, nothing resumes it
    /// unless a resume is asked for.
    pub fn mark_stopped(&self, id: &str) {
        let mut recs = self.records.lock();
        if let Some(r) = recs.iter_mut().find(|r| r.info.agent_id == id) {
            r.stopped = true;
        }
    }

    pub fn record(&self, id: &str) -> Option<InstanceRecord> {
        self.records
            .lock()
            .iter()
            .find(|r| r.info.agent_id == id)
            .cloned()
    }

    pub fn records(&self) -> Vec<InstanceRecord> {
        self.records.lock().clone()
    }

    /// Whether `viewer` may see `id`: the session agent sees every
    /// instance of the session; a sub-agent sees its own descendants.
    pub fn visible_to(&self, id: &str, viewer: Option<&str>) -> bool {
        let Some(viewer) = viewer else { return true };
        let recs = self.records.lock();
        let mut cur = recs.iter().find(|r| r.info.agent_id == id);
        while let Some(r) = cur {
            match &r.info.parent_id {
                Some(p) if p == viewer => return true,
                Some(p) => cur = recs.iter().find(|x| &x.info.agent_id == p),
                None => return false,
            }
        }
        false
    }

    /// Cancel an instance and its descendants.
    pub fn cancel(&self, id: &str) -> usize {
        let ids: Vec<String> = {
            let recs = self.records.lock();
            let mut out = vec![id.to_string()];
            let mut i = 0;
            while i < out.len() {
                let p = out[i].clone();
                for r in recs.iter() {
                    if r.info.parent_id.as_deref() == Some(p.as_str())
                        && !out.contains(&r.info.agent_id)
                    {
                        out.push(r.info.agent_id.clone());
                    }
                }
                i += 1;
            }
            out
        };
        let live = self.live.lock();
        let mut n = 0;
        for i in &ids {
            if let Some(l) = live.get(i)
                && !*l.done.borrow()
            {
                l.cancel.cancel();
                n += 1;
            }
        }
        n
    }

    /// Cancel every instance of a root run.
    pub fn cancel_root(&self, root: &str) -> usize {
        let ids: Vec<String> = self
            .records
            .lock()
            .iter()
            .filter(|r| r.info.root_run_id == root && !r.state.is_terminal())
            .map(|r| r.info.agent_id.clone())
            .collect();
        ids.iter().map(|i| self.cancel(i)).sum()
    }

    /// Wait for an instance's end (bounded, cancellable). Its record.
    pub async fn wait(
        &self,
        id: &str,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Option<InstanceRecord> {
        let rx = self.live.lock().get(id).map(|l| l.done.clone());
        if let Some(mut rx) = rx {
            tokio::select! {
                _ = rx.wait_for(|d| *d) => {}
                _ = tokio::time::sleep(timeout) => {}
                _ = cancel.cancelled() => {}
            }
        }
        self.record(id)
    }

    /// Instances not finished yet (of one root run, or all).
    pub fn pending(&self, root: Option<&str>) -> Vec<InstanceRecord> {
        self.records
            .lock()
            .iter()
            .filter(|r| !r.state.is_terminal() && root.is_none_or(|x| r.info.root_run_id == x))
            .cloned()
            .collect()
    }

    /// Wait (bounded) for the given instances' tasks to end.
    pub async fn settle(&self, ids: &[String], timeout: Duration) {
        let handles: Vec<tokio::task::JoinHandle<()>> = {
            let mut live = self.live.lock();
            ids.iter()
                .filter_map(|i| live.get_mut(i).and_then(|l| l.handle.take()))
                .collect()
        };
        let _ = tokio::time::timeout(timeout, futures::future::join_all(handles)).await;
    }

    /// Close the session: cancel everything, wait for cleanup (bounded).
    pub async fn shutdown(&self, timeout: Duration) {
        self.session_token.cancel();
        let ids: Vec<String> = self
            .pending(None)
            .into_iter()
            .map(|r| r.info.agent_id)
            .collect();
        for i in &ids {
            self.cancel(i);
        }
        self.settle(&ids, timeout).await;
    }

    // ── Usage ────────────────────────────────────────────────────────────

    /// One response of one agent of `root`: added once, where it belongs.
    pub fn add_usage(&self, root: &str, is_root: bool, usage: &Usage) {
        let mut u = self.usage.lock();
        let e = u.entry(root.to_string()).or_default();
        if is_root {
            e.own.merge(usage);
        } else {
            e.descendants.merge(usage);
        }
    }

    pub fn run_usage(&self, root: &str) -> RunUsage {
        self.usage.lock().get(root).cloned().unwrap_or_default()
    }

    /// The session agent's run ended: its total, partial while descendants
    /// still run. Returns the event to publish.
    pub fn root_finished(&self, root: &str) -> AgentEvent {
        {
            let mut u = self.usage.lock();
            u.entry(root.to_string()).or_default().root_finished = true;
        }
        self.usage_event(root)
    }

    fn usage_event(&self, root: &str) -> AgentEvent {
        let pending = self.pending(Some(root)).len();
        let ru = self.run_usage(root);
        AgentEvent::SubAgent(SubAgentEvent::RunUsage {
            root_run_id: root.to_string(),
            total: Box::new(ru.total()),
            own: Box::new(ru.own),
            descendants: Box::new(ru.descendants),
            pending_agents: pending,
            final_total: pending == 0 && ru.root_finished,
        })
    }

    /// After a descendant ended: once the root run is done and nothing is
    /// pending any more, publish the final total (once).
    fn maybe_final_usage(&self, id: &str) {
        let Some(root) = self.record(id).map(|r| r.info.root_run_id) else {
            return;
        };
        let finished = self
            .usage
            .lock()
            .get(&root)
            .is_some_and(|u| u.root_finished);
        if finished && self.pending(Some(&root)).is_empty() {
            let ev = self.usage_event(&root);
            self.emit(ev);
        }
    }
}

/// The runtime of the agent whose tool context this is.
#[derive(Clone)]
pub struct RuntimeHandle(pub Arc<AgentRuntime>);

/// The section of a git patch (`git diff` output) that concerns `path`: each
/// section starts with a `diff --git a/<old> b/<new>` line.
fn patch_section(patch: &str, path: &str) -> Option<String> {
    let bytes = patch.as_bytes();
    let mut starts: Vec<usize> = patch
        .match_indices("diff --git ")
        .map(|(i, _)| i)
        .filter(|&i| i == 0 || bytes[i - 1] == b'\n')
        .collect();
    starts.push(patch.len());
    for pair in starts.windows(2) {
        let section = &patch[pair[0]..pair[1]];
        let Some(header) = section.lines().next() else {
            continue;
        };
        let new_path = header.rsplit_once(" b/").map(|(_, p)| p);
        if new_path == Some(path) {
            return Some(section.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(max_concurrent: usize, max_depth: u32, _unused: u32, max_queued: usize) -> Limits {
        Limits {
            max_concurrent,
            max_depth,
            max_queued,
            admission_timeout: Duration::from_secs(5),
            background_drain: Duration::from_secs(5),
        }
    }

    #[test]
    fn only_the_depth_is_checked_and_there_is_no_cumulative_cap() {
        let s = Scheduler::new(limits(4, 2, 0, 8));
        // Any number of batches, one after the other: the slots pace them.
        for _ in 0..40 {
            assert!(s.reserve("r", 0, 3).is_ok());
        }
        // Grandchildren (depth 2) allowed, depth 3 refused.
        assert!(s.reserve("r", 1, 1).is_ok());
        assert_eq!(
            s.reserve("r", 2, 1),
            Err(AdmissionError::DepthExceeded { depth: 3, max: 2 })
        );
    }

    #[tokio::test]
    async fn queue_full_timeout_and_cancellation() {
        let s = Scheduler::new(Limits {
            admission_timeout: Duration::from_millis(50),
            ..limits(1, 2, 10, 1)
        });
        let never = CancellationToken::new();
        let p = s.acquire(&never, || {}).await.unwrap();
        // One may wait; it times out.
        let t = s.acquire(&never, || {}).await;
        assert_eq!(t.err(), Some(AdmissionError::Timeout { ms: 50 }));
        // Queue full: a waiter is there, a second is refused at once.
        let s2 = Arc::clone(&s);
        let waiter = tokio::spawn(async move {
            let c = CancellationToken::new();
            s2.acquire(&c, || {}).await.is_ok()
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            s.acquire(&never, || {}).await.err(),
            Some(AdmissionError::QueueFull { max: 1 })
        );
        // Cancellation while queued gives the place back.
        let _ = waiter.await;
        let c = CancellationToken::new();
        c.cancel();
        assert_eq!(
            s.acquire(&c, || {}).await.err(),
            Some(AdmissionError::Cancelled)
        );
        drop(p);
        assert!(s.acquire(&never, || {}).await.is_ok());
        assert_eq!(s.stats().queued, 0);
    }

    #[tokio::test]
    async fn a_waiting_parent_gives_its_slot_to_its_child_with_one_slot() {
        let s = Scheduler::new(limits(1, 3, 10, 4));
        let cancel = CancellationToken::new();
        let parent_slot = s.acquire(&cancel, || {}).await.unwrap();
        let lease = ActivityLease::new(
            Arc::clone(&s),
            "parent".into(),
            parent_slot,
            None,
            cancel.clone(),
        );
        let s2 = Arc::clone(&s);
        // The child needs the only slot: it gets it while the parent waits.
        let got = lease
            .waiting(async move {
                let c = CancellationToken::new();
                let p = s2.acquire(&c, || {}).await;
                p.is_ok()
            })
            .await;
        assert!(got);
        // The parent has its slot back: nobody else can take one.
        assert_eq!(s.stats().active, 1);
        lease.release().await;
        assert_eq!(s.stats().active, 0);
    }

    /// A child that waits for its own child gives its slot up. A sibling
    /// queued before it takes that slot, so the grandchild expires, explicitly,
    /// with nothing left reserved: once the sibling lets go, the slot is free.
    #[tokio::test]
    async fn an_expired_grandchild_leaks_no_slot_of_a_waiting_child() {
        let s = Scheduler::new(Limits {
            admission_timeout: Duration::from_millis(50),
            ..limits(2, 3, 10, 4)
        });
        let never = CancellationToken::new();
        let sibling_holds = s.acquire(&never, || {}).await.unwrap();
        let child_slot = s.acquire(&never, || {}).await.unwrap();
        let lease = ActivityLease::new(
            Arc::clone(&s),
            "child".into(),
            child_slot,
            None,
            never.clone(),
        );
        // The sibling queues now; the child's slot goes to it when released.
        // It holds the slot for a while, then lets go: the child takes its
        // slot back then.
        let s_sibling = Arc::clone(&s);
        let sibling = tokio::spawn(async move {
            let slot = s_sibling
                .acquire(&CancellationToken::new(), || {})
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(slot);
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let s2 = Arc::clone(&s);
        let grandchild = lease
            .waiting(async move { s2.acquire(&CancellationToken::new(), || {}).await.err() })
            .await;
        assert_eq!(grandchild, Some(AdmissionError::Timeout { ms: 50 }));
        assert_eq!(s.stats().queued, 0, "the expired wait gave its place back");
        sibling.await.unwrap();
        lease.release().await;
        drop(sibling_holds);
        assert_eq!(s.stats().active, 0);
        assert!(s.acquire(&CancellationToken::new(), || {}).await.is_ok());
    }

    #[tokio::test]
    async fn the_writer_lease_is_released_while_waiting() {
        let d = tempfile::tempdir().unwrap();
        let s = Scheduler::new(limits(2, 3, 10, 4));
        let cancel = CancellationToken::new();
        let slot = s.acquire(&cancel, || {}).await.unwrap();
        let g = writers_for(d.path())
            .acquire("child", &cancel, |_| {})
            .await
            .unwrap();
        let lease = ActivityLease::new(
            Arc::clone(&s),
            "child".into(),
            slot,
            Some((d.path().to_path_buf(), g)),
            cancel.clone(),
        );
        let dir = d.path().to_path_buf();
        let grandchild_got_it = lease
            .waiting(async move {
                writers_for(&dir)
                    .acquire("grandchild", &CancellationToken::new(), |_| {})
                    .await
                    .is_some()
            })
            .await;
        assert!(
            grandchild_got_it,
            "no deadlock: the parent's writer lease was released"
        );
        assert_eq!(
            writers_for(d.path()).holder().as_deref(),
            Some("child"),
            "taken back"
        );
        lease.release().await;
        assert!(writers_for(d.path()).holder().is_none());
    }

    #[test]
    fn interrupted_instances_are_marked_on_recovery() {
        let d = tempfile::tempdir().unwrap();
        let manifest = d.path().join("manifest.json");
        let ws = Arc::new(super::super::workspace::WorkspaceManager::new(
            None,
            Default::default(),
        ));
        let rt = AgentRuntime::new(limits(2, 2, 10, 4), Arc::clone(&ws), Some(manifest.clone()));
        // A record written before 0.4.8 (with `max_turns`) still loads.
        let info: SpawnInfo = serde_json::from_value(serde_json::json!({
            "agent_id": "agent_x", "root_run_id": "run_1", "profile": "p", "profile_source": "s",
            "profile_revision": "r", "model": {"requested": "inherit", "applied": "m"},
            "reasoning": {"requested": "inherit", "applied": "(none)"}, "max_turns": 3,
            "workspace": "/w", "isolation": "shared", "task": "t", "created_at": "now"
        }))
        .unwrap();
        rt.register(info, true, CancellationToken::new());
        rt.set_state("agent_x", InstanceState::Running, None);
        drop(rt);
        let rt = AgentRuntime::new(limits(2, 2, 10, 4), ws, Some(manifest));
        let r = rt.record("agent_x").unwrap();
        assert_eq!(r.state, InstanceState::Interrupted);
        assert!(rt.pending(None).is_empty(), "nothing is restarted");
    }

    /// One record that cannot be read does not cost the others, and the
    /// original manifest is kept as a backup before the next save.
    #[test]
    fn an_unreadable_record_keeps_the_others_and_the_original_file() {
        let d = tempfile::tempdir().unwrap();
        let manifest = d.path().join("manifest.json");
        let ws = Arc::new(super::super::workspace::WorkspaceManager::new(
            None,
            Default::default(),
        ));
        let rt = AgentRuntime::new(limits(2, 2, 10, 4), Arc::clone(&ws), Some(manifest.clone()));
        let info: SpawnInfo = serde_json::from_value(serde_json::json!({
            "agent_id": "agent_x", "root_run_id": "run_1", "profile": "p", "profile_source": "s",
            "profile_revision": "r", "model": {"requested": "inherit", "applied": "m"},
            "reasoning": {"requested": "inherit", "applied": "(none)"},
            "workspace": "/w", "isolation": "shared", "task": "t", "created_at": "now"
        }))
        .unwrap();
        rt.register(info, true, CancellationToken::new());
        rt.set_state("agent_x", InstanceState::Running, None);
        drop(rt);
        let mut items: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
        items.push(serde_json::json!({"info": "unreadable"}));
        let original = serde_json::to_string(&items).unwrap();
        std::fs::write(&manifest, &original).unwrap();

        let rt = AgentRuntime::new(limits(2, 2, 10, 4), ws, Some(manifest));
        assert_eq!(
            rt.record("agent_x").unwrap().state,
            InstanceState::Interrupted
        );
        let backups: Vec<PathBuf> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().ends_with(".bak"))
            .collect();
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
    }

    #[test]
    fn usage_is_counted_once_per_response() {
        let ws = Arc::new(super::super::workspace::WorkspaceManager::new(
            None,
            Default::default(),
        ));
        let rt = AgentRuntime::new(limits(2, 2, 10, 4), ws, None);
        let u = |i, o| Usage {
            input_tokens: i,
            output_tokens: o,
            ..Default::default()
        };
        rt.add_usage("r", true, &u(100, 10));
        rt.add_usage("r", false, &u(7, 3)); // child
        rt.add_usage("r", false, &u(5, 1)); // grandchild
        rt.add_usage("r", true, &u(50, 5));
        let ru = rt.run_usage("r");
        assert_eq!((ru.own.input_tokens, ru.own.output_tokens), (150, 15));
        assert_eq!(
            (ru.descendants.input_tokens, ru.descendants.output_tokens),
            (12, 4)
        );
        assert_eq!(
            (ru.total().input_tokens, ru.total().output_tokens),
            (162, 19)
        );
        // Reading or replaying never changes the counts.
        let _ = rt.root_finished("r");
        let _ = rt.root_finished("r");
        assert_eq!(rt.run_usage("r").total().input_tokens, 162);
        // No price: the cost stays unknown, not zero.
        assert!(rt.run_usage("r").total().cost_usd.is_none());
    }

    #[test]
    fn a_changeset_patch_is_split_into_the_section_of_each_file() {
        let patch = "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-a\n+b\n\
                     diff --git a/y.bin b/y.bin\nnew file mode 100644\nGIT binary patch\n";
        let x = patch_section(patch, "x.rs").unwrap();
        assert!(x.contains("+b") && !x.contains("y.bin"), "{x}");
        assert!(
            patch_section(patch, "y.bin")
                .unwrap()
                .starts_with("diff --git a/y.bin")
        );
        assert_eq!(patch_section(patch, "missing.rs"), None);
        assert_eq!(patch_section("", "x.rs"), None);
    }

    fn info_of(id: &str) -> SpawnInfo {
        serde_json::from_value(serde_json::json!({
            "agent_id": id, "root_run_id": "run_1", "parent_id": "main_1",
            "profile": "p", "profile_source": "s", "profile_revision": "r",
            "model": {"requested": "inherit", "applied": "m"},
            "reasoning": {"requested": "inherit", "applied": "(none)"},
            "workspace": "/w", "isolation": "shared", "task": "t", "created_at": "now",
            "depth": 1
        }))
        .unwrap()
    }

    fn runtime(manifest: Option<PathBuf>) -> Arc<AgentRuntime> {
        let ws = Arc::new(super::super::workspace::WorkspaceManager::new(
            None,
            Default::default(),
        ));
        AgentRuntime::new(limits(4, 3, 10, 4), ws, manifest)
    }

    /// The end of an older execution, arriving after a resume, finishes
    /// nothing: the new execution keeps its state and its result.
    #[test]
    fn a_late_end_of_an_older_execution_does_not_finish_the_new_one() {
        let rt = runtime(None);
        rt.register(info_of("agent_x"), false, CancellationToken::new());
        rt.set_state("agent_x", InstanceState::Running, None);
        let mut first = AgentResult::lost(&info_of("agent_x"));
        first.status = "completed".into();
        first.error = None;
        rt.finish("agent_x", first.clone());
        rt.set_state("agent_x", InstanceState::Completed, None);
        let (exec, _cancel) = rt.begin_resume("agent_x", None).unwrap();
        assert_eq!(exec, 2);
        // Still the first execution's late end: ignored.
        rt.finish("agent_x", first);
        let r = rt.record("agent_x").unwrap();
        assert_eq!(r.execution, 2);
        assert_eq!(r.state, InstanceState::Queued);
        assert!(r.result.is_none(), "the new execution has no result yet");
        // A second resume while this one is not over: refused.
        assert_eq!(
            rt.begin_resume("agent_x", None).err(),
            Some(ResumeRefusal::NotEnded(InstanceState::Queued))
        );
        // Its own end counts.
        let mut own = AgentResult::lost(&info_of("agent_x"));
        own.execution = 2;
        own.status = "completed".into();
        own.error = None;
        rt.finish("agent_x", own);
        let r = rt.record("agent_x").unwrap();
        assert_eq!(r.result.as_ref().map(|x| x.execution), Some(2));
    }

    /// The context of a tool call: by the sub-agent `who`, or by the
    /// session's agent (`None`). Sub-agents are children of `main_1`, as the
    /// controller names the session's agent.
    fn tool_ctx(rt: &Arc<AgentRuntime>, who: Option<&str>) -> bricks_tools::ToolContext {
        use bricks_tools::{CostTracker, Extensions, ToolContext};
        let extensions = Extensions::default();
        extensions.insert(RuntimeHandle(Arc::clone(rt)));
        if let Some(id) = who {
            extensions.insert(super::super::spawn::AgentIdentity {
                agent_id: id.into(),
                parent_id: Some("main_1".into()),
                root_run_id: "run_1".into(),
            });
        }
        ToolContext {
            working_dir: std::env::temp_dir(),
            session_id: "main_1".into(),
            permissions: Arc::new(bricks_tools::permissions::AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions,
        }
    }

    /// Runs `Messagent` as `who` (`None`: the session's agent).
    async fn messagent(
        rt: &Arc<AgentRuntime>,
        who: Option<&str>,
        input: serde_json::Value,
    ) -> bricks_tools::ToolResult {
        use bricks_tools::Tool;
        super::super::messagent::MessagentTool
            .execute(input, &tool_ctx(rt, who))
            .await
    }

    /// A sub-agent of the session, running, with its message queue.
    fn running(rt: &Arc<AgentRuntime>, id: &str) -> Arc<crate::events::StreamControls> {
        rt.register(info_of(id), false, CancellationToken::new());
        rt.set_state(id, InstanceState::Running, None);
        let queue = Arc::new(crate::events::StreamControls::new(CancellationToken::new()));
        rt.set_controls(id, 1, Some(Arc::clone(&queue)));
        queue
    }

    /// The orchestrator's queue, for one of its runs going on.
    fn orchestrator_queue(rt: &Arc<AgentRuntime>) -> Arc<crate::events::StreamControls> {
        let queue = Arc::new(crate::events::StreamControls::new(CancellationToken::new()));
        rt.set_main_controls(Some(Arc::clone(&queue)));
        queue
    }

    /// `send` reaches one agent, several (each once), or the team. Each
    /// recipient reads it with its header; the sender is never one of them.
    #[tokio::test]
    async fn messagent_reaches_one_several_or_the_team() {
        use serde_json::json;
        let rt = runtime(None);
        let c0 = running(&rt, "sub_0000");
        let c1 = running(&rt, "sub_0001");
        let cm = orchestrator_queue(&rt);

        let one = messagent(
            &rt,
            None,
            json!({"action": "send", "to": "sub_0000", "content": "one"}),
        )
        .await;
        assert!(!one.is_error, "{}", one.content);
        assert_eq!(one.content, "m0001 sent to sub_0000.");
        assert_eq!(
            c0.drain_messages(),
            vec!["[Messagent m0001 from main to sub_0000]\none".to_string()]
        );
        assert_eq!(c1.pending_messages(), 0);

        let several = messagent(
            &rt,
            None,
            json!({"action": "send", "to": ["sub_0000", "sub_0001", "sub_0000"], "content": "both"}),
        )
        .await;
        assert_eq!(several.content, "m0002 sent to sub_0000, sub_0001.");
        assert_eq!(c0.pending_messages(), 1, "a repeated id is one recipient");
        assert_eq!(c1.pending_messages(), 1);

        let team = messagent(
            &rt,
            Some("sub_0000"),
            json!({"action": "send", "to": "team", "content": "all"}),
        )
        .await;
        assert_eq!(team.content, "m0003 sent to the team.");
        assert_eq!(
            cm.drain_messages(),
            vec!["[Messagent m0003 from sub_0000 to team]\nall".to_string()]
        );
        assert_eq!(c1.pending_messages(), 2, "the team reaches the other sub-agent");
        assert_eq!(c0.pending_messages(), 1, "the sender is not a recipient");

        let from_main = messagent(
            &rt,
            None,
            json!({"action": "send", "to": "team", "content": "again"}),
        )
        .await;
        assert_eq!(from_main.content, "m0004 sent to the team.");
        assert_eq!(cm.pending_messages(), 0, "the orchestrator is not its own recipient");
        assert_eq!(c0.pending_messages(), 2);
        assert_eq!(c1.pending_messages(), 3);
    }

    /// A refusal says what was not sent and sends nothing, to anyone.
    #[tokio::test]
    async fn messagent_refusals_send_nothing() {
        use serde_json::json;
        let rt = runtime(None);
        let c0 = running(&rt, "sub_0000");
        let c1 = running(&rt, "sub_0001");
        let too_large = "x".repeat(super::super::messagent::MAX_TEXT_BYTES + 1);
        let refused = [
            (json!({"action": "send", "to": "sub_0000", "content": "  \n"}), "the message is empty; nothing was sent."),
            (
                json!({"action": "send", "to": "sub_0000", "content": too_large}),
                "is over 15872 bytes",
            ),
            (json!({"action": "send", "to": "sub_9999", "content": "x"}), "no agent `sub_9999` in this team; nothing was sent."),
            (json!({"action": "send", "to": "sub_0000", "content": "x", "reply_to": "m9999"}), "no message `m9999` to reply to; nothing was sent."),
            (json!({"action": "send", "content": "x"}), "`send` needs `to`"),
        ];
        for (input, why) in refused {
            let r = messagent(&rt, None, input).await;
            assert!(r.is_error, "{}", r.content);
            assert!(r.content.contains(why), "{} / {why}", r.content);
        }
        let myself = messagent(
            &rt,
            Some("sub_0000"),
            json!({"action": "send", "to": "sub_0000", "content": "me"}),
        )
        .await;
        assert!(myself.is_error && myself.content.contains("yourself"), "{}", myself.content);
        assert_eq!(c0.pending_messages(), 0);
        assert_eq!(c1.pending_messages(), 0);

        // A full queue refuses, and says so.
        for _ in 0..crate::events::MAX_PENDING_MESSAGES {
            let ok = messagent(
                &rt,
                None,
                json!({"action": "send", "to": "sub_0000", "content": "q"}),
            )
            .await;
            assert!(!ok.is_error, "{}", ok.content);
        }
        let full = messagent(
            &rt,
            None,
            json!({"action": "send", "to": "sub_0000", "content": "overflow"}),
        )
        .await;
        assert!(
            full.is_error && full.content.contains("already has 32 messages waiting"),
            "{}",
            full.content
        );
        assert_eq!(
            c0.pending_messages(),
            crate::events::MAX_PENDING_MESSAGES,
            "nothing of the refused message is queued"
        );

        // An ended agent reads nothing: a message does not restart it.
        rt.set_state("sub_0001", InstanceState::Completed, None);
        rt.finish("sub_0001", AgentResult::lost(&info_of("sub_0001")));
        let ended = messagent(
            &rt,
            None,
            json!({"action": "send", "to": "sub_0001", "content": "late"}),
        )
        .await;
        assert!(
            ended.is_error && ended.content.contains("does not restart it"),
            "{}",
            ended.content
        );

        // Alone in the team, the orchestrator has nobody to write to.
        let alone = runtime(None);
        let nobody = messagent(
            &alone,
            None,
            json!({"action": "send", "to": "team", "content": "hello?"}),
        )
        .await;
        assert!(
            nobody.is_error && nobody.content.contains("nobody else is in the team now"),
            "{}",
            nobody.content
        );
    }

    /// The largest text a message takes is delivered whole, its header
    /// included: a recipient never gets a refusal for a text the tool accepted.
    #[tokio::test]
    async fn a_message_of_the_largest_size_is_delivered_whole() {
        use serde_json::json;
        let rt = runtime(None);
        let queue = running(&rt, "sub_0000");
        let largest = "x".repeat(super::super::messagent::MAX_TEXT_BYTES);
        let r = messagent(
            &rt,
            None,
            json!({"action": "send", "to": "sub_0000", "content": largest}),
        )
        .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(queue.pending_messages(), 1);
    }

    /// A chain of replies runs to eight; the ninth is refused, and the
    /// orchestrator is told when a sub-agent is the one who tried it.
    #[test]
    fn messagent_chain_stops_after_eight_replies_and_tells_the_orchestrator() {
        use super::super::messagent::{MAIN, MAX_CHAIN, Target};
        let rt = runtime(None);
        running(&rt, "sub_0000");
        let (first, _) = rt
            .post(MAIN, Target::Agents(vec!["sub_0000".into()]), "start".into(), None)
            .unwrap();
        assert_eq!(first.chain, 0);
        let mut last = first.id;
        for step in 1..=MAX_CHAIN {
            let (from, to) = if step % 2 == 1 {
                ("sub_0000", MAIN)
            } else {
                (MAIN, "sub_0000")
            };
            let (post, missed) = rt
                .post(
                    from,
                    Target::Agents(vec![to.into()]),
                    format!("reply {step}"),
                    Some(last.clone()),
                )
                .unwrap();
            assert_eq!(post.chain, step);
            assert!(missed.is_empty());
            last = post.id;
        }
        let err = rt
            .post(
                "sub_0000",
                Target::Agents(vec![MAIN.into()]),
                "one more".into(),
                Some(last),
            )
            .unwrap_err();
        assert!(
            err.contains("8 replies in a row") && err.contains("was refused"),
            "{err}"
        );
        let told = rt.mail.lock().backlog.last().cloned().unwrap_or_default();
        assert!(
            told.contains("sub_0000 tried to continue") && told.contains("past 8 replies"),
            "{told}"
        );
    }

    /// `read` gives the team thread's posts the reader has not got yet: a
    /// team post is delivered to its recipients' queues at once, so `read`
    /// gives it to a late member (or one whose delivery failed), once; `all`
    /// gives the whole thread.
    #[tokio::test]
    async fn messagent_read_gives_only_the_new_team_posts() {
        use serde_json::json;
        let rt = runtime(None);
        running(&rt, "sub_0000");
        let read = |who: Option<&'static str>, input: serde_json::Value| {
            let rt = Arc::clone(&rt);
            async move { messagent(&rt, who, input).await.content }
        };
        messagent(&rt, None, json!({"action": "send", "to": "team", "content": "one"})).await;
        // A member who joins later has not got it: `read` gives it, once.
        running(&rt, "sub_0001");
        let first = read(Some("sub_0001"), json!({"action": "read"})).await;
        assert!(first.contains("[Messagent m0001 from main to team]\none"), "{first}");
        assert_eq!(
            read(Some("sub_0001"), json!({"action": "read"})).await,
            "No new message in the team thread."
        );
        // A team post reaches both queues: nothing is left for `read`.
        messagent(&rt, Some("sub_0000"), json!({"action": "send", "to": "team", "content": "two"})).await;
        assert_eq!(
            read(Some("sub_0001"), json!({"action": "read"})).await,
            "No new message in the team thread."
        );
        let whole = read(Some("sub_0001"), json!({"action": "read", "all": true})).await;
        assert!(whole.contains("\none") && whole.contains("\ntwo"), "{whole}");
        // The orchestrator got `two` in its queue: `read` does not give it again.
        let main_reads = read(None, json!({"action": "read"})).await;
        assert!(main_reads.contains("\none") && !main_reads.contains("\ntwo"), "{main_reads}");
    }

    /// `members` lists the orchestrator, then every sub-agent, with its state.
    #[tokio::test]
    async fn messagent_members_lists_the_team_with_its_states() {
        use serde_json::json;
        let rt = runtime(None);
        running(&rt, "sub_0000");
        running(&rt, "sub_0001");
        rt.set_state("sub_0001", InstanceState::Completed, None);
        let listed = messagent(&rt, Some("sub_0000"), json!({"action": "members"})).await;
        assert!(!listed.is_error, "{}", listed.content);
        let lines: Vec<&str> = listed.content.lines().collect();
        assert_eq!(lines[0], "- main · Orchestrateur · running", "{}", listed.content);
        assert_eq!(lines[1], "- sub_0000 · Agent p · running (you)", "{}", listed.content);
        assert_eq!(lines[2], "- sub_0001 · Agent p · completed", "{}", listed.content);
    }

    /// What is sent to the orchestrator between its runs waits for its next
    /// run, and is moved into it.
    #[test]
    fn what_the_orchestrator_gets_between_its_runs_waits_for_the_next_one() {
        use super::super::messagent::{MAIN, Target};
        let rt = runtime(None);
        running(&rt, "sub_0000");
        rt.post("sub_0000", Target::Agents(vec![MAIN.into()]), "waiting".into(), None)
            .unwrap();
        assert_eq!(rt.waiting_for(MAIN), 1);
        let queue = orchestrator_queue(&rt);
        assert_eq!(queue.pending_messages(), 1, "moved into the run that starts");
        assert_eq!(rt.waiting_for(MAIN), 1);
        assert_eq!(queue.drain_messages().len(), 1, "the run reads it between its turns");
        rt.set_main_controls(None);
        rt.post("sub_0000", Target::Agents(vec![MAIN.into()]), "later".into(), None)
            .unwrap();
        assert_eq!(rt.waiting_for(MAIN), 1, "kept until the next run");
        let next = orchestrator_queue(&rt);
        assert_eq!(next.pending_messages(), 1, "and moved into it");
        assert_eq!(rt.waiting_for(MAIN), 1);
    }

    /// A wait ends at once when something waits for the waiter or when the
    /// awaited agent has ended; without either, it times out, consuming nothing.
    #[tokio::test]
    async fn wait_ends_on_a_delivery_or_on_the_end_of_the_awaited_agent() {
        use super::super::messagent::{MAIN, Target};
        let rt = runtime(None);
        running(&rt, "sub_0000");
        let cancel = CancellationToken::new();
        assert_eq!(
            rt.wait_event(MAIN, &[], Duration::from_millis(30), &cancel).await,
            WaitOutcome::TimedOut
        );
        rt.post("sub_0000", Target::Agents(vec![MAIN.into()]), "hi".into(), None)
            .unwrap();
        for _ in 0..2 {
            assert_eq!(
                rt.wait_event(MAIN, &[], Duration::from_secs(5), &cancel).await,
                WaitOutcome::Delivered(1),
                "read next turn, not consumed here"
            );
        }
        let awaited = vec!["sub_0000".to_string()];
        assert_eq!(
            rt.wait_event("sub_0001", &awaited, Duration::from_millis(30), &cancel)
                .await,
            WaitOutcome::TimedOut
        );
        rt.set_state("sub_0000", InstanceState::Completed, None);
        rt.finish("sub_0000", AgentResult::lost(&info_of("sub_0000")));
        assert_eq!(
            rt.wait_event("sub_0001", &awaited, Duration::from_secs(5), &cancel)
                .await,
            WaitOutcome::Ended("sub_0000".into())
        );
    }

    /// The report of a sub-agent whose parent is the orchestrator (`main`,
    /// the id the team uses) waits in the orchestrator's queue.
    #[test]
    fn the_report_of_a_sub_agent_waits_for_the_orchestrator() {
        use super::super::messagent::MAIN;
        let rt = runtime(None);
        let mut info = info_of("sub_0000");
        info.parent_id = Some(MAIN.into());
        rt.register(info.clone(), false, CancellationToken::new());
        rt.set_state("sub_0000", InstanceState::Completed, None);
        rt.finish("sub_0000", AgentResult::lost(&info));
        assert_eq!(rt.waiting_for(MAIN), 1);
    }

    /// A top-level run's identity is `main_<hex>` (runner.rs), and its
    /// sub-agents carry that id as their parent: their report still reaches
    /// the orchestrator, which the team names `main`.
    #[test]
    fn the_report_of_a_top_level_sub_agent_waits_for_the_orchestrator() {
        use super::super::messagent::MAIN;
        let rt = runtime(None);
        rt.register(info_of("sub_0000"), false, CancellationToken::new());
        rt.set_state("sub_0000", InstanceState::Completed, None);
        rt.finish("sub_0000", AgentResult::lost(&info_of("sub_0000")));
        assert_eq!(rt.waiting_for(MAIN), 1);
    }

    /// Sub-agent ids are `sub_0000`, `sub_0001`…, in order and never reused.
    #[test]
    fn allocate_id_counts_from_zero_and_is_never_reused() {
        let rt = runtime(None);
        assert_eq!(rt.allocate_id().unwrap(), "sub_0000");
        assert_eq!(rt.allocate_id().unwrap(), "sub_0001");
        let ids: std::collections::HashSet<String> =
            (0..100).map(|_| rt.allocate_id().unwrap()).collect();
        assert_eq!(ids.len(), 100);
        assert!(ids.contains("sub_0101"));
        assert_eq!(rt.allocate_id().unwrap(), "sub_0102");
    }

    /// After a restart, the ids go on after the last one recorded.
    #[test]
    fn allocate_id_goes_on_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("manifest.json");
        {
            let rt = runtime(Some(manifest.clone()));
            for _ in 0..3 {
                let id = rt.allocate_id().unwrap();
                rt.register(info_of(&id), false, CancellationToken::new());
            }
        }
        let rt = runtime(Some(manifest));
        assert_eq!(rt.allocate_id().unwrap(), "sub_0003");
    }

    /// A team post delivered to a member is not given again by its `read`,
    /// even when the member joined after an earlier post.
    #[tokio::test]
    async fn a_late_member_does_not_read_again_what_was_delivered_to_it() {
        use serde_json::json;
        let rt = runtime(None);
        running(&rt, "sub_0000");
        messagent(&rt, None, json!({"action": "send", "to": "team", "content": "one"})).await;
        running(&rt, "sub_0001");
        messagent(&rt, None, json!({"action": "send", "to": "team", "content": "two"})).await;
        let got = messagent(&rt, Some("sub_0001"), json!({"action": "read"})).await;
        assert!(!got.content.contains("\ntwo"), "{}", got.content);
    }

    /// An id is not given again because its record cannot be read after a
    /// restart: the unreadable record is left out, its id is not.
    #[test]
    fn an_unreadable_record_keeps_its_id_out_of_reach() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("manifest.json");
        {
            let rt = runtime(Some(manifest.clone()));
            for _ in 0..2 {
                let id = rt.allocate_id().unwrap();
                rt.register(info_of(&id), false, CancellationToken::new());
            }
        }
        let mut items: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
        // Its state is no longer one the runtime knows: the record cannot be read.
        items[1]["state"] = serde_json::json!("invented");
        std::fs::write(&manifest, serde_json::to_string(&items).unwrap()).unwrap();
        let rt = runtime(Some(manifest));
        assert_eq!(rt.allocate_id().unwrap(), "sub_0002");
    }

    /// Ten thousand sub-agents per session, then no id is left: the refusal
    /// says so and names the range.
    #[test]
    fn allocate_id_runs_out_after_ten_thousand() {
        let rt = runtime(None);
        for n in 0..MAX_SUB_AGENTS {
            assert_eq!(rt.allocate_id().unwrap(), format!("sub_{n:04}"));
        }
        let err = rt.allocate_id().unwrap_err();
        assert!(
            err.contains("10000 sub-agents") && err.contains("sub_9999"),
            "{err}"
        );
    }


    /// A manifest written by an earlier version, with a termination kind this
    /// version does not know, loads: the record is kept as historical and the
    /// file is not replaced by a backup.
    #[test]
    fn an_unknown_termination_kind_loads_as_historical_and_is_not_wiped() {
        let d = tempfile::tempdir().unwrap();
        let manifest = d.path().join("manifest.json");
        let mut result = serde_json::to_value(AgentResult::lost(&info_of("agent_old"))).unwrap();
        result["status"] = "incomplete".into();
        result["termination"] = serde_json::json!({"kind": "max_turns", "limit": 30});
        let record = serde_json::json!({
            "info": info_of("agent_old"),
            "state": "incomplete",
            "background": false,
            "updated_at_ms": 1,
            "result": result,
        });
        std::fs::write(&manifest, serde_json::to_string(&vec![record]).unwrap()).unwrap();
        let rt = runtime(Some(manifest.clone()));
        let r = rt.record("agent_old").expect("the record is kept");
        assert_eq!(r.state, InstanceState::Incomplete, "not interrupted");
        assert_eq!(
            r.result.unwrap().termination,
            Some(crate::Termination::Historical {
                kind: "max_turns".into()
            })
        );
        let on_disk: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
        assert_eq!(on_disk.len(), 1, "not wiped");
        assert_eq!(on_disk[0]["result"]["termination"]["kind"], "max_turns");
        let backups = std::fs::read_dir(d.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().to_string_lossy().ends_with(".bak"))
            .count();
        assert_eq!(backups, 0, "readable: no backup needed");
    }

    /// The admission window is the configured one, measured on a controlled
    /// clock; an expired admission leaves nothing reserved.
    #[tokio::test(start_paused = true)]
    async fn an_admission_window_of_sixty_seconds_expires_exactly_and_releases_nothing() {
        let s = Scheduler::new(Limits {
            admission_timeout: Duration::from_secs(60),
            ..limits(1, 3, 10, 4)
        });
        let never = CancellationToken::new();
        let holder = s.acquire(&never, || {}).await.unwrap();
        let t0 = tokio::time::Instant::now();
        let r = s.acquire(&never, || {}).await;
        assert_eq!(r.err(), Some(AdmissionError::Timeout { ms: 60_000 }));
        assert_eq!(t0.elapsed(), Duration::from_secs(60));
        assert_eq!(s.stats().queued, 0);
        drop(holder);
        assert_eq!(s.stats().active, 0);
        assert!(s.acquire(&never, || {}).await.is_ok());
    }

    /// A shared-workspace lease of `holder`, with a slot of its own.
    async fn shared_lease(
        s: &Arc<Scheduler>,
        holder: &str,
        dir: &Path,
        cancel: &CancellationToken,
    ) -> Arc<ActivityLease> {
        let slot = s.acquire(cancel, || {}).await.expect("a free slot");
        ActivityLease::shared(
            Arc::clone(s),
            holder.into(),
            slot,
            dir.to_path_buf(),
            cancel.clone(),
        )
    }

    /// A shared-workspace child holds no writer lease at launch: while it
    /// has only read, another agent writes at once.
    #[tokio::test]
    async fn a_shared_child_holds_no_writer_lease_until_it_claims_one() {
        let d = tempfile::tempdir().unwrap();
        let s = Scheduler::new(limits(2, 3, 10, 4));
        let never = CancellationToken::new();
        let lease = shared_lease(&s, "sub_0000", d.path(), &never).await;
        assert_eq!(lease.writer_dir(), Some(d.path()));
        assert!(writers_for(d.path()).holder().is_none(), "nothing claimed at launch");
        let other = tokio::time::timeout(
            Duration::from_secs(1),
            writers_for(d.path()).acquire("sub_0001", &never, |_| {}),
        )
        .await
        .expect("an unclaimed lease does not block another writer")
        .unwrap();
        drop(other);
        lease.release().await;
        assert!(writers_for(d.path()).holder().is_none());
    }

    /// The first writing call takes the free workspace; a later one keeps it
    /// at once; the end of the child lets it go.
    #[tokio::test]
    async fn the_first_writing_call_claims_the_free_workspace_until_the_end() {
        let d = tempfile::tempdir().unwrap();
        let s = Scheduler::new(limits(2, 3, 10, 4));
        let never = CancellationToken::new();
        let lease = shared_lease(&s, "sub_0000", d.path(), &never).await;
        assert_eq!(lease.claim_writer(Duration::from_millis(50)).await, Ok(true));
        assert_eq!(writers_for(d.path()).holder().as_deref(), Some("sub_0000"));
        assert_eq!(lease.claim_writer(Duration::from_millis(50)).await, Ok(true));
        lease.release().await;
        assert!(writers_for(d.path()).holder().is_none());
    }

    /// Another writer keeps the workspace past the timeout: the claim fails
    /// with its name, without taking it away; once it lets go, the claim works.
    #[tokio::test]
    async fn a_claim_is_refused_with_the_holder_name_while_another_keeps_the_workspace() {
        let d = tempfile::tempdir().unwrap();
        let s = Scheduler::new(limits(2, 3, 10, 4));
        let never = CancellationToken::new();
        let other = writers_for(d.path())
            .acquire("sub_0009", &never, |_| {})
            .await
            .unwrap();
        let lease = shared_lease(&s, "sub_0000", d.path(), &never).await;
        assert_eq!(
            lease.claim_writer(Duration::from_millis(50)).await,
            Err("sub_0009".to_string())
        );
        assert_eq!(
            writers_for(d.path()).holder().as_deref(),
            Some("sub_0009"),
            "not taken from its holder"
        );
        drop(other);
        assert_eq!(lease.claim_writer(Duration::from_millis(50)).await, Ok(true));
        assert_eq!(writers_for(d.path()).holder().as_deref(), Some("sub_0000"));
        lease.release().await;
    }

    /// A child that only reads never takes the workspace: a sibling that
    /// writes gets it at once, and the reader's own waits change nothing.
    #[tokio::test]
    async fn a_reader_child_never_blocks_a_writer_sibling() {
        let d = tempfile::tempdir().unwrap();
        let s = Scheduler::new(limits(3, 3, 10, 4));
        let never = CancellationToken::new();
        let reader = shared_lease(&s, "sub_0000", d.path(), &never).await;
        let writer = shared_lease(&s, "sub_0001", d.path(), &never).await;
        reader.waiting(async {}).await;
        assert!(writers_for(d.path()).holder().is_none());
        assert_eq!(writer.claim_writer(Duration::from_millis(50)).await, Ok(true));
        assert_eq!(writers_for(d.path()).holder().as_deref(), Some("sub_0001"));
        writer.release().await;
        reader.release().await;
    }

    /// A claimed child that waits for its own child gives the workspace up
    /// for that wait, and takes it back when the wait ends.
    #[tokio::test]
    async fn a_claimed_lease_is_released_during_a_wait_and_taken_back_after_it() {
        let d = tempfile::tempdir().unwrap();
        let s = Scheduler::new(limits(2, 3, 10, 4));
        let never = CancellationToken::new();
        let lease = shared_lease(&s, "sub_0000", d.path(), &never).await;
        assert_eq!(lease.claim_writer(Duration::from_millis(50)).await, Ok(true));
        let dir = d.path().to_path_buf();
        let during = lease.waiting(async move { writers_for(&dir).holder() }).await;
        assert_eq!(during, None, "released while waiting");
        assert_eq!(
            writers_for(d.path()).holder().as_deref(),
            Some("sub_0000"),
            "taken back after the wait"
        );
        lease.release().await;
        assert!(writers_for(d.path()).holder().is_none());
    }

    /// A lease that never claimed does not take the workspace back after a
    /// wait: another writer that took it in the meantime keeps it. (Had it
    /// tried, the wait would never end, hence the timeout.)
    #[tokio::test]
    async fn an_unclaimed_lease_is_not_taken_back_after_a_wait() {
        let d = tempfile::tempdir().unwrap();
        let s = Scheduler::new(limits(2, 3, 10, 4));
        let never = CancellationToken::new();
        let lease = shared_lease(&s, "sub_0000", d.path(), &never).await;
        let dir = d.path().to_path_buf();
        let guard = tokio::time::timeout(
            Duration::from_secs(2),
            lease.waiting(async move {
                writers_for(&dir)
                    .acquire("sub_0009", &CancellationToken::new(), |_| {})
                    .await
            }),
        )
        .await
        .expect("the wait ends: nothing is taken back from another writer");
        assert!(guard.is_some());
        assert_eq!(writers_for(d.path()).holder().as_deref(), Some("sub_0009"));
        drop(guard);
        lease.release().await;
        assert!(writers_for(d.path()).holder().is_none());
    }

    /// `members` names the worktree of each sub-agent that has one; a
    /// sub-agent of the shared workspace names none.
    #[tokio::test]
    async fn messagent_members_names_each_worktree_and_none_for_the_shared_workspace() {
        use serde_json::json;
        let rt = runtime(None);
        running(&rt, "sub_0000");
        let mut info = info_of("sub_0001");
        info.isolation = "worktree".into();
        info.workspace = "/wt/sub_0001".into();
        rt.register(info, false, CancellationToken::new());
        rt.set_state("sub_0001", InstanceState::Running, None);
        let listed = messagent(&rt, Some("sub_0000"), json!({"action": "members"})).await;
        assert!(!listed.is_error, "{}", listed.content);
        let lines: Vec<&str> = listed.content.lines().collect();
        assert_eq!(lines[1], "- sub_0000 · Agent p · running (you)", "{}", listed.content);
        assert_eq!(
            lines[2],
            "- sub_0001 · Agent p · running · worktree /wt/sub_0001",
            "{}",
            listed.content
        );
    }
}
