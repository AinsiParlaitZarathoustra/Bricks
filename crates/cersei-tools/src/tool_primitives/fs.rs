//! Async file operation primitives.
//!
//! Read, write, edit, diff, and patch files. All async via tokio::fs.

use super::diff;
use std::path::Path;

/// File content with metadata.
#[derive(Debug, Clone)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub total_lines: usize,
    pub offset: usize,
    pub lines_returned: usize,
}

/// File metadata.
#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub path: String,
    pub size_bytes: u64,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub modified: Option<u64>,
    pub readonly: bool,
}

/// Result of an edit operation.
#[derive(Debug, Clone)]
pub struct EditResult {
    pub replacements_made: usize,
    /// How the text was located.
    pub stage: super::replace::Stage,
    /// Replaced regions of the original (1-based inclusive line ranges).
    pub ranges: Vec<(usize, usize)>,
}

/// Edit errors. In every case the file is left byte-for-byte unchanged.
#[derive(Debug)]
pub enum EditError {
    Io(std::io::Error),
    /// Refused by the matcher: absent, ambiguous, or not an admissible
    /// variation (see [`super::replace`]).
    Refused(super::replace::ReplaceError),
    /// The file changed between the resolution of the match and the write.
    Changed,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Refused(e) => write!(f, "{}", super::replace::describe_failure(e, "the file")),
            Self::Changed => write!(f, "the file was modified while the edit was being prepared"),
        }
    }
}

impl std::error::Error for EditError {}

impl From<std::io::Error> for EditError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// At most `max` characters of `s`, with an ellipsis when cut.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Replace `path` with `content` through a temporary file in the same
/// directory and a rename, keeping the original permissions: a reader never
/// sees a half-written file.
pub async fn write_atomic(path: &Path, content: &[u8]) -> Result<(), std::io::Error> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(
        ".{name}.bricks-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let perms = tokio::fs::metadata(path)
        .await
        .ok()
        .map(|m| m.permissions());
    tokio::fs::write(&tmp, content).await?;
    if let Some(p) = perms {
        let _ = tokio::fs::set_permissions(&tmp, p).await;
    }
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

/// What a file looks like from its first bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    /// UTF-8 text (`bom`: it starts with a UTF-8 byte-order mark, not shown).
    Text { bom: bool },
    /// Not text; `mime` is a guess from magic bytes.
    Binary { mime: &'static str },
    /// Text in UTF-16 (not supported).
    Utf16,
    /// Text-like but not valid UTF-8 (probably Latin-1 / Windows-1252).
    OtherEncoding,
}

/// A window of a text file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub kind: FileKind,
    pub size: u64,
    /// `(line number from 1, text without its line ending)`.
    pub lines: Vec<(usize, String)>,
    /// Counted by reading the whole file (exact).
    pub total_lines: usize,
    pub offset: usize,
    /// Lines of the window that held invalid UTF-8 (shown with U+FFFD).
    pub invalid_lines: usize,
    /// Lines of the window cut at the per-line limit.
    pub clipped_lines: usize,
    pub crlf: bool,
    /// Size or modification time changed while reading.
    pub changed_during_read: bool,
}

fn sniff(head: &[u8]) -> FileKind {
    if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return FileKind::Text { bom: true };
    }
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return FileKind::Utf16;
    }
    let magic: &[(&[u8], &str)] = &[
        (b"\x89PNG", "image/png"),
        (b"\xFF\xD8\xFF", "image/jpeg"),
        (b"GIF8", "image/gif"),
        (b"%PDF", "application/pdf"),
        (b"PK\x03\x04", "application/zip"),
        (b"\x1F\x8B", "application/gzip"),
        (b"\x7FELF", "application/x-elf"),
        (b"\xCF\xFA\xED\xFE", "application/x-mach-binary"),
        (b"\x00asm", "application/wasm"),
        (b"SQLite format 3", "application/vnd.sqlite3"),
        (b"RIFF", "application/riff"),
    ];
    for (m, mime) in magic {
        if head.starts_with(m) {
            return FileKind::Binary { mime };
        }
    }
    let nul = head.iter().filter(|b| **b == 0).count();
    if nul > 0 {
        // ASCII in UTF-16 without a BOM: every other byte is NUL.
        let odd = head.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
        let even = head.iter().step_by(2).filter(|b| **b == 0).count();
        let half = head.len() / 2;
        if half > 8 && (odd * 10 >= half * 9 || even * 10 >= half * 9) {
            return FileKind::Utf16;
        }
        return FileKind::Binary {
            mime: "application/octet-stream",
        };
    }
    // Invalid UTF-8 in the sample (ignoring a character cut at its end).
    match std::str::from_utf8(head) {
        Ok(_) => FileKind::Text { bom: false },
        Err(e) if e.error_len().is_none() => FileKind::Text { bom: false },
        Err(_) => FileKind::OtherEncoding,
    }
}

fn stamp(path: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    std::fs::metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

/// Read lines `offset+1 ..= offset+limit` (all when `limit` is 0), counting
/// the total by streaming: memory stays bounded by the window.
pub fn read_window(
    path: &Path,
    offset: usize,
    limit: usize,
    max_line_chars: usize,
) -> std::io::Result<Window> {
    use std::io::{BufRead, Read};
    let before = stamp(path);
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut head = vec![0u8; 8192];
    let n = file.read(&mut head)?;
    head.truncate(n);
    let kind = sniff(&head);
    let mut window = Window {
        kind: kind.clone(),
        size,
        lines: Vec::new(),
        total_lines: 0,
        offset,
        invalid_lines: 0,
        clipped_lines: 0,
        crlf: false,
        changed_during_read: false,
    };
    if !matches!(kind, FileKind::Text { .. }) {
        return Ok(window);
    }
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    if let FileKind::Text { bom: true } = kind {
        let mut skip = [0u8; 3];
        reader.read_exact(&mut skip)?;
    }
    let end = if limit == 0 {
        usize::MAX
    } else {
        offset.saturating_add(limit)
    };
    let mut buf = Vec::new();
    let mut index = 0usize;
    loop {
        buf.clear();
        let read = reader.read_until(b'\n', &mut buf)?;
        if read == 0 {
            break;
        }
        if index >= offset && index < end {
            if buf.last() == Some(&b'\n') {
                buf.pop();
            }
            if buf.last() == Some(&b'\r') {
                buf.pop();
                window.crlf = true;
            }
            let text = match std::str::from_utf8(&buf) {
                Ok(t) => t.to_string(),
                Err(_) => {
                    window.invalid_lines += 1;
                    String::from_utf8_lossy(&buf).into_owned()
                }
            };
            let text = if text.chars().count() > max_line_chars {
                window.clipped_lines += 1;
                let extra = text.chars().count() - max_line_chars;
                format!(
                    "{}… [+{extra} characters on this line]",
                    text.chars().take(max_line_chars).collect::<String>()
                )
            } else {
                text
            };
            window.lines.push((index + 1, text));
        }
        index += 1;
    }
    window.total_lines = index;
    window.changed_during_read = before != stamp(path);
    Ok(window)
}

/// Format window lines as `  12 | text`, aligned on the widest number.
pub fn number_lines(lines: &[(usize, String)], widest: usize) -> String {
    let w = widest.max(1).to_string().len();
    let mut out = String::new();
    for (n, text) in lines {
        out.push_str(&format!("{n:>w$} | {text}\n"));
    }
    out
}

/// Read a file with optional line offset and limit, numbered from 1
/// (`  12 | text`). `offset` is 0-based; a `limit` of 0 reads everything.
pub async fn read_file(
    path: &Path,
    offset: usize,
    limit: usize,
) -> Result<FileContent, std::io::Error> {
    let p = path.to_path_buf();
    let w = tokio::task::spawn_blocking(move || read_window(&p, offset, limit, usize::MAX))
        .await
        .map_err(std::io::Error::other)??;
    if !matches!(w.kind, FileKind::Text { .. }) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a UTF-8 text file",
        ));
    }
    Ok(FileContent {
        path: path.display().to_string(),
        content: number_lines(&w.lines, w.total_lines),
        total_lines: w.total_lines,
        offset,
        lines_returned: w.lines.len(),
    })
}

/// Write content to a file, creating parent directories automatically.
pub async fn write_file(path: &Path, content: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await
}

/// Replace text in a file (see [`super::replace`] for the three matching
/// stages and what each tolerates).
///
/// The file is re-read just before writing; if it changed since the match
/// was resolved, nothing is written ([`EditError::Changed`]). The write is
/// atomic (temporary file + rename).
pub async fn edit_file(
    path: &Path,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<EditResult, EditError> {
    let original = tokio::fs::read(path).await?;
    let content = String::from_utf8(original.clone()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the file is not valid UTF-8 text",
        )
    })?;
    let plan = super::replace::plan_edit(&content, old_text, new_text, replace_all, path.to_str())
        .map_err(EditError::Refused)?;
    #[cfg(test)]
    tests::BEFORE_WRITE.with(|h| {
        if let Some(f) = h.borrow_mut().take() {
            f(path)
        }
    });
    if tokio::fs::read(path).await? != original {
        return Err(EditError::Changed);
    }
    write_atomic(path, plan.content.as_bytes()).await?;
    Ok(EditResult {
        replacements_made: plan.replacements,
        stage: plan.stage,
        ranges: plan.ranges,
    })
}

/// Produce a unified diff between the file's current content and proposed new content.
pub async fn diff_file(
    path: &Path,
    new_content: &str,
    context_lines: usize,
) -> Result<String, std::io::Error> {
    let old_content = tokio::fs::read_to_string(path).await?;
    Ok(diff::unified_diff(&old_content, new_content, context_lines))
}

/// Apply a unified diff patch to a file.
pub async fn patch_file(path: &Path, patch: &str) -> Result<(), PatchFileError> {
    let original = tokio::fs::read_to_string(path)
        .await
        .map_err(PatchFileError::Io)?;
    let patched =
        diff::apply_patch(&original, patch).map_err(|e| PatchFileError::Patch(e.message))?;
    tokio::fs::write(path, &patched)
        .await
        .map_err(PatchFileError::Io)?;
    Ok(())
}

/// Errors from patch_file.
#[derive(Debug)]
pub enum PatchFileError {
    Io(std::io::Error),
    Patch(String),
}

impl std::fmt::Display for PatchFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Patch(msg) => write!(f, "patch failed: {msg}"),
        }
    }
}

impl std::error::Error for PatchFileError {}

/// Check if a file exists.
pub async fn file_exists(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

/// Get file size in bytes.
pub async fn file_size(path: &Path) -> Result<u64, std::io::Error> {
    let meta = tokio::fs::metadata(path).await?;
    Ok(meta.len())
}

/// Get detailed file metadata.
pub async fn file_metadata(path: &Path) -> Result<FileMetadata, std::io::Error> {
    let meta = tokio::fs::metadata(path).await?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    Ok(FileMetadata {
        path: path.display().to_string(),
        size_bytes: meta.len(),
        is_file: meta.is_file(),
        is_dir: meta.is_dir(),
        is_symlink: meta.file_type().is_symlink(),
        modified,
        readonly: meta.permissions().readonly(),
    })
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_write() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");

        write_file(&path, "line1\nline2\nline3\n").await.unwrap();
        let fc = read_file(&path, 0, 0).await.unwrap();
        assert_eq!(fc.total_lines, 3);
        assert_eq!(fc.lines_returned, 3);
        assert!(fc.content.contains("line2"));
    }

    #[tokio::test]
    async fn test_read_with_offset() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        write_file(&path, "a\nb\nc\nd\ne\n").await.unwrap();

        let fc = read_file(&path, 2, 2).await.unwrap();
        assert_eq!(fc.lines_returned, 2);
        assert!(fc.content.contains("c"));
        assert!(fc.content.contains("d"));
        assert!(!fc.content.contains("a"));
    }

    #[tokio::test]
    async fn test_edit_single() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        write_file(&path, "hello world").await.unwrap();

        let result = edit_file(&path, "world", "earth", false).await.unwrap();
        assert_eq!(result.replacements_made, 1);

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "hello earth");
    }

    type BeforeWrite = Option<Box<dyn FnOnce(&Path)>>;

    thread_local! {
        /// Runs between the resolution of an edit and its write (tests only).
        pub(super) static BEFORE_WRITE: std::cell::RefCell<BeforeWrite> = std::cell::RefCell::new(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_file_changed_before_the_write_is_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("race.txt");
        write_file(&path, "alpha\nbeta\n").await.unwrap();
        BEFORE_WRITE.with(|h| {
            *h.borrow_mut() = Some(Box::new(|p: &Path| {
                std::fs::write(p, "alpha\nbeta\nadded by someone else\n").unwrap()
            }))
        });
        let result = edit_file(&path, "beta", "BETA", false).await;
        assert!(matches!(result, Err(EditError::Changed)), "{result:?}");
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "alpha\nbeta\nadded by someone else\n"
        );
    }

    #[tokio::test]
    async fn test_edit_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        write_file(&path, "hello").await.unwrap();

        let result = edit_file(&path, "xyz", "abc", false).await;
        assert!(matches!(
            result,
            Err(EditError::Refused(
                crate::tool_primitives::replace::ReplaceError::NotFound { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn test_edit_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        write_file(&path, "aaa bbb aaa").await.unwrap();

        let result = edit_file(&path, "aaa", "ccc", false).await;
        assert!(matches!(
            result,
            Err(EditError::Refused(
                crate::tool_primitives::replace::ReplaceError::Ambiguous { count: 2, .. }
            ))
        ));
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "aaa bbb aaa"
        );

        // replace_all works
        let result = edit_file(&path, "aaa", "ccc", true).await.unwrap();
        assert_eq!(result.replacements_made, 2);
    }

    #[tokio::test]
    async fn test_diff_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        write_file(&path, "hello\nworld\n").await.unwrap();

        let d = diff_file(&path, "hello\nearth\n", 3).await.unwrap();
        assert!(d.contains("-world"));
        assert!(d.contains("+earth"));
    }

    #[tokio::test]
    async fn test_file_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.txt");
        write_file(&path, "content").await.unwrap();

        let meta = file_metadata(&path).await.unwrap();
        assert!(meta.is_file);
        assert!(!meta.is_dir);
        assert_eq!(meta.size_bytes, 7);
    }

    #[tokio::test]
    async fn test_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!file_exists(&tmp.path().join("nope")).await);

        let path = tmp.path().join("yes.txt");
        write_file(&path, "").await.unwrap();
        assert!(file_exists(&path).await);
    }
}
