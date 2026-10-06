//! The project index behind `@` mentions.
//!
//! Built with `ignore` (`.gitignore`, `.ignore`, hidden files excluded),
//! bounded in entries, matched with nucleo's fuzzy matcher. The index is
//! not assumed valid for the whole session: it is rebuilt when it is older
//! than `max_age` at the moment a mention starts, after the agent wrote
//! files, and on demand (Ctrl+R in the list). A selected entry is checked
//! again on disk: a file that disappeared is reported, not attached.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the root, `/` separated.
    pub path: String,
    pub is_dir: bool,
}

pub struct FileIndex {
    root: PathBuf,
    entries: Vec<Entry>,
    built: Option<Instant>,
    stale: bool,
    pub max_entries: usize,
    pub max_age: Duration,
    /// Entries left out by `max_entries` at the last build.
    pub truncated: bool,
}

impl FileIndex {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            entries: Vec::new(),
            built: None,
            stale: true,
            max_entries: 50_000,
            max_age: Duration::from_secs(10),
            truncated: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Files may have changed (the agent wrote some): rebuild before the
    /// next use.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Rebuild if stale or too old.
    pub fn ensure_fresh(&mut self) {
        let old = self.built.is_none_or(|b| b.elapsed() > self.max_age);
        if self.stale || old {
            self.rebuild();
        }
    }

    pub fn rebuild(&mut self) {
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

    /// The best matches for `query` (all entries, shortest first, when the
    /// query is empty).
    pub fn search(&self, query: &str, limit: usize) -> Vec<Entry> {
        if query.is_empty() {
            let mut v = self.entries.clone();
            v.sort_by_key(|e| (e.path.matches('/').count(), e.path.len()));
            v.truncate(limit);
            return v;
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

    /// Check an entry still exists, as what it was.
    pub fn resolve(&self, entry: &Entry) -> Result<PathBuf, String> {
        let p = self.root.join(&entry.path);
        match std::fs::metadata(&p) {
            Ok(m) if m.is_dir() == entry.is_dir => Ok(p),
            Ok(_) => Err(format!("`{}` changed type since it was listed", entry.path)),
            Err(_) => Err(format!("`{}` no longer exists", entry.path)),
        }
    }
}

impl AsRef<str> for Entry {
    fn as_ref(&self) -> &str {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_respects_ignores_matches_fuzzily_and_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/dossier avec espaces")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("src/dossier avec espaces/été.md"), "").unwrap();
        std::fs::write(root.join("target/debug/out"), "").unwrap();
        let mut idx = FileIndex::new(root);
        idx.ensure_fresh();
        assert!(
            idx.search("", 100)
                .iter()
                .all(|e| !e.path.starts_with("target")),
            "gitignored"
        );
        let hits = idx.search("smain", 5);
        assert_eq!(hits[0].path, "src/main.rs");
        let hits = idx.search("ete", 5);
        assert_eq!(
            hits[0].path, "src/dossier avec espaces/été.md",
            "normalised accents"
        );
        assert!(idx.search("espaces", 5).iter().any(|e| e.is_dir));

        // New files appear after a refresh; removed ones are reported.
        std::fs::write(root.join("src/new.rs"), "").unwrap();
        assert!(idx.search("new.rs", 5).is_empty());
        idx.invalidate();
        idx.ensure_fresh();
        let new = idx.search("new.rs", 5)[0].clone();
        std::fs::remove_file(root.join("src/new.rs")).unwrap();
        assert!(idx.resolve(&new).unwrap_err().contains("no longer exists"));

        let mut small = FileIndex::new(root);
        small.max_entries = 2;
        small.rebuild();
        assert!(small.truncated && small.len() == 2);
    }
}
