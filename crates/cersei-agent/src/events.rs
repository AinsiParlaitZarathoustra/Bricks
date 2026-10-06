//! Agent events: the full event enum, AgentStream, and control messages.

use cersei_tools::permissions::{PermissionDecision, PermissionRequest};
use cersei_tools::PermissionLevel;
use cersei_types::*;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::AgentOutput;

// ─── Agent events ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AgentEvent {
    // Streaming content
    TextDelta(String),
    ThinkingDelta(String),

    // Tool lifecycle
    ToolStart {
        name: String,
        id: String,
        input: serde_json::Value,
    },
    ToolEnd {
        name: String,
        id: String,
        result: String,
        is_error: bool,
        duration: Duration,
        /// RTK compression metrics for this tool output, when compression ran
        /// (None for error results, which are not compressed).
        compression: Option<cersei_compression::CompressionStats>,
    },
    /// Progress of a long tool call (a shell command still running, a
    /// timeout being enforced).
    ToolProgress {
        name: String,
        message: String,
    },
    ToolPermissionCheck {
        name: String,
        id: String,
        level: PermissionLevel,
    },

    // Permission interaction
    PermissionRequired(PermissionRequest),

    // Turn lifecycle
    TurnStart {
        turn: u32,
    },
    TurnComplete {
        turn: u32,
        stop_reason: StopReason,
        usage: Usage,
    },
    ModelRequestStart {
        turn: u32,
        message_count: usize,
        token_estimate: u64,
    },
    ModelResponseStart {
        turn: u32,
        model: String,
    },

    // Context management
    TokenWarning {
        pct_used: f64,
        state: WarningState,
    },
    CompactStart {
        reason: CompactReason,
        messages_before: usize,
    },
    CompactEnd {
        messages_after: usize,
        tokens_freed: u64,
    },
    /// Every compaction attempt ends with its outcome, applied or not.
    CompactionResult {
        reason: CompactReason,
        outcome: crate::compact::CompactionOutcome,
    },
    /// Occupation, window and totals after a response or a rewrite.
    ContextUpdate(crate::context::ContextStatus),

    // Session lifecycle
    SessionLoaded {
        session_id: String,
        message_count: usize,
    },
    SessionSaved {
        session_id: String,
    },

    // Cost tracking (realtime)
    CostUpdate {
        turn_cost: f64,
        cumulative_cost: f64,
        input_tokens: u64,
        output_tokens: u64,
    },

    // Agent coordination (multi-agent)
    SubAgentSpawned {
        agent_id: String,
        prompt: String,
    },
    SubAgentComplete {
        agent_id: String,
        result: AgentOutput,
    },

    // Hook activity
    HookFired {
        event: cersei_hooks::HookEvent,
        hook_name: String,
    },
    HookBlocked {
        event: cersei_hooks::HookEvent,
        hook_name: String,
        reason: String,
    },

    // Long-term memory
    /// What was recalled into this run's system prompt (zero items when
    /// nothing was relevant).
    MemoryRecalled {
        items: usize,
        tokens: u64,
        omitted: usize,
        /// Token budget the recall had.
        budget: usize,
    },
    /// The maintenance after the answer (extraction, embeddings) started.
    MemoryMaintenanceStarted,
    /// It ended: its report (possibly cancelled), or why it failed. The
    /// answer was delivered before and is not affected.
    MemoryMaintenanceFinished(std::result::Result<cersei_memory::MaintenanceReport, String>),

    // Approvals and changes
    /// A tool call waits for a decision (see `control::ApprovalGate`).
    ApprovalRequested(crate::control::ApprovalRequest),
    /// The decision taken for it, and by whom.
    ApprovalResolved {
        approval_id: String,
        tool_call_id: String,
        decision: crate::control::Decision,
        by: crate::control::DecidedBy,
    },
    /// A tool call whose change was previewed succeeded: these files were
    /// written.
    EditApplied {
        tool_call_id: String,
        tool: String,
        files: Vec<cersei_tools::preview::FileChange>,
    },

    // Terminal
    Status(String),
    Error(String),
    Complete(AgentOutput),
}

#[derive(Debug, Clone, Copy)]
pub enum WarningState {
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactReason {
    /// The occupation crossed `compact_threshold` after a turn.
    ThresholdExceeded,
    ManualTrigger,
    /// The server refused a request for exceeding the context.
    ContextOverflow,
    /// The next request would manifestly not fit: compacted before sending.
    BudgetExceeded,
}

// ─── Agent stream ────────────────────────────────────────────────────────────

/// Returned by `agent.run_stream()`. Provides async iteration over events
/// and bidirectional control (permissions, cancellation, message injection).
pub struct AgentStream {
    rx: mpsc::Receiver<AgentEvent>,
    control_tx: mpsc::Sender<AgentControl>,
}

impl AgentStream {
    pub(crate) fn new(
        rx: mpsc::Receiver<AgentEvent>,
        control_tx: mpsc::Sender<AgentControl>,
    ) -> Self {
        Self { rx, control_tx }
    }

    /// Respond to a PermissionRequired event.
    pub fn respond_permission(&self, request_id: String, decision: PermissionDecision) {
        let _ = self.control_tx.try_send(AgentControl::PermissionResponse {
            request_id,
            decision,
        });
    }

    /// Send a cancellation signal.
    pub fn cancel(&self) {
        let _ = self.control_tx.try_send(AgentControl::Cancel);
    }

    /// Inject a user message mid-stream.
    pub fn inject_message(&self, message: String) {
        let _ = self
            .control_tx
            .try_send(AgentControl::InjectMessage(message));
    }

    /// Receive the next event.
    pub async fn next(&mut self) -> Option<AgentEvent> {
        self.rx.recv().await
    }

    /// Collect all events and return the final output.
    pub async fn collect(mut self) -> cersei_types::Result<AgentOutput> {
        while let Some(event) = self.rx.recv().await {
            match event {
                AgentEvent::Complete(output) => return Ok(output),
                AgentEvent::Error(e) => return Err(CerseiError::Other(anyhow::anyhow!(e))),
                _ => continue,
            }
        }
        Err(CerseiError::Cancelled)
    }

    /// Collect only text deltas into a single string.
    pub async fn collect_text(mut self) -> cersei_types::Result<String> {
        let mut text = String::new();
        while let Some(event) = self.rx.recv().await {
            match event {
                AgentEvent::TextDelta(t) => text.push_str(&t),
                AgentEvent::Complete(_) => return Ok(text),
                AgentEvent::Error(e) => return Err(CerseiError::Other(anyhow::anyhow!(e))),
                _ => continue,
            }
        }
        Ok(text)
    }
}

// ─── Control messages ────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) enum AgentControl {
    #[allow(dead_code)]
    PermissionResponse {
        request_id: String,
        decision: PermissionDecision,
    },
    Cancel,
    #[allow(dead_code)]
    InjectMessage(String),
}
