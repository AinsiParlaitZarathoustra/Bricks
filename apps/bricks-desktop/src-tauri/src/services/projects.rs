//! Registered project folders, kept in `<bricks_home>/desktop/projects.json`.
//!
//! A project's id is derived from its canonical path, so forgetting a
//! project and adding it again gives the same id, and its change journal
//! is found again. The file is only ever rewritten whole, atomically.

use super::paths::{check_folder, desktop_dir, write_atomic};
use super::{error, io_error, invalid, not_found, now_ms};
use crate::contract::{IpcError, IpcErrorCode, IpcResult, Project};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SCHEMA: u32 = 1;
const MAX_NAME_CHARS: usize = 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileDoc {
    schema: u32,
    projects: Vec<StoredProject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredProject {
    id: String,
    name: String,
    /// Canonical absolute path.
    path: String,
    added_at: i64,
    last_opened_at: i64,
}

/// The project registry. Reads happen in memory; every change writes the
/// whole file before it is reported as done.
pub struct ProjectStore {
    file: PathBuf,
    state: parking_lot::Mutex<State>,
    notes: Vec<String>,
}

struct State {
    projects: Vec<StoredProject>,
    /// Set when the file exists but cannot be read safely: nothing is
    /// written over it.
    blocked: Option<String>,
}

impl ProjectStore {
    /// The registry of this installation.
    pub fn open() -> Self {
        Self::load(desktop_dir().join("projects.json"))
    }

    /// Loads `file`. An unreadable (corrupt) file is kept under a new name
    /// and reported in [`Self::notes`]; a file of another schema, or one that
    /// cannot be read, blocks changes so nothing is overwritten.
    pub fn load(file: PathBuf) -> Self {
        let mut notes = Vec::new();
        let mut blocked = None;
        let projects = match std::fs::read(&file) {
            Ok(bytes) => match serde_json::from_slice::<FileDoc>(&bytes) {
                Ok(doc) if doc.schema == SCHEMA => doc.projects,
                Ok(doc) => {
                    let msg = format!(
                        "{} : version {} du fichier de projets non reconnue ; les projets \
                         ne sont pas modifiés",
                        file.display(),
                        doc.schema
                    );
                    blocked = Some(msg.clone());
                    notes.push(msg);
                    Vec::new()
                }
                Err(e) => {
                    let aside = file.with_extension(format!("json.unreadable-{}", now_ms()));
                    let moved = std::fs::rename(&file, &aside).is_ok();
                    let msg = format!(
                        "fichier de projets illisible ({e}) : {}",
                        if moved {
                            format!("conservé sous {}", aside.display())
                        } else {
                            format!("{} n'a pas pu être mis de côté", file.display())
                        }
                    );
                    if !moved {
                        blocked = Some(msg.clone());
                    }
                    notes.push(msg);
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                let msg = format!("fichier de projets {} illisible : {e}", file.display());
                blocked = Some(msg.clone());
                notes.push(msg);
                Vec::new()
            }
        };
        Self {
            file,
            state: parking_lot::Mutex::new(State { projects, blocked }),
            notes,
        }
    }

    /// Problems found while loading (shown in the application's
    /// configuration errors).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Every project, most recently opened first. `missing` is computed now.
    pub fn list(&self) -> Vec<Project> {
        let state = self.state.lock();
        let mut out: Vec<Project> = state.projects.iter().map(view).collect();
        out.sort_by(|a, b| {
            b.last_opened_at
                .cmp(&a.last_opened_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    pub fn get(&self, id: &str) -> IpcResult<Project> {
        let state = self.state.lock();
        state
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(view)
            .ok_or_else(|| not_found("projet inconnu"))
    }

    /// Registers a folder. The folder must exist and be readable; it is never
    /// created. Re-adding a known folder returns the existing project.
    pub fn add(&self, path: &str) -> IpcResult<Project> {
        let given = PathBuf::from(path.trim());
        if path.trim().is_empty() {
            return Err(invalid("aucun dossier indiqué"));
        }
        let canonical = std::fs::canonicalize(&given).map_err(|e| folder_missing(&given, e))?;
        check_folder(&canonical).map_err(|e| {
            error(
                IpcErrorCode::FolderMissing,
                format!("{} : {e}", canonical.display()),
            )
        })?;
        let key = canonical.display().to_string();
        let mut state = self.state.lock();
        if let Some(existing) = state.projects.iter().find(|p| p.path == key) {
            return Ok(view(existing));
        }
        let now = now_ms();
        let stored = StoredProject {
            id: project_id(&key),
            name: canonical
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| key.clone()),
            path: key,
            added_at: now,
            last_opened_at: now,
        };
        state.projects.push(stored.clone());
        if let Err(e) = self.save(&state) {
            state.projects.pop();
            return Err(e);
        }
        Ok(view(&stored))
    }

    /// Forgets a project. Its files and sessions are left untouched.
    pub fn remove(&self, id: &str) -> IpcResult<()> {
        let mut state = self.state.lock();
        let Some(index) = state.projects.iter().position(|p| p.id == id) else {
            return Err(not_found("projet inconnu"));
        };
        let removed = state.projects.remove(index);
        if let Err(e) = self.save(&state) {
            state.projects.insert(index, removed);
            return Err(e);
        }
        Ok(())
    }

    pub fn rename(&self, id: &str, name: &str) -> IpcResult<Project> {
        let name = name.trim();
        if name.is_empty() {
            return Err(invalid("le nom ne peut pas être vide"));
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(invalid(format!(
                "le nom dépasse {MAX_NAME_CHARS} caractères"
            )));
        }
        let mut state = self.state.lock();
        let Some(index) = state.projects.iter().position(|p| p.id == id) else {
            return Err(not_found("projet inconnu"));
        };
        let previous = std::mem::replace(&mut state.projects[index].name, name.to_string());
        if let Err(e) = self.save(&state) {
            state.projects[index].name = previous;
            return Err(e);
        }
        Ok(view(&state.projects[index]))
    }

    /// Records that a project was opened (orders the list).
    pub fn touch(&self, id: &str) {
        let mut state = self.state.lock();
        if let Some(p) = state.projects.iter_mut().find(|p| p.id == id) {
            p.last_opened_at = now_ms();
        }
        let _ = self.save(&state);
    }

    fn save(&self, state: &State) -> IpcResult<()> {
        if let Some(why) = &state.blocked {
            return Err(error(IpcErrorCode::Io, why.clone()));
        }
        let doc = FileDoc {
            schema: SCHEMA,
            projects: state.projects.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&doc)
            .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))?;
        write_atomic(&self.file, &bytes)
            .map_err(|e| io_error(&format!("projets ({})", self.file.display()), e))
    }
}

fn folder_missing(path: &Path, e: std::io::Error) -> IpcError {
    error(
        IpcErrorCode::FolderMissing,
        format!("{} : {e}", path.display()),
    )
}

fn view(p: &StoredProject) -> Project {
    Project {
        id: p.id.clone(),
        name: p.name.clone(),
        path: p.path.clone(),
        added_at: p.added_at,
        last_opened_at: p.last_opened_at,
        missing: check_folder(Path::new(&p.path)).is_err(),
    }
}

/// `p` followed by 32 hex digits: two 64-bit FNV-1a hashes of the canonical
/// path. An identifier, not a security measure, stable across builds.
pub(crate) fn project_id(canonical: &str) -> String {
    fn fnv(bytes: &[u8], seed: u64) -> u64 {
        let mut h = seed;
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
    let bytes = canonical.as_bytes();
    format!(
        "p{:016x}{:016x}",
        fnv(bytes, 0xcbf2_9ce4_8422_2325),
        fnv(bytes, 0x8422_2325_cbf2_9ce4)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> ProjectStore {
        ProjectStore::load(dir.join("desktop").join("projects.json"))
    }

    #[test]
    fn a_folder_is_added_once_by_its_canonical_path() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("atlas");
        std::fs::create_dir(&folder).unwrap();
        let s = store(root.path());
        let first = s.add(folder.to_str().unwrap()).unwrap();
        // The same folder, named differently (a `.` segment), is the same project.
        let alias = format!("{}/./", folder.display());
        let second = s.add(&alias).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(s.list().len(), 1);
        assert!(!first.missing);
        assert_eq!(first.name, "atlas");
    }

    #[test]
    fn a_missing_folder_is_refused_and_never_created() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        let absent = root.path().join("absent");
        let err = s.add(absent.to_str().unwrap()).unwrap_err();
        assert_eq!(err.code, IpcErrorCode::FolderMissing);
        assert!(!absent.exists());
        let file = root.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            s.add(file.to_str().unwrap()).unwrap_err().code,
            IpcErrorCode::FolderMissing
        );
        assert_eq!(s.add("  ").unwrap_err().code, IpcErrorCode::Invalid);
    }

    #[test]
    fn projects_survive_a_restart_and_report_a_vanished_folder() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("atlas");
        std::fs::create_dir(&folder).unwrap();
        let id = store(root.path()).add(folder.to_str().unwrap()).unwrap().id;
        let reopened = store(root.path());
        assert_eq!(reopened.get(&id).unwrap().path, folder.canonicalize().unwrap().display().to_string());
        std::fs::remove_dir(&folder).unwrap();
        assert!(reopened.list()[0].missing, "computed on list");
    }

    #[test]
    fn rename_and_remove_report_unknown_projects_and_validate_names() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("atlas");
        std::fs::create_dir(&folder).unwrap();
        let s = store(root.path());
        let id = s.add(folder.to_str().unwrap()).unwrap().id;
        assert_eq!(s.rename(&id, "  Nouveau  ").unwrap().name, "Nouveau");
        assert_eq!(s.rename(&id, " ").unwrap_err().code, IpcErrorCode::Invalid);
        assert_eq!(
            s.rename("p0", "x").unwrap_err().code,
            IpcErrorCode::NotFound
        );
        s.remove(&id).unwrap();
        assert!(s.list().is_empty());
        assert_eq!(s.remove(&id).unwrap_err().code, IpcErrorCode::NotFound);
    }

    #[test]
    fn a_corrupt_registry_is_kept_aside_and_reported() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("desktop").join("projects.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{ not json").unwrap();
        let s = ProjectStore::load(file.clone());
        assert_eq!(s.notes().len(), 1);
        assert!(s.notes()[0].contains("conservé"), "{:?}", s.notes());
        assert!(!file.exists());
        assert!(s.list().is_empty());
    }

    #[test]
    fn ids_are_stable_and_distinct_per_path() {
        assert_eq!(project_id("/a"), project_id("/a"));
        assert_ne!(project_id("/a"), project_id("/b"));
        assert_eq!(project_id("/a").len(), 33);
    }
}
