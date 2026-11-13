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
//! Embedders and lightweight fact extractors are resolved from the user's
//! configured providers (`providers.toml` / environment), falling back
//! explicitly to local `HashingEmbeddings` when offline or unconfigured. Project
//! `[memory]` overrides in `bricks.toml` are respected when present.

use crate::memory::{embedder_with_registry, open_memories, resolve_auto_embedder, resolve_extractor};
use bricks_agent::control::RunMemory;
use bricks_embeddings::{EmbeddingProvider, HashingEmbeddings};
use bricks_memory::MemoryScope;
use bricks_memory::structured::{
    ClearReport, EntryCounts, EntryFilter, EntryPage, Extractor, MemoryConfig, MemoryError,
    MemoryToml, MismatchMode, Processing, ScopedMemory, SharedStore, global_space, project_space,
};
use bricks_provider::ProviderRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Dimensions of the hashing embeddings fallback of the desktop's stores.
const DIMENSIONS: usize = 384;

type RegistrySource = Arc<dyn Fn() -> Option<Arc<ProviderRegistry>> + Send + Sync>;

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
    registry: Option<RegistrySource>,
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
            registry: None,
        }
    }

    /// Attach a provider registry lookup so desktop runs and store operations
    /// use configured embedding and extraction models.
    pub fn with_registry(
        mut self,
        f: impl Fn() -> Option<Arc<ProviderRegistry>> + Send + Sync + 'static,
    ) -> Self {
        self.registry = Some(Arc::new(f));
        self
    }

    fn current_registry(&self) -> Option<Arc<ProviderRegistry>> {
        self.registry.as_ref().and_then(|f| f())
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

    fn hashing_embedder() -> Arc<dyn EmbeddingProvider> {
        Arc::new(HashingEmbeddings::new(DIMENSIONS))
    }

    fn read_project_toml(project_dir: Option<&Path>) -> (Option<MemoryToml>, Vec<String>) {
        let Some(dir) = project_dir else {
            return (None, Vec::new());
        };
        let path = dir.join("bricks.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (None, Vec::new());
        };
        match MemoryConfig::parse_bricks_toml(&text) {
            Ok(parsed) => (Some(parsed), Vec::new()),
            Err(e) => (
                None,
                vec![format!(
                    "{}: invalid [memory] configuration ({e}); using desktop defaults",
                    path.display()
                )],
            ),
        }
    }

    fn resolve_runtime(
        &self,
        project_dir: Option<&Path>,
    ) -> (
        MemoryConfig,
        Option<MemoryToml>,
        Arc<dyn EmbeddingProvider>,
        Option<Arc<dyn Extractor>>,
        bool,
        Vec<String>,
    ) {
        let (parsed, mut warnings) = Self::read_project_toml(project_dir);
        let mut config = parsed
            .as_ref()
            .map(|t| t.config.clone())
            .unwrap_or_default();
        config.enabled = true;

        let registry = self.current_registry();
        let explicit_embeddings = parsed.as_ref().is_some_and(|t| t.explicit_embeddings);
        let embedder = if explicit_embeddings {
            match embedder_with_registry(&config.embeddings, registry.as_deref()) {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!(
                        "{e}; falling back to local hashing/lexical memory"
                    ));
                    Self::hashing_embedder()
                }
            }
        } else {
            let (e, _is_fallback) = resolve_auto_embedder(registry.as_deref(), DIMENSIONS);
            e
        };

        let explicit_extractor = parsed
            .as_ref()
            .filter(|t| t.explicit_extractor_model)
            .and_then(|t| t.config.extractor_model.as_deref());
        let extractor =
            match resolve_extractor(explicit_extractor, registry.as_deref(), &config.extraction) {
                Ok(ext) => ext,
                Err(e) => {
                    warnings.push(format!("{e}; fact extraction disabled for this run"));
                    None
                }
            };

        (
            config,
            parsed,
            embedder,
            extractor,
            explicit_embeddings,
            warnings,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_store(
        &self,
        path: &Path,
        space: &str,
        embedder: Arc<dyn EmbeddingProvider>,
        config: &MemoryConfig,
        extractor: Option<Arc<dyn Extractor>>,
        explicit_embeddings: bool,
        allow_rebuild: bool,
        warnings: &mut Vec<String>,
    ) -> Result<SharedStore, MemoryError> {
        match open_memories().open_with_mode(
            path,
            space,
            Arc::clone(&embedder),
            config,
            extractor.clone(),
            MismatchMode::Error,
        ) {
            Ok(s) => Ok(s),
            Err(MemoryError::EmbeddingMismatch { stored, configured }) => {
                let is_hashing = |id: &str| id.starts_with("hashing");
                let upgrading_from_hashing = is_hashing(&stored) && !is_hashing(&configured);
                let mode = if allow_rebuild && (upgrading_from_hashing || explicit_embeddings) {
                    warnings.push(format!(
                        "upgrading memory store `{}` embeddings from `{stored}` to `{configured}`",
                        path.display()
                    ));
                    MismatchMode::Rebuild
                } else {
                    warnings.push(format!(
                        "memory store `{}` uses `{stored}` embeddings while `{configured}` is active; \
                         keeping existing vectors untouched and using lexical recall",
                        path.display()
                    ));
                    MismatchMode::LexicalFallback
                };
                open_memories().open_with_mode(path, space, embedder, config, extractor, mode)
            }
            Err(e) => Err(e),
        }
    }

    pub fn global(&self) -> Result<SharedStore, MemoryError> {
        let (config, _, embedder, extractor, explicit_embeddings, mut warnings) =
            self.resolve_runtime(None);
        self.open_store(
            &self.global_path(),
            global_space(),
            embedder,
            &config,
            extractor,
            explicit_embeddings,
            true,
            &mut warnings,
        )
    }

    /// The store of `project_id`; `name` is recorded as its last known name.
    pub fn project(
        &self,
        project_id: &str,
        name: Option<&str>,
    ) -> Result<SharedStore, MemoryError> {
        check_project_id(project_id)?;
        let (config, _, embedder, extractor, explicit_embeddings, mut warnings) =
            self.resolve_runtime(None);
        let store = self.open_store(
            &self.project_path(project_id),
            &project_space(project_id),
            embedder,
            &config,
            extractor,
            explicit_embeddings,
            true,
            &mut warnings,
        )?;
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
        self.for_run_in_project(project.map(|(id, name)| (id, name, None)))
    }

    /// Like [`Self::for_run`], also reading `[memory]` overrides from the
    /// project's `bricks.toml` when `project_dir` is provided.
    pub fn for_run_in_project(
        &self,
        project: Option<(&str, &str, Option<&Path>)>,
    ) -> RunMemory {
        let project_dir = project.and_then(|(_, _, dir)| dir);
        let (config, parsed, embedder, extractor, explicit_embeddings, mut warnings) =
            self.resolve_runtime(project_dir);

        if parsed.as_ref().and_then(|t| t.explicit_enabled) == Some(false) {
            return RunMemory {
                memory: None,
                guard: None,
                warnings,
            };
        }

        let project_slot = project.map(|(id, name, _)| {
            check_project_id(id)
                .and_then(|()| {
                    let store = self.open_store(
                        &self.project_path(id),
                        &project_space(id),
                        Arc::clone(&embedder),
                        &config,
                        extractor.clone(),
                        explicit_embeddings,
                        true,
                        &mut warnings,
                    )?;
                    self.remember_name(id, name);
                    Ok(store)
                })
                .map_err(|e| e.to_string())
        });
        let global_slot = Some(
            self.open_store(
                &self.global_path(),
                global_space(),
                embedder,
                &config,
                extractor.clone(),
                false,
                true,
                &mut warnings,
            )
            .map_err(|e| e.to_string()),
        );
        let (memory, leases) = ScopedMemory::for_run(
            project.map(|(id, _, _)| id.to_string()),
            project_slot,
            global_slot,
            Processing::of(&config, extractor),
        );
        RunMemory {
            memory: Some(Arc::new(memory)),
            guard: Some(Box::new(leases)),
            warnings,
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
        let (config, _, embedder, extractor, _, mut warnings) = self.resolve_runtime(None);
        let space = match (scope, project_id) {
            (MemoryScope::Project, Some(id)) => project_space(id),
            _ => global_space().to_string(),
        };
        self.open_store(
            &path,
            &space,
            embedder,
            &config,
            extractor,
            false,
            false,
            &mut warnings,
        )
        .map(Some)
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

    #[tokio::test]
    async fn desktop_service_uses_registry_providers_and_merges_project_toml() {
        let home = tempfile::tempdir().unwrap();
        let proj_dir = tempfile::tempdir().unwrap();
        let reg = Arc::new(
            ProviderRegistry::from_toml_str(
                r#"
schema_version = 1

[[providers]]
id = "openai"
name = "OpenAI"
endpoint = "https://api.openai.com/v1"
protocol = "responses"
auth = "bearer"
api_key = "sk-test-key"

[[providers.models]]
id = "big"
name = "GPT Big"
api_model = "gpt-5"
limits = { max_input_tokens = 128000, max_output_tokens = 8192 }

[[providers.models]]
id = "mini"
name = "GPT Mini"
api_model = "gpt-5-mini"
limits = { max_input_tokens = 128000, max_output_tokens = 8192 }
"#,
                "test.toml",
            )
            .unwrap(),
        );

        let svc = MemoryService::in_home(home.path()).with_registry({
            let reg = Arc::clone(&reg);
            move || Some(Arc::clone(&reg))
        });
        let run = svc.for_run_in_project(Some(("p1", "Project One", Some(proj_dir.path()))));
        assert!(run.memory.is_some());
        drop(run.guard);
        drop(run.memory);

        // Verify the project store recorded OpenAI's embedding model (not hashing).
        let stored = svc.project("p1", None).unwrap();
        assert_eq!(
            stored.memory.stored_embedding_model(),
            "openai/text-embedding-3-small:1536"
        );
        drop(stored);

        // Now simulate offline / no-registry service opening the existing semantic store:
        // it must not overwrite stored metadata/vectors and should fall back lexically.
        let offline = MemoryService::in_home(home.path());
        let off_run = offline.for_run(Some(("p1", "Project One")));
        assert!(off_run.memory.is_some());
        assert!(
            off_run
                .warnings
                .iter()
                .any(|w| w.contains("lexical recall")),
            "expected lexical fallback warning, got {:?}",
            off_run.warnings
        );
        drop(off_run.guard);
        drop(off_run.memory);

        // Re-check project TOML: `[memory]` without `enabled = false` keeps memory active,
        // while explicit `enabled = false` disables memory for that project.
        std::fs::write(
            proj_dir.path().join("bricks.toml"),
            "[memory]\n[memory.recall]\nscoped_max_results = 9\n",
        )
        .unwrap();
        let active_toml = svc.for_run_in_project(Some(("p1", "Project One", Some(proj_dir.path()))));
        assert!(active_toml.memory.is_some());
        drop(active_toml.guard);
        drop(active_toml.memory);

        std::fs::write(
            proj_dir.path().join("bricks.toml"),
            "[memory]\nenabled = false\n",
        )
        .unwrap();
        let disabled_toml =
            svc.for_run_in_project(Some(("p1", "Project One", Some(proj_dir.path()))));
        assert!(disabled_toml.memory.is_none());

        // Upgrading an existing hashing store to a semantic provider succeeds cleanly.
        let p2_offline = offline.for_run(Some(("p2", "Project Two")));
        p2_offline
            .memory
            .as_ref()
            .unwrap()
            .write_note(MemoryScope::Project, Some("s1"), "legacy hashing note")
            .await
            .unwrap();
        drop(p2_offline.guard);
        drop(p2_offline.memory);
        let p2_upgraded = svc.for_run(Some(("p2", "Project Two")));
        assert!(p2_upgraded.memory.is_some());
        assert!(
            p2_upgraded
                .warnings
                .iter()
                .any(|w| w.contains("upgrading memory store")),
            "expected upgrade warning, got {:?}",
            p2_upgraded.warnings
        );
        drop(p2_upgraded.guard);
        drop(p2_upgraded.memory);
        let p2_store = svc.project("p2", None).unwrap();
        assert_eq!(
            p2_store.memory.stored_embedding_model(),
            "openai/text-embedding-3-small:1536"
        );
    }
}
