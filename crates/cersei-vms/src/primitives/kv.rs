//! KvStore — shared key/value state across sandboxes.
//!
//! In-memory `DashMap` with optional journal-on-disk persistence (one
//! JSON file written on each `set` / `delete`). Versioned CAS via the
//! `version` field returned with each entry.

use crate::error::{Result, VmError};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KvEntry {
    pub value: Vec<u8>,
    pub version: u64,
    pub updated_at_unix_ms: i64,
}

/// Full serialisable snapshot of the store (used by `snapshot`/`Snapshot Manifest`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KvSnapshot {
    pub entries: Vec<(String, KvEntry)>,
    pub next_version: u64,
}

#[derive(Clone)]
pub struct KvStore {
    inner: Arc<KvInner>,
}

struct KvInner {
    map: DashMap<String, KvEntry>,
    version_seq: AtomicU64,
    journal: Mutex<Option<PathBuf>>,
}

impl KvStore {
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(KvInner {
                map: DashMap::new(),
                version_seq: AtomicU64::new(0),
                journal: Mutex::new(None),
            }),
        }
    }

    /// Open (or create) a journalled store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let store = Self {
            inner: Arc::new(KvInner {
                map: DashMap::new(),
                version_seq: AtomicU64::new(0),
                journal: Mutex::new(Some(path.clone())),
            }),
        };
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            if !bytes.is_empty() {
                let snap: KvSnapshot = serde_json::from_slice(&bytes)?;
                store.restore_inner(snap);
            }
        }
        Ok(store)
    }

    fn restore_inner(&self, snap: KvSnapshot) {
        self.inner.map.clear();
        for (k, v) in snap.entries {
            self.inner.map.insert(k, v);
        }
        self.inner
            .version_seq
            .store(snap.next_version, Ordering::Relaxed);
    }

    pub fn get(&self, key: &str) -> Option<KvEntry> {
        self.inner.map.get(key).map(|kv| kv.value().clone())
    }

    pub fn set(&self, key: impl Into<String>, value: impl Into<Vec<u8>>) -> Result<KvEntry> {
        let version = self.inner.version_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = KvEntry {
            value: value.into(),
            version,
            updated_at_unix_ms: chrono::Utc::now().timestamp_millis(),
        };
        self.inner.map.insert(key.into(), entry.clone());
        self.persist()?;
        Ok(entry)
    }

    /// Compare-and-swap. Sets only if current entry's version matches
    /// `expected_version`. Returns the new entry on success, `Ok(None)` on
    /// version mismatch (caller should retry).
    pub fn cas(
        &self,
        key: &str,
        expected_version: Option<u64>,
        value: impl Into<Vec<u8>>,
    ) -> Result<Option<KvEntry>> {
        let current = self.get(key).map(|e| e.version);
        if current != expected_version {
            return Ok(None);
        }
        Ok(Some(self.set(key, value)?))
    }

    pub fn delete(&self, key: &str) -> Result<bool> {
        let removed = self.inner.map.remove(key).is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    pub fn keys(&self) -> Vec<String> {
        self.inner.map.iter().map(|kv| kv.key().clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.map.is_empty()
    }

    /// Capture a full snapshot for persistence / restore.
    pub fn snapshot(&self) -> KvSnapshot {
        let entries = self
            .inner
            .map
            .iter()
            .map(|kv| (kv.key().clone(), kv.value().clone()))
            .collect();
        KvSnapshot {
            entries,
            next_version: self.inner.version_seq.load(Ordering::Relaxed),
        }
    }

    /// Restore from a snapshot (e.g. after sandbox restore).
    pub fn restore(&self, snap: KvSnapshot) -> Result<()> {
        self.restore_inner(snap);
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        let Some(path) = self.inner.journal.lock().clone() else {
            return Ok(());
        };
        let snap = self.snapshot();
        let bytes = serde_json::to_vec_pretty(&snap)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path).map_err(VmError::Io)
    }
}

impl Default for KvStore {
    fn default() -> Self {
        Self::in_memory()
    }
}
