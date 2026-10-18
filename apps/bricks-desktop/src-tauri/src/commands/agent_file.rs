//! Reveal in the Finder a file a sub-agent changed (`result.files_changed`).
//!
//! The agent is looked up in an open session of the engine, and the root of the
//! workspace it works in is the one the engine recorded for it: the project
//! folder of a shared agent, the root of the worktree of an isolated one. The
//! frontend sends no folder. `resolve_reveal_target` checks the path against that root,
//! symbolic links resolved, and returns what to reveal. The reveal itself is one
//! call to the opener plugin, which selects the item in its folder: nothing is
//! opened, executed or run.

use crate::commands::inspect::open_or_not_found;
use crate::contract::{IpcError, IpcErrorCode, IpcResult};
use crate::engine::AppState;
use crate::git::validate_relative_path;
use crate::services::{error, invalid, io_error, join_error, not_found};
use std::path::{Component, Path, PathBuf};
use tauri::State;
use tauri_plugin_opener::OpenerExt;

/// Longest path accepted, as for the other path commands.
const PATH_LIMIT: usize = 4096;

/// The root of the workspace an agent works in, as the engine recorded it. A
/// shared agent works in `workspace`, the project folder. An isolated agent
/// works in its worktree, and the root of that worktree is where `git diff`
/// runs: the relative paths of its `files_changed` are relative to it, not to
/// its working folder `workspace` (a sub-folder of the root when the project is
/// one). `worktree` is the engine's record of that worktree, `(state, root)`,
/// `None` when it is not registered; a removed worktree has nothing to reveal.
pub(crate) fn agent_root(
    isolation: &str,
    workspace: &str,
    worktree: Option<(&str, &Path)>,
) -> IpcResult<PathBuf> {
    let root = match isolation {
        "shared" => PathBuf::from(workspace),
        "worktree" => match worktree {
            None => {
                return Err(not_found(
                    "l'espace isolé de cet agent n'est pas enregistré",
                ));
            }
            Some(("removed", _)) => {
                return Err(not_found("l'espace isolé de cet agent a été supprimé"));
            }
            Some((_, root)) => root.to_path_buf(),
        },
        other => return Err(invalid(format!("mode d'isolation inconnu : `{other}`"))),
    };
    if root.as_os_str().is_empty() || root.to_string_lossy().contains('\0') {
        return Err(invalid("l'espace de travail de l'agent est inconnu"));
    }
    Ok(root)
}

/// The workspace root of the agent `agent_id` of the open session
/// `session_id`, read from the engine (never from the frontend).
pub(crate) fn agent_root_of(
    state: &AppState,
    session_id: &str,
    agent_id: &str,
) -> IpcResult<PathBuf> {
    let entry = open_or_not_found(state, session_id)?;
    let runtime = entry
        .controller
        .agent_runtime()
        .ok_or_else(|| not_found("cette conversation n'a pas de sous-agents"))?;
    let record = runtime
        .record(agent_id)
        .ok_or_else(|| not_found("agent inconnu dans cette conversation"))?;
    let registered = runtime
        .workspaces
        .worktree(agent_id)
        .map(|w| (w.state, w.root));
    let worktree = registered
        .as_ref()
        .map(|(state, root)| (state.as_str(), root.as_path()));
    agent_root(&record.info.isolation, &record.info.workspace, worktree)
}

/// What to reveal for `path` (a path of `result.files_changed`) under `root`,
/// the workspace of the agent: the file itself when it exists, otherwise its
/// nearest existing folder under `root`. `path` is absolute or relative to
/// `root`. It may not leave `root`: no `..`, no absolute path outside it, and no
/// symbolic link leading outside it (a broken link included). Reads the file
/// system only.
pub fn resolve_reveal_target(root: &Path, path: &str) -> IpcResult<PathBuf> {
    let root = std::fs::canonicalize(root).map_err(|e| IpcError {
        code: IpcErrorCode::FolderMissing,
        message: format!("l'espace de travail de l'agent n'est plus accessible : {e}"),
    })?;
    let mut existing = candidate_of(&root, path)?;
    // The deepest existing entry: a missing file or folder is replaced by its parent.
    loop {
        match std::fs::symlink_metadata(&existing) {
            Ok(_) => break,
            Err(e) if is_missing(&e) => {
                if !existing.pop() {
                    return Err(outside(path));
                }
            }
            Err(e) => return Err(io_error(&format!("lecture de `{path}`"), e)),
        }
    }
    // Canonical form: every link on the way is followed, so a link out of `root` shows here.
    let resolved = std::fs::canonicalize(&existing).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => invalid(format!(
            "`{path}` passe par un lien symbolique cassé : refusé"
        )),
        _ => io_error(&format!("lecture de `{path}`"), e),
    })?;
    if !resolved.starts_with(&root) {
        return Err(outside(path));
    }
    Ok(resolved)
}

/// The path to resolve, not yet canonical: `path` itself when absolute, or
/// `path` joined to `root` when relative (checked by the shared path rules).
fn candidate_of(root: &Path, path: &str) -> IpcResult<PathBuf> {
    if Path::new(path).is_absolute() {
        if path.is_empty() || path.len() > PATH_LIMIT || path.contains('\0') {
            return Err(invalid("chemin invalide"));
        }
        // An absolute path holds no `..`: a folder not yet there cannot be resolved lexically.
        let escapes = Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)));
        if escapes {
            return Err(outside(path));
        }
        return Ok(PathBuf::from(path));
    }
    validate_relative_path(path)?;
    Ok(root.join(path))
}

/// A path that does not exist (its entry is missing, or a parent is a file).
fn is_missing(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

fn outside(path: &str) -> IpcError {
    invalid(format!(
        "`{path}` sort de l'espace de travail de l'agent : refusé"
    ))
}

/// Reveals in the Finder the file `path` that sub-agent `agent_id` of the open
/// session `session_id` changed. Nothing is opened or run.
#[tauri::command]
pub async fn reveal_agent_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    agent_id: String,
    path: String,
) -> IpcResult<()> {
    let root = agent_root_of(&state, &session_id, &agent_id)?;
    tokio::task::spawn_blocking(move || {
        let target = resolve_reveal_target(&root, &path)?;
        app.opener().reveal_item_in_dir(&target).map_err(|e| {
            error(
                IpcErrorCode::Io,
                format!("affichage dans le Finder impossible : {e}"),
            )
        })
    })
    .await
    .map_err(join_error)?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap()
    }

    /// A workspace with `src/a.rs`, a folder `dossier é/` holding a file with
    /// spaces, and a folder outside it holding a secret.
    struct Fixture {
        _tmp: tempfile::TempDir,
        _outside: tempfile::TempDir,
        root: PathBuf,
        outside: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = tmp.path().join("espace");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::create_dir_all(root.join("dossier é")).unwrap();
        std::fs::write(root.join("dossier é/mon fichier (1).rs"), "x").unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        Fixture {
            root,
            outside: outside.path().to_path_buf(),
            _tmp: tmp,
            _outside: outside,
        }
    }

    fn invalid_code(result: IpcResult<PathBuf>) -> bool {
        matches!(
            result,
            Err(IpcError {
                code: IpcErrorCode::Invalid,
                ..
            })
        )
    }

    #[test]
    fn a_relative_file_resolves_to_its_canonical_path() {
        let f = fixture();
        let got = resolve_reveal_target(&f.root, "src/a.rs").unwrap();
        assert_eq!(got, canon(&f.root.join("src/a.rs")));
    }

    #[test]
    fn an_absolute_path_under_the_root_is_accepted_in_any_spelling() {
        let f = fixture();
        // The root as given (possibly a symbolic link on macOS) is not canonical.
        let absolute = f.root.join("src/a.rs");
        let got = resolve_reveal_target(&f.root, absolute.to_str().unwrap()).unwrap();
        assert_eq!(got, canon(&absolute));
    }

    #[test]
    fn parent_components_are_refused() {
        let f = fixture();
        assert!(invalid_code(resolve_reveal_target(
            &f.root,
            "../secret.txt"
        )));
        assert!(invalid_code(resolve_reveal_target(&f.root, "src/../../x")));
        let escaping = format!("{}/../secret.txt", f.root.display());
        assert!(invalid_code(resolve_reveal_target(&f.root, &escaping)));
    }

    #[test]
    fn an_absolute_path_outside_the_root_is_refused() {
        let f = fixture();
        let outside = f.outside.join("secret.txt");
        assert!(invalid_code(resolve_reveal_target(
            &f.root,
            outside.to_str().unwrap()
        )));
    }

    #[test]
    fn a_symbolic_link_to_a_file_outside_the_root_is_refused() {
        let f = fixture();
        std::os::unix::fs::symlink(f.outside.join("secret.txt"), f.root.join("lien.txt")).unwrap();
        assert!(invalid_code(resolve_reveal_target(&f.root, "lien.txt")));
    }

    #[test]
    fn a_missing_file_under_a_symbolic_link_to_outside_is_refused() {
        let f = fixture();
        std::os::unix::fs::symlink(&f.outside, f.root.join("dehors")).unwrap();
        assert!(invalid_code(resolve_reveal_target(
            &f.root,
            "dehors/nouveau.rs"
        )));
    }

    #[test]
    fn a_broken_symbolic_link_is_refused() {
        let f = fixture();
        std::os::unix::fs::symlink(f.outside.join("absent"), f.root.join("casse")).unwrap();
        assert!(invalid_code(resolve_reveal_target(&f.root, "casse")));
        std::os::unix::fs::symlink(f.root.join("absent"), f.root.join("casse_interne")).unwrap();
        assert!(invalid_code(resolve_reveal_target(
            &f.root,
            "casse_interne/x.rs"
        )));
    }

    #[test]
    fn a_symbolic_link_that_stays_under_the_root_is_followed() {
        let f = fixture();
        std::os::unix::fs::symlink(f.root.join("src"), f.root.join("alias")).unwrap();
        let got = resolve_reveal_target(&f.root, "alias/a.rs").unwrap();
        assert_eq!(got, canon(&f.root.join("src/a.rs")));
    }

    #[test]
    fn a_deleted_file_reveals_its_existing_folder() {
        let f = fixture();
        let got = resolve_reveal_target(&f.root, "src/supprime.rs").unwrap();
        assert_eq!(got, canon(&f.root.join("src")));
    }

    #[test]
    fn deleted_nested_folders_reveal_the_nearest_existing_one() {
        let f = fixture();
        let got = resolve_reveal_target(&f.root, "src/x/y/z.rs").unwrap();
        assert_eq!(got, canon(&f.root.join("src")));
    }

    #[test]
    fn a_path_under_a_regular_file_reveals_that_file() {
        let f = fixture();
        let got = resolve_reveal_target(&f.root, "src/a.rs/encore").unwrap();
        assert_eq!(got, canon(&f.root.join("src/a.rs")));
    }

    #[test]
    fn a_path_with_nothing_left_reveals_the_root() {
        let f = fixture();
        let got = resolve_reveal_target(&f.root, "absent/absent.rs").unwrap();
        assert_eq!(got, canon(&f.root));
    }

    #[test]
    fn a_missing_workspace_is_reported_as_folder_missing() {
        let f = fixture();
        let gone = f.root.join("disparu");
        let result = resolve_reveal_target(&gone, "src/a.rs");
        assert!(matches!(
            result,
            Err(IpcError {
                code: IpcErrorCode::FolderMissing,
                ..
            })
        ));
    }

    #[test]
    fn unicode_names_and_spaces_resolve() {
        let f = fixture();
        let got = resolve_reveal_target(&f.root, "dossier é/mon fichier (1).rs").unwrap();
        assert_eq!(got, canon(&f.root.join("dossier é/mon fichier (1).rs")));
        let deleted = resolve_reveal_target(&f.root, "dossier é/absent é.rs").unwrap();
        assert_eq!(deleted, canon(&f.root.join("dossier é")));
    }

    #[test]
    fn empty_nul_and_too_long_paths_are_invalid() {
        let f = fixture();
        assert!(invalid_code(resolve_reveal_target(&f.root, "")));
        assert!(invalid_code(resolve_reveal_target(&f.root, "src/a\0.rs")));
        let long = "a".repeat(PATH_LIMIT + 1);
        assert!(invalid_code(resolve_reveal_target(&f.root, &long)));
        let long_absolute = format!("/{}", "a/".repeat(PATH_LIMIT));
        assert!(invalid_code(resolve_reveal_target(&f.root, &long_absolute)));
    }

    #[test]
    fn a_shared_agent_works_in_the_recorded_workspace() {
        let f = fixture();
        let root = agent_root("shared", f.root.to_str().unwrap(), None).unwrap();
        assert_eq!(root, f.root);
    }

    #[test]
    fn an_isolated_agent_needs_a_registered_worktree() {
        let f = fixture();
        let ws = f.root.to_str().unwrap();
        assert!(matches!(
            agent_root("worktree", ws, None),
            Err(IpcError {
                code: IpcErrorCode::NotFound,
                ..
            })
        ));
        assert!(matches!(
            agent_root("worktree", ws, Some(("removed", f.root.as_path()))),
            Err(IpcError {
                code: IpcErrorCode::NotFound,
                ..
            })
        ));
        for state in ["active", "kept"] {
            let root = agent_root("worktree", ws, Some((state, f.root.as_path()))).unwrap();
            assert_eq!(root, f.root);
        }
    }

    #[test]
    fn an_isolated_agent_resolves_from_its_worktree_root_not_its_folder() {
        // A project in a sub-folder of its repository: `git diff` gives the paths from
        // the worktree root, while the agent's working folder is `app/` below it.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("depot");
        std::fs::create_dir_all(repo.join("app/src")).unwrap();
        std::fs::write(repo.join("app/src/a.rs"), "fn a() {}\n").unwrap();
        let cwd = repo.join("app");
        let root = agent_root(
            "worktree",
            cwd.to_str().unwrap(),
            Some(("kept", repo.as_path())),
        )
        .unwrap();
        assert_eq!(root, repo);
        // The path as git reports it resolves from the worktree root...
        let got = resolve_reveal_target(&root, "app/src/a.rs").unwrap();
        assert_eq!(got, canon(&repo.join("app/src/a.rs")));
        // ...and a path relative to the working folder is not taken from there: the
        // worktree root has no `src/`, so the nearest existing folder is revealed.
        let not_from_cwd = resolve_reveal_target(&root, "src/a.rs").unwrap();
        assert_eq!(not_from_cwd, canon(&repo));
        // A shared agent keeps working in the recorded folder, the sub-folder itself.
        let shared = agent_root("shared", cwd.to_str().unwrap(), None).unwrap();
        assert_eq!(shared, cwd);
    }

    #[test]
    fn an_unknown_isolation_or_an_empty_workspace_is_invalid() {
        assert!(matches!(
            agent_root("auto", "/tmp/x", None),
            Err(IpcError {
                code: IpcErrorCode::Invalid,
                ..
            })
        ));
        assert!(matches!(
            agent_root("shared", "", None),
            Err(IpcError {
                code: IpcErrorCode::Invalid,
                ..
            })
        ));
    }
}
