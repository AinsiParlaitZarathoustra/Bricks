//! Snapshot bookkeeping — backend-agnostic manifest stored on the host.
//!
//! A snapshot manifest captures everything we need to fully restore a
//! sandbox: the backend-specific FS state pointer (e.g. a Docker image
//! tag), env vars, mailbox subscriptions, KV snapshot, mounted volumes.

use crate::error::{Result, VmError};
use crate::primitives::KvSnapshot;
use crate::types::{SandboxOpts, SnapshotId, VolumeMount};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub id: SnapshotId,
    pub backend: String,
    /// Backend-specific FS state pointer.
    /// - docker: `"image:tag"` produced by `docker commit`
    /// - local:  the snapshot dir's relative path
    pub fs_pointer: String,
    pub original_opts: SandboxOpts,
    pub volumes: Vec<VolumeMount>,
    pub kv: KvSnapshot,
    pub mailbox_topics: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub labels: std::collections::HashMap<String, String>,
}

/// On-disk store of snapshot manifests. Backend implementations are
/// responsible for the backing FS state; this registry is the source
/// of truth for the *metadata*.
#[derive(Clone)]
pub struct SnapshotRegistry {
    root: PathBuf,
    cache: Arc<DashMap<SnapshotId, SnapshotManifest>>,
    write_lock: Arc<Mutex<()>>,
}

impl SnapshotRegistry {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let reg = Self {
            root,
            cache: Arc::new(DashMap::new()),
            write_lock: Arc::new(Mutex::new(())),
        };
        reg.load_existing()?;
        Ok(reg)
    }

    pub fn default_user() -> Result<Self> {
        let base = dirs::home_dir()
            .ok_or_else(|| VmError::Invalid("no home directory".into()))?
            .join(".cersei")
            .join("vms")
            .join("snapshots");
        Self::open(base)
    }

    fn load_existing(&self) -> Result<()> {
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            match serde_json::from_slice::<SnapshotManifest>(&bytes) {
                Ok(m) => {
                    self.cache.insert(m.id.clone(), m);
                }
                Err(e) => {
                    tracing::warn!(?path, %e, "skipping invalid snapshot manifest");
                }
            }
        }
        Ok(())
    }

    pub fn put(&self, manifest: SnapshotManifest) -> Result<()> {
        let _g = self.write_lock.lock();
        let bytes = serde_json::to_vec_pretty(&manifest)?;
        let path = self.root.join(format!("{}.json", manifest.id.as_str()));
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        self.cache.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    pub fn get(&self, id: &SnapshotId) -> Result<SnapshotManifest> {
        self.cache
            .get(id)
            .map(|kv| kv.value().clone())
            .ok_or_else(|| VmError::SnapshotNotFound(id.to_string()))
    }

    pub fn list(&self) -> Vec<SnapshotManifest> {
        self.cache.iter().map(|kv| kv.value().clone()).collect()
    }

    pub fn remove(&self, id: &SnapshotId) -> Result<()> {
        let _g = self.write_lock.lock();
        let path = self.root.join(format!("{}.json", id.as_str()));
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        self.cache.remove(id);
        Ok(())
    }
}
