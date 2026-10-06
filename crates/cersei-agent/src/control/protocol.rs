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
use cersei_memory::MaintenanceReport;
use cersei_tools::preview::ChangeKind;
use cersei_types::Usage;
use serde::{Deserialize, Serialize};

/// Version of the event and command schema. Incremented on any change a
/// consumer could notice; documented in `docs/cli.md`.
pub const SCHEMA_VERSION: u32 = 2;

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

/// A file written by an approved change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrittenFile {
    pub path: String,
    pub kind: ChangeKind,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The session is ready (new or resumed).
    SessionOpened {
        working_dir: String,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
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
    /// An approved, previewed change was written.
    EditApplied {
        tool_call_id: String,
        tool: String,
        files: Vec<WrittenFile>,
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
        /// `next_turn` during a run, `next_run` otherwise.
        applies: String,
    },
    ContextCleared {
        messages_removed: usize,
    },
    SessionSaved,
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
            Event::EditApplied { .. } => "edit_applied",
            Event::MemoryRecalled { .. } => "memory_recalled",
            Event::Context { .. } => "context",
            Event::Usage { .. } => "usage",
            Event::Compaction { .. } => "compaction",
            Event::ModelChanged { .. } => "model_changed",
            Event::ContextCleared { .. } => "context_cleared",
            Event::SessionSaved => "session_saved",
            Event::Notice { .. } => "notice",
            Event::RunFinished { .. } => "run_finished",
            Event::MemoryMaintenanceStarted => "memory_maintenance_started",
            Event::MemoryMaintenanceFinished { .. } => "memory_maintenance_finished",
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
}

impl Prompt {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            blocks: vec![PromptBlock::Text { text: text.into() }],
        }
    }
}

/// Commands to the engine. Presentation gestures (opening a window,
/// expanding a block) are not commands: they stay in the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Start a run. Refused while a run is active (never queued silently).
    Submit { prompt: Prompt },
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
    /// Compact the context now. Refused during a run.
    Compact,
    /// Empty the active context (kept in the raw history and as a
    /// snapshot). Refused during a run.
    ClearContext,
    /// Switch to another stored session. Refused during a run.
    Resume { session_id: String },
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
            Command::Cancel => "cancel",
            Command::SetModel { .. } => "set_model",
            Command::Compact => "compact",
            Command::ClearContext => "clear_context",
            Command::Resume { .. } => "resume",
            Command::Approve { .. } => "approve",
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
            r#"{"type":"approve","approval_id":"ap_1","decision":"allow_for_session"}"#,
        )
        .unwrap();
        assert_eq!(c.name(), "approve");
        let p: Prompt = serde_json::from_str(
            r#"{"blocks":[{"kind":"text","text":"hi"},{"kind":"file","path":"a b.rs"}]}"#,
        )
        .unwrap();
        assert_eq!(p.blocks.len(), 2);
    }
}
