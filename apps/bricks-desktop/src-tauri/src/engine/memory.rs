//! The long-term memory of the desktop: each run's memory, from the
//! preference and the session's project as they are when it starts, and
//! the reading and clearing of the stores for the settings.
//!
//! The preference (`memory_enabled`) is the only switch: the `[memory]`
//! section of a project's `bricks.toml` is not read here. Reading and
//! clearing work whatever the preference.

use super::AppState;
use crate::contract::*;
use crate::services::error;
use crate::services::prefs::PrefsStore;
use crate::services::projects::ProjectStore;
use bricks_agent::control::{MemoryResolver, RunMemory};
use bricks_config::MemoryService;
use bricks_memory::MemoryScope as Scope;
use bricks_memory::structured::{EntryFilter, EntryKind, MemoryError, StoredEntry};
use std::path::Path;
use std::sync::Arc;

/// The memory of each run of one session of `project_id`.
pub(crate) struct DesktopMemory {
    pub service: Arc<MemoryService>,
    pub prefs: Arc<PrefsStore>,
    pub projects: Arc<ProjectStore>,
    pub project_id: String,
}

impl MemoryResolver for DesktopMemory {
    fn resolve(&self, _session_id: &str) -> RunMemory {
        if !self.prefs.get().memory_enabled {
            return RunMemory::default();
        }
        // The conversations of no project have no project memory.
        let project = self
            .projects
            .get(&self.project_id)
            .ok()
            .filter(|p| !p.chats);
        self.service
            .for_run(project.as_ref().map(|p| (p.id.as_str(), p.name.as_str())))
    }
}

fn scope(s: MemoryScope) -> Scope {
    match s {
        MemoryScope::Project => Scope::Project,
        MemoryScope::Global => Scope::Global,
    }
}

fn filter(f: MemoryEntryFilter) -> EntryFilter {
    match f {
        MemoryEntryFilter::All => EntryFilter::All,
        MemoryEntryFilter::Notes => EntryFilter::Notes,
        MemoryEntryFilter::Exchanges => EntryFilter::Exchanges,
        MemoryEntryFilter::Facts => EntryFilter::Facts,
        MemoryEntryFilter::FactHistory => EntryFilter::FactHistory,
    }
}

fn ipc_error(e: MemoryError) -> IpcError {
    match e {
        MemoryError::InUse(_) => error(
            IpcErrorCode::SessionBusy,
            "La mémoire est utilisée par une exécution en cours : attendez sa fin ou arrêtez-la, puis recommencez.",
        ),
        MemoryError::Config(m) => error(IpcErrorCode::Invalid, m),
        other => error(IpcErrorCode::Io, other.to_string()),
    }
}

fn counts(c: bricks_memory::structured::EntryCounts) -> MemoryCounts {
    let n = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
    MemoryCounts {
        notes: n(c.notes),
        exchanges: n(c.exchanges),
        facts: n(c.facts),
    }
}

/// Whether the `bricks.toml` of `folder` has a `[memory]` section.
fn toml_has_memory(folder: &Path) -> bool {
    std::fs::read_to_string(folder.join("bricks.toml")).is_ok_and(|t| {
        t.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with("[memory]") || l.starts_with("[memory.")
        })
    })
}

impl AppState {
    /// The project id a memory command uses: none for the conversations of
    /// no project.
    fn memory_project(&self, project_id: Option<&str>) -> IpcResult<Option<String>> {
        match project_id {
            None => Ok(None),
            Some(id) => {
                let p = self.projects.get(id)?;
                Ok((!p.chats).then_some(p.id))
            }
        }
    }

    /// The project id of a scope: a project scope needs a project.
    fn scoped_project(
        &self,
        s: MemoryScope,
        project_id: Option<&str>,
    ) -> IpcResult<Option<String>> {
        let project = self.memory_project(project_id)?;
        if s == MemoryScope::Project && project.is_none() {
            return Err(error(IpcErrorCode::Invalid, "aucun projet ouvert"));
        }
        Ok(project)
    }

    pub async fn memory_overview(&self, project_id: Option<String>) -> IpcResult<MemoryOverview> {
        let project = self.memory_project(project_id.as_deref())?;
        let folder = match &project {
            Some(id) => Some(self.projects.get(id)?.path),
            None => None,
        };
        let known: Vec<String> = self.projects.list().into_iter().map(|p| p.id).collect();
        let service = Arc::clone(&self.memory);
        tokio::task::spawn_blocking(move || {
            let global = service.counts(Scope::Global, None).map_err(ipc_error)?;
            let project_counts = match &project {
                Some(id) => Some(counts(
                    service
                        .counts(Scope::Project, Some(id))
                        .map_err(ipc_error)?,
                )),
                None => None,
            };
            Ok(MemoryOverview {
                global: counts(global),
                project: project_counts,
                legacy_store: service.legacy_store_exists(),
                toml_section_ignored: folder.is_some_and(|f| toml_has_memory(Path::new(&f))),
                retired: service
                    .retired(&known)
                    .into_iter()
                    .map(|r| RetiredMemory {
                        project_id: r.project_id,
                        name: r.name,
                    })
                    .collect(),
            })
        })
        .await
        .map_err(crate::services::join_error)?
    }

    pub async fn memory_counts(
        &self,
        s: MemoryScope,
        project_id: Option<String>,
    ) -> IpcResult<MemoryCounts> {
        let project = self.scoped_project(s, project_id.as_deref())?;
        let service = Arc::clone(&self.memory);
        tokio::task::spawn_blocking(move || {
            service
                .counts(scope(s), project.as_deref())
                .map(counts)
                .map_err(ipc_error)
        })
        .await
        .map_err(crate::services::join_error)?
    }

    pub async fn memory_entries(
        &self,
        s: MemoryScope,
        project_id: Option<String>,
        f: MemoryEntryFilter,
        query: String,
        offset: u32,
        limit: u32,
    ) -> IpcResult<MemoryEntryPage> {
        if !(1..=50).contains(&limit) {
            return Err(error(
                IpcErrorCode::Invalid,
                "une page compte de 1 à 50 entrées",
            ));
        }
        let project = self.scoped_project(s, project_id.as_deref())?;
        let names: std::collections::HashMap<String, String> = self
            .projects
            .list()
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect();
        let service = Arc::clone(&self.memory);
        tokio::task::spawn_blocking(move || {
            let page = service
                .entries(
                    scope(s),
                    project.as_deref(),
                    filter(f),
                    &query,
                    offset as usize,
                    limit as usize,
                )
                .map_err(ipc_error)?;
            let entry = |e: StoredEntry| MemoryEntry {
                project_name: e
                    .project_id
                    .as_ref()
                    .and_then(|id| names.get(id).cloned().or_else(|| service.known_name(id))),
                id: e.id,
                kind: match e.kind {
                    EntryKind::Note => MemoryEntryKind::Note,
                    EntryKind::Exchange => MemoryEntryKind::Exchange,
                    EntryKind::Fact => MemoryEntryKind::Fact,
                },
                text: e.text,
                at: e.at,
                role: e.role,
                project_id: e.project_id,
                session_id: e.session_id,
                status: e.status.map(|s| s.as_str().to_string()),
                evidence: e
                    .evidence
                    .into_iter()
                    .map(|v| MemoryEvidence {
                        quote: v.quote,
                        role: v.role,
                        session_id: v.session_id,
                        occurred_at: v.occurred_at,
                    })
                    .collect(),
            };
            Ok(MemoryEntryPage {
                total: u32::try_from(page.total).unwrap_or(u32::MAX),
                entries: page.entries.into_iter().map(entry).collect(),
            })
        })
        .await
        .map_err(crate::services::join_error)?
    }

    /// Clears a scope's store; refused while a run or a maintenance uses it.
    pub async fn memory_delete(&self, s: MemoryScope, project_id: Option<String>) -> IpcResult<()> {
        let project = self.scoped_project(s, project_id.as_deref())?;
        self.memory
            .delete(scope(s), project.as_deref())
            .await
            .map(|_| ())
            .map_err(ipc_error)
    }

    /// Deletes the store of a project no longer in the list.
    pub async fn memory_delete_retired(&self, project_id: String) -> IpcResult<()> {
        if self.projects.get(&project_id).is_ok() {
            return Err(error(
                IpcErrorCode::Invalid,
                "ce projet est encore dans la liste",
            ));
        }
        self.memory
            .delete_retired(&project_id)
            .await
            .map(|_| ())
            .map_err(ipc_error)
    }
}
