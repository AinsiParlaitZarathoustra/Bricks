//! One engine per workspace and configuration, shared by every client of
//! the process (agents, sub-agents, the controller).
//!
//! A directory reuses the engine of a workspace that contains it, unless a
//! repository boundary (a `.git` file or folder: another checkout or a
//! worktree) lies between them: worktrees and other checkouts get their
//! own engine.

use crate::config::SemanticConfig;
use crate::engine::SemanticEngine;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[derive(Default)]
pub struct SemanticRegistry {
    engines: Mutex<Vec<(PathBuf, String, Arc<SemanticEngine>)>>,
}

static GLOBAL: OnceLock<SemanticRegistry> = OnceLock::new();

impl SemanticRegistry {
    pub fn global() -> &'static SemanticRegistry {
        GLOBAL.get_or_init(SemanticRegistry::default)
    }

    /// The engine for `dir`: an existing compatible one, or a new one
    /// rooted at `dir`.
    pub fn engine_for(&self, dir: &Path, config: &SemanticConfig) -> Arc<SemanticEngine> {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let fp = config.fingerprint();
        let mut engines = self.engines.lock();
        if let Some(e) = find(&engines, &dir, Some(&fp)) {
            return e;
        }
        let e = SemanticEngine::new(dir.clone(), config.clone());
        engines.push((dir, fp, Arc::clone(&e)));
        e
    }

    /// An existing engine covering `dir`, whatever its configuration.
    pub fn existing_for(&self, dir: &Path) -> Option<Arc<SemanticEngine>> {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        find(&self.engines.lock(), &dir, None)
    }

    /// Every engine covering `dir` (to notify changes).
    pub fn all_for(&self, dir: &Path) -> Vec<Arc<SemanticEngine>> {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        self.engines
            .lock()
            .iter()
            .filter(|(root, _, _)| covers(root, &dir) || covers(&dir, root))
            .map(|(_, _, e)| Arc::clone(e))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.engines.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn find(
    engines: &[(PathBuf, String, Arc<SemanticEngine>)],
    dir: &Path,
    fp: Option<&str>,
) -> Option<Arc<SemanticEngine>> {
    engines
        .iter()
        .filter(|(root, f, _)| fp.is_none_or(|fp| f == fp) && covers(root, dir))
        // The innermost workspace wins.
        .max_by_key(|(root, _, _)| root.components().count())
        .map(|(_, _, e)| Arc::clone(e))
}

/// `root` contains `dir` with no repository boundary in between.
fn covers(root: &Path, dir: &Path) -> bool {
    if !dir.starts_with(root) {
        return false;
    }
    let mut d = Some(dir);
    while let Some(p) = d {
        if p == root {
            return true;
        }
        if p.join(".git").exists() {
            return false;
        }
        d = p.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_within_a_workspace_not_across_worktrees_or_configs() {
        let ws = tempfile::tempdir().unwrap();
        let sub = ws.path().join("crates/a");
        let wt = ws.path().join(".worktrees/feature");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: elsewhere\n").unwrap();
        let reg = SemanticRegistry::default();
        let cfg = SemanticConfig::default();
        let a = reg.engine_for(ws.path(), &cfg);
        let b = reg.engine_for(&sub, &cfg);
        assert!(
            Arc::ptr_eq(&a, &b),
            "a sub-folder shares its workspace's engine"
        );
        let w = reg.engine_for(&wt, &cfg);
        assert!(!Arc::ptr_eq(&a, &w), "a worktree has its own engine");
        let other = SemanticConfig {
            max_results: 3,
            ..SemanticConfig::default()
        };
        let c = reg.engine_for(ws.path(), &other);
        assert!(
            !Arc::ptr_eq(&a, &c),
            "another configuration, another engine"
        );
        assert_eq!(reg.len(), 3);
    }
}
