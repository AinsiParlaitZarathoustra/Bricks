//! Previews of the file changes a tool call would make, computed without
//! writing anything, so an approval can show them before the write.
//!
//! A preview records, for each file, the SHA-256 of the text its diff was
//! computed from. [`ChangePreview::check_current`] tells whether the files are
//! still that text: an approval given for an older state is not applied to a
//! newer one.
//!
//! The approval travels with the call as an [`ApprovedState`]. The tool that
//! executes the call checks it again under the file's lock, just before it
//! writes ([`ensure_still_approved`]): a file changed in between is not written,
//! and the runner asks again.

use crate::ToolResult;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Key of the `metadata` of a [`ToolResult`] that reports a change refused
/// because the file changed after it was approved. The runner decides again
/// on the current state; nothing was written.
pub const STALE_APPROVAL_KEY: &str = "approval_stale";

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
        // The text the diff was computed from, not whatever the disk holds
        // when the preview is read back: the approval is for this text.
        before_sha256: before.map(|text| sha256_hex(text.as_bytes())),
        diff: text,
        added,
        removed,
    }
}

/// The state of the files an approval was given for: for each file of the
/// preview, its absolute path and the SHA-256 of its text before the change
/// (`None`: the file did not exist).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApprovedState {
    files: Vec<(PathBuf, Option<String>)>,
}

impl ApprovedState {
    pub fn of(preview: &ChangePreview) -> Self {
        Self {
            files: preview
                .files
                .iter()
                .map(|f| (f.absolute.clone(), f.before_sha256.clone()))
                .collect(),
        }
    }

    /// The state approved for `absolute`, if the approval covers it.
    pub fn expected(&self, absolute: &Path) -> Option<&Option<String>> {
        self.files
            .iter()
            .find(|(p, _)| p == absolute)
            .map(|(_, sha)| sha)
    }
}

/// Under the file's lock, before a write: `Err` with the refusal when the
/// call carries an approval for `absolute` and the file's current bytes
/// (`None`: absent) are not the state it was approved for. Nothing may be
/// written then; the runner decides again.
///
/// Without an approval in `ctx`, or for a file the approval does not cover,
/// there is nothing to check and the call proceeds.
pub fn ensure_still_approved(
    ctx: &crate::ToolContext,
    absolute: &Path,
    shown: &str,
    current: Option<&[u8]>,
) -> Result<(), ToolResult> {
    let Some(approved) = ctx.extensions.get::<ApprovedState>() else {
        return Ok(());
    };
    let Some(expected) = approved.expected(absolute) else {
        return Ok(());
    };
    if current.map(sha256_hex) == *expected {
        return Ok(());
    }
    Err(stale_approval_result(shown))
}

/// The refusal for a change whose file moved on after its approval.
pub fn stale_approval_result(shown: &str) -> ToolResult {
    ToolResult::error(format!(
        "{shown} changed after its change was approved, so nothing was written. Read it again \
         and retry."
    ))
    .with_metadata(json!({ STALE_APPROVAL_KEY: true }))
}

/// Whether `result` is the refusal of [`stale_approval_result`].
pub fn is_stale_approval(result: &ToolResult) -> bool {
    result
        .metadata
        .as_ref()
        .and_then(|m| m.get(STALE_APPROVAL_KEY))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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
            files: vec![change.clone()],
            refusal: None,
        };
        assert_eq!(preview.check_current(), Ok(()));
        // The recorded state is the text the diff came from, byte for byte.
        assert_eq!(
            change.before_sha256.as_deref(),
            Some(sha256_hex(b"one\ntwo\n").as_str())
        );
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
