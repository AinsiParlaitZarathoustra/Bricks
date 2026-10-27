//! The stores a process has open, one handle per file, and who uses them.
//!
//! [`OpenMemories`] opens each store once per canonical path; opening is
//! serialised, so two sessions asking at the same time share one handle. A
//! second user must ask for the same identity — space, embedding model and
//! dimensions (the schema version is the code's, checked at open) — or gets
//! an error naming the file, the parameter and both values. Processing
//! parameters (extractor, extraction limits, recall budgets) are not part of
//! the identity: each user passes its own ([`super::Processing`]).
//!
//! A run or a maintenance holds a [`Lease`] on each store it uses.
//! [`SharedStore::forget_all`] is refused while a lease is held, and no lease
//! is granted while it runs: the check and the start of the deletion are one
//! step for both sides.

use super::{MemoryConfig, MemoryError, MemoryResult, StructuredMemory};
use bricks_embeddings::EmbeddingProvider;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Users of a store, and whether it is being cleared.
#[derive(Debug, Default)]
struct Usage {
    leases: usize,
    clearing: bool,
}

/// One open store.
#[derive(Clone)]
pub struct SharedStore {
    pub memory: Arc<StructuredMemory>,
    path: PathBuf,
    space: String,
    usage: Arc<parking_lot::Mutex<Usage>>,
}

/// A use of a store by a run or a maintenance; released when dropped.
pub struct Lease(Arc<parking_lot::Mutex<Usage>>);

impl Drop for Lease {
    fn drop(&mut self) {
        let mut u = self.0.lock();
        u.leases = u.leases.saturating_sub(1);
    }
}

impl SharedStore {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn space(&self) -> &str {
        &self.space
    }

    /// Start using the store; refused while it is being cleared.
    pub fn lease(&self) -> MemoryResult<Lease> {
        let mut u = self.usage.lock();
        if u.clearing {
            return Err(MemoryError::InUse(format!(
                "{} is being cleared",
                self.path.display()
            )));
        }
        u.leases += 1;
        Ok(Lease(Arc::clone(&self.usage)))
    }

    /// Whether a run or a maintenance uses the store now.
    pub fn in_use(&self) -> bool {
        self.usage.lock().leases > 0
    }

    /// Clear everything the store remembers, on the shared handle (see
    /// [`StructuredMemory::forget_all`]). Refused with
    /// [`MemoryError::InUse`] while a run or a maintenance uses it.
    pub async fn forget_all(&self) -> MemoryResult<super::ClearReport> {
        {
            let mut u = self.usage.lock();
            if u.leases > 0 || u.clearing {
                return Err(MemoryError::InUse(format!(
                    "{} is in use by a run or a maintenance",
                    self.path.display()
                )));
            }
            u.clearing = true;
        }
        let result = self.memory.forget_all().await;
        self.usage.lock().clearing = false;
        result
    }
}

/// The stores this process has open, by canonical path.
#[derive(Default)]
pub struct OpenMemories {
    stores: parking_lot::Mutex<HashMap<PathBuf, SharedStore>>,
}

/// `path` with its folder made canonical (the folder is created; the file
/// may not exist yet).
fn canonical(path: &Path) -> MemoryResult<PathBuf> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| MemoryError::Store(format!("{}: {e}", parent.display())))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| MemoryError::Store(format!("{}: {e}", parent.display())))?;
    let name = path
        .file_name()
        .ok_or_else(|| MemoryError::Config(format!("{} is not a file", path.display())))?;
    Ok(parent.join(name))
}

impl OpenMemories {
    /// The store at `path` for `space`, opened now or shared. `config`
    /// gives the defaults of a store opened here (its space is replaced by
    /// `space`); a store already open keeps its own.
    pub fn open(
        &self,
        path: &Path,
        space: &str,
        embedder: Arc<dyn EmbeddingProvider>,
        config: &MemoryConfig,
        extractor: Option<Arc<dyn super::Extractor>>,
    ) -> MemoryResult<SharedStore> {
        let key = canonical(path)?;
        let mut stores = self.stores.lock();
        if let Some(open) = stores.get(&key) {
            let (model, dims) = open.memory.embedding_identity();
            let conflict = |param: &str, active: &str, asked: &str| {
                MemoryError::Config(format!(
                    "{}: {param} is `{active}` for the store already open, `{asked}` requested",
                    key.display()
                ))
            };
            if open.space != space {
                return Err(conflict("the space", &open.space, space));
            }
            if model != embedder.model_id() {
                return Err(conflict(
                    "the embedding model",
                    &model,
                    &embedder.model_id(),
                ));
            }
            if dims != embedder.dimensions() {
                return Err(conflict(
                    "the embedding dimensions",
                    &dims.to_string(),
                    &embedder.dimensions().to_string(),
                ));
            }
            return Ok(open.clone());
        }
        let config = config.clone().with_space(space);
        let mut builder = StructuredMemory::builder(embedder)
            .path(&key)
            .config(config);
        if let Some(x) = extractor {
            builder = builder.extractor(x);
        }
        let memory = builder.open().map_err(|e| match e {
            MemoryError::Store(m) => MemoryError::Store(format!("{}: {m}", key.display())),
            other => other,
        })?;
        let shared = SharedStore {
            memory: Arc::new(memory),
            path: key.clone(),
            space: space.to_string(),
            usage: Arc::default(),
        };
        stores.insert(key, shared.clone());
        Ok(shared)
    }

    /// Close the store at `path` so its files can go: refused with
    /// [`MemoryError::InUse`] while a run uses it or anything else still
    /// holds its handle. Nothing to do when it is not open.
    pub fn close(&self, path: &Path) -> MemoryResult<()> {
        let key = canonical(path)?;
        let mut stores = self.stores.lock();
        let Some(open) = stores.get(&key) else {
            return Ok(());
        };
        if open.in_use() || Arc::strong_count(&open.memory) > 1 {
            return Err(MemoryError::InUse(format!("{} is in use", key.display())));
        }
        let open = stores.remove(&key).expect("present");
        open.memory.close()
    }

    /// The store at `path`, when open.
    pub fn get(&self, path: &Path) -> Option<SharedStore> {
        let key = canonical(path).ok()?;
        self.stores.lock().get(&key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::GLOBAL_SPACE;
    use bricks_embeddings::HashingEmbeddings;

    fn hashing(dims: usize) -> Arc<dyn EmbeddingProvider> {
        Arc::new(HashingEmbeddings::new(dims))
    }

    #[test]
    fn one_handle_per_file_and_conflicts_are_named() {
        let dir = tempfile::tempdir().unwrap();
        let open = OpenMemories::default();
        let cfg = MemoryConfig::default();
        let path = dir.path().join("memory").join("global.grafeo");
        let a = open
            .open(&path, GLOBAL_SPACE, hashing(64), &cfg, None)
            .unwrap();
        // Another spelling of the same file.
        let other = dir.path().join("memory").join(".").join("global.grafeo");
        let b = open
            .open(&other, GLOBAL_SPACE, hashing(64), &cfg, None)
            .unwrap();
        assert!(Arc::ptr_eq(&a.memory, &b.memory));
        let e = open
            .open(&path, "project:p1", hashing(64), &cfg, None)
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("global.grafeo") && e.contains("space"), "{e}");
        assert!(e.contains(GLOBAL_SPACE) && e.contains("project:p1"), "{e}");
        let e = open
            .open(&path, GLOBAL_SPACE, hashing(32), &cfg, None)
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("embedding"), "{e}");
    }

    #[tokio::test]
    async fn clearing_waits_for_no_user_and_blocks_new_ones() {
        let dir = tempfile::tempdir().unwrap();
        let open = OpenMemories::default();
        let s = open
            .open(
                &dir.path().join("p.grafeo"),
                "project:p",
                hashing(64),
                &MemoryConfig::default(),
                None,
            )
            .unwrap();
        let lease = s.lease().unwrap();
        assert!(matches!(s.forget_all().await, Err(MemoryError::InUse(_))));
        drop(lease);
        assert!(!s.in_use());
        s.forget_all().await.unwrap();
        s.lease().unwrap();
    }
}
