//! IPC contract between the Rust services and the Svelte frontend.
//!
//! Owner: the orchestrator. Agents implement against these shapes and do
//! not change them without coordination. Every type here derives `TS`
//! and is exported to `src/lib/ipc/bindings/` (`cargo test -p
//! bricks-desktop export_bindings`). Engine types (`Envelope`, `Snapshot`,
//! `ModelChoice`, `Command`, ...) cross the bridge as their own serde JSON
//! and are typed on the frontend by `src/lib/ipc/engine.ts`, which mirrors
//! `cersei_agent::control::protocol`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Error returned by every command. `code` is stable, `message` is for
/// the user.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct IpcError {
    pub code: IpcErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    NotFound,
    /// The project folder no longer exists or is not readable.
    FolderMissing,
    /// The session is owned by a running controller (delete, second open).
    SessionBusy,
    /// No provider configured / configuration error.
    Config,
    /// The engine refused the command.
    Rejected,
    Invalid,
    Io,
    Internal,
}

pub type IpcResult<T> = Result<T, IpcError>;

// ─── Application ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AppInfo {
    pub version: String,
    pub bricks_home: String,
    pub sessions_dir: String,
    pub platform: String,
    /// Providers declared in the configuration, without secrets.
    pub providers: Vec<ProviderInfo>,
    /// Configuration problems to show the user (empty when fine).
    pub config_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    /// A credential is available (the value itself never leaves Rust).
    pub has_credentials: bool,
    pub model_count: u32,
}

// ─── Projects ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Canonical absolute path.
    pub path: String,
    #[ts(type = "number")]
    pub added_at: i64,
    #[ts(type = "number")]
    pub last_opened_at: i64,
    /// The folder is gone or unreadable.
    pub missing: bool,
}

// ─── Conversations ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionItem {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    pub message_count: u32,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub working_dir: Option<String>,
    /// A controller of this application currently owns the session.
    pub open: bool,
    pub running: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatch {
    Title,
    Message,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionSearchHit {
    pub session: SessionItem,
    pub matched: SearchMatch,
    /// Excerpt around the match (messages only).
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OpenSessionRequest {
    pub project_id: String,
    /// `None`: new session.
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

/// An open session as the frontend first sees it. Later changes arrive as
/// `DesktopEvent`s; `last_seq` lets the view drop older duplicates.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OpenedSession {
    pub session: SessionItem,
    /// `cersei_agent::control::Snapshot`.
    #[ts(type = "import('../engine').Snapshot")]
    pub snapshot: serde_json::Value,
    pub history: Vec<HistoryMessage>,
    /// Envelopes of the current/last run kept by the bridge (bounded),
    /// for redisplay after switching conversations.
    #[ts(type = "import('../engine').Envelope[]")]
    pub recent_events: Vec<serde_json::Value>,
    #[ts(type = "number")]
    pub last_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct HistoryMessage {
    pub role: HistoryRole,
    pub blocks: Vec<HistoryBlock>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryBlock {
    Text { text: String },
    /// Reasoning the provider exposed.
    Thinking { text: String },
    ToolUse { id: String, name: String, #[ts(type = "unknown")] input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Image { path: Option<String>, media_type: Option<String> },
}

// ─── Events (one Tauri Channel per window, every session multiplexed) ──────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DesktopEvent {
    /// An engine envelope, unchanged, with the project it belongs to.
    Engine {
        project_id: Option<String>,
        #[ts(type = "import('../engine').Envelope")]
        envelope: serde_json::Value,
    },
    /// Ownership/activity of a session changed.
    SessionState { session_id: String, open: bool, running: bool },
    /// Title or metadata changed (rename, first prompt).
    SessionUpdated { session: SessionItem },
    SessionDeleted { session_id: String },
    /// A new entry in the persistent change journal.
    ChangeRecorded { record: ChangeRecord },
    /// Deltas were dropped for this subscriber (slow consumer): reload
    /// the session with `session_open`.
    Lagged { session_id: String },
}

// ─── Attachments ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SavedAttachment {
    /// Absolute path of the application-owned temporary file.
    pub path: String,
    pub media_type: String,
    #[ts(type = "number")]
    pub bytes: u64,
}

// ─── Change journal (persistent, Bricks-recorded edits) ─────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChangeRecord {
    pub id: String,
    pub project_id: String,
    pub session_id: String,
    pub run_id: Option<String>,
    pub agent_id: Option<String>,
    pub changeset_id: Option<String>,
    pub tool: String,
    pub tool_call_id: String,
    #[ts(type = "number")]
    pub at: i64,
    pub files: Vec<ChangeFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChangeFile {
    pub path: String,
    /// `create` | `modify` | `delete` (engine `ChangeKind`).
    pub kind: String,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    /// A patch is stored and can be fetched with `change_log_diff`.
    pub has_patch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChangeJournalInfo {
    /// When the journal started recording for this project.
    #[ts(type = "number | null")]
    pub since: Option<i64>,
    /// Effects of shells and MCP tools are not attributed.
    pub coverage_note: String,
}

// ─── Git ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub detached: bool,
    pub head: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<GitEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitEntry {
    pub path: String,
    pub orig_path: Option<String>,
    pub area: GitArea,
    /// Porcelain letter: M A D R C T U ?.
    pub code: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GitArea {
    Staged,
    Unstaged,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FileDiff {
    pub path: String,
    pub patch: String,
    pub binary: bool,
    /// The patch was cut to stay within the size limit.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitCommit {
    pub hash: String,
    pub short: String,
    pub author: String,
    #[ts(type = "number")]
    pub at: i64,
    pub subject: String,
}

// ─── Terminal ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PtyOpenRequest {
    pub project_id: String,
    pub session_id: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PtyEvent {
    /// Output bytes, base64.
    Data { data: String },
    Exit { code: Option<i32> },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PtyInfo {
    pub id: String,
    pub project_id: String,
    pub session_id: Option<String>,
    pub shell: String,
    pub cwd: String,
}

// ─── Preferences ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct Preferences {
    pub theme: Theme,
    /// User's explicit choices (not the size-forced visibility).
    pub left_open: bool,
    pub right_open: bool,
    pub left_width: u32,
    pub right_width: u32,
    pub terminal_open: bool,
    pub terminal_height: u32,
    pub last_project_id: Option<String>,
    pub last_session_id: Option<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            left_open: true,
            right_open: true,
            left_width: 260,
            right_width: 280,
            terminal_open: false,
            terminal_height: 240,
            last_project_id: None,
            last_session_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Light,
    Dark,
    System,
}
