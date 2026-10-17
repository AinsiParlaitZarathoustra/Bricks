//! The project file index behind `@` mentions (owner: Haiku 1, Sprint 14.1).
//!
//! Ported from the former terminal interface's `@` mention index (removed in Sprint 14.1).
//! One index per project, built with `ignore` (`.gitignore`, `.ignore`,
//! `.git/info/exclude`; hidden files excluded), bounded in entries, matched
//! with nucleo's fuzzy matcher. An index is rebuilt when it is older than
//! `MAX_AGE` at a query, after `invalidate` (the agent wrote files), and on
//! `refresh`. The walk and the matching run off the UI thread. A selected
//! entry is checked again on disk with `check_entry`.
//!
//! The logic lives here (no service module): the command is its only user.

use crate::contract::*;
use crate::engine::AppState;
use crate::services::join_error;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::State;

/// Entries one project's index keeps; the rest is left out (truncated).
const MAX_ENTRIES: usize = 50_000;
/// An index older than this is rebuilt at the next query.
const MAX_AGE: Duration = Duration::from_secs(10);
/// Results one query returns at most.
const MAX_LIMIT: u32 = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// Relative to the project root, `/` separated.
    path: String,
    is_dir: bool,
}

impl AsRef<str> for Entry {
    fn as_ref(&self) -> &str {
        &self.path
    }
}

struct Index {
    root: PathBuf,
    entries: Vec<Entry>,
    built: Option<Instant>,
    stale: bool,
    /// Entries left out by `max_entries` at the last build.
    truncated: bool,
    max_entries: usize,
    max_age: Duration,
}

impl Index {
    fn new(root: &Path, max_entries: usize, max_age: Duration) -> Self {
        Self {
            root: root.to_path_buf(),
            entries: Vec::new(),
            built: None,
            stale: true,
            truncated: false,
            max_entries,
            max_age,
        }
    }

    /// Files may have changed: rebuild before the next use.
    fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Rebuild if stale or too old.
    fn ensure_fresh(&mut self) {
        let old = self.built.is_none_or(|b| b.elapsed() > self.max_age);
        if self.stale || old {
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        let mut entries = Vec::new();
        self.truncated = false;
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            .require_git(false)
            .sort_by_file_path(|a, b| a.cmp(b))
            .build();
        for e in walker.flatten() {
            if e.depth() == 0 {
                continue;
            }
            if entries.len() >= self.max_entries {
                self.truncated = true;
                break;
            }
            let rel = e.path().strip_prefix(&self.root).unwrap_or(e.path());
            let path = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            entries.push(Entry {
                path,
                is_dir: e.file_type().is_some_and(|t| t.is_dir()),
            });
        }
        self.entries = entries;
        self.built = Some(Instant::now());
        self.stale = false;
    }

    /// The best matches for `query`; with an empty query, the shallowest
    /// and shortest entries first.
    fn matches(&self, query: &str, limit: usize) -> Vec<Entry> {
        if query.trim().is_empty() {
            let mut refs: Vec<&Entry> = self.entries.iter().collect();
            refs.sort_by_key(|e| (e.path.matches('/').count(), e.path.len()));
            return refs.into_iter().take(limit).cloned().collect();
        }
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut scored: Vec<(&Entry, u32)> = pattern
            .match_list(self.entries.iter(), &mut matcher)
            .into_iter()
            .collect();
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.path.len().cmp(&b.0.path.len()))
        });
        scored
            .into_iter()
            .take(limit)
            .map(|(e, _)| e.clone())
            .collect()
    }
}

/// One index per project. The map lock is never held while an index is
/// built or searched: a project's walk does not block the others.
type Shared = Arc<Mutex<Index>>;

fn cache() -> &'static Mutex<HashMap<String, (PathBuf, Shared)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (PathBuf, Shared)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The index of `project_id` for `root`; a new one when the folder moved.
fn index_of(project_id: &str, root: &Path) -> Shared {
    let mut map = cache().lock();
    match map.get(project_id) {
        Some((known, index)) if known == root => Arc::clone(index),
        _ => {
            let index = Arc::new(Mutex::new(Index::new(root, MAX_ENTRIES, MAX_AGE)));
            map.insert(
                project_id.to_string(),
                (root.to_path_buf(), Arc::clone(&index)),
            );
            index
        }
    }
}

/// The agent wrote files in this project: the next query rebuilds the index.
pub fn invalidate(project_id: &str) {
    let index = cache().lock().get(project_id).map(|(_, i)| Arc::clone(i));
    if let Some(index) = index {
        index.lock().invalidate();
    }
}

/// The entries matching `query` in the project at `root` (blocking: call it
/// from `spawn_blocking`).
pub fn search(
    project_id: &str,
    root: &Path,
    query: &str,
    limit: usize,
    refresh: bool,
) -> Vec<ProjectFileEntry> {
    let index = index_of(project_id, root);
    let mut idx = index.lock();
    if refresh {
        idx.rebuild();
    } else {
        idx.ensure_fresh();
    }
    idx.matches(query, limit)
        .into_iter()
        .map(|e| ProjectFileEntry {
            path: e.path,
            is_dir: e.is_dir,
        })
        .collect()
}

/// Checks on disk that a listed entry still exists as it was listed. The
/// absolute path when it does; otherwise the reason, in French.
pub fn check_entry(root: &Path, path: &str, is_dir: bool) -> Result<PathBuf, String> {
    let p = root.join(path);
    match std::fs::metadata(&p) {
        Ok(m) if m.is_dir() == is_dir => Ok(p),
        Ok(_) => Err(format!("`{path}` a changé de type depuis la liste")),
        Err(_) => Err(format!("`{path}` n'existe plus")),
    }
}

/// Fuzzy-matched entries of the project (best first, at most `limit`).
/// `refresh` rebuilds the index now. An empty query lists the first entries.
#[tauri::command]
pub async fn project_files(
    state: State<'_, AppState>,
    project_id: String,
    query: String,
    limit: u32,
    refresh: bool,
) -> IpcResult<Vec<ProjectFileEntry>> {
    let root = state.project_path(&project_id)?;
    let limit = limit.clamp(1, MAX_LIMIT) as usize;
    tokio::task::spawn_blocking(move || search(&project_id, &root, &query, limit, refresh))
        .await
        .map_err(join_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(root: &Path) -> Index {
        Index::new(root, MAX_ENTRIES, MAX_AGE)
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/dossier avec espaces")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("src/dossier avec espaces/été.md"), "").unwrap();
        std::fs::write(root.join("target/debug/out"), "").unwrap();
        dir
    }

    #[test]
    fn the_index_respects_ignores_matches_fuzzily_and_refreshes() {
        let dir = fixture();
        let root = dir.path();
        let mut idx = index(root);
        idx.ensure_fresh();
        assert!(
            idx.matches("", 100)
                .iter()
                .all(|e| !e.path.starts_with("target")),
            "gitignored"
        );
        let hits = idx.matches("smain", 5);
        assert_eq!(hits[0].path, "src/main.rs");
        let hits = idx.matches("ete", 5);
        assert_eq!(
            hits[0].path, "src/dossier avec espaces/été.md",
            "normalised accents"
        );
        assert!(idx.matches("espaces", 5).iter().any(|e| e.is_dir));

        // New files appear after a rebuild; removed ones are reported.
        std::fs::write(root.join("src/new.rs"), "").unwrap();
        assert!(idx.matches("new.rs", 5).is_empty());
        idx.invalidate();
        idx.ensure_fresh();
        let new = idx.matches("new.rs", 5)[0].clone();
        std::fs::remove_file(root.join("src/new.rs")).unwrap();
        assert!(
            check_entry(root, &new.path, new.is_dir)
                .unwrap_err()
                .contains("n'existe plus")
        );
    }

    #[test]
    fn an_empty_query_lists_the_shallowest_entries_first() {
        let dir = fixture();
        let mut idx = index(dir.path());
        idx.ensure_fresh();
        let first = idx.matches("", 3);
        assert_eq!(first.len(), 3);
        let depths: Vec<usize> = first.iter().map(|e| e.path.matches('/').count()).collect();
        assert!(depths.windows(2).all(|w| w[0] <= w[1]), "{depths:?}");
        assert!(idx.matches("   ", 1).len() == 1, "blank query is empty");
    }

    #[test]
    fn a_bounded_index_says_it_is_truncated() {
        let dir = fixture();
        let mut small = Index::new(dir.path(), 2, MAX_AGE);
        small.rebuild();
        assert!(small.truncated && small.entries.len() == 2);
        let mut all = index(dir.path());
        all.rebuild();
        assert!(!all.truncated);
    }

    #[test]
    fn an_index_older_than_max_age_is_rebuilt_at_the_next_query() {
        let dir = fixture();
        let mut idx = Index::new(dir.path(), MAX_ENTRIES, Duration::ZERO);
        idx.ensure_fresh();
        std::fs::write(dir.path().join("late.txt"), "").unwrap();
        idx.ensure_fresh();
        assert_eq!(idx.matches("late", 5)[0].path, "late.txt");
    }

    #[test]
    fn the_index_is_kept_per_project_until_refreshed_or_invalidated() {
        let dir = fixture();
        let root = dir.path();
        let id = uuid::Uuid::new_v4().to_string();
        let first = search(&id, root, "", 500, false);
        assert!(first.iter().any(|e| e.path == "src/main.rs"));

        std::fs::write(root.join("b-cached.rs"), "").unwrap();
        assert!(
            !search(&id, root, "cached", 10, false)
                .iter()
                .any(|e| e.path == "b-cached.rs"),
            "served from the index, not rebuilt"
        );
        let refreshed = search(&id, root, "cached", 10, true);
        assert_eq!(refreshed[0].path, "b-cached.rs", "refresh rebuilds now");

        std::fs::write(root.join("c-agent.rs"), "").unwrap();
        invalidate(&id);
        assert_eq!(
            search(&id, root, "agent", 10, false)[0].path,
            "c-agent.rs",
            "an agent write invalidates the index"
        );
    }

    #[test]
    fn another_project_has_its_own_index() {
        let dir = fixture();
        let root = dir.path();
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        search(&a, root, "", 10, false);
        std::fs::write(root.join("only-a-sees-it-later.rs"), "").unwrap();
        // `b` builds now and sees the new file; `a` keeps its cache.
        assert!(
            search(&b, root, "later", 10, false)
                .iter()
                .any(|e| e.path == "only-a-sees-it-later.rs")
        );
        assert!(
            !search(&a, root, "later", 10, false)
                .iter()
                .any(|e| e.path == "only-a-sees-it-later.rs")
        );
    }

    #[test]
    fn a_moved_project_folder_gets_a_new_index() {
        let first = fixture();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(second.path().join("elsewhere.md"), "").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        search(&id, first.path(), "", 10, false);
        let hits = search(&id, second.path(), "", 10, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "elsewhere.md");
    }

    #[test]
    fn a_selected_entry_is_checked_on_disk() {
        let dir = fixture();
        let root = dir.path();
        assert_eq!(
            check_entry(root, "src/main.rs", false).unwrap(),
            root.join("src/main.rs")
        );
        assert!(
            check_entry(root, "src/main.rs", true)
                .unwrap_err()
                .contains("a changé de type")
        );
        assert!(
            check_entry(root, "src/absent.rs", false)
                .unwrap_err()
                .contains("n'existe plus")
        );
    }

    #[test]
    fn the_limit_bounds_the_results() {
        let dir = fixture();
        let id = uuid::Uuid::new_v4().to_string();
        assert_eq!(search(&id, dir.path(), "", 2, false).len(), 2);
        assert_eq!(search(&id, dir.path(), "rs", 1, false).len(), 1);
    }
}
