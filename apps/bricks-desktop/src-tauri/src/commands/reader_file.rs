//! The reader of a conversation: read-only windows of the files it names (a
//! message reference, an edit, a run summary). A file is read in the workspace
//! of the agent that named it, or in the project folder of the session when no
//! agent is given. Nothing is written, run or opened.
//!
//! A path is relative to that workspace (no `..`), or absolute and inside it.
//! Once every symbolic link is followed, the file must still be inside the
//! workspace. The window itself is the one of `project_file_read`: at most
//! `WINDOW_LINES` lines, `READ_LIMIT` bytes read, binary and non UTF-8 content
//! refused.

use crate::commands::agent_file::{agent_root_of, candidate_of};
use crate::commands::inspect::open_or_not_found;
use crate::commands::project_file::{canonical_or_missing, canonical_root, read_target};
use crate::contract::{IpcResult, ProjectFileWindow};
use crate::engine::AppState;
use crate::services::{invalid, join_error, not_found};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;

/// A window of a file read by the reader: the window of `project_file_read`,
/// and the canonical absolute path of the file (every link followed). The
/// canonical path is what identifies the file in the reader.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderFileWindow {
    #[serde(flatten)]
    pub window: ProjectFileWindow,
    /// The file itself, canonical: links followed, absolute.
    pub absolute: String,
}

/// The folder a reader path is resolved from. An agent (`agent_id` not empty)
/// works in the workspace the engine recorded for it: the project folder when
/// shared, its worktree root when isolated. Without an agent, the project folder
/// of the session. The session must be open, and an agent must belong to it.
pub(crate) fn reader_root(
    state: &AppState,
    session_id: &str,
    agent_id: Option<&str>,
) -> IpcResult<PathBuf> {
    match agent_id.filter(|id| !id.is_empty()) {
        Some(agent) => agent_root_of(state, session_id, agent),
        None => {
            let entry = open_or_not_found(state, session_id)?;
            state.project_path(&entry.project_id)
        }
    }
}

/// The window of `path` (relative to `root`, or absolute inside it) asked by
/// `line` and `end_line` (see `project_file_read`). `root` is the workspace of
/// the agent, or the project folder.
pub fn read_in_root(
    root: &Path,
    path: &str,
    line: Option<u32>,
    end_line: Option<u32>,
) -> IpcResult<ReaderFileWindow> {
    let root = canonical_root(root, "le dossier de lecture")?;
    let candidate = candidate_of(&root, path)?;
    let target = canonical_or_missing(&candidate, path)?.ok_or_else(|| {
        not_found(format!(
            "`{path}` n'existe plus : le fichier a été supprimé ou déplacé"
        ))
    })?;
    if !target.starts_with(&root) {
        return Err(invalid(format!(
            "`{path}` sort de l'espace de travail : refusé"
        )));
    }
    let window = read_target(&target, path, line, end_line)?;
    Ok(ReaderFileWindow {
        window,
        absolute: target.to_string_lossy().into_owned(),
    })
}

/// Reads a window of a file for the reader of the open session `session_id`:
/// in the workspace of `agent_id` when given, else in the project folder.
#[tauri::command]
pub async fn reader_file_read(
    state: State<'_, AppState>,
    session_id: String,
    agent_id: Option<String>,
    path: String,
    line: Option<u32>,
    end_line: Option<u32>,
) -> IpcResult<ReaderFileWindow> {
    let root = reader_root(&state, &session_id, agent_id.as_deref())?;
    tokio::task::spawn_blocking(move || read_in_root(&root, &path, line, end_line))
        .await
        .map_err(join_error)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::agent_file::agent_root;
    use crate::commands::project_file::READ_LIMIT;
    use crate::contract::{IpcError, IpcErrorCode};

    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().expect("temporary folder")
    }

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("folders");
        std::fs::write(path, bytes).expect("file");
    }

    /// `ligne 1\nligne 2\n…` with `n` lines.
    fn numbered(n: u32) -> Vec<u8> {
        (1..=n)
            .map(|i| format!("ligne {i}\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn code_of<T: std::fmt::Debug>(result: IpcResult<T>) -> (IpcErrorCode, String) {
        let error: IpcError = result.expect_err("an error is expected");
        (error.code, error.message)
    }

    #[test]
    fn a_relative_path_reads_the_file_and_gives_its_canonical_path() {
        let dir = folder();
        write(dir.path(), "src/a.rs", b"fn a() {}\n");
        let read = read_in_root(dir.path(), "src/a.rs", None, None).unwrap();
        assert_eq!(read.window.path, "src/a.rs");
        assert_eq!(read.window.lines, ["fn a() {}"]);
        assert_eq!(read.window.language.as_deref(), Some("rust"));
        let canonical = std::fs::canonicalize(dir.path().join("src/a.rs")).unwrap();
        assert_eq!(read.absolute, canonical.to_string_lossy());
    }

    #[test]
    fn an_absolute_path_inside_the_root_is_accepted_in_any_spelling() {
        let dir = folder();
        write(dir.path(), "src/a.rs", b"a\n");
        // The root as given is not canonical on macOS (a symbolic link in /var).
        let absolute = dir.path().join("src/a.rs");
        let read = read_in_root(dir.path(), absolute.to_str().unwrap(), None, None).unwrap();
        let canonical = std::fs::canonicalize(&absolute).unwrap();
        assert_eq!(read.absolute, canonical.to_string_lossy());
        assert_eq!(read.window.lines, ["a"]);
    }

    #[test]
    fn an_absolute_path_outside_the_root_is_refused() {
        let dir = folder();
        let outside = folder();
        write(outside.path(), "secret.txt", b"secret\n");
        let path = outside.path().join("secret.txt");
        let (code, message) = code_of(read_in_root(dir.path(), path.to_str().unwrap(), None, None));
        assert_eq!(code, IpcErrorCode::Invalid);
        assert!(message.contains("sort de l'espace de travail"), "{message}");
    }

    #[test]
    fn parent_components_are_refused_in_relative_and_absolute_paths() {
        let dir = folder();
        let outside = folder();
        write(outside.path(), "secret.txt", b"secret\n");
        write(dir.path(), "src/a.rs", b"a\n");
        for path in [
            "../secret.txt",
            "src/../../secret.txt",
            "src/../src/a.rs",
            "",
            "./src/a.rs",
        ] {
            let (code, _) = code_of(read_in_root(dir.path(), path, None, None));
            assert_eq!(code, IpcErrorCode::Invalid, "refused: {path:?}");
        }
        let escaping = format!("{}/../secret.txt", dir.path().display());
        let (code, _) = code_of(read_in_root(dir.path(), &escaping, None, None));
        assert_eq!(code, IpcErrorCode::Invalid);
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_leaving_the_root_is_refused() {
        let dir = folder();
        let outside = folder();
        write(outside.path(), "secret.txt", b"secret\n");
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("lien.txt"),
        )
        .unwrap();
        let (code, message) = code_of(read_in_root(dir.path(), "lien.txt", None, None));
        assert_eq!(code, IpcErrorCode::Invalid);
        assert!(message.contains("sort de l'espace de travail"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_inside_the_root_is_followed_to_its_target() {
        let dir = folder();
        write(dir.path(), "real/notes.md", b"# titre\n");
        std::os::unix::fs::symlink(
            dir.path().join("real/notes.md"),
            dir.path().join("alias.md"),
        )
        .unwrap();
        let read = read_in_root(dir.path(), "alias.md", None, None).unwrap();
        let target = std::fs::canonicalize(dir.path().join("real/notes.md")).unwrap();
        assert_eq!(read.absolute, target.to_string_lossy());
        assert_eq!(read.window.lines, ["# titre"]);
        assert_eq!(read.window.language.as_deref(), Some("markdown"));
    }

    #[test]
    fn a_deleted_file_is_not_found_with_a_clear_message() {
        let dir = folder();
        write(dir.path(), "src/a.rs", b"a\n");
        let (code, message) = code_of(read_in_root(dir.path(), "src/supprime.rs", None, None));
        assert_eq!(code, IpcErrorCode::NotFound);
        assert!(message.contains("n'existe plus"), "{message}");
        let (code, _) = code_of(read_in_root(dir.path(), "src/a.rs/encore", None, None));
        assert_eq!(
            code,
            IpcErrorCode::NotFound,
            "a path under a file is not a file"
        );
    }

    #[test]
    fn a_folder_is_not_a_file() {
        let dir = folder();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let (code, message) = code_of(read_in_root(dir.path(), "src", None, None));
        assert_eq!(code, IpcErrorCode::Invalid);
        assert!(message.contains("n'est pas un fichier"), "{message}");
    }

    #[test]
    fn a_binary_or_non_utf8_file_is_refused() {
        let dir = folder();
        write(dir.path(), "image.dat", b"PNG\0\x01\x02 texte");
        write(dir.path(), "latin.txt", b"caf\xe9\n");
        let (code, message) = code_of(read_in_root(dir.path(), "image.dat", None, None));
        assert_eq!(code, IpcErrorCode::Invalid);
        assert!(message.contains("binaire"), "{message}");
        let (code, message) = code_of(read_in_root(dir.path(), "latin.txt", None, None));
        assert_eq!(code, IpcErrorCode::Invalid);
        assert!(message.contains("UTF-8"), "{message}");
    }

    #[test]
    fn a_large_file_is_cut_at_the_read_limit_and_flagged() {
        let dir = folder();
        let line = format!("{}\n", "x".repeat(99));
        let content = line.repeat(3 * 1024 * 1024 / line.len());
        write(dir.path(), "large.log", content.as_bytes());
        let read = read_in_root(dir.path(), "large.log", None, None).unwrap();
        assert!(read.window.truncated, "the read limit cut the file");
        assert_eq!(read.window.lines.len(), 400);
        let read_lines = u32::try_from(READ_LIMIT / line.len() as u64).unwrap();
        assert!(
            read.window.total_lines + 2 >= read_lines && read.window.total_lines <= read_lines + 1
        );
        // A window further down the file is read the same way, with the same flag.
        let later = read_in_root(dir.path(), "large.log", Some(1000), Some(1500)).unwrap();
        assert_eq!(later.window.start, 1000);
        assert!(later.window.truncated);
    }

    #[test]
    fn windows_follow_the_line_numbers_of_project_file_read() {
        let dir = folder();
        write(dir.path(), "big.txt", &numbered(1000));
        let centred = read_in_root(dir.path(), "big.txt", Some(500), None).unwrap();
        assert_eq!(
            (centred.window.start, centred.window.lines.len()),
            (300, 400)
        );
        let paged = read_in_root(dir.path(), "big.txt", Some(401), Some(800)).unwrap();
        assert_eq!(paged.window.start, 401);
        assert_eq!(paged.window.lines[0], "ligne 401");
        assert_eq!(paged.window.lines.last().unwrap(), "ligne 800");
        let (code, message) = code_of(read_in_root(dir.path(), "big.txt", Some(1001), None));
        assert_eq!(code, IpcErrorCode::Invalid);
        assert!(message.contains("au-delà de la fin"), "{message}");
    }

    #[test]
    fn spaces_and_unicode_in_names_work() {
        let dir = folder();
        write(dir.path(), "dossier é/été élève.md", "été\n".as_bytes());
        let read = read_in_root(dir.path(), "dossier é/été élève.md", None, None).unwrap();
        assert_eq!(read.window.lines, ["été"]);
        assert!(
            read.absolute.ends_with("dossier é/été élève.md"),
            "{}",
            read.absolute
        );
    }

    #[test]
    fn two_files_with_the_same_basename_keep_their_own_identity() {
        let dir = folder();
        write(dir.path(), "a/notes.md", b"premier\n");
        write(dir.path(), "b/notes.md", b"second\n");
        let first = read_in_root(dir.path(), "a/notes.md", None, None).unwrap();
        let second = read_in_root(dir.path(), "b/notes.md", None, None).unwrap();
        assert_ne!(first.absolute, second.absolute);
        assert_eq!(first.window.lines, ["premier"]);
        assert_eq!(second.window.lines, ["second"]);
    }

    #[test]
    fn a_missing_root_is_reported_as_folder_missing() {
        let dir = folder();
        let gone = dir.path().join("disparu");
        let (code, _) = code_of(read_in_root(&gone, "src/a.rs", None, None));
        assert_eq!(code, IpcErrorCode::FolderMissing);
    }

    #[test]
    fn an_isolated_agent_reads_its_worktree_and_a_shared_one_the_project() {
        let project = folder();
        let worktree = folder();
        write(project.path(), "src/a.rs", b"projet\n");
        write(worktree.path(), "src/a.rs", b"worktree\n");
        write(worktree.path(), "src/seulement.rs", b"isole\n");
        let project_str = project.path().to_str().unwrap();
        let shared = agent_root("shared", project_str, None).unwrap();
        let isolated =
            agent_root("worktree", project_str, Some(("kept", worktree.path()))).unwrap();
        assert_eq!(
            read_in_root(&shared, "src/a.rs", None, None)
                .unwrap()
                .window
                .lines,
            ["projet"]
        );
        assert_eq!(
            read_in_root(&isolated, "src/a.rs", None, None)
                .unwrap()
                .window
                .lines,
            ["worktree"]
        );
        let (code, _) = code_of(read_in_root(&shared, "src/seulement.rs", None, None));
        assert_eq!(code, IpcErrorCode::NotFound, "the project has no such file");
        // An absolute path of the worktree is read from the worktree root only.
        let absolute = worktree.path().join("src/seulement.rs");
        let (code, _) = code_of(read_in_root(
            &shared,
            absolute.to_str().unwrap(),
            None,
            None,
        ));
        assert_eq!(code, IpcErrorCode::Invalid);
        let read = read_in_root(&isolated, absolute.to_str().unwrap(), None, None).unwrap();
        assert_eq!(read.window.lines, ["isole"]);
    }

    #[test]
    fn an_unopened_session_or_agent_is_not_found() {
        let home = folder();
        let work = folder();
        let state = AppState::assemble(
            home.path().to_path_buf(),
            Err("aucun registre dans ce test".into()),
        );
        state
            .projects
            .add(work.path().to_str().unwrap())
            .expect("project folder");
        let (code, message) = code_of(reader_root(&state, "absente", None));
        assert_eq!(code, IpcErrorCode::NotFound);
        assert!(message.contains("n'est pas ouverte"), "{message}");
        let (code, _) = code_of(reader_root(&state, "absente", Some("agent-1")));
        assert_eq!(code, IpcErrorCode::NotFound);
        let (code, _) = code_of(reader_root(&state, "absente", Some("")));
        assert_eq!(code, IpcErrorCode::NotFound, "an empty agent is no agent");
    }

    #[test]
    fn the_window_serializes_with_its_canonical_path_next_to_the_window_fields() {
        let dir = folder();
        write(dir.path(), "a.md", b"# a\n");
        let read = read_in_root(dir.path(), "a.md", None, None).unwrap();
        let json = serde_json::to_value(&read).unwrap();
        for key in [
            "path",
            "start",
            "lines",
            "totalLines",
            "truncated",
            "language",
            "absolute",
        ] {
            assert!(json.get(key).is_some(), "{key} in {json}");
        }
        assert_eq!(json["language"], "markdown");
    }
}
