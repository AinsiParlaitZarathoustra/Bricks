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
    if let Some(rest) = content.strip_prefix("---")
        && let Some(close_pos) = rest.find("\n---")
    {
        return rest[close_pos + 4..].trim_start_matches('\n').to_string();
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

    /// Store the active and raw histories of a session together.
    /// `revision` orders the saves of one owner (an agent): a save older
    /// than one this owner already published for the session is not
    /// written. Saves of different owners are not ordered.
    ///
    /// The default stores the active history, then the raw one
    /// ([`session_keys::raw_history`]): no transaction, an interruption
    /// between them leaves a mixed pair. Backends that publish both at one
    /// commit point override it and say so in
    /// [`Memory::histories_are_atomic`].
    async fn store_histories(
        &self,
        session_id: &str,
        histories: SessionHistories,
        revision: SaveRevision,
    ) -> Result<HistoriesSaved> {
        let _ = revision;
        self.store(session_id, &histories.active).await?;
        self.store(&session_keys::raw_history(session_id), &histories.raw)
            .await?;
        Ok(HistoriesSaved::Committed)
    }

    /// Load both histories of a session. The default makes two loads (no
    /// common point: a save between them can mix two pairs); an absent or
    /// unreadable raw history is empty.
    async fn load_histories(&self, session_id: &str) -> Result<SessionHistories> {
        Ok(SessionHistories {
            active: self.load(session_id).await?,
            raw: self
                .load(&session_keys::raw_history(session_id))
                .await
                .unwrap_or_default(),
        })
    }

    /// Whether [`Memory::store_histories`] publishes both histories at one
    /// commit point and [`Memory::load_histories`] reads them from one.
    fn histories_are_atomic(&self) -> bool {
        false
    }
}

/// The active history of a session (what the model sees) and its raw
/// history (every message as it happened), saved and restored as a pair.
#[derive(Debug, Clone, Default)]
pub struct SessionHistories {
    pub active: Vec<Message>,
    pub raw: Vec<Message>,
}

/// The place of a save among its owner's saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveRevision {
    /// Identifies the owner (an agent instance).
    pub owner: u64,
    /// Increases with each save of the owner.
    pub seq: u64,
}

/// What [`Memory::store_histories`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoriesSaved {
    /// Published as the session's current pair.
    Committed,
    /// The same pair is already published: nothing was written.
    Unchanged,
    /// A later revision of the same owner is already published: this one
    /// was not written.
    Superseded,
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
///
/// Each key is a `<key>.jsonl` file, one message per line, replaced
/// atomically: the new content goes to a temporary file in the same
/// directory (`.<key>.jsonl.<random>.tmp`), is written and synced
/// (`fsync`; `F_FULLFSYNC` on Apple platforms), then renamed over the old
/// file, and the directory is synced (best effort). A reader sees the old
/// file or the new one, never a truncated one.
///
/// ## Session checkpoints
///
/// [`Memory::store_histories`] writes a session's active and raw histories
/// as **one checkpoint**, `<id>.jsonl`: a header line
/// `{"bricks_checkpoint":{"version":1,"generation":…,"active":N,"raw":M,"digest":"…"}}`,
/// then the N active messages, then the M raw messages. The rename of that
/// one file is the commit point of the pair; a load checks the counts and
/// the digest (FNV-1a 64 of the message lines), so it returns one
/// complete pair, the previous one or the new one. Publications of a
/// session are serialized (per session, not globally); a save whose
/// revision is older than the last one published by this backend is not
/// written, nor one identical to the published checkpoint.
///
/// A session saved before this format (`<id>.jsonl` with messages only and
/// `<id>.raw.jsonl`) is read as before; its next pair save writes a
/// checkpoint, then removes its old raw file. Nothing else is migrated.
/// Compaction snapshots (`<id>.compaction-<n>.jsonl`) are separate files,
/// each written atomically on its own.
///
/// Durability: a pair is published only after its file is synced; whether
/// the data survives a power cut also depends on the filesystem and the
/// drive honouring the sync. Checked on macOS (APFS).
pub struct JsonlMemory {
    dir: PathBuf,
    /// Per session: its publications in order, and what was published.
    published: parking_lot::Mutex<
        std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<Published>>>,
    >,
    #[cfg(test)]
    fault: Option<std::sync::Arc<dyn Fn(WriteStage) -> std::io::Result<()> + Send + Sync>>,
}

/// What a backend published last for a session.
#[derive(Debug, Default)]
struct Published {
    /// Read from the file once.
    known: bool,
    generation: u64,
    digest: Option<u64>,
    /// The last save published, by owner.
    revisions: std::collections::HashMap<u64, u64>,
}

/// Steps of a write, where tests inject failures.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteStage {
    Serialize,
    Write,
    Sync,
    Publish,
}

const CHECKPOINT_KEY: &str = "bricks_checkpoint";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CheckpointHeader {
    version: u32,
    generation: u64,
    active: usize,
    raw: usize,
    /// FNV-1a 64 of the message lines, hex.
    digest: String,
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The header of a checkpoint, if `line` is one.
fn checkpoint_header(line: &str) -> Option<CheckpointHeader> {
    if !line.trim_start().starts_with("{\"bricks_checkpoint\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    serde_json::from_value(v.get(CHECKPOINT_KEY)?.clone()).ok()
}

/// A `<key>.jsonl` file as read.
enum Stored {
    Messages(Vec<Message>),
    Checkpoint {
        header: CheckpointHeader,
        active: Vec<Message>,
        raw: Vec<Message>,
    },
}

fn corrupt(path: &std::path::Path, why: &str) -> CerseiError {
    CerseiError::Config(format!("{}: {why}", path.display()))
}

fn parse_stored(path: &std::path::Path, content: &str) -> Result<Stored> {
    let mut lines = content.lines().filter(|l| !l.trim().is_empty());
    let first = lines.clone().next();
    if let Some(header) = first.and_then(checkpoint_header) {
        lines.next();
        let body_start = content.find('\n').map(|i| i + 1).unwrap_or(content.len());
        if fnv1a(&content.as_bytes()[body_start..])
            != u64::from_str_radix(&header.digest, 16).unwrap_or(0)
        {
            return Err(corrupt(path, "checkpoint digest mismatch"));
        }
        let mut active = Vec::with_capacity(header.active);
        let mut raw = Vec::with_capacity(header.raw);
        for (i, line) in lines.enumerate() {
            let msg: Message = serde_json::from_str(line)?;
            if i < header.active {
                active.push(msg);
            } else {
                raw.push(msg);
            }
        }
        if active.len() != header.active || raw.len() != header.raw {
            return Err(corrupt(path, "incomplete checkpoint"));
        }
        return Ok(Stored::Checkpoint {
            header,
            active,
            raw,
        });
    }
    let mut messages = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        messages.push(serde_json::from_str(line)?);
    }
    Ok(Stored::Messages(messages))
}

fn jsonl_lines(out: &mut Vec<u8>, messages: &[Message]) -> Result<()> {
    for msg in messages {
        serde_json::to_writer(&mut *out, msg)?;
        out.push(b'\n');
    }
    Ok(())
}

/// Sync a directory so a rename in it is durable (best effort: not every
/// platform or filesystem accepts it).
fn sync_dir(dir: &std::path::Path) {
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

impl JsonlMemory {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            published: parking_lot::Mutex::new(std::collections::HashMap::new()),
            #[cfg(test)]
            fault: None,
        }
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", session_id))
    }

    /// An internal file of an existing session (see [`session_keys`]).
    fn is_internal(&self, stem: &str) -> bool {
        session_keys::parent(stem).is_some_and(|base| self.session_path(base).exists())
    }

    fn state(&self, session_id: &str) -> std::sync::Arc<tokio::sync::Mutex<Published>> {
        self.published
            .lock()
            .entry(session_id.to_string())
            .or_default()
            .clone()
    }

    async fn read(&self, key: &str) -> Result<Option<Stored>> {
        let path = self.session_path(key);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(Some(parse_stored(&path, &content)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Replace `<key>.jsonl` atomically with `body` (blocking).
    fn write_atomic(
        dir: &std::path::Path,
        key: &str,
        body: &[u8],
        #[cfg(test)] fault: &Option<
            std::sync::Arc<dyn Fn(WriteStage) -> std::io::Result<()> + Send + Sync>,
        >,
    ) -> std::io::Result<()> {
        use std::io::Write;
        #[cfg(test)]
        let check = |s: WriteStage| fault.as_ref().map(|f| f(s)).unwrap_or(Ok(()));
        #[cfg(not(test))]
        let check = |_: ()| -> std::io::Result<()> { Ok(()) };
        std::fs::create_dir_all(dir)?;
        // Dropped (and removed) on any error below.
        let mut tmp = tempfile::Builder::new()
            .prefix(&format!(".{key}.jsonl."))
            .suffix(".tmp")
            .tempfile_in(dir)?;
        #[cfg(test)]
        check(WriteStage::Write)?;
        #[cfg(not(test))]
        check(())?;
        tmp.write_all(body)?;
        tmp.flush()?;
        #[cfg(test)]
        check(WriteStage::Sync)?;
        tmp.as_file().sync_all()?;
        #[cfg(test)]
        check(WriteStage::Publish)?;
        tmp.persist(dir.join(format!("{key}.jsonl")))
            .map_err(|e| e.error)?;
        sync_dir(dir);
        Ok(())
    }

    async fn write_messages(&self, key: &str, messages: &[Message]) -> Result<()> {
        let mut body = Vec::new();
        jsonl_lines(&mut body, messages)?;
        let dir = self.dir.clone();
        let key = key.to_string();
        #[cfg(test)]
        let fault = self.fault.clone();
        tokio::task::spawn_blocking(move || {
            Self::write_atomic(
                &dir,
                &key,
                &body,
                #[cfg(test)]
                &fault,
            )
        })
        .await
        .map_err(|e| CerseiError::Config(format!("session write: {e}")))??;
        Ok(())
    }

    /// Publish a pair (the session's state locked by the caller).
    async fn publish(
        &self,
        session_id: &str,
        state: &mut Published,
        histories: SessionHistories,
        revision: Option<SaveRevision>,
    ) -> Result<HistoriesSaved> {
        if !state.known {
            if let Ok(Some(Stored::Checkpoint { header, .. })) = self.read(session_id).await {
                state.generation = header.generation;
                state.digest = u64::from_str_radix(&header.digest, 16).ok();
            }
            state.known = true;
        }
        if let Some(r) = revision
            && state.revisions.get(&r.owner).is_some_and(|p| r.seq < *p)
        {
            return Ok(HistoriesSaved::Superseded);
        }
        let dir = self.dir.clone();
        let key = session_id.to_string();
        let generation = state.generation + 1;
        let previous = state.digest;
        #[cfg(test)]
        let fault = self.fault.clone();
        // Serialization and I/O off the async threads; no lock of the
        // caller's histories is held here (they were moved in).
        let written = tokio::task::spawn_blocking(move || -> Result<Option<u64>> {
            #[cfg(test)]
            if let Some(f) = &fault {
                f(WriteStage::Serialize)?;
            }
            let mut body = Vec::new();
            jsonl_lines(&mut body, &histories.active)?;
            jsonl_lines(&mut body, &histories.raw)?;
            let digest = fnv1a(&body);
            if previous == Some(digest) {
                return Ok(None);
            }
            let header = serde_json::json!({ CHECKPOINT_KEY: CheckpointHeader {
                version: 1,
                generation,
                active: histories.active.len(),
                raw: histories.raw.len(),
                digest: format!("{digest:016x}"),
            }});
            let mut file = serde_json::to_vec(&header)?;
            file.push(b'\n');
            file.extend_from_slice(&body);
            Self::write_atomic(
                &dir,
                &key,
                &file,
                #[cfg(test)]
                &fault,
            )?;
            Ok(Some(digest))
        })
        .await
        .map_err(|e| CerseiError::Config(format!("session write: {e}")))??;
        if let Some(r) = revision {
            let p = state.revisions.entry(r.owner).or_insert(r.seq);
            *p = (*p).max(r.seq);
        }
        let Some(digest) = written else {
            return Ok(HistoriesSaved::Unchanged);
        };
        state.generation = generation;
        state.digest = Some(digest);
        // The old format's raw history is now inside the checkpoint.
        let old_raw = self.session_path(&session_keys::raw_history(session_id));
        if tokio::fs::try_exists(&old_raw).await.unwrap_or(false) {
            let _ = tokio::fs::remove_file(&old_raw).await;
        }
        Ok(HistoriesSaved::Committed)
    }

    /// The message count of a stored file, from a checkpoint's header or by
    /// counting lines.
    async fn message_count(path: &std::path::Path) -> usize {
        let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
        let mut lines = content.lines().filter(|l| !l.trim().is_empty());
        match lines.clone().next().and_then(checkpoint_header) {
            Some(h) => h.active,
            None => lines.by_ref().count(),
        }
    }
}

#[async_trait]
impl Memory for JsonlMemory {
    async fn store(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        // A history of a session stored as a checkpoint is replaced inside
        // it, with the other one kept, at the same commit point.
        let (base, is_raw) = match session_id.strip_suffix(".raw") {
            Some(b) if !b.is_empty() => (b, true),
            _ => (session_id, false),
        };
        if base == session_id && session_keys::parent(session_id).is_some() {
            // A snapshot: a file of its own.
            return self.write_messages(session_id, messages).await;
        }
        let state = self.state(base);
        let mut state = state.lock().await;
        if let Some(Stored::Checkpoint { active, raw, .. }) = self.read(base).await? {
            let histories = if is_raw {
                SessionHistories {
                    active,
                    raw: messages.to_vec(),
                }
            } else {
                SessionHistories {
                    active: messages.to_vec(),
                    raw,
                }
            };
            self.publish(base, &mut state, histories, None).await?;
            return Ok(());
        }
        self.write_messages(session_id, messages).await
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Message>> {
        if let Some(base) = session_id.strip_suffix(".raw").filter(|b| !b.is_empty())
            && let Some(Stored::Checkpoint { raw, .. }) = self.read(base).await?
        {
            return Ok(raw);
        }
        Ok(match self.read(session_id).await? {
            None => Vec::new(),
            Some(Stored::Messages(m)) => m,
            Some(Stored::Checkpoint { active, .. }) => active,
        })
    }

    async fn store_histories(
        &self,
        session_id: &str,
        histories: SessionHistories,
        revision: SaveRevision,
    ) -> Result<HistoriesSaved> {
        let state = self.state(session_id);
        let mut state = state.lock().await;
        self.publish(session_id, &mut state, histories, Some(revision))
            .await
    }

    async fn load_histories(&self, session_id: &str) -> Result<SessionHistories> {
        // One file read: one generation, whatever is published meanwhile.
        match self.read(session_id).await? {
            Some(Stored::Checkpoint { active, raw, .. }) => Ok(SessionHistories { active, raw }),
            Some(Stored::Messages(active)) => {
                // The old format: two files, no common commit point.
                let raw = match self.read(&session_keys::raw_history(session_id)).await {
                    Ok(Some(Stored::Messages(r))) => r,
                    _ => Vec::new(),
                };
                Ok(SessionHistories { active, raw })
            }
            None => Ok(SessionHistories::default()),
        }
    }

    fn histories_are_atomic(&self) -> bool {
        true
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
            // Temporary files (`.<key>.jsonl.<random>.tmp`) end in `.tmp`.
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
                let message_count = Self::message_count(&path).await;
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
        let state = self.state(session_id);
        let mut published = state.lock().await;
        // A later session with this id starts from nothing.
        *published = Published::default();
        let path = self.session_path(session_id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        // Its raw history, snapshots, saved outputs and any temporary file
        // left by an interrupted write go with it.
        if self.dir.exists() {
            let mut entries = tokio::fs::read_dir(&self.dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let p = entry.path();
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let key = if let Some(tmp) = name.strip_prefix('.').filter(|n| n.ends_with(".tmp"))
                {
                    tmp.split(".jsonl.").next().unwrap_or("")
                } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    p.file_stem().and_then(|s| s.to_str()).unwrap_or("")
                } else {
                    continue;
                };
                let temporary = name.ends_with(".tmp");
                if session_keys::parent(key) == Some(session_id) || (temporary && key == session_id)
                {
                    tokio::fs::remove_file(&p).await?;
                }
            }
        }
        if let Some(files) = self.session_files_dir(session_id)
            && files.exists()
        {
            tokio::fs::remove_dir_all(&files).await?;
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

    async fn store_histories(
        &self,
        session_id: &str,
        histories: SessionHistories,
        _revision: SaveRevision,
    ) -> Result<HistoriesSaved> {
        let mut store = self.store.lock();
        store.insert(session_id.to_string(), histories.active);
        store.insert(session_keys::raw_history(session_id), histories.raw);
        Ok(HistoriesSaved::Committed)
    }

    async fn load_histories(&self, session_id: &str) -> Result<SessionHistories> {
        let store = self.store.lock();
        Ok(SessionHistories {
            active: store.get(session_id).cloned().unwrap_or_default(),
            raw: store
                .get(&session_keys::raw_history(session_id))
                .cloned()
                .unwrap_or_default(),
        })
    }

    fn histories_are_atomic(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn msgs(prefix: &str, n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| Message::user(format!("{prefix} {i} {}", "x".repeat(50))))
            .collect()
    }

    fn pair(prefix: &str, n: usize) -> SessionHistories {
        SessionHistories {
            active: msgs(&format!("{prefix} active"), n),
            raw: msgs(&format!("{prefix} raw"), n + 3),
        }
    }

    fn texts(m: &[Message]) -> Vec<String> {
        m.iter()
            .map(|m| m.get_text().unwrap_or_default().to_string())
            .collect()
    }

    fn rev(seq: u64) -> SaveRevision {
        SaveRevision { owner: 1, seq }
    }

    fn files(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        v.sort();
        v
    }

    /// A backend whose writes fail at `stage` while `armed`.
    fn failing(dir: &std::path::Path, stage: WriteStage, armed: Arc<AtomicBool>) -> JsonlMemory {
        let mut m = JsonlMemory::new(dir);
        m.fault = Some(Arc::new(move |s| {
            if s == stage && armed.load(Ordering::SeqCst) {
                Err(std::io::Error::other(format!("injected at {s:?}")))
            } else {
                Ok(())
            }
        }));
        m
    }

    #[tokio::test]
    async fn an_interrupted_save_leaves_the_previous_complete_pair() {
        for stage in [
            WriteStage::Serialize,
            WriteStage::Write,
            WriteStage::Sync,
            WriteStage::Publish,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let armed = Arc::new(AtomicBool::new(false));
            let m = failing(dir.path(), stage, armed.clone());
            let old = pair("old", 4);
            assert_eq!(
                m.store_histories("s", old.clone(), rev(1)).await.unwrap(),
                HistoriesSaved::Committed
            );
            armed.store(true, Ordering::SeqCst);
            let err = m.store_histories("s", pair("new", 9), rev(2)).await;
            assert!(err.is_err(), "{stage:?}");
            // A new backend (a restart) reads the last complete pair.
            let reopened = JsonlMemory::new(dir.path());
            let got = reopened.load_histories("s").await.unwrap();
            assert_eq!(texts(&got.active), texts(&old.active), "{stage:?}");
            assert_eq!(texts(&got.raw), texts(&old.raw), "{stage:?}");
            // No temporary file left, no extra session.
            assert_eq!(files(dir.path()), ["s.jsonl"], "{stage:?}");
            assert_eq!(reopened.sessions().await.unwrap().len(), 1);
            // The next save works.
            armed.store(false, Ordering::SeqCst);
            m.store_histories("s", pair("new", 9), rev(3))
                .await
                .unwrap();
            let got = reopened.load_histories("s").await.unwrap();
            assert_eq!(got.active.len(), 9);
            assert_eq!(got.raw.len(), 12);
        }
    }

    #[tokio::test]
    async fn a_truncated_or_altered_checkpoint_is_refused_not_misread() {
        let dir = tempfile::tempdir().unwrap();
        let m = JsonlMemory::new(dir.path());
        m.store_histories("s", pair("a", 5), rev(1)).await.unwrap();
        let path = dir.path().join("s.jsonl");
        let full = std::fs::read_to_string(&path).unwrap();
        // What a crash during a direct (non-atomic) rewrite could leave.
        std::fs::write(&path, &full[..full.len() / 2]).unwrap();
        assert!(
            JsonlMemory::new(dir.path())
                .load_histories("s")
                .await
                .is_err()
        );
        std::fs::write(&path, full.replace("a active 1", "a active X")).unwrap();
        assert!(
            JsonlMemory::new(dir.path())
                .load_histories("s")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn old_sessions_are_read_and_migrated_by_their_next_save_only() {
        let dir = tempfile::tempdir().unwrap();
        let m = JsonlMemory::new(dir.path());
        let mut body = String::new();
        for msg in msgs("legacy active", 3) {
            body.push_str(&serde_json::to_string(&msg).unwrap());
            body.push('\n');
        }
        std::fs::write(dir.path().join("old.jsonl"), &body).unwrap();
        let mut raw = String::new();
        for msg in msgs("legacy raw", 5) {
            raw.push_str(&serde_json::to_string(&msg).unwrap());
            raw.push('\n');
        }
        std::fs::write(dir.path().join("old.raw.jsonl"), &raw).unwrap();
        std::fs::write(dir.path().join("other.jsonl"), &body).unwrap();
        let got = m.load_histories("old").await.unwrap();
        assert_eq!((got.active.len(), got.raw.len()), (3, 5));
        assert_eq!(m.load("old").await.unwrap().len(), 3);
        assert_eq!(m.load("old.raw").await.unwrap().len(), 5);
        let mut ids: Vec<(String, usize)> = m
            .sessions()
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.id, s.message_count))
            .collect();
        ids.sort();
        assert_eq!(ids, [("old".into(), 3), ("other".into(), 3)]);
        // The next save of that session writes a checkpoint and removes its
        // old raw file; the other session is untouched.
        let mut next = got.clone();
        next.active.push(Message::user("one more"));
        m.store_histories("old", next, rev(1)).await.unwrap();
        assert_eq!(files(dir.path()), ["old.jsonl", "other.jsonl"]);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("other.jsonl")).unwrap(),
            body
        );
        let got = JsonlMemory::new(dir.path())
            .load_histories("old")
            .await
            .unwrap();
        assert_eq!((got.active.len(), got.raw.len()), (4, 5));
        let ids: Vec<usize> = m
            .sessions()
            .await
            .unwrap()
            .into_iter()
            .filter(|s| s.id == "old")
            .map(|s| s.message_count)
            .collect();
        assert_eq!(ids, [4]);
    }

    #[tokio::test]
    async fn unchanged_older_and_concurrent_saves() {
        let dir = tempfile::tempdir().unwrap();
        let m = Arc::new(JsonlMemory::new(dir.path()));
        let p = pair("a", 3);
        assert_eq!(
            m.store_histories("s", p.clone(), rev(1)).await.unwrap(),
            HistoriesSaved::Committed
        );
        let before = std::fs::metadata(dir.path().join("s.jsonl"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            m.store_histories("s", p.clone(), rev(2)).await.unwrap(),
            HistoriesSaved::Unchanged
        );
        let after = std::fs::metadata(dir.path().join("s.jsonl"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "nothing written");
        // An older save of the same owner never replaces a newer one.
        m.store_histories("s", pair("b", 4), rev(5)).await.unwrap();
        assert_eq!(
            m.store_histories("s", pair("stale", 1), rev(4))
                .await
                .unwrap(),
            HistoriesSaved::Superseded
        );
        assert_eq!(m.load_histories("s").await.unwrap().active.len(), 4);
        // Another owner is not ordered against it.
        let other = SaveRevision { owner: 2, seq: 1 };
        assert_eq!(
            m.store_histories("s", pair("c", 2), other).await.unwrap(),
            HistoriesSaved::Committed
        );
        // Readers during many saves always see one complete pair.
        let writer = {
            let m = m.clone();
            tokio::spawn(async move {
                for i in 0..40u64 {
                    let n = (i % 7 + 1) as usize;
                    m.store_histories("s", pair("w", n), rev(10 + i))
                        .await
                        .unwrap();
                }
            })
        };
        let reader = {
            let m = m.clone();
            tokio::spawn(async move {
                for _ in 0..200 {
                    let h = m.load_histories("s").await.unwrap();
                    assert_eq!(h.raw.len(), h.active.len() + 3, "a mixed pair");
                    tokio::task::yield_now().await;
                }
            })
        };
        writer.await.unwrap();
        reader.await.unwrap();
        // A second session is saved in parallel without waiting for it.
        m.store_histories("t", pair("t", 1), rev(1)).await.unwrap();
    }

    #[tokio::test]
    async fn single_stores_keep_the_pair_and_delete_removes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let m = JsonlMemory::new(dir.path());
        m.store_histories("s", pair("a", 3), rev(1)).await.unwrap();
        // The individual API replaces one history inside the checkpoint.
        m.store("s", &msgs("only active", 2)).await.unwrap();
        let h = m.load_histories("s").await.unwrap();
        assert_eq!((h.active.len(), h.raw.len()), (2, 6));
        m.store("s.raw", &msgs("only raw", 1)).await.unwrap();
        let h = m.load_histories("s").await.unwrap();
        assert_eq!((h.active.len(), h.raw.len()), (2, 1));
        // Snapshots stay files of their own, written atomically.
        m.store(&session_keys::snapshot("s", 1), &msgs("snap", 4))
            .await
            .unwrap();
        assert_eq!(
            m.load(&session_keys::snapshot("s", 1)).await.unwrap().len(),
            4
        );
        std::fs::create_dir_all(dir.path().join("s.files")).unwrap();
        std::fs::write(dir.path().join(".s.jsonl.abc123.tmp"), "left by a crash").unwrap();
        std::fs::write(dir.path().join(".t.jsonl.abc123.tmp"), "another session").unwrap();
        assert_eq!(
            m.sessions().await.unwrap().len(),
            1,
            "no internal file listed"
        );
        m.delete("s").await.unwrap();
        assert_eq!(files(dir.path()), [".t.jsonl.abc123.tmp"]);
        // A new session with the same id starts from nothing.
        m.store_histories("s", pair("fresh", 1), rev(1))
            .await
            .unwrap();
        assert_eq!(m.load_histories("s").await.unwrap().active.len(), 1);
    }

    #[tokio::test]
    async fn the_default_pair_methods_keep_external_backends_working() {
        struct External(InMemory);
        #[async_trait]
        impl Memory for External {
            async fn store(&self, id: &str, m: &[Message]) -> Result<()> {
                self.0.store(id, m).await
            }
            async fn load(&self, id: &str) -> Result<Vec<Message>> {
                self.0.load(id).await
            }
            async fn search(&self, q: &str, l: usize) -> Result<Vec<MemoryEntry>> {
                self.0.search(q, l).await
            }
            async fn sessions(&self) -> Result<Vec<SessionInfo>> {
                self.0.sessions().await
            }
            async fn delete(&self, id: &str) -> Result<()> {
                self.0.delete(id).await
            }
        }
        let e = External(InMemory::new());
        assert!(!e.histories_are_atomic());
        e.store_histories("s", pair("a", 2), rev(1)).await.unwrap();
        let h = e.load_histories("s").await.unwrap();
        assert_eq!((h.active.len(), h.raw.len()), (2, 5));
        assert!(JsonlMemory::new("/nonexistent").histories_are_atomic());
    }
}
