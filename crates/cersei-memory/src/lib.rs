//! cersei-memory: Memory trait and backends for the Cersei SDK.
//!
//! Memory provides session persistence and retrieval, enabling resumable
//! conversations and long-term knowledge storage.
//!
//! ## Modules
//! - `memdir` — Flat file memory scanning
//! - `claudemd` — Hierarchical CLAUDE.md loading
//! - `session_storage` — JSONL transcript persistence

pub mod claudemd;
#[cfg(feature = "embed")]
pub mod embedding_memory;
pub mod graph;
pub mod graph_migrate;
pub mod manager;
pub mod memdir;
pub mod session_storage;
#[cfg(feature = "structured")]
pub mod structured;

use async_trait::async_trait;
use cersei_types::*;
use std::path::PathBuf;

/// Strip YAML frontmatter from content.
pub fn strip_frontmatter(content: &str) -> String {
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(close_pos) = rest.find("\n---") {
            return rest[close_pos + 4..].trim_start_matches('\n').to_string();
        }
    }
    content.to_string()
}

// ─── Memory trait ────────────────────────────────────────────────────────────

#[async_trait]
pub trait Memory: Send + Sync {
    /// Store conversation messages for a session.
    async fn store(&self, session_id: &str, messages: &[Message]) -> Result<()>;

    /// Load conversation history for a session.
    async fn load(&self, session_id: &str) -> Result<Vec<Message>>;

    /// Search memories relevant to a query (for RAG-style retrieval).
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// List available sessions.
    async fn sessions(&self) -> Result<Vec<SessionInfo>>;

    /// Delete a session, with everything stored for it (see [`session_keys`]).
    async fn delete(&self, session_id: &str) -> Result<()>;

    /// Directory for files that belong to a session (the saved originals of
    /// reduced tool outputs). It lives and dies with the session. `None` for
    /// backends without a filesystem location.
    fn session_files_dir(&self, _session_id: &str) -> Option<PathBuf> {
        None
    }
}

// ─── Long-term memory (recall into prompts) ──────────────────────────────────

/// A recalled context block, ready to append to a system prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct RecalledContext {
    pub text: String,
    /// Estimated tokens of `text`.
    pub tokens: u64,
    pub items: usize,
    /// Admissible items left out by the budget.
    pub omitted: usize,
}

/// One turn of a conversation, to remember.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryTurn {
    /// `user`, `assistant`, `tool`.
    pub role: String,
    pub content: String,
    /// Milliseconds since the Unix epoch, when known.
    pub at: Option<i64>,
}

/// What a maintenance pass did with the recorded turns.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MaintenanceReport {
    /// Episodes sent to the extractor successfully.
    pub extracted: usize,
    pub facts_created: usize,
    /// Facts confirmed, replaced or contested by new evidence.
    pub facts_updated: usize,
    pub embedded: usize,
    /// Episodes whose extraction failed in this pass (retried by a later
    /// pass, up to the configured attempts).
    pub failed: usize,
    pub embedding_errors: usize,
    /// Work a later pass would still do.
    pub pending: usize,
    /// The pass stopped early because it was cancelled.
    pub cancelled: bool,
    /// Error messages (never secrets), at most a few.
    pub errors: Vec<String>,
}

/// What an agent needs from a long-term memory: recall for a prompt,
/// remembering an exchange, and processing what was remembered. Implemented
/// by `structured::StructuredMemory` (feature `structured`).
#[async_trait]
pub trait LongTermMemory: Send + Sync {
    /// Context for `query`, within `max_tokens`; `None` when nothing is
    /// relevant.
    async fn recall_context(
        &self,
        query: &str,
        max_tokens: usize,
    ) -> Result<Option<RecalledContext>>;

    /// Store the turns of a session durably. Local and quick: no model or
    /// embedding call happens here; [`Self::maintain`] does that work.
    async fn record_turns(&self, session_id: Option<&str>, turns: &[MemoryTurn]) -> Result<()>;

    /// Process what was recorded and not processed yet (extraction,
    /// embeddings). Resumable: a pass that is cancelled or interrupted
    /// leaves its remaining work for the next pass, and an extraction that
    /// succeeded is never repeated.
    async fn maintain(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<MaintenanceReport>;
}

/// Keys under which an agent stores, next to a session `<id>`, its raw
/// history (`<id>.raw`) and its pre-compaction snapshots
/// (`<id>.compaction-<n>`, `n` ≥ 1). They are part of the session, not
/// sessions of their own.
pub mod session_keys {
    pub fn raw_history(session_id: &str) -> String {
        format!("{session_id}.raw")
    }

    pub fn snapshot(session_id: &str, number: usize) -> String {
        format!("{session_id}.compaction-{number}")
    }

    /// The session a key belongs to, when `key` follows one of the
    /// conventions above. Whether that session exists is for the caller to
    /// check: a real session may be named like this when no base exists.
    pub fn parent(key: &str) -> Option<&str> {
        if let Some(base) = key.strip_suffix(".raw") {
            return (!base.is_empty()).then_some(base);
        }
        let (base, n) = key.rsplit_once(".compaction-")?;
        (!base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(base)
    }
}

// ─── JSONL Memory ────────────────────────────────────────────────────────────

/// File-based memory backend using JSONL format.
/// Each session is stored as a `.jsonl` file with one message per line.
pub struct JsonlMemory {
    dir: PathBuf,
}

impl JsonlMemory {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", session_id))
    }

    /// An internal file of an existing session (see [`session_keys`]).
    fn is_internal(&self, stem: &str) -> bool {
        session_keys::parent(stem).is_some_and(|base| self.session_path(base).exists())
    }
}

#[async_trait]
impl Memory for JsonlMemory {
    async fn store(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let path = self.session_path(session_id);
        let mut content = String::new();
        for msg in messages {
            let line = serde_json::to_string(msg)?;
            content.push_str(&line);
            content.push('\n');
        }
        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Message>> {
        let path = self.session_path(session_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = tokio::fs::read_to_string(&path).await?;
        let mut messages = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let msg: Message = serde_json::from_str(line)?;
            messages.push(msg);
        }
        Ok(messages)
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<MemoryEntry>> {
        // JSONL memory doesn't support semantic search
        Ok(Vec::new())
    }

    async fn sessions(&self) -> Result<Vec<SessionInfo>> {
        let mut sessions = Vec::new();
        if !self.dir.exists() {
            return Ok(sessions);
        }
        let mut entries = tokio::fs::read_dir(&self.dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if self.is_internal(&id) {
                    continue;
                }
                let metadata = tokio::fs::metadata(&path).await?;
                let created_at = metadata
                    .created()
                    .ok()
                    .and_then(|t| {
                        let dur = t.duration_since(std::time::UNIX_EPOCH).ok()?;
                        chrono::DateTime::from_timestamp(dur.as_secs() as i64, 0)
                    })
                    .unwrap_or_else(chrono::Utc::now);
                let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
                let message_count = content.lines().filter(|l| !l.trim().is_empty()).count();
                sessions.push(SessionInfo {
                    id,
                    created_at,
                    message_count,
                    model: None,
                });
            }
        }
        Ok(sessions)
    }

    async fn delete(&self, session_id: &str) -> Result<()> {
        let path = self.session_path(session_id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        // Its raw history, snapshots and saved outputs go with it.
        if self.dir.exists() {
            let mut entries = tokio::fs::read_dir(&self.dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if session_keys::parent(stem) == Some(session_id) {
                    tokio::fs::remove_file(&p).await?;
                }
            }
        }
        if let Some(files) = self.session_files_dir(session_id) {
            if files.exists() {
                tokio::fs::remove_dir_all(&files).await?;
            }
        }
        Ok(())
    }

    fn session_files_dir(&self, session_id: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("{session_id}.files")))
    }
}

// ─── In-Memory Store ─────────────────────────────────────────────────────────

/// In-memory store for tests and short-lived agents.
pub struct InMemory {
    store: std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<String, Vec<Message>>>>,
}

impl InMemory {
    pub fn new() -> Self {
        Self {
            store: std::sync::Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

impl Default for InMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Memory for InMemory {
    async fn store(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        self.store
            .lock()
            .insert(session_id.to_string(), messages.to_vec());
        Ok(())
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Message>> {
        Ok(self
            .store
            .lock()
            .get(session_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn sessions(&self) -> Result<Vec<SessionInfo>> {
        let store = self.store.lock();
        Ok(store
            .iter()
            .map(|(id, msgs)| SessionInfo {
                id: id.clone(),
                created_at: chrono::Utc::now(),
                message_count: msgs.len(),
                model: None,
            })
            .collect())
    }

    async fn delete(&self, session_id: &str) -> Result<()> {
        self.store.lock().remove(session_id);
        Ok(())
    }
}
