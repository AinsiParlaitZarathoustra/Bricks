//! What a session needs to be resumed beyond its messages: working
//! directory, model, reasoning profile and memory space.
//!
//! Stored as `session.json` in the session's files directory
//! (`Memory::session_files_dir`), so it is deleted with the session. The
//! messages stay in the session store (`<id>.jsonl`), the authority on the
//! conversation; nothing here duplicates them.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const META_SCHEMA: u32 = 1;
const FILE: &str = "session.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub schema: u32,
    pub id: String,
    /// The first prompt, shortened.
    #[serde(default)]
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub working_dir: PathBuf,
    /// `provider_id/model_id`.
    pub model: String,
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Long-term memory space, when one was attached.
    #[serde(default)]
    pub memory_space: Option<String>,
}

impl SessionMeta {
    pub fn new(id: &str, working_dir: &Path, model: &str, reasoning: Option<String>) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            schema: META_SCHEMA,
            id: id.to_string(),
            title: String::new(),
            created_at: now,
            updated_at: now,
            working_dir: working_dir.to_path_buf(),
            model: model.to_string(),
            reasoning,
            memory_space: None,
        }
    }

    pub fn path(files_dir: &Path) -> PathBuf {
        files_dir.join(FILE)
    }

    /// `None` when the session has no metadata (an older session, or one
    /// written by another program).
    pub fn load(files_dir: &Path) -> Result<Option<Self>, String> {
        let p = Self::path(files_dir);
        match std::fs::read_to_string(&p) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    /// Write atomically (temporary file, then rename).
    pub fn save(&self, files_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(files_dir).map_err(|e| e.to_string())?;
        let p = Self::path(files_dir);
        let tmp = files_dir.join(format!(".{FILE}.tmp"));
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().timestamp_millis();
    }

    /// Set the title from a first prompt (once).
    pub fn title_from(&mut self, prompt: &str) {
        if !self.title.is_empty() {
            return;
        }
        let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let mut t: String = line.trim().chars().take(80).collect();
        if line.trim().chars().count() > 80 {
            t.push('…');
        }
        self.title = t;
    }
}

/// One stored session, for listings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: usize,
    pub working_dir: Option<PathBuf>,
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Which stored sessions a listing shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionScope {
    /// Every stored session (older ones without a folder included).
    All,
    /// The sessions of one workspace folder.
    Workspace(PathBuf),
}

impl SessionScope {
    pub fn includes(&self, s: &SessionSummary) -> bool {
        match self {
            SessionScope::All => true,
            SessionScope::Workspace(w) => s
                .working_dir
                .as_deref()
                .is_some_and(|d| same_workspace(d, w)),
        }
    }
}

/// Whether a recorded folder is the workspace `workspace`: the same folder
/// once both are canonical (a symbolic alias of it matches), never a mere
/// string prefix (`atlas` is not `atlas-old`), and neither a sub-folder nor
/// a parent. A folder that no longer resolves matches nothing. Case is
/// left to the file system.
pub fn same_workspace(recorded: &Path, workspace: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    match (canon(recorded), canon(workspace)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_and_titles_are_short() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(SessionMeta::load(dir.path()).unwrap(), None);
        let mut m = SessionMeta::new(
            "s1",
            Path::new("/tmp/projet été"),
            "p/m",
            Some("deep".into()),
        );
        m.title_from(&format!("\n  {}\nsecond line", "é".repeat(100)));
        assert_eq!(m.title.chars().count(), 81);
        m.title_from("ignored: the title is set once");
        m.save(dir.path()).unwrap();
        assert_eq!(SessionMeta::load(dir.path()).unwrap(), Some(m));
    }
}
