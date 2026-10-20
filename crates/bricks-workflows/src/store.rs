//! Run persistence. MVP: an in-memory snapshot store keyed by `run_id`, used to
//! support suspend/resume within a process. Phase 2 swaps in a JSONL/`bricks-memory`
//! backend so suspended runs survive restarts (the `RunStore` API stays the same).

use crate::ir::NodeId;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// A resumable snapshot of a run: everything needed to continue from a suspend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub run_id: String,
    pub input: Value,
    pub state: Value,
    pub outputs: HashMap<NodeId, Value>,
}

/// In-memory snapshot store. Cloneable handle over shared state.
#[derive(Default, Clone)]
pub struct RunStore {
    snapshots: Arc<DashMap<String, RunSnapshot>>,
}

impl RunStore {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn save(&self, snapshot: RunSnapshot) {
        self.snapshots.insert(snapshot.run_id.clone(), snapshot);
    }

    pub fn load(&self, run_id: &str) -> Option<RunSnapshot> {
        self.snapshots.get(run_id).map(|s| s.value().clone())
    }

    pub fn remove(&self, run_id: &str) {
        self.snapshots.remove(run_id);
    }
}
