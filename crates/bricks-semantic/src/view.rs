//! Documents, views and per-query snapshots.
//!
//! * The **shared view** is the disk: approved, applied edits are on disk,
//!   so it sees them on the next read.
//! * A **private view** overlays buffers a client really gave to the engine
//!   (unsaved editor content). Only the holder of its handle can query it.
//! * A **preview view** overlays pending diffs on another view, isolated:
//!   nothing it contains reaches any other view.
//!
//! A [`Snapshot`] reads each document at most once per query, so every
//! offset, tree and excerpt of a response refers to the same text.

use crate::position::PositionMapper;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Identity of one document version: the hash of its exact bytes, plus the
/// buffer version when the content came from a buffer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Revision {
    /// First 16 hex digits of the SHA-256 of the content.
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buffer_version: Option<i64>,
}

pub fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Where a document's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocSource {
    Disk,
    Buffer,
    Preview,
}

/// One exact document version.
#[derive(Debug)]
pub struct Document {
    /// Absolute path.
    pub path: PathBuf,
    pub text: Arc<str>,
    pub revision: Revision,
    pub source: DocSource,
    mapper: OnceLock<PositionMapper>,
}

impl Document {
    pub fn new(path: PathBuf, text: Arc<str>, source: DocSource, version: Option<i64>) -> Self {
        let revision = Revision {
            hash: content_hash(&text),
            buffer_version: version,
        };
        Self {
            path,
            text,
            revision,
            source,
            mapper: OnceLock::new(),
        }
    }

    pub fn mapper(&self) -> &PositionMapper {
        self.mapper
            .get_or_init(|| PositionMapper::new(Arc::clone(&self.text)))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Overlay {
    /// `None`: the document is deleted in this view.
    pub text: Option<Arc<str>>,
    pub version: i64,
    pub source: DocSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Shared,
    Private,
    Preview,
}

pub(crate) struct ViewInner {
    pub id: u64,
    pub kind: ViewKind,
    pub overlays: RwLock<HashMap<PathBuf, Overlay>>,
    /// Bumped on every overlay change.
    pub generation: AtomicU64,
}

/// A handle on a view. The shared view's handle is public; a private or
/// preview handle is a capability: only its holder can read its overlays.
#[derive(Clone)]
pub struct ViewHandle {
    pub(crate) inner: Arc<ViewInner>,
}

static NEXT_VIEW: AtomicU64 = AtomicU64::new(1);

impl ViewHandle {
    pub(crate) fn new(kind: ViewKind, overlays: HashMap<PathBuf, Overlay>) -> Self {
        Self {
            inner: Arc::new(ViewInner {
                id: if kind == ViewKind::Shared {
                    0
                } else {
                    NEXT_VIEW.fetch_add(1, Ordering::Relaxed)
                },
                kind,
                overlays: RwLock::new(overlays),
                generation: AtomicU64::new(0),
            }),
        }
    }

    pub fn id(&self) -> u64 {
        self.inner.id
    }

    pub fn kind(&self) -> ViewKind {
        self.inner.kind
    }

    pub fn is_shared(&self) -> bool {
        self.inner.kind == ViewKind::Shared
    }

    /// Register (or update) an unsaved buffer in a private view. The
    /// shared view refuses buffers: it is the disk.
    pub fn set_buffer(
        &self,
        path: impl Into<PathBuf>,
        text: impl Into<Arc<str>>,
        version: i64,
    ) -> Result<(), String> {
        if self.inner.kind != ViewKind::Private {
            return Err("buffers can only be registered in a private view".into());
        }
        self.inner.overlays.write().insert(
            path.into(),
            Overlay {
                text: Some(text.into()),
                version,
                source: DocSource::Buffer,
            },
        );
        self.inner.generation.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Drop a buffer (the disk version becomes visible again).
    pub fn close_buffer(&self, path: &Path) {
        if self.inner.overlays.write().remove(path).is_some() {
            self.inner.generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Whether this view overlays `path`.
    pub fn overlays(&self, path: &Path) -> bool {
        self.inner.overlays.read().contains_key(path)
    }

    /// Paths this view adds or changes (for the search walk: a buffer for
    /// a file not on disk yet is still searchable).
    pub fn overlay_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.inner.overlays.read().keys().cloned().collect();
        v.sort();
        v
    }

    pub(crate) fn overlay(&self, path: &Path) -> Option<Overlay> {
        self.inner.overlays.read().get(path).cloned()
    }

    /// A preview of `changes` on top of this view: `Some(text)` replaces a
    /// file, `None` deletes it. The preview is isolated.
    pub fn preview(&self, changes: Vec<(PathBuf, Option<String>)>) -> ViewHandle {
        let mut overlays = self.inner.overlays.read().clone();
        for (path, text) in changes {
            let version = overlays.get(&path).map(|o| o.version + 1).unwrap_or(1);
            overlays.insert(
                path,
                Overlay {
                    text: text.map(Arc::from),
                    version,
                    source: DocSource::Preview,
                },
            );
        }
        ViewHandle::new(ViewKind::Preview, overlays)
    }
}

/// Why a document could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum Unreadable {
    NotFound,
    Deleted,
    TooLarge { bytes: u64, limit: u64 },
    NotUtf8,
    Binary,
    Io { message: String },
}

impl Unreadable {
    pub fn label(&self) -> String {
        match self {
            Self::NotFound => "not found".into(),
            Self::Deleted => "deleted in this view".into(),
            Self::TooLarge { bytes, limit } => format!("{bytes} bytes > limit {limit}"),
            Self::NotUtf8 => "not UTF-8".into(),
            Self::Binary => "binary".into(),
            Self::Io { message } => message.clone(),
        }
    }
}

/// The documents one query sees.
pub struct Snapshot {
    pub view: ViewHandle,
    pub max_file_bytes: u64,
    docs: Mutex<HashMap<PathBuf, Result<Arc<Document>, Unreadable>>>,
}

impl Snapshot {
    pub fn new(view: ViewHandle, max_file_bytes: u64) -> Self {
        Self {
            view,
            max_file_bytes,
            docs: Mutex::new(HashMap::new()),
        }
    }

    /// The document at `path` (absolute), read once per snapshot.
    pub fn document(&self, path: &Path) -> Result<Arc<Document>, Unreadable> {
        if let Some(d) = self.docs.lock().get(path) {
            return d.clone();
        }
        let loaded = self.load(path);
        self.docs
            .lock()
            .entry(path.to_path_buf())
            .or_insert(loaded)
            .clone()
    }

    /// A document already read by this snapshot, or an overlay of its
    /// view (loaded now); `None` for a disk file not read yet.
    pub fn known(&self, path: &Path) -> Option<Result<Arc<Document>, Unreadable>> {
        if let Some(d) = self.docs.lock().get(path) {
            return Some(d.clone());
        }
        if self.view.overlay(path).is_some() {
            return Some(self.document(path));
        }
        None
    }

    /// The bytes of a disk file, within the size limit, not binary.
    pub fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, Unreadable> {
        let meta = std::fs::metadata(path).map_err(|e| io_unreadable(&e))?;
        if meta.len() > self.max_file_bytes {
            return Err(Unreadable::TooLarge {
                bytes: meta.len(),
                limit: self.max_file_bytes,
            });
        }
        let bytes = std::fs::read(path).map_err(|e| io_unreadable(&e))?;
        // Check again: the file may have grown since `stat`.
        if bytes.len() as u64 > self.max_file_bytes {
            return Err(Unreadable::TooLarge {
                bytes: bytes.len() as u64,
                limit: self.max_file_bytes,
            });
        }
        if bytes.iter().take(8192).any(|&b| b == 0) {
            return Err(Unreadable::Binary);
        }
        Ok(bytes)
    }

    /// Keep `text` (read from disk by the caller) as this snapshot's
    /// version of `path`, unless the snapshot already has one: the first
    /// version read stays the reference.
    pub fn adopt(&self, path: &Path, text: String) -> Arc<Document> {
        let mut docs = self.docs.lock();
        if let Some(Ok(d)) = docs.get(path) {
            return Arc::clone(d);
        }
        let doc = Arc::new(Document::new(
            path.to_path_buf(),
            Arc::from(text),
            DocSource::Disk,
            None,
        ));
        docs.insert(path.to_path_buf(), Ok(Arc::clone(&doc)));
        doc
    }

    fn load(&self, path: &Path) -> Result<Arc<Document>, Unreadable> {
        if let Some(o) = self.view.overlay(path) {
            let text = o.text.ok_or(Unreadable::Deleted)?;
            return Ok(Arc::new(Document::new(
                path.to_path_buf(),
                text,
                o.source,
                Some(o.version),
            )));
        }
        let bytes = self.read_bytes(path)?;
        let text = String::from_utf8(bytes).map_err(|_| Unreadable::NotUtf8)?;
        Ok(Arc::new(Document::new(
            path.to_path_buf(),
            Arc::from(text),
            DocSource::Disk,
            None,
        )))
    }

    /// Documents read so far.
    pub fn loaded(&self) -> Vec<Arc<Document>> {
        let mut v: Vec<Arc<Document>> = self
            .docs
            .lock()
            .values()
            .filter_map(|d| d.as_ref().ok().cloned())
            .collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    /// Disk documents of `paths` whose content changed since this snapshot
    /// read them (the response is then flagged stale).
    pub fn changed_since(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let docs = self.docs.lock();
        let mut changed = Vec::new();
        for p in paths {
            let Some(Ok(doc)) = docs.get(p) else { continue };
            if doc.source != DocSource::Disk {
                continue;
            }
            let now = std::fs::read(p)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .map(|t| content_hash(&t));
            if now.as_deref() != Some(doc.revision.hash.as_str()) {
                changed.push(p.clone());
            }
        }
        changed
    }
}

fn io_unreadable(e: &std::io::Error) -> Unreadable {
    if e.kind() == std::io::ErrorKind::NotFound {
        Unreadable::NotFound
    } else {
        Unreadable::Io {
            message: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reads_once_and_detects_changes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.rs");
        std::fs::write(&f, "fn a() {}\n").unwrap();
        let snap = Snapshot::new(ViewHandle::new(ViewKind::Shared, HashMap::new()), 1 << 20);
        let d1 = snap.document(&f).unwrap();
        std::fs::write(&f, "fn b() {}\n").unwrap();
        let d2 = snap.document(&f).unwrap();
        assert!(Arc::ptr_eq(&d1, &d2), "one read per snapshot");
        assert_eq!(snap.changed_since(std::slice::from_ref(&f)), vec![f]);
    }

    #[test]
    fn private_buffers_and_isolated_preview() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.rs");
        std::fs::write(&f, "disk\n").unwrap();
        let shared = ViewHandle::new(ViewKind::Shared, HashMap::new());
        assert!(shared.set_buffer(&f, "x", 1).is_err());

        let private = ViewHandle::new(ViewKind::Private, HashMap::new());
        private.set_buffer(&f, "buffer\n", 3).unwrap();
        let preview = private.preview(vec![(f.clone(), Some("preview\n".into()))]);

        let text = |v: &ViewHandle| {
            Snapshot::new(v.clone(), 1 << 20)
                .document(&f)
                .unwrap()
                .text
                .to_string()
        };
        assert_eq!(text(&shared), "disk\n");
        assert_eq!(text(&private), "buffer\n");
        assert_eq!(text(&preview), "preview\n");
        // The preview did not leak into its base.
        assert_eq!(text(&private), "buffer\n");
        let deleted = shared.preview(vec![(f.clone(), None)]);
        assert_eq!(
            Snapshot::new(deleted, 1 << 20).document(&f).unwrap_err(),
            Unreadable::Deleted
        );
    }

    #[test]
    fn limits_binary_and_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.txt");
        std::fs::write(&big, "x".repeat(100)).unwrap();
        let bin = dir.path().join("bin");
        std::fs::write(&bin, [1u8, 0, 2]).unwrap();
        let latin = dir.path().join("latin.txt");
        std::fs::write(&latin, [0xe9u8, b'\n']).unwrap();
        let snap = Snapshot::new(ViewHandle::new(ViewKind::Shared, HashMap::new()), 50);
        assert!(matches!(
            snap.document(&big),
            Err(Unreadable::TooLarge {
                bytes: 100,
                limit: 50
            })
        ));
        assert_eq!(snap.document(&bin).unwrap_err(), Unreadable::Binary);
        assert_eq!(snap.document(&latin).unwrap_err(), Unreadable::NotUtf8);
        assert_eq!(
            snap.document(&dir.path().join("none")).unwrap_err(),
            Unreadable::NotFound
        );
    }
}
