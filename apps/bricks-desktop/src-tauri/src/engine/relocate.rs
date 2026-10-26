//! Moving a project to another folder (its folder was moved or renamed).
//!
//! The project keeps its id; its stored conversations are moved with it:
//! each one's recorded working folder (`SessionMeta::working_dir`) is
//! rewritten, since a resume works in that folder. The steps are written
//! first to a journal (`<home>/desktop/relocations/<project id>.json`), so a
//! move interrupted between files is finished or undone at the next start.
//! File paths inside the transcripts are history and are never rewritten.

use super::live::files_dir;
use super::{AppState, error};
use crate::contract::{IpcErrorCode, IpcResult, Project};
use crate::services::paths::write_atomic;
use crate::services::{invalid, io_error, join_error};
use bricks_agent::control::{SessionMeta, list_sessions, same_workspace};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const JOURNAL_SCHEMA: u32 = 1;

/// A move in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    schema: u32,
    project_id: String,
    from: PathBuf,
    to: PathBuf,
    /// The conversations recorded in `from` when the move started.
    sessions: Vec<String>,
}

/// Whether a conversation recorded in `recorded` belongs to `folder`: the same
/// path as recorded (a vanished folder cannot be resolved), or the same
/// folder once canonical. Used to attribute conversations, never to grant
/// access to files.
pub(crate) fn recorded_in(recorded: &Path, folder: &Path) -> bool {
    recorded == folder || same_workspace(recorded, folder)
}

impl AppState {
    /// Moves a project to `path`. Refused while one of its conversations
    /// runs; its open conversations are closed first: opened again, they work
    /// in the new folder (with that folder's settings).
    pub async fn relocate_project(&self, project_id: &str, path: &str) -> IpcResult<Project> {
        let _gate = self.lifecycle.lock().await;
        let project = self.projects.get(project_id)?;
        if project.chats {
            return Err(invalid(
                "les conversations sans projet n'ont pas de dossier à déplacer",
            ));
        }
        let to = self.projects.relocation_target(project_id, path)?;
        let from = PathBuf::from(&project.path);
        if to == from {
            return Ok(project);
        }
        let open = self.entries_of_project(project_id);
        if open.iter().any(|e| e.is_busy()) {
            return Err(error(
                IpcErrorCode::SessionBusy,
                "une conversation de ce projet est en cours : annulez-la ou attendez sa fin",
            ));
        }
        for entry in &open {
            self.retire(entry).await;
        }
        let sessions: Vec<String> = list_sessions(self.sessions_dir())
            .await
            .map_err(|e| io_error("conversations", e))?
            .into_iter()
            .filter(|s| {
                s.working_dir
                    .as_deref()
                    .is_some_and(|d| recorded_in(d, &from))
            })
            .map(|s| s.id)
            .collect();
        let journal = Journal {
            schema: JOURNAL_SCHEMA,
            project_id: project_id.to_string(),
            from,
            to,
            sessions,
        };
        let journal_file = journal_path(&self.relocations_dir(), project_id);
        let sessions_dir = self.sessions_dir().to_path_buf();
        let projects = std::sync::Arc::clone(&self.projects);
        tokio::task::spawn_blocking(move || -> IpcResult<Project> {
            let bytes = serde_json::to_vec_pretty(&journal)
                .map_err(|e| error(IpcErrorCode::Internal, e.to_string()))?;
            write_atomic(&journal_file, &bytes)
                .map_err(|e| io_error("journal du déplacement", e))?;
            let moved = rewrite(&sessions_dir, &journal.sessions, &journal.from, &journal.to)
                .and_then(|()| projects.relocate(&journal.project_id, &journal.to));
            match moved {
                Ok(project) => {
                    let _ = std::fs::remove_file(&journal_file);
                    Ok(project)
                }
                Err(e) => {
                    // Undone: every conversation goes back to the old folder.
                    match rewrite(&sessions_dir, &journal.sessions, &journal.to, &journal.from) {
                        Ok(()) => {
                            let _ = std::fs::remove_file(&journal_file);
                        }
                        Err(undo) => eprintln!(
                            "bricks-desktop: déplacement du projet {} non annulé ({}) : repris au prochain démarrage",
                            journal.project_id, undo.message
                        ),
                    }
                    Err(e)
                }
            }
        })
        .await
        .map_err(join_error)?
    }

    /// `<home>/desktop/relocations`.
    fn relocations_dir(&self) -> PathBuf {
        self.home.join("desktop").join("relocations")
    }

    /// Finishes or undoes the moves an earlier run left in its journal: a
    /// project already recorded in its new folder keeps it and every
    /// conversation follows; otherwise they all go back.
    pub(crate) fn recover_relocations(&self) {
        let dir = self.relocations_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let file = entry.path();
            if file.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let journal = match std::fs::read(&file)
                .map_err(|e| e.to_string())
                .and_then(|b| serde_json::from_slice::<Journal>(&b).map_err(|e| e.to_string()))
            {
                Ok(j) if j.schema == JOURNAL_SCHEMA => j,
                Ok(j) => {
                    eprintln!(
                        "bricks-desktop: journal de déplacement {} : version {} non reconnue",
                        file.display(),
                        j.schema
                    );
                    continue;
                }
                Err(e) => {
                    eprintln!(
                        "bricks-desktop: journal de déplacement {} illisible : {e}",
                        file.display()
                    );
                    continue;
                }
            };
            let arrived = self
                .projects
                .get(&journal.project_id)
                .is_ok_and(|p| Path::new(&p.path) == journal.to);
            let (from, to) = if arrived {
                (&journal.from, &journal.to)
            } else {
                (&journal.to, &journal.from)
            };
            match rewrite(self.sessions_dir(), &journal.sessions, from, to) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&file);
                }
                Err(e) => eprintln!(
                    "bricks-desktop: déplacement du projet {} non repris : {}",
                    journal.project_id, e.message
                ),
            }
        }
    }
}

fn journal_path(dir: &Path, project_id: &str) -> PathBuf {
    dir.join(format!("{project_id}.json"))
}

/// Records `to` as the working folder of each listed conversation still
/// recorded in `from`. Conversations already moved, or recorded elsewhere,
/// are left as they are, so it can run again after an interruption.
fn rewrite(sessions_dir: &Path, sessions: &[String], from: &Path, to: &Path) -> IpcResult<()> {
    for id in sessions {
        let dir = files_dir(sessions_dir, id);
        let Some(mut meta) = SessionMeta::load(&dir).map_err(|e| io_error("conversation", e))?
        else {
            continue;
        };
        if meta.working_dir == to || !recorded_in(&meta.working_dir, from) {
            continue;
        }
        meta.working_dir = to.to_path_buf();
        meta.save(&dir).map_err(|e| io_error("conversation", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(sessions: &Path, id: &str, dir: &Path) {
        SessionMeta::new(id, dir, "p/m", None)
            .save(&files_dir(sessions, id))
            .unwrap();
    }

    fn dir_of(sessions: &Path, id: &str) -> PathBuf {
        SessionMeta::load(&files_dir(sessions, id))
            .unwrap()
            .unwrap()
            .working_dir
    }

    #[test]
    fn rewriting_moves_only_the_conversations_of_the_old_folder_and_can_run_again() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        let from = root.path().join("gone");
        let to = root.path().join("here");
        let elsewhere = root.path().join("elsewhere");
        meta(&sessions, "a", &from);
        meta(&sessions, "b", &elsewhere);
        let ids = vec!["a".to_string(), "b".to_string(), "absent".to_string()];
        rewrite(&sessions, &ids, &from, &to).unwrap();
        assert_eq!(dir_of(&sessions, "a"), to);
        assert_eq!(dir_of(&sessions, "b"), elsewhere);
        rewrite(&sessions, &ids, &from, &to).unwrap();
        assert_eq!(dir_of(&sessions, "a"), to);
        rewrite(&sessions, &ids, &to, &from).unwrap();
        assert_eq!(dir_of(&sessions, "a"), from);
    }

    #[test]
    fn a_recorded_folder_belongs_to_a_vanished_folder_only_by_its_exact_path() {
        let root = tempfile::tempdir().unwrap();
        let gone = root.path().join("atlas");
        assert!(recorded_in(&gone, &gone));
        assert!(!recorded_in(&root.path().join("atlas-old"), &gone));
        assert!(!recorded_in(&gone.join("sub"), &gone));
    }
}
