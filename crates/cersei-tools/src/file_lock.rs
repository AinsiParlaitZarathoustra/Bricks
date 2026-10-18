//! Per-file write locks, shared by the whole process.
//!
//! A write-capable tool takes the lock of each file it writes from the moment
//! it reads the file until the write has landed. Two calls that edit different
//! parts of one file therefore run one after the other instead of each
//! rewriting the other's change, and a call never waits for an approval while
//! holding one: the tools take their locks only after the decision.
//!
//! The lock of a file is found by the path its writes land in (see
//! `tool_primitives::fs::write_target`), so a symbolic link and its target share
//! one lock. A file that does not exist yet is keyed by the
//! canonical path of its nearest existing ancestor followed by the rest of its
//! name. Several locks are always taken in sorted order, so two calls cannot
//! wait for each other.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use tokio::sync::{Mutex, OwnedMutexGuard};

type Table = parking_lot::Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>;

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

/// The identity a file is locked under: the path its writes land in
/// ([`crate::tool_primitives::fs::write_target`]), so a symbolic link, a chain
/// of them, and the file they point to (dangling or not) share one lock.
pub fn lock_key(path: &Path) -> PathBuf {
    crate::tool_primitives::fs::write_target(path).unwrap_or_else(|_| path.to_path_buf())
}

fn lock_for(key: &Path) -> Arc<Mutex<()>> {
    let mut table = table().lock();
    if let Some(existing) = table.get(key).and_then(Weak::upgrade) {
        return existing;
    }
    // Entries whose files are no longer locked go away, so the table does not
    // keep every path a process ever wrote.
    table.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(Mutex::new(()));
    table.insert(key.to_path_buf(), Arc::downgrade(&lock));
    lock
}

/// The locks of a set of files, held until dropped.
pub struct FileLocks {
    _held: Vec<OwnedMutexGuard<()>>,
}

/// Waits for, then holds, the locks of `paths`. Each file is locked once even
/// when it is named twice, through any of its spellings.
pub async fn lock_paths(paths: &[PathBuf]) -> FileLocks {
    let mut keys: Vec<PathBuf> = paths.iter().map(|p| lock_key(p)).collect();
    keys.sort();
    keys.dedup();
    let mut held = Vec::with_capacity(keys.len());
    for key in keys {
        held.push(lock_for(&key).lock_owned().await);
    }
    FileLocks { _held: held }
}

/// Test-only hooks run inside a write's critical section, after the file was
/// read and checked and before it is written. They are keyed by file, so tests
/// that run in parallel in one process do not see each other's hooks.
#[cfg(test)]
pub(crate) mod hooks {
    use super::*;

    type Hook = Arc<dyn Fn() + Send + Sync>;

    fn hooks() -> &'static parking_lot::Mutex<Vec<(PathBuf, Hook)>> {
        static HOOKS: OnceLock<parking_lot::Mutex<Vec<(PathBuf, Hook)>>> = OnceLock::new();
        HOOKS.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
    }

    /// Runs `f` each time `path` is about to be written.
    pub fn before_commit(path: &Path, f: impl Fn() + Send + Sync + 'static) {
        hooks().lock().push((lock_key(path), Arc::new(f)));
    }

    /// Called by the tools in their critical section.
    pub fn run_before_commit(path: &Path) {
        let key = lock_key(path);
        let matching: Vec<Hook> = hooks()
            .lock()
            .iter()
            .filter(|(p, _)| *p == key)
            .map(|(_, f)| Arc::clone(f))
            .collect();
        for f in matching {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_symlink_and_its_target_share_one_key() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "x").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(lock_key(&link), lock_key(&target));
    }

    /// The key of a link, a chain of links and a dangling link is the key of
    /// the file a write through them lands in: one lock for one file.
    #[test]
    fn a_chain_and_a_dangling_link_share_the_key_of_the_file_they_write() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        std::fs::write(&target, "x").unwrap();
        let middle = dir.path().join("middle");
        std::os::unix::fs::symlink("real.txt", &middle).unwrap();
        let chained = dir.path().join("chained");
        std::os::unix::fs::symlink(&middle, &chained).unwrap();
        let dangling = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("made.txt"), &dangling).unwrap();

        assert_eq!(lock_key(&chained), lock_key(&target));
        assert_eq!(
            lock_key(&dangling),
            std::fs::canonicalize(dir.path()).unwrap().join("made.txt")
        );
        assert_eq!(
            lock_key(&dangling),
            crate::tool_primitives::fs::write_target(&dangling).unwrap()
        );
    }

    #[test]
    fn a_missing_file_is_keyed_under_its_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let direct = dir.path().join("new.txt");
        let dotted = dir.path().join("sub").join("..").join("new.txt");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert_eq!(lock_key(&direct), lock_key(&dotted));
        assert_eq!(
            lock_key(&direct),
            std::fs::canonicalize(dir.path()).unwrap().join("new.txt")
        );
    }

    /// Edits of two different files do not wait for each other.
    #[tokio::test]
    async fn different_files_do_not_wait_for_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let _held_a = lock_paths(std::slice::from_ref(&a)).await;
        let got_b =
            tokio::time::timeout(Duration::from_secs(2), lock_paths(std::slice::from_ref(&b)))
                .await;
        assert!(got_b.is_ok(), "b waited for an unrelated lock on a");
    }

    /// A file already held is waited for, through any spelling of its path.
    #[tokio::test]
    async fn one_file_is_locked_through_its_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("t.txt");
        std::fs::write(&target, "x").unwrap();
        let link = dir.path().join("l.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let _held = lock_paths(std::slice::from_ref(&target)).await;
        let waited = tokio::time::timeout(
            Duration::from_millis(100),
            lock_paths(std::slice::from_ref(&link)),
        )
        .await;
        assert!(
            waited.is_err(),
            "the alias did not wait for the locked file"
        );
    }

    /// Naming one file twice takes its lock once: no self-deadlock.
    #[tokio::test]
    async fn a_file_named_twice_is_locked_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let spelled = dir.path().join(".").join("a.txt");
        let got = tokio::time::timeout(Duration::from_secs(2), lock_paths(&[a, spelled])).await;
        assert!(got.is_ok());
    }
}
