//! Documents kept for a session: the bytes actually downloaded and the
//! extracted Markdown, so passages can be checked against their full text
//! and pages re-read without downloading them again.
//!
//! The store lives in a directory of the session (next to the saved tool
//! outputs, see `cersei-compression`'s `RawStore`): it survives a session
//! restore and is deleted with the session. An `index.json` maps documents
//! (`D1`, `D2`, …) to their files and metadata. When the session quota is
//! exceeded the oldest documents are removed (never the one being added).
//! A download cut at the size limit is recorded as `truncated`: its raw file
//! is the beginning of the page, never the page.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocEntry {
    pub id: String,
    pub requested_url: String,
    pub final_url: String,
    pub title: String,
    pub content_type: Option<String>,
    /// Seconds since the Unix epoch.
    pub fetched_at: u64,
    /// Raw bytes as received (after HTTP decompression).
    pub raw_file: String,
    pub raw_bytes: u64,
    /// The download stopped at the size limit or broke off.
    pub truncated: bool,
    pub markdown_file: String,
    pub markdown_chars: usize,
    /// How the Markdown was obtained.
    pub strategy: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    next: u64,
    docs: Vec<DocEntry>,
}

/// What [`WebStore::put`] saves.
#[derive(Debug, Clone, Copy)]
pub struct NewDoc<'a> {
    pub requested_url: &'a str,
    pub final_url: &'a str,
    pub title: &'a str,
    pub content_type: Option<&'a str>,
    pub raw: &'a [u8],
    pub truncated: bool,
    pub markdown: &'a str,
    pub strategy: &'a str,
    pub notes: &'a [String],
}

pub struct WebStore {
    dir: PathBuf,
    max_bytes: u64,
    index: parking_lot::Mutex<Index>,
}

impl WebStore {
    /// Open (or create on first write) the store in `dir`; an existing index
    /// — a restored session — is read back.
    pub fn open(dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        let dir = dir.into();
        let index = std::fs::read(dir.join("index.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Index>(&b).ok())
            .map(|mut i| {
                // Entries whose files are gone are forgotten.
                i.docs.retain(|d| dir.join(&d.markdown_file).exists());
                i
            })
            .unwrap_or_default();
        Self {
            dir,
            max_bytes,
            index: parking_lot::Mutex::new(index),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Save a document; returns its entry.
    pub fn put(&self, doc: NewDoc<'_>) -> std::io::Result<DocEntry> {
        let NewDoc {
            requested_url,
            final_url,
            title,
            content_type,
            raw,
            truncated,
            markdown,
            strategy,
            notes,
        } = doc;
        std::fs::create_dir_all(&self.dir)?;
        let mut index = self.index.lock();
        index.next += 1;
        let id = format!("D{}", index.next);
        let raw_file = format!("{id}.raw");
        let markdown_file = format!("{id}.md");
        write_atomic(&self.dir.join(&raw_file), raw)?;
        write_atomic(&self.dir.join(&markdown_file), markdown.as_bytes())?;
        let entry = DocEntry {
            id,
            requested_url: requested_url.to_string(),
            final_url: final_url.to_string(),
            title: title.to_string(),
            content_type: content_type.map(str::to_string),
            fetched_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            raw_file,
            raw_bytes: raw.len() as u64,
            truncated,
            markdown_file,
            markdown_chars: markdown.chars().count(),
            strategy: strategy.to_string(),
            notes: notes.to_vec(),
        };
        index.docs.push(entry.clone());
        self.enforce_quota(&mut index, &entry.id);
        self.save(&index)?;
        Ok(entry)
    }

    fn enforce_quota(&self, index: &mut Index, keep: &str) {
        let size = |d: &DocEntry| {
            std::fs::metadata(self.dir.join(&d.markdown_file))
                .map(|m| m.len())
                .unwrap_or(0)
                + d.raw_bytes
        };
        let mut total: u64 = index.docs.iter().map(size).sum();
        while total > self.max_bytes {
            let Some(pos) = index.docs.iter().position(|d| d.id != keep) else {
                break;
            };
            let d = index.docs.remove(pos);
            total = total.saturating_sub(size(&d));
            let _ = std::fs::remove_file(self.dir.join(&d.raw_file));
            let _ = std::fs::remove_file(self.dir.join(&d.markdown_file));
        }
    }

    fn save(&self, index: &Index) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(index).map_err(std::io::Error::other)?;
        write_atomic(&self.dir.join("index.json"), &bytes)
    }

    /// The latest document for a URL (requested or final).
    pub fn find_url(&self, url: &str) -> Option<DocEntry> {
        let key = crate::search::dedup_key(url);
        self.index
            .lock()
            .docs
            .iter()
            .rev()
            .find(|d| {
                crate::search::dedup_key(&d.requested_url) == key
                    || crate::search::dedup_key(&d.final_url) == key
            })
            .cloned()
    }

    pub fn get(&self, id: &str) -> Option<DocEntry> {
        self.index.lock().docs.iter().find(|d| d.id == id).cloned()
    }

    pub fn list(&self) -> Vec<DocEntry> {
        self.index.lock().docs.clone()
    }

    pub fn markdown(&self, d: &DocEntry) -> std::io::Result<String> {
        std::fs::read_to_string(self.dir.join(&d.markdown_file))
    }

    pub fn markdown_path(&self, d: &DocEntry) -> PathBuf {
        self.dir.join(&d.markdown_file)
    }

    pub fn raw_path(&self, d: &DocEntry) -> PathBuf {
        self.dir.join(&d.raw_file)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_survive_reopening_and_the_quota_evicts_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let s = WebStore::open(dir.path().join("web"), 250);
        let doc = |url: &'static str, raw: &'static [u8], truncated, md: &'static str| NewDoc {
            requested_url: url,
            final_url: url,
            title: "T",
            content_type: Some("text/html"),
            raw,
            truncated,
            markdown: md,
            strategy: "readability",
            notes: &[],
        };
        let a = s
            .put(doc("https://a.example/x", &[b'a'; 60], false, "# A\n"))
            .unwrap();
        assert_eq!(a.id, "D1");
        let reopened = WebStore::open(dir.path().join("web"), 250);
        let found = reopened.find_url("https://a.example/x#frag").unwrap();
        assert_eq!(found.id, "D1");
        assert_eq!(reopened.markdown(&found).unwrap(), "# A\n");
        // Numbering continues; the quota removes D1 when D3 arrives.
        reopened
            .put(doc("https://b.example/", &[b'b'; 100], true, "b"))
            .unwrap();
        let c = reopened
            .put(doc("https://c.example/", &[b'c'; 100], false, "c"))
            .unwrap();
        assert_eq!(c.id, "D3");
        assert!(reopened.get("D1").is_none());
        assert!(!dir.path().join("web/D1.raw").exists());
        assert!(reopened.get("D2").unwrap().truncated);
    }
}
