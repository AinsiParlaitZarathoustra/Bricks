//! Where the full text of a transformed output can be read back.
//!
//! Every reduced output names its original. For a file view that is the file
//! itself (read it with `offset`/`limit`); for a command output it is a copy
//! saved by a [`RawStore`] in a session directory, readable with the ordinary
//! `Read` tool. Nothing is promised beyond what was actually saved: when no
//! store is available the header says so.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A saved original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRef {
    pub path: PathBuf,
    pub lines: usize,
    pub bytes: usize,
}

impl RawRef {
    /// How to read it, in the terms of the `Read` tool.
    pub fn hint(&self) -> String {
        format!(
            "Read file_path=\"{}\" ({} lines; use offset/limit to page)",
            self.path.display(),
            self.lines
        )
    }
}

/// Saves originals as files in one directory.
#[derive(Debug)]
pub struct RawStore {
    dir: PathBuf,
    seq: AtomicU64,
}

impl RawStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            seq: AtomicU64::new(0),
        }
    }

    /// A per-session directory under the system temporary directory.
    pub fn for_session(session_id: &str) -> Self {
        Self::new(
            std::env::temp_dir()
                .join("bricks")
                .join("raw-outputs")
                .join(sanitize(session_id, 80)),
        )
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// True when `path` is one of this store's files: reading it back must
    /// never be reduced again.
    pub fn contains(&self, path: &str) -> bool {
        let p = Path::new(path);
        p.starts_with(&self.dir)
            || std::fs::canonicalize(p)
                .ok()
                .zip(std::fs::canonicalize(&self.dir).ok())
                .is_some_and(|(a, d)| a.starts_with(d))
    }

    /// Save `content`; the file name carries a sequence number and `label`.
    pub fn put(&self, label: &str, content: &str) -> std::io::Result<RawRef> {
        std::fs::create_dir_all(&self.dir)?;
        // A store reopened on an existing directory (a restored session)
        // continues the numbering: files already referenced are never
        // overwritten.
        let label = sanitize(label, 60);
        let path = loop {
            let n = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
            let prefix = format!("{n:05}-");
            let taken = std::fs::read_dir(&self.dir)?
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().starts_with(&prefix));
            if !taken {
                break self.dir.join(format!("{prefix}{label}.txt"));
            }
        };
        std::fs::write(&path, content)?;
        Ok(RawRef {
            path,
            lines: content.lines().count(),
            bytes: content.len(),
        })
    }
}

fn sanitize(s: &str, max: usize) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(max)
        .collect();
    if out.is_empty() {
        "output".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_recognises_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = RawStore::new(dir.path().join("raw"));
        let r = store.put("Bash toolu_01/../x", "a\nb\nc\n").unwrap();
        assert_eq!((r.lines, r.bytes), (3, 6));
        assert_eq!(std::fs::read_to_string(&r.path).unwrap(), "a\nb\nc\n");
        assert!(r
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("00001-Bash_toolu_01____x"));
        assert!(store.contains(r.path.to_str().unwrap()));
        assert!(!store.contains("/etc/hosts"));
        assert!(r.hint().contains("3 lines"));

        // Reopened on the same directory: numbering continues.
        let again = RawStore::new(dir.path().join("raw"));
        let r2 = again.put("Bash", "other").unwrap();
        assert!(r2
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("00002-"));
        assert_eq!(std::fs::read_to_string(&r.path).unwrap(), "a\nb\nc\n");
    }
}
