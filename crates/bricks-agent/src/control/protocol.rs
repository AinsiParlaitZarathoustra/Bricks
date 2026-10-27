//! The command/event contract between the engine and its frontends.
//!
//! Frontends send [`Command`]s to a [`super::Controller`] and receive
//! [`Envelope`]s: schema version, session, run, sequence number and one
//! [`Event`]. The same contract serves the headless JSONL output and the
//! terminal interface; future frontends (editor extension, desktop app,
//! WebSocket server) can use it unchanged.
//!
//! Guarantees:
//! * `seq` starts at 1 per controller and increases by one per delivered
//!   envelope: a gap never happens silently (text and reasoning deltas may
//!   be merged when the consumer is slow; their text is kept whole).
//! * Within a run, events keep the order the engine produced them in, and
//!   the run ends with exactly one `run_finished` (succeeded, incomplete,
//!   failed or cancelled). Memory maintenance comes after it, as its own
//!   phase, and never restarts the task.
//! * `thinking_delta` carries only reasoning the provider exposed; nothing
//!   is reconstructed.
//! * No API key or authentication header ever appears in an event.

use super::approval::{ApprovalRequest, DecidedBy, Decision};
use crate::context::ContextStatus;
use bricks_memory::MaintenanceReport;
use bricks_tools::preview::{ChangeKind, FileChange};
use bricks_tools::tool_report::ToolStatus;
use bricks_types::Usage;
use serde::{Deserialize, Serialize};

/// Version of the event and command schema. Incremented on any change a
/// consumer could notice; documented in `docs/cli.md`.
pub const SCHEMA_VERSION: u32 = 8;

/// Largest patch an edit event carries for one file. A longer one is cut and
/// marked (`patch_truncated`).
pub const MAX_PATCH_BYTES: usize = 512 * 1024;

/// Largest output a sub-agent's tool event carries (`output_truncated` says
/// when it was cut).
pub const MAX_SUBAGENT_OUTPUT_BYTES: usize = 96 * 1024;

/// The wire name of a tool call's status (`status` of the tool events).
pub fn tool_status_name(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Success => "success",
        ToolStatus::Failure => "failure",
        ToolStatus::TimedOut => "timed_out",
        ToolStatus::Cancelled => "cancelled",
        ToolStatus::Running => "running",
    }
}

/// Cuts `text` to at most `max` bytes on a character boundary. The flag says
/// whether anything was cut.
pub fn cap_text(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// One delivered event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub schema: u32,
    pub session_id: String,
    /// The run the event belongs to (absent for session-level events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub at: i64,
    #[serde(flatten)]
    pub event: Event,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// A final answer, no tool call left.
    Succeeded,
    /// Stopped at a limit before a final answer (turns, output tokens, no
    /// progress, content filter, empty response): `termination` says which.
    /// The history and partial results are kept.
    Incomplete,
    Failed,
    Cancelled,
}

/// Why a run failed, when the reason is one a script acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// A step needed an approval nobody could give (non-interactive).
    ApprovalRequired,
    /// Any other error (provider, configuration, tool infrastructure).
    Error,
}

/// How a maintenance pass ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceOutcome {
    Completed,
    Cancelled,
    Failed,
}

/// An attachment as the run received it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub kind: String,
    pub path: String,
    pub bytes: u64,
    /// SHA-256 of the captured content (files and images).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// What was left out to stay within the limits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A file written by an applied change. `added` / `removed` count lines
/// of the complete diff; a binary file has none (`binary`, and no `patch`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrittenFile {
    /// The path as the change names it (relative to the workspace).
    pub path: String,
    pub kind: ChangeKind,
    pub added: usize,
    pub removed: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
    /// The file actually written, absolute. Absent when the engine does not
    /// know where it is (an older event).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute: Option<String>,
    /// Unified diff of the entry approved and written (old to new). Absent
    /// for a binary file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// The patch was longer than [`MAX_PATCH_BYTES`] and is cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub patch_truncated: bool,
}

impl WrittenFile {
    /// The file of an applied change, with the diff that was approved and
    /// written.
    pub fn from_change(change: &FileChange) -> Self {
        let (added, removed, patch, patch_truncated) = if change.binary {
            (0, 0, None, false)
        } else {
            let (patch, truncated) = cap_text(&change.diff, MAX_PATCH_BYTES);
            (change.added, change.removed, Some(patch), truncated)
        };
        Self {
            path: change.path.clone(),
            kind: change.kind,
            added,
            removed,
            binary: change.binary,
            absolute: Some(change.absolute.display().to_string()),
            patch,
            patch_truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The session is ready (new or resumed).
    SessionOpened {
        working_dir: String,
        model: String,
        /// The profile requested for the session (`None`: the model's
        /// default).
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        /// The profile that applies: the requested one, else the model's
        /// declared default; absent when neither exists.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_reasoning: Option<String>,
        resumed: bool,
        message_count: usize,
        warnings: Vec<String>,
    },
    RunStarted {
        prompt: String,
        attachments: Vec<AttachmentInfo>,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
    },
    TextDelta {
        text: String,
    },
    /// Reasoning the provider exposed (raw or summarised, as it sent it).
    ThinkingDelta {
        text: String,
    },
    ToolStarted {
        tool_call_id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Progress of a long tool call. The engine's progress hook names the
    /// tool, not the call: `tool_call_id` is absent when several calls of
    /// the same tool run at once.
    ToolProgress {
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        name: String,
        message: String,
    },
    ToolFinished {
        tool_call_id: String,
        name: String,
        is_error: bool,
        duration_ms: u64,
        output: String,
        /// `false` when the call never ran (refused, denied, blocked by a
        /// hook). Absent: it ran.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        executed: Option<bool>,
        /// `success`, `failure`, `timed_out`, `cancelled`, or `running` (a
        /// background command still going).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    ApprovalRequested {
        approval: ApprovalRequest,
    },
    ApprovalResolved {
        approval_id: String,
        tool_call_id: String,
        decision: Decision,
        by: DecidedBy,
    },
    /// A question for the person (`Ask`), waiting for an answer.
    QuestionAsked {
        question: crate::control::QuestionRequest,
    },
    /// How a question ended: answered, or skipped (`user`, `timeout`,
    /// `cancelled`, `non_interactive`).
    QuestionResolved {
        question_id: String,
        tool_call_id: String,
        outcome: crate::control::QuestionOutcome,
    },
    /// The session's permission policy changed (by a person); calls
    /// decided from now on follow `revision`.
    PermissionPolicyChanged {
        policy: crate::permission::SessionPolicy,
        revision: u64,
    },
    /// A change was applied to the session's workspace: a tool call of the
    /// session agent or of a sub-agent working in that workspace
    /// (`agent_id`) succeeded, or a ChangeSet was applied (`changeset_id`).
    /// One event per change; `(agent_id, tool_call_id)` or `changeset_id`
    /// identifies it.
    EditApplied {
        tool_call_id: String,
        tool: String,
        files: Vec<WrittenFile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changeset_id: Option<String>,
    },
    MemoryRecalled {
        items: usize,
        tokens: u64,
        omitted: usize,
        budget: usize,
    },
    /// Occupation of the context after a response or a rewrite.
    Context {
        status: Box<ContextStatus>,
    },
    /// Usage of one model response and the session totals. `cost_usd` is
    /// absent when no price is configured: unknown, not zero.
    Usage {
        turn: Box<Usage>,
        total: Box<Usage>,
    },
    Compaction {
        reason: String,
        outcome: String,
        compacted: bool,
    },
    ModelChanged {
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        /// See `session_opened.effective_reasoning`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_reasoning: Option<String>,
        /// `next_turn` during a run, `next_run` otherwise.
        applies: String,
    },
    /// The reasoning profile changed (`set_reasoning`): the effective
    /// profile before and after (ids and configured labels), and the
    /// requested one (`None`: the model's default). `applies`: `next_turn`
    /// during a run, `next_run` otherwise.
    ReasoningChanged {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_label: Option<String>,
        applies: String,
    },
    ContextCleared {
        messages_removed: usize,
    },
    /// A `revise` cut the active context before the user message at
    /// `user_index` (among the user-role messages); the run that replaces
    /// it follows.
    HistoryRevised {
        user_index: usize,
        messages_removed: usize,
    },
    SessionSaved,
    /// The model named the session (its first title was the shortened
    /// first prompt).
    TitleChanged {
        title: String,
    },
    /// The session's plan (TodoWrite) was saved: the complete state, whose
    /// `revision` orders it against the snapshot's and other updates.
    PlanUpdated {
        plan: bricks_tools::todo_write::PlanState,
    },
    /// Something worth showing that is not an error (retries, configuration
    /// diagnostics, nudges).
    Notice {
        message: String,
    },
    /// The single end of a run.
    RunFinished {
        outcome: RunOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<FailureKind>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// How the engine ended the run (`succeeded` and `incomplete` only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        termination: Option<crate::Termination>,
        /// The final answer (possibly partial when incomplete, cancelled or
        /// failed).
        text: String,
        /// Generation turns that got a response.
        turns: u32,
        /// Approvals that were needed and could not be given.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        approvals_unsatisfied: Vec<ApprovalRequest>,
    },
    MemoryMaintenanceStarted,
    MemoryMaintenanceFinished {
        outcome: MaintenanceOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        report: Option<MaintenanceReport>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Results of a `search` command (text search in the workspace, by the
    /// shared code understanding engine).
    SearchResults {
        query: String,
        /// `complete`, `partial` (limits reached: absence proves nothing),
        /// `cancelled` or `error`.
        status: String,
        hits: Vec<SearchHit>,
        /// Hits found but not listed.
        omitted: usize,
        /// Limits reached, files skipped, errors.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        notes: Vec<String>,
        elapsed_ms: u64,
    },
    /// A sub-agent was created (`agent` gives its identity, profile,
    /// model and reasoning as requested and applied, workspace).
    AgentSpawned {
        agent: Box<crate::agents::SpawnInfo>,
    },
    /// A sub-agent changed state (`waiting_admission`, `starting`,
    /// `running`, `cancelling`, then one terminal state).
    AgentState {
        agent_id: String,
        state: crate::agents::InstanceState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// The execution of the identity this state belongs to: 1 at launch,
        /// one more for each resume. A state of an older execution is stale.
        #[serde(default = "crate::agents::spawn::first_execution")]
        execution: u32,
    },
    /// A sub-agent's tool call started (its text is never streamed).
    AgentToolStarted {
        agent_id: String,
        tool_call_id: String,
        name: String,
        input: serde_json::Value,
    },
    AgentToolFinished {
        agent_id: String,
        tool_call_id: String,
        name: String,
        is_error: bool,
        duration_ms: u64,
        /// See `tool_finished.executed`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        executed: Option<bool>,
        /// See `tool_finished.status`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        /// The tool's output, cut to [`MAX_SUBAGENT_OUTPUT_BYTES`]. Absent
        /// for events written before the output was carried.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        output_truncated: bool,
    },
    /// The compact result of a sub-agent (its usage is its own: the run's
    /// `usage` events do not include it).
    AgentFinished {
        result: Box<crate::agents::AgentResult>,
    },
    /// An isolated sub-agent's work, ready to inspect and apply.
    ChangesReady {
        changeset: Box<crate::agents::ChangeSet>,
    },
    /// A ChangeSet was applied, discarded, or hit a conflict (nothing
    /// written on conflict).
    ChangesUpdated {
        changeset_id: String,
        state: crate::agents::ChangeSetState,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Usage of a root run: the session agent's own, its descendants' and
    /// the total, each response counted once. Published when the run ends
    /// (`final_total: false` while descendants still run), then once more
    /// when the last descendant ends.
    RunUsage {
        root_run_id: String,
        own: Box<Usage>,
        descendants: Box<Usage>,
        total: Box<Usage>,
        pending_agents: usize,
        final_total: bool,
    },
    /// A background job started (`Bash` with `background: true`).
    JobStarted {
        job_id: String,
        agent_id: String,
        root_run_id: String,
        command: String,
        cwd: String,
        pid: u32,
    },
    /// Output so far (throttled; read the output with `Job output`).
    JobOutput {
        job_id: String,
        stdout_bytes: u64,
        stderr_bytes: u64,
    },
    /// A job ended: `completed`, `failed`, `stopped` (one per job).
    JobFinished {
        job_id: String,
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        logs: Vec<String>,
    },
    /// Answer to `agent_control`.
    AgentControlResult {
        action: String,
        ok: bool,
        text: String,
    },
    /// Answer to `list_agent_profiles` / `reload_agent_profiles`.
    AgentProfiles {
        profiles: Vec<crate::agents::ProfileListing>,
        total: usize,
        page: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        diagnostics: Vec<crate::agents::ProfileDiagnostic>,
    },
    /// A skill was activated for the session agent (`agent_id` absent) or
    /// a sub-agent. `origin`: `user`, `model`, `profile`, `restored`.
    SkillActivated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        name: String,
        source: String,
        revision: String,
        origin: String,
    },
    /// A skill was not activated, or a restored activation was suspended.
    SkillActivationFailed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        name: String,
        reason: String,
    },
    /// Answer to `list_skills` / `reload_skills`: every candidate with its
    /// provenance and state (project ones included, for a person to
    /// inspect; they reach the model only once approved).
    Skills {
        listing: Box<SkillsListing>,
    },
    /// Project skills were found and wait for a person's decision
    /// (`trust_project_skills`). Asked once per session.
    SkillsTrustRequested {
        project: String,
        sources: Vec<SkillSourceInfo>,
    },
    /// The project decision of this session (`by`: `user` or `caller`).
    SkillsTrustResolved {
        project: String,
        allowed: bool,
        by: String,
    },
    /// A command was refused; nothing changed.
    CommandRejected {
        command: String,
        reason: String,
    },
}

impl Event {
    /// The `type` tag.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::SessionOpened { .. } => "session_opened",
            Event::RunStarted { .. } => "run_started",
            Event::TextDelta { .. } => "text_delta",
            Event::ThinkingDelta { .. } => "thinking_delta",
            Event::ToolStarted { .. } => "tool_started",
            Event::ToolProgress { .. } => "tool_progress",
            Event::ToolFinished { .. } => "tool_finished",
            Event::ApprovalRequested { .. } => "approval_requested",
            Event::ApprovalResolved { .. } => "approval_resolved",
            Event::QuestionAsked { .. } => "question_asked",
            Event::QuestionResolved { .. } => "question_resolved",
            Event::PermissionPolicyChanged { .. } => "permission_policy_changed",
            Event::EditApplied { .. } => "edit_applied",
            Event::MemoryRecalled { .. } => "memory_recalled",
            Event::Context { .. } => "context",
            Event::Usage { .. } => "usage",
            Event::Compaction { .. } => "compaction",
            Event::ModelChanged { .. } => "model_changed",
            Event::ReasoningChanged { .. } => "reasoning_changed",
            Event::ContextCleared { .. } => "context_cleared",
            Event::HistoryRevised { .. } => "history_revised",
            Event::SessionSaved => "session_saved",
            Event::TitleChanged { .. } => "title_changed",
            Event::PlanUpdated { .. } => "plan_updated",
            Event::Notice { .. } => "notice",
            Event::RunFinished { .. } => "run_finished",
            Event::MemoryMaintenanceStarted => "memory_maintenance_started",
            Event::MemoryMaintenanceFinished { .. } => "memory_maintenance_finished",
            Event::SearchResults { .. } => "search_results",
            Event::AgentSpawned { .. } => "agent_spawned",
            Event::AgentState { .. } => "agent_state",
            Event::AgentToolStarted { .. } => "agent_tool_started",
            Event::AgentToolFinished { .. } => "agent_tool_finished",
            Event::AgentFinished { .. } => "agent_finished",
            Event::AgentProfiles { .. } => "agent_profiles",
            Event::ChangesReady { .. } => "changes_ready",
            Event::ChangesUpdated { .. } => "changes_updated",
            Event::RunUsage { .. } => "run_usage",
            Event::AgentControlResult { .. } => "agent_control_result",
            Event::JobStarted { .. } => "job_started",
            Event::JobOutput { .. } => "job_output",
            Event::JobFinished { .. } => "job_finished",
            Event::SkillActivated { .. } => "skill_activated",
            Event::SkillActivationFailed { .. } => "skill_activation_failed",
            Event::Skills { .. } => "skills",
            Event::SkillsTrustRequested { .. } => "skills_trust_requested",
            Event::SkillsTrustResolved { .. } => "skills_trust_resolved",
            Event::CommandRejected { .. } => "command_rejected",
        }
    }

    /// Deltas may be merged with an adjacent delta of the same kind (the
    /// text is concatenated, nothing is lost). Every other event is
    /// delivered as is.
    pub fn is_delta(&self) -> bool {
        matches!(self, Event::TextDelta { .. } | Event::ThinkingDelta { .. })
    }
}

/// A source holding project skills, for the trust request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSourceInfo {
    pub source: String,
    pub path: String,
    pub skills: usize,
}

/// One candidate of the skills listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillListing {
    pub name: String,
    pub source: String,
    /// `available` (wins its name, usable), `active` (also active in the
    /// session agent), `shadowed` (a higher source has this name),
    /// `invalid`, `incompatible`, `pending_approval`, `refused` (project
    /// skills not approved), `disabled`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Error, incompatibilities, notes, collisions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
}

/// The skills of the session, as `skills` reports them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsListing {
    pub generation: u64,
    /// `pending`, `granted` or `denied` for this session.
    pub project_decision: String,
    pub entries: Vec<SkillListing>,
    pub total: usize,
    pub page: usize,
    /// Discovery diagnostics (unreadable folders, links not followed,
    /// limits reached).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
    /// Activable skills left out of the model's catalog by its bounds.
    #[serde(default)]
    pub catalog_omitted: usize,
}

/// One hit of a `search`: 1-based line, 1-based column in characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub text: String,
}

/// A block of a prompt, as a frontend builds it. The engine converts it at
/// submission: files are read then (what was sent is what the session
/// keeps), images are checked against the model's capabilities by the
/// provider before sending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptBlock {
    Text { text: String },
    File { path: String },
    Folder { path: String },
    Image { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Prompt {
    pub blocks: Vec<PromptBlock>,
    /// Skills to activate, in order, before the prompt (a person's choice:
    /// `/skill`, `--skill`). All are checked when the command is received;
    /// if one cannot be used, the command is refused and nothing is sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// The question timer the person set, captured when the message is
    /// received and kept for the whole run (absent: no timer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_timer: Option<crate::control::AskTimer>,
}

impl Prompt {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            blocks: vec![PromptBlock::Text { text: text.into() }],
            skills: Vec::new(),
            ask_timer: None,
        }
    }

    /// The same prompt, with skills activated before it.
    pub fn with_skills(mut self, skills: Vec<String>) -> Self {
        self.skills = skills;
        self
    }
}

/// Commands to the engine. Presentation gestures (opening a window,
/// expanding a block) are not commands: they stay in the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Start a run. Refused while a run is active (never queued silently),
    /// or when one of the prompt's `skills` cannot be used.
    Submit { prompt: Prompt },
    /// Replace a past user message and run again from it: the active
    /// context is cut before that message (the context as it was is kept
    /// as a snapshot, and stays in the raw history), then `prompt` is
    /// submitted as by `submit`. `user_index` is the message's index among
    /// the user-role messages of the active history; `original` its text,
    /// as the person saw it. Refused, with nothing changed, while anything
    /// runs, when that message is not there with that text (the history
    /// changed, or it was compacted away), or for the same reasons as a
    /// submit.
    Revise {
        user_index: usize,
        original: String,
        prompt: Prompt,
    },
    /// Cancel the active run, or else the memory maintenance in progress.
    /// Always accepted immediately.
    Cancel,
    /// Change model and/or reasoning profile (names from the
    /// configuration). During a run, applies from its next turn.
    SetModel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
    },
    /// Select a reasoning profile of the current model (`profile`: one of
    /// its declared ids, `default` included when a profile has that id), or
    /// go back to the model's declared default (`profile` absent or null).
    /// An unknown id is refused and nothing changes. During a run, applies
    /// from its next request.
    SetReasoning {
        #[serde(default)]
        profile: Option<String>,
    },
    /// Compact the context now. Refused during a run.
    Compact,
    /// Empty the active context (kept in the raw history and as a
    /// snapshot). Refused during a run.
    ClearContext,
    /// Switch to another stored session. Refused during a run.
    Resume { session_id: String },
    /// Search the workspace's text (literal, or a regex). Read-only:
    /// accepted during a run; answered by `search_results`.
    Search {
        text: String,
        #[serde(default)]
        regex: bool,
    },
    /// List or search the sub-agent profiles (20 per page); answered by
    /// `agent_profiles`. Accepted at any time.
    ListAgentProfiles {
        #[serde(default)]
        query: String,
        #[serde(default)]
        page: usize,
    },
    /// Read the profile files again; running sub-agents keep theirs.
    ReloadAgentProfiles,
    /// Act on the session's sub-agents and ChangeSets (`list`, `status`,
    /// `result`, `cancel`, `inspect_changes`, `apply_changes`,
    /// `discard_changes`); answered by `agent_control_result`. A person's
    /// command: the session's working tree is the destination of `apply`.
    AgentControl {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changeset_id: Option<String>,
        /// For `jobs` (list the session's jobs) and `stop_job`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job_id: Option<String>,
        /// For `message`: the text for the running sub-agent (bounded).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// List the skills (all candidates, 20 per page, filtered by `query`);
    /// answered by `skills`. Accepted at any time.
    ListSkills {
        #[serde(default)]
        query: String,
        #[serde(default)]
        page: usize,
    },
    /// Read the skill sources again; activations already made keep their
    /// text. Answered by `skills`.
    ReloadSkills,
    /// A person's decision on this project's skills, for this session
    /// only. Never sent by the model.
    TrustProjectSkills { allow: bool },
    /// Set this session's permission policy (a person's choice; never
    /// sent by the model). Answered by `permission_policy_changed`.
    SetPermissionPolicy {
        policy: crate::permission::SessionPolicy,
    },
    /// Answer a question of `Ask`: a choice, a free text, or a skip. The
    /// first resolution accepted closes it.
    AnswerQuestion {
        question_id: String,
        answer: crate::control::QuestionAnswer,
    },
    /// Answer an approval request.
    Approve {
        approval_id: String,
        decision: Decision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Submit { .. } => "submit",
            Command::Revise { .. } => "revise",
            Command::Cancel => "cancel",
            Command::SetModel { .. } => "set_model",
            Command::SetReasoning { .. } => "set_reasoning",
            Command::Compact => "compact",
            Command::ClearContext => "clear_context",
            Command::Resume { .. } => "resume",
            Command::Search { .. } => "search",
            Command::ListAgentProfiles { .. } => "list_agent_profiles",
            Command::ReloadAgentProfiles => "reload_agent_profiles",
            Command::AgentControl { .. } => "agent_control",
            Command::ListSkills { .. } => "list_skills",
            Command::ReloadSkills => "reload_skills",
            Command::TrustProjectSkills { .. } => "trust_project_skills",
            Command::SetPermissionPolicy { .. } => "set_permission_policy",
            Command::Approve { .. } => "approve",
            Command::AnswerQuestion { .. } => "answer_question",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_are_flat_and_tagged() {
        let env = Envelope {
            schema: SCHEMA_VERSION,
            session_id: "s1".into(),
            run_id: Some("r1".into()),
            seq: 3,
            at: 0,
            event: Event::TextDelta { text: "hé".into() },
        };
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["type"], "text_delta");
        assert_eq!(v["text"], "hé");
        assert_eq!(v["seq"], 3);
        let back: Envelope = serde_json::from_value(v).unwrap();
        assert_eq!(back, env);
        let c: Command = serde_json::from_str(
            r#"{"type":"approve","approval_id":"ap_1","decision":"allow"}"#,
        )
        .unwrap();
        assert_eq!(c.name(), "approve");
        // An answer covers one call: a grant for the session no longer exists.
        assert!(
            serde_json::from_str::<Command>(
                r#"{"type":"approve","approval_id":"ap_1","decision":"allow_for_session"}"#,
            )
            .is_err()
        );
        let p: Prompt = serde_json::from_str(
            r#"{"blocks":[{"kind":"text","text":"hi"},{"kind":"file","path":"a b.rs"}]}"#,
        )
        .unwrap();
        assert_eq!(p.blocks.len(), 2);
    }

    fn written(binary: bool) -> WrittenFile {
        WrittenFile {
            path: "a.txt".into(),
            kind: ChangeKind::Modify,
            added: 1,
            removed: 0,
            binary,
            absolute: None,
            patch: None,
            patch_truncated: false,
        }
    }

    #[test]
    fn the_schema_is_seven_and_the_new_fields_are_only_written_when_set() {
        assert_eq!(SCHEMA_VERSION, 8);
        let mut w = written(false);
        let v = serde_json::to_value(&w).unwrap();
        assert!(
            v.get("absolute").is_none()
                && v.get("patch").is_none()
                && v.get("patch_truncated").is_none()
        );
        assert!(
            v.get("binary").is_none(),
            "a text file says nothing of binary"
        );
        w.absolute = Some("/w/a.txt".into());
        w.patch = Some("@@".into());
        let v = serde_json::to_value(&w).unwrap();
        assert_eq!(v["absolute"], "/w/a.txt");
        assert_eq!(v["patch"], "@@");
    }

    #[test]
    fn an_event_of_an_older_engine_reads_with_the_new_fields_absent() {
        let old = serde_json::json!({
            "schema": 5, "session_id": "s", "seq": 1, "at": 0,
            "type": "edit_applied", "tool_call_id": "c", "tool": "Write",
            "files": [{ "path": "a.txt", "kind": "modify", "added": 1, "removed": 0 }]
        });
        let env: Envelope = serde_json::from_value(old).unwrap();
        let Event::EditApplied { files, .. } = env.event else {
            panic!("an edit event");
        };
        assert_eq!(files[0].absolute, None);
        assert_eq!(files[0].patch, None);
        assert!(!files[0].patch_truncated && !files[0].binary);

        let tool: Event = serde_json::from_value(serde_json::json!({
            "type": "tool_finished", "tool_call_id": "c", "name": "Bash",
            "is_error": false, "duration_ms": 3, "output": "ok"
        }))
        .unwrap();
        assert!(matches!(
            tool,
            Event::ToolFinished {
                executed: None,
                status: None,
                ..
            }
        ));

        let agent: Event = serde_json::from_value(serde_json::json!({
            "type": "agent_tool_finished", "agent_id": "ag", "tool_call_id": "c",
            "name": "Read", "is_error": false, "duration_ms": 1
        }))
        .unwrap();
        assert!(matches!(
            agent,
            Event::AgentToolFinished {
                executed: None,
                output: None,
                output_truncated: false,
                ..
            }
        ));
    }

    #[test]
    fn a_refused_call_and_its_status_are_written() {
        let event = Event::ToolFinished {
            tool_call_id: "c".into(),
            name: "Bash".into(),
            is_error: true,
            duration_ms: 0,
            output: "refusé".into(),
            executed: Some(false),
            status: Some(tool_status_name(ToolStatus::Failure).into()),
        };
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["executed"], false);
        assert_eq!(v["status"], "failure");
    }

    #[test]
    fn a_change_carries_the_diff_it_was_approved_with_and_a_long_one_is_cut() {
        let change = FileChange {
            path: "big.txt".into(),
            absolute: "/w/big.txt".into(),
            kind: ChangeKind::Modify,
            before_sha256: None,
            diff: "+é".repeat(MAX_PATCH_BYTES),
            added: 3,
            removed: 0,
            binary: false,
        };
        let w = WrittenFile::from_change(&change);
        assert_eq!(w.absolute.as_deref(), Some("/w/big.txt"));
        assert!(w.patch_truncated);
        let patch = w.patch.unwrap();
        assert!(patch.len() <= MAX_PATCH_BYTES && patch.starts_with("+é"));
        assert_eq!((w.added, w.removed), (3, 0));
    }

    #[test]
    fn a_binary_change_has_no_patch_and_no_line_count() {
        let change = FileChange {
            path: "logo.png".into(),
            absolute: "/w/logo.png".into(),
            kind: ChangeKind::Modify,
            before_sha256: None,
            diff: "garbage\n+lines\n".into(),
            added: 1,
            removed: 1,
            binary: true,
        };
        let w = WrittenFile::from_change(&change);
        assert!(w.binary && w.patch.is_none() && !w.patch_truncated);
        assert_eq!((w.added, w.removed), (0, 0));
    }

    #[test]
    fn a_cut_text_stays_on_a_character_boundary() {
        let (cut, truncated) = cap_text("aé", 2);
        assert_eq!((cut.as_str(), truncated), ("a", true));
        assert_eq!(cap_text("aé", 3), ("aé".to_string(), false));
    }

    /// A sub-agent's tool output keeps 96 KiB (98 304 bytes); an emoji
    /// across that boundary is dropped whole, and the flag says so.
    #[test]
    fn a_sub_agent_output_is_cut_at_96_kib_on_a_character_boundary() {
        assert_eq!(MAX_SUBAGENT_OUTPUT_BYTES, 98_304);
        let exact = "a".repeat(MAX_SUBAGENT_OUTPUT_BYTES);
        assert_eq!(
            cap_text(&exact, MAX_SUBAGENT_OUTPUT_BYTES),
            (exact.clone(), false)
        );
        let across = "a".repeat(MAX_SUBAGENT_OUTPUT_BYTES - 2) + "😀" + "tail";
        let (cut, truncated) = cap_text(&across, MAX_SUBAGENT_OUTPUT_BYTES);
        assert!(truncated);
        assert_eq!(cut.len(), MAX_SUBAGENT_OUTPUT_BYTES - 2);
        assert!(cut.bytes().all(|b| b == b'a'));
    }

    #[test]
    fn the_status_names_are_the_wire_names() {
        let names: Vec<&str> = [
            ToolStatus::Success,
            ToolStatus::Failure,
            ToolStatus::TimedOut,
            ToolStatus::Cancelled,
            ToolStatus::Running,
        ]
        .into_iter()
        .map(tool_status_name)
        .collect();
        assert_eq!(
            names,
            ["success", "failure", "timed_out", "cancelled", "running"]
        );
    }
}
