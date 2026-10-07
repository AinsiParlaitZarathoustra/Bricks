//! The session's agent runtime: admission (depth, cumulative total, active
//! slots, bounded queue), the instances and their results, and the usage
//! ledger of each root run.
//!
//! Definitions:
//! * depth: the session's agent is 0, its children 1, their children 2;
//! * `max_total_per_run`: descendants **admitted** for one root run,
//!   cumulatively (finished ones still count);
//! * `max_concurrent`: active descendants. A slot is held while a child
//!   generates or runs tools; it is **released while the child waits for
//!   its own children** (and taken back before it continues), so a chain
//!   root → child → grandchild works with `max_concurrent = 1`. The
//!   session's own agent holds no slot.
//! * a full queue, an admission timeout or a hard limit is an explicit
//!   error; the tools stay in the catalogue.

use super::admission::{writers_for, WriterGuard};
use super::spawn::{AgentResult, InstanceState, SpawnInfo, SubAgentEvent};
use crate::events::AgentEvent;
use cersei_types::Usage;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

// ─── Limits ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub max_concurrent: usize,
    pub max_depth: u32,
    pub max_total_per_run: u32,
    pub max_queued: usize,
    pub admission_timeout: Duration,
    pub max_batch: usize,
    /// Headless: how long to wait for background children after the answer.
    pub background_drain: Duration,
}

/// Why admission refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    DepthExceeded { depth: u32, max: u32 },
    TotalExceeded { admitted: u32, asked: u32, max: u32 },
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
            Self::TotalExceeded { admitted, asked, max } => write!(
                f,
                "AgentTotalExceeded: {admitted} sub-agent(s) already admitted in this run, {asked} more asked, [agents] max_total_per_run = {max}"
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
    totals: Mutex<HashMap<String, u32>>,
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
            totals: Mutex::new(HashMap::new()),
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

    /// Admitted descendants of a root run so far.
    pub fn admitted(&self, root: &str) -> u32 {
        self.totals.lock().get(root).copied().unwrap_or(0)
    }

    /// Check the depth of `n` new children of an agent at `parent_depth`,
    /// and reserve them on the root run's total, atomically (a batch
    /// either fits whole or is refused whole).
    pub fn reserve(&self, root: &str, parent_depth: u32, n: u32) -> Result<(), AdmissionError> {
        let depth = parent_depth + 1;
        if depth > self.limits.max_depth {
            return Err(AdmissionError::DepthExceeded {
                depth,
                max: self.limits.max_depth,
            });
        }
        let mut totals = self.totals.lock();
        let admitted = totals.get(root).copied().unwrap_or(0);
        if admitted + n > self.limits.max_total_per_run {
            return Err(AdmissionError::TotalExceeded {
                admitted,
                asked: n,
                max: self.limits.max_total_per_run,
            });
        }
        totals.insert(root.to_string(), admitted + n);
        Ok(())
    }

    /// Give back reservations that never started (a batch refused after
    /// reservation, before any child existed).
    pub fn unreserve(&self, root: &str, n: u32) {
        let mut totals = self.totals.lock();
        if let Some(t) = totals.get_mut(root) {
            *t = t.saturating_sub(n);
        }
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

/// What an active child holds: its slot and, in a shared workspace, the
/// writer lease. Both are let go while it waits for its own children.
pub struct ActivityLease {
    scheduler: Arc<Scheduler>,
    holder: String,
    writer_dir: Option<PathBuf>,
    slot: tokio::sync::Mutex<Option<OwnedSemaphorePermit>>,
    writer: tokio::sync::Mutex<Option<WriterGuard>>,
    waits: AtomicUsize,
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
        Arc::new(Self {
            scheduler,
            holder,
            writer_dir,
            slot: tokio::sync::Mutex::new(Some(slot)),
            writer: tokio::sync::Mutex::new(guard),
            waits: AtomicUsize::new(0),
            cancel,
        })
    }

    /// Run `fut` (waiting for children) without holding the slot or the
    /// writer lease; take them back afterwards (writer first, then slot).
    pub async fn waiting<F: std::future::Future>(&self, fut: F) -> F::Output {
        if self.waits.fetch_add(1, Ordering::SeqCst) == 0 {
            self.slot.lock().await.take();
            self.writer.lock().await.take();
        }
        let out = fut.await;
        if self.waits.fetch_sub(1, Ordering::SeqCst) == 1 {
            if let Some(dir) = &self.writer_dir {
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
    ext: &cersei_tools::Extensions,
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
}

struct Live {
    cancel: CancellationToken,
    done: tokio::sync::watch::Receiver<bool>,
    done_tx: tokio::sync::watch::Sender<bool>,
    handle: Option<tokio::task::JoinHandle<()>>,
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

    pub fn emit(&self, ev: AgentEvent) {
        if let Some(s) = self.session_sink.lock().clone() {
            s(ev);
        }
    }

    /// A previous process's instances that never ended are marked
    /// interrupted; nothing is restarted, no old pid is touched.
    fn recover(&self) {
        let Some(path) = &self.manifest else { return };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(mut records) = serde_json::from_str::<Vec<InstanceRecord>>(&text) else {
            return;
        };
        for r in records.iter_mut() {
            if !r.state.is_terminal() {
                r.state = InstanceState::Interrupted;
                r.reason = Some("the process that ran it ended before it finished".into());
                r.updated_at_ms = now_ms();
            }
        }
        *self.records.lock() = records;
        self.save();
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
            },
        );
        self.records.lock().push(InstanceRecord {
            info,
            state: InstanceState::Created,
            reason: None,
            result: None,
            background,
            updated_at_ms: now_ms(),
        });
        self.save();
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

    pub fn finish(&self, id: &str, result: AgentResult) {
        {
            let mut recs = self.records.lock();
            if let Some(r) = recs.iter_mut().find(|r| r.info.agent_id == id) {
                if r.result.is_none() {
                    r.result = Some(result);
                    r.updated_at_ms = now_ms();
                }
            }
        }
        self.save();
        if let Some(l) = self.live.lock().get(id) {
            let _ = l.done_tx.send(true);
        }
        self.maybe_final_usage(id);
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
            if let Some(l) = live.get(i) {
                if !*l.done.borrow() {
                    l.cancel.cancel();
                    n += 1;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(max_concurrent: usize, max_depth: u32, max_total: u32, max_queued: usize) -> Limits {
        Limits {
            max_concurrent,
            max_depth,
            max_total_per_run: max_total,
            max_queued,
            admission_timeout: Duration::from_secs(5),
            max_batch: 8,
            background_drain: Duration::from_secs(5),
        }
    }

    #[test]
    fn depth_and_cumulative_total_are_checked_atomically() {
        let s = Scheduler::new(limits(4, 2, 5, 8));
        assert!(s.reserve("r", 0, 3).is_ok());
        // A batch either fits whole or is refused whole.
        assert_eq!(
            s.reserve("r", 0, 3),
            Err(AdmissionError::TotalExceeded {
                admitted: 3,
                asked: 3,
                max: 5
            })
        );
        assert_eq!(s.admitted("r"), 3);
        // Grandchildren (depth 2) allowed, depth 3 refused.
        assert!(s.reserve("r", 1, 1).is_ok());
        assert_eq!(
            s.reserve("r", 2, 1),
            Err(AdmissionError::DepthExceeded { depth: 3, max: 2 })
        );
        // Another root run has its own total.
        assert!(s.reserve("other", 0, 5).is_ok());
        // Rollback of a refused batch.
        s.unreserve("r", 1);
        assert_eq!(s.admitted("r"), 3);
    }

    #[test]
    fn concurrent_reservations_never_exceed_the_total() {
        let s = Scheduler::new(limits(4, 2, 10, 8));
        let ok = std::sync::Arc::new(AtomicUsize::new(0));
        std::thread::scope(|sc| {
            for _ in 0..16 {
                let s = &s;
                let ok = &ok;
                sc.spawn(move || {
                    if s.reserve("r", 0, 3).is_ok() {
                        ok.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(ok.load(Ordering::SeqCst), 3, "3 batches of 3 fit in 10");
        assert_eq!(s.admitted("r"), 9);
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
}
