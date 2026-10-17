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
    /// The file changed on disk since it was read (`revision` mismatch).
    Conflict,
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
    /// Desktop-wide tool approval policy (Sprint 14.2), applied by the engine
    /// to every session of this application and its sub-agents.
    pub tool_approval: ToolApproval,
    /// Custom colours per resolved mode (Sprint 14.3). Empty = theme tokens.
    pub colors: ThemeColors,
    /// Terminal pane share of the centre column, 0.1–0.8 (Sprint 14.3).
    /// `None`: not set yet — derived once from the legacy `terminal_height`.
    pub terminal_ratio: Option<f32>,
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
            tool_approval: ToolApproval::Standard,
            colors: ThemeColors::default(),
            terminal_ratio: None,
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

// ─── Sprint 14.1: slash commands and session inspection ────────────────────

/// One entry of the shared `/` command registry
/// (`cersei_agent::control::slash`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SlashCommandInfo {
    pub name: String,
    pub aliases: Vec<String>,
    /// Argument syntax, e.g. `[provider/model [profil]]`.
    pub args: String,
    /// French description shown in the palette and the help.
    pub description: String,
}

/// What a parsed `/` line does. `Engine` commands go through
/// `session_command` unchanged; `Present` actions are handled by the
/// desktop (a view, a picker, an attachment). A `resume` with an id is
/// returned as `Present { view: resume }` so the desktop reopens it through
/// its own registry (`session_open`), never by retargeting a controller.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlashAction {
    Engine {
        #[ts(type = "import('../engine').Command")]
        command: serde_json::Value,
    },
    Present { present: SlashPresent },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum SlashPresent {
    ModelPicker,
    EffortPicker,
    Memory,
    Context,
    ContextDetails,
    Cost,
    CostDetails,
    Session,
    SessionPicker,
    Resume { session_id: String },
    Diff,
    Tools,
    Mcp,
    Config,
    Help,
    Quit,
    /// `kind`: `file` | `folder` | `image`; the path as typed (quotes removed).
    Attach { kind: String, path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionToolInfo {
    pub name: String,
    pub description: String,
    /// Permission level (`none`, `read_only`, `write`, `execute`, `dangerous`).
    pub level: String,
    /// What the session's rules decide for it (`allow`, `ask`, `deny`), and
    /// whether it was allowed for this session.
    pub decision: String,
    pub allowed_for_session: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct McpServerInfo {
    pub name: String,
    /// State as the engine reports it.
    pub status: String,
}

/// Effective configuration of an open session, without secrets.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SessionConfigInfo {
    pub session_id: String,
    pub working_dir: String,
    pub model: String,
    pub provider: String,
    pub reasoning: Option<String>,
    pub effective_reasoning: Option<String>,
    pub interactive: bool,
    pub memory_space: Option<String>,
    /// `cersei_agent::control::ApprovalRules` (actions per level and tool).
    #[ts(type = "Record<string, unknown>")]
    pub approval_rules: serde_json::Value,
    pub allowed_for_session: Vec<String>,
    pub agents_enabled: bool,
    /// Configuration files read for this session, in order.
    pub sources: Vec<String>,
    /// Other effective settings as label/value pairs (no keys, no headers).
    pub settings: Vec<ConfigEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConfigEntry {
    pub label: String,
    pub value: String,
}

/// One entry of the project file index behind `@` mentions (paths relative
/// to the project, `/` separated; `.gitignore` respected).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProjectFileEntry {
    pub path: String,
    pub is_dir: bool,
}

// ─── Sprint 14.2: tool approval policy and provider/model editing ───────────

/// Global approval policy of the desktop. Priority (one place, in the engine
/// gate): `forbidden` tools → refused; an explicit `deny` rule → refused;
/// then `AlwaysAsk` → ask (session grants ignored), `NoConfirmation` →
/// allow; `Standard` → the project's rules and session grants unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ToolApproval {
    Standard,
    AlwaysAsk,
    NoConfirmation,
}

/// The providers file as loaded, without any secret value.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProvidersFile {
    /// Absolute path of the file read (`providers.toml`).
    pub path: String,
    /// Opaque content revision (hash); required by every write to refuse a
    /// stale overwrite with `Conflict`.
    pub revision: String,
    pub providers: Vec<ProviderView>,
    /// The file exists but cannot be read or validated (message for the user).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    /// `chat_completions` | `responses` | `anthropic_messages`.
    pub protocol: String,
    /// `bearer` | `api_key_header` | `none`.
    pub auth: String,
    pub auth_header: Option<String>,
    /// Name of the environment variable holding the key (never its value).
    pub api_key_env: Option<String>,
    /// A key is written in the file (its value never leaves Rust).
    pub has_inline_key: bool,
    /// The credential is available to this application right now.
    pub has_credentials: bool,
    pub path: Option<String>,
    /// Names of extra headers (values are secrets).
    pub header_names: Vec<String>,
    pub models: Vec<ModelView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelView {
    pub id: String,
    /// `provider_id/model_id`.
    pub selection: String,
    pub name: String,
    pub api_model: String,
    /// Model-level override, if any.
    pub protocol: Option<String>,
    pub endpoint: Option<String>,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub streaming: bool,
    pub tool_calls: bool,
    #[ts(type = "number")]
    pub max_input_tokens: u64,
    #[ts(type = "number")]
    pub max_output_tokens: u64,
    #[ts(type = "number | null")]
    pub context_window_tokens: Option<u64>,
    pub reasoning_default: Option<String>,
    pub profiles: Vec<ModelProfileView>,
    pub priced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelProfileView {
    pub id: String,
    pub label: Option<String>,
    /// Sprint 14.3: the profile's request changes (no secret values).
    #[ts(type = "Record<string, unknown>")]
    pub parameters: serde_json::Value,
    pub remove: Vec<String>,
}

/// Fields of a provider the desktop form edits. `None` = unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct ProviderPatch {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub protocol: Option<String>,
    pub auth: Option<String>,
    pub auth_header: Option<String>,
    /// Environment variable name of the key. Setting it on a provider that
    /// had an inline key is refused (the two are exclusive; the form only
    /// edits the variable name).
    pub api_key_env: Option<String>,
}

/// Fields of a model the desktop form edits. `None` = unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct ModelPatch {
    pub name: Option<String>,
    pub api_model: Option<String>,
    #[ts(type = "number | null")]
    pub max_input_tokens: Option<u64>,
    #[ts(type = "number | null")]
    pub max_output_tokens: Option<u64>,
    #[ts(type = "number | null")]
    pub context_window_tokens: Option<u64>,
    /// Remove `context_window_tokens` (unknown window).
    pub clear_context_window: bool,
    pub input_modalities: Option<Vec<String>>,
    /// Default reasoning profile id (must be one of the model's profiles).
    pub reasoning_default: Option<String>,
    pub clear_reasoning_default: bool,
    /// Sprint 14.3: replaces the whole profile list (ids unique; the
    /// default must stay one of them or be cleared in the same patch).
    pub profiles: Option<Vec<ReasoningProfileEdit>>,
}

// ─── Sprint 14.3: colours, reasoning profiles, skills catalogue ─────────────

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct ThemeColors {
    pub light: ColorSet,
    pub dark: ColorSet,
}

/// `#rrggbb` values; `None` keeps the theme's token.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct ColorSet {
    /// Actions, selections, focus ring.
    pub accent: Option<String>,
    /// Central surface.
    pub background: Option<String>,
    /// Navigation and side panels.
    pub panel: Option<String>,
}

/// A reasoning profile as edited by the desktop (engine `ReasoningProfile`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReasoningProfileEdit {
    pub id: String,
    pub label: Option<String>,
    /// JSON object merged into the request (validated by the engine:
    /// protected fields refused).
    #[ts(type = "Record<string, unknown>")]
    pub parameters: serde_json::Value,
    /// JSON Pointers removed from the request.
    pub remove: Vec<String>,
}

/// Skills catalogue outside any session (settings), for a project or the
/// user/built-in sources only. Consulting grants no trust.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SkillsCatalog {
    /// Project whose sources were included (`None`: user and built-in only).
    pub project_id: Option<String>,
    /// `cersei_agent::control::SkillsListing`.
    #[ts(type = "import('../engine').SkillsListing")]
    pub listing: serde_json::Value,
    /// Sources scanned, in order.
    pub sources: Vec<SkillSourceView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SkillSourceView {
    /// `project` | `user` | `builtin` (engine wording).
    pub source: String,
    pub path: String,
    pub skills: u32,
}
