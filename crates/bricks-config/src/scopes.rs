//! The desktop's memory: a global store shared by every project, and one
//! store per project, under `<bricks_home>/memory`.
//!
//! | Scope   | File                                  | Space              |
//! |---------|---------------------------------------|--------------------|
//! | global  | `memory/global.grafeo`                | `global:default`   |
//! | project | `memory/projects/<project_id>.grafeo` | `project:<id>`     |
//!
//! The project id is the desktop's, kept when a project moves. Each store
//! keeps the last name of its project next to it (`<project_id>.json`), so a
//! project removed from the list can still be named. The former single
//! store (`memory/memory.grafeo`) is left as it is.
//!
//! Stores are local and free: hashing embeddings, no extractor (exchanges
//! and notes are kept and searched; no fact is extracted).

use crate::memory::open_memories;
use bricks_agent::control::RunMemory;
use bricks_embeddings::{EmbeddingProvider, HashingEmbeddings};
use bricks_memory::MemoryScope;
use bricks_memory::structured::{
    ClearReport, EntryCounts, EntryFilter, EntryPage, MemoryConfig, MemoryError, Processing,
    ScopedMemory, SharedStore, global_space, project_space,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Dimensions of the hashing embeddings of the desktop's stores.
const DIMENSIONS: usize = 384;

/// A store of a project no longer in the list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RetiredStore {
    pub project_id: String,
    /// The last name known for it, when recorded.
    pub name: Option<String>,
}

/// The desktop's memory stores.
pub struct MemoryService {
    dir: PathBuf,
}

/// A project id usable as a file name.
fn check_project_id(id: &str) -> Result<(), MemoryError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(MemoryError::Config(format!("invalid project id `{id}`")))
    }
}

impl MemoryService {
    /// The stores under `<home>/memory`.
    pub fn in_home(home: &Path) -> Self {
        Self {
            dir: home.join("memory"),
        }
    }

    pub fn global_path(&self) -> PathBuf {
        self.dir.join("global.grafeo")
    }

    pub fn project_path(&self, project_id: &str) -> PathBuf {
        self.dir
            .join("projects")
            .join(format!("{project_id}.grafeo"))
    }

    /// Whether the former single store exists (never migrated).
    pub fn legacy_store_exists(&self) -> bool {
        self.dir.join("memory.grafeo").exists()
    }

    fn embedder() -> Arc<dyn EmbeddingProvider> {
        Arc::new(HashingEmbeddings::new(DIMENSIONS))
    }

    fn open(&self, path: &Path, space: &str) -> Result<SharedStore, MemoryError> {
        open_memories().open(
            path,
            space,
            Self::embedder(),
            &MemoryConfig::default(),
            None,
        )
    }

    pub fn global(&self) -> Result<SharedStore, MemoryError> {
        self.open(&self.global_path(), global_space())
    }

    /// The store of `project_id`; `name` is recorded as its last known name.
    pub fn project(
        &self,
        project_id: &str,
        name: Option<&str>,
    ) -> Result<SharedStore, MemoryError> {
        check_project_id(project_id)?;
        let store = self.open(&self.project_path(project_id), &project_space(project_id))?;
        if let Some(name) = name {
            self.remember_name(project_id, name);
        }
        Ok(store)
    }

    /// Record the project's name next to its store (best effort).
    fn remember_name(&self, project_id: &str, name: &str) {
        let path = self.dir.join("projects").join(format!("{project_id}.json"));
        if self.known_name(project_id).as_deref() != Some(name) {
            let _ = std::fs::write(&path, serde_json::json!({ "name": name }).to_string());
        }
    }

    /// The memory of a run starting now: the project's store (none without
    /// a project: no capture, no project search, never the global store in
    /// its place) and the global one, with the leases that keep them from
    /// being cleared until the run and its maintenance end. A store that
    /// cannot be opened is unavailable to this run, and said so.
    pub fn for_run(&self, project: Option<(&str, &str)>) -> RunMemory {
        let project_slot =
            project.map(|(id, name)| self.project(id, Some(name)).map_err(|e| e.to_string()));
        let global_slot = Some(self.global().map_err(|e| e.to_string()));
        let (memory, leases) = ScopedMemory::for_run(
            project.map(|(id, _)| id.to_string()),
            project_slot,
            global_slot,
            Processing::of(&MemoryConfig::default(), None),
        );
        RunMemory {
            memory: Some(Arc::new(memory)),
            guard: Some(Box::new(leases)),
            warnings: Vec::new(),
        }
    }

    /// The path of a scope, after checking its project id.
    fn path_of(
        &self,
        scope: MemoryScope,
        project_id: Option<&str>,
    ) -> Result<PathBuf, MemoryError> {
        match (scope, project_id) {
            (MemoryScope::Global, _) => Ok(self.global_path()),
            (MemoryScope::Project, Some(id)) => {
                check_project_id(id)?;
                Ok(self.project_path(id))
            }
            (MemoryScope::Project, None) => Err(MemoryError::Config(
                "the project memory needs a project".into(),
            )),
        }
    }

    /// The store of a scope when it exists on disk (`None`: nothing was
    /// ever remembered there; reading it creates nothing).
    fn existing(
        &self,
        scope: MemoryScope,
        project_id: Option<&str>,
    ) -> Result<Option<SharedStore>, MemoryError> {
        let path = self.path_of(scope, project_id)?;
        if !path.exists() {
            return Ok(None);
        }
        match (scope, project_id) {
            (MemoryScope::Project, Some(id)) => self.project(id, None).map(Some),
            _ => self.global().map(Some),
        }
    }

    pub fn counts(
        &self,
        scope: MemoryScope,
        project_id: Option<&str>,
    ) -> Result<EntryCounts, MemoryError> {
        match self.existing(scope, project_id)? {
            Some(s) => s.memory.counts(),
            None => Ok(EntryCounts::default()),
        }
    }

    pub fn entries(
        &self,
        scope: MemoryScope,
        project_id: Option<&str>,
        filter: EntryFilter,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<EntryPage, MemoryError> {
        match self.existing(scope, project_id)? {
            Some(s) => s.memory.entries(filter, query, offset, limit),
            None => {
                // Same validation as a store's.
                if limit == 0 || limit > bricks_memory::structured::browse::PAGE_MAX {
                    return Err(MemoryError::Config(format!("invalid page size {limit}")));
                }
                Ok(EntryPage::default())
            }
        }
    }

    /// Clear a scope's store. Refused with [`MemoryError::InUse`] while a
    /// run or a maintenance uses it. The sessions' histories are untouched.
    pub async fn delete(
        &self,
        scope: MemoryScope,
        project_id: Option<&str>,
    ) -> Result<ClearReport, MemoryError> {
        match self.existing(scope, project_id)? {
            Some(s) => s.forget_all().await,
            None => Ok(ClearReport::default()),
        }
    }

    /// The last name recorded for a project's store.
    pub fn known_name(&self, project_id: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.join("projects").join(format!("{project_id}.json")))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
    }

    /// Stores of projects not in `known` (ids), with their last names.
    pub fn retired(&self, known: &[String]) -> Vec<RetiredStore> {
        let Ok(dir) = std::fs::read_dir(self.dir.join("projects")) else {
            return Vec::new();
        };
        let mut out: Vec<RetiredStore> = dir
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let id = name.strip_suffix(".grafeo")?.to_string();
                (check_project_id(&id).is_ok() && !known.contains(&id)).then_some(id)
            })
            .map(|project_id| RetiredStore {
                name: self.known_name(&project_id),
                project_id,
            })
            .collect();
        out.sort_by(|a, b| a.project_id.cmp(&b.project_id));
        out
    }

    /// Delete the store of a project no longer in the list: cleared on its
    /// handle when it is open (refused while in use), its files removed when
    /// they are not.
    pub async fn delete_retired(&self, project_id: &str) -> Result<ClearReport, MemoryError> {
        check_project_id(project_id)?;
        let path = self.project_path(project_id);
        // Closed first (refused while in use), so its files can go and it
        // leaves the list.
        open_memories().close(&path)?;
        let io = |e: std::io::Error| MemoryError::Store(format!("{}: {e}", path.display()));
        if path.is_dir() {
            std::fs::remove_dir_all(&path).map_err(io)?;
        } else if path.exists() {
            std::fs::remove_file(&path).map_err(io)?;
        }
        let wal = PathBuf::from(format!("{}.wal", path.display()));
        if wal.exists() {
            std::fs::remove_dir_all(&wal).map_err(io)?;
        }
        let _ = std::fs::remove_file(self.dir.join("projects").join(format!("{project_id}.json")));
        Ok(ClearReport::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn projects_are_isolated_and_global_notes_keep_their_project() {
        let home = tempfile::tempdir().unwrap();
        let svc = MemoryService::in_home(home.path());
        let a = svc.for_run(Some(("pa", "Alpha")));
        let b = svc.for_run(Some(("pb", "Beta")));
        let (ma, mb) = (a.memory.unwrap(), b.memory.unwrap());
        ma.write_note(MemoryScope::Project, Some("s1"), "alpha uses postgres")
            .await
            .unwrap();
        ma.write_note(MemoryScope::Global, Some("s1"), "prefers short answers")
            .await
            .unwrap();
        assert!(
            mb.search(MemoryScope::Project, "postgres", 5)
                .await
                .unwrap()
                .is_none(),
            "isolated"
        );
        let g = mb
            .search(MemoryScope::Global, "short answers", 5)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(g.items, 1);
        let page = svc
            .entries(MemoryScope::Global, None, EntryFilter::Notes, "", 0, 50)
            .unwrap();
        assert_eq!(page.entries[0].project_id.as_deref(), Some("pa"));
        // No project: no capture, no project search, no fallback.
        let none = svc.for_run(None).memory.unwrap();
        let e = none
            .write_note(MemoryScope::Project, None, "x")
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("no project"), "{e}");
        none.record_turns(
            Some("s"),
            &[bricks_memory::MemoryTurn {
                role: "user".into(),
                content: "hello".into(),
                at: None,
            }],
        )
        .await
        .unwrap();
        assert_eq!(svc.counts(MemoryScope::Global, None).unwrap().exchanges, 0);
        // A run holds its stores: clearing waits.
        assert!(matches!(
            svc.delete(MemoryScope::Project, Some("pa")).await,
            Err(MemoryError::InUse(_))
        ));
        drop(a.guard);
        assert_eq!(
            svc.delete(MemoryScope::Project, Some("pa"))
                .await
                .unwrap()
                .episodes,
            1
        );
        assert_eq!(
            svc.retired(&["pb".into()]),
            vec![RetiredStore {
                project_id: "pa".into(),
                name: Some("Alpha".into())
            }]
        );
    }
}
