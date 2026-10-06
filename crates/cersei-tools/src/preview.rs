//! Previews of the file changes a tool call would make, computed without
//! writing anything, so an approval can show them before the write.
//!
//! A preview records, for each file, its SHA-256 at the time of the preview.
//! [`ChangePreview::check_current`] tells whether the files are still what
//! the preview was computed from: an approval given for an older state is
//! not applied to a newer one.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// What happens to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Create,
    Modify,
    Delete,
}

/// One file of a preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// The path as the tool call names it (relative paths stay relative).
    pub path: String,
    /// The file actually written.
    pub absolute: PathBuf,
    pub kind: ChangeKind,
    /// SHA-256 (hex) of the file when the preview was made; `None` when the
    /// file did not exist.
    pub before_sha256: Option<String>,
    /// Unified diff, old → new.
    pub diff: String,
    pub added: usize,
    pub removed: usize,
}

/// The changes one tool call would make.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChangePreview {
    pub files: Vec<FileChange>,
    /// The call would be refused by the tool itself (nothing would be
    /// written): why.
    pub refusal: Option<String>,
}

impl ChangePreview {
    pub fn refused(reason: impl Into<String>) -> Self {
        Self {
            files: Vec::new(),
            refusal: Some(reason.into()),
        }
    }

    /// `Ok` when every file is still in the state the preview was computed
    /// from; otherwise the files that changed since.
    pub fn check_current(&self) -> Result<(), Vec<String>> {
        let changed: Vec<String> = self
            .files
            .iter()
            .filter(|f| file_sha256(&f.absolute) != f.before_sha256)
            .map(|f| f.path.clone())
            .collect();
        if changed.is_empty() {
            Ok(())
        } else {
            Err(changed)
        }
    }

    pub fn added(&self) -> usize {
        self.files.iter().map(|f| f.added).sum()
    }

    pub fn removed(&self) -> usize {
        self.files.iter().map(|f| f.removed).sum()
    }
}

/// SHA-256 (hex) of a file's bytes; `None` when it cannot be read.
pub fn file_sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha256_hex(&bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A file change from the old text (`None`: absent) to the new text
/// (`None`: deleted).
pub fn file_change(
    path: &str,
    absolute: &Path,
    before: Option<&str>,
    after: Option<&str>,
) -> FileChange {
    let kind = match (before, after) {
        (None, _) => ChangeKind::Create,
        (Some(_), None) => ChangeKind::Delete,
        (Some(_), Some(_)) => ChangeKind::Modify,
    };
    let (old, new) = (before.unwrap_or(""), after.unwrap_or(""));
    let diff = similar::TextDiff::from_lines(old, new);
    let (mut added, mut removed) = (0, 0);
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    let label = |side: &str, present: bool| {
        if present {
            format!("{side}/{path}")
        } else {
            "/dev/null".to_string()
        }
    };
    let text = diff
        .unified_diff()
        .context_radius(3)
        .header(&label("a", before.is_some()), &label("b", after.is_some()))
        .to_string();
    FileChange {
        path: path.to_string(),
        absolute: absolute.to_path_buf(),
        kind,
        before_sha256: before.map(|_| file_sha256(absolute)).unwrap_or(None),
        diff: text,
        added,
        removed,
    }
}

/// Resolve a path named by a tool call against the working directory.
pub fn resolve(working_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        working_dir.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_records_the_state_it_was_computed_from() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let change = file_change("a.txt", &path, Some("one\ntwo\n"), Some("one\n2\n"));
        assert_eq!(change.kind, ChangeKind::Modify);
        assert_eq!((change.added, change.removed), (1, 1));
        assert!(
            change.diff.contains("-two") && change.diff.contains("+2"),
            "{}",
            change.diff
        );
        let preview = ChangePreview {
            files: vec![change],
            refusal: None,
        };
        assert_eq!(preview.check_current(), Ok(()));
        std::fs::write(&path, "changed meanwhile\n").unwrap();
        assert_eq!(preview.check_current(), Err(vec!["a.txt".to_string()]));

        let created = file_change("new.txt", &dir.path().join("new.txt"), None, Some("x\n"));
        assert_eq!(
            (created.kind, created.before_sha256.clone()),
            (ChangeKind::Create, None)
        );
        assert!(created.diff.contains("/dev/null"));
        let p = ChangePreview {
            files: vec![created],
            refusal: None,
        };
        assert_eq!(p.check_current(), Ok(()), "still absent");
    }
}
