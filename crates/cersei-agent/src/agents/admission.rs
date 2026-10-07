//! Writer admission for a shared workspace.
//!
//! The runner can run several tool calls of one turn at once, and a
//! foreground sub-agent works in its parent's checkout. One holder at a time
//! may write there: a sub-agent holds the workspace for its whole run; any
//! other agent's writing tool call (level `write`, `execute` or
//! `dangerous`: a command not proven read-only is a potential writer) waits
//! for it. The same holder can hold it several times at once (an agent's
//! own concurrent calls keep running in parallel, as before).
//!
//! A delegation call does not hold the workspace for its parent: the child
//! does. The parent waiting for its child holds nothing, so it cannot
//! deadlock.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct State {
    holder: Option<String>,
    count: usize,
}

/// Who may write in one workspace.
#[derive(Default)]
pub struct WorkspaceWriters {
    state: Mutex<State>,
    released: Notify,
}

/// Held while writing; released on drop.
pub struct WriterGuard {
    gate: Arc<WorkspaceWriters>,
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        let mut s = self.gate.state.lock();
        s.count = s.count.saturating_sub(1);
        if s.count == 0 {
            s.holder = None;
            drop(s);
            self.gate.released.notify_waiters();
        }
    }
}

impl WorkspaceWriters {
    /// The current holder, if any.
    pub fn holder(&self) -> Option<String> {
        self.state.lock().holder.clone()
    }

    fn try_acquire(self: &Arc<Self>, holder: &str) -> Option<WriterGuard> {
        let mut s = self.state.lock();
        match &s.holder {
            Some(h) if h != holder => None,
            _ => {
                s.holder = Some(holder.to_string());
                s.count += 1;
                Some(WriterGuard {
                    gate: Arc::clone(self),
                })
            }
        }
    }

    /// Wait for the workspace (cancellable). `on_wait` is told once who
    /// holds it, when waiting is needed.
    pub async fn acquire(
        self: &Arc<Self>,
        holder: &str,
        cancel: &CancellationToken,
        mut on_wait: impl FnMut(&str),
    ) -> Option<WriterGuard> {
        let mut told = false;
        loop {
            let released = self.released.notified();
            if let Some(g) = self.try_acquire(holder) {
                return Some(g);
            }
            if !told {
                if let Some(h) = self.holder() {
                    on_wait(&h);
                }
                told = true;
            }
            tokio::select! {
                _ = released => {}
                _ = cancel.cancelled() => return None,
            }
        }
    }
}

static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<WorkspaceWriters>>>> = OnceLock::new();

/// The gate of a workspace (one per canonical path, process-wide).
pub fn writers_for(dir: &Path) -> Arc<WorkspaceWriters> {
    let key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let map = GATES.get_or_init(|| Mutex::new(HashMap::new()));
    Arc::clone(map.lock().entry(key).or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn one_holder_reentrant_others_wait() {
        let g = Arc::new(WorkspaceWriters::default());
        let never = CancellationToken::new();
        let a1 = g.acquire("a", &never, |_| {}).await.unwrap();
        let a2 = g.acquire("a", &never, |_| {}).await.unwrap();
        let g2 = Arc::clone(&g);
        let waited = Arc::new(Mutex::new(None));
        let w = Arc::clone(&waited);
        let b = tokio::spawn(async move {
            let t = std::time::Instant::now();
            let _g = g2
                .acquire("b", &CancellationToken::new(), |h| {
                    *w.lock() = Some(h.to_string())
                })
                .await
                .unwrap();
            t.elapsed()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(waited.lock().as_deref(), Some("a"));
        drop(a1);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!b.is_finished(), "still held once by a");
        drop(a2);
        assert!(b.await.unwrap() >= Duration::from_millis(50));
        assert!(g.holder().is_none());
    }

    #[tokio::test]
    async fn waiting_is_cancellable() {
        let g = Arc::new(WorkspaceWriters::default());
        let _a = g
            .acquire("a", &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            c.cancel();
        });
        assert!(g.acquire("b", &cancel, |_| {}).await.is_none());
    }
}
