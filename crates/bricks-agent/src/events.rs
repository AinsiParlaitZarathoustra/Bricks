//! Agent events: the full event enum, AgentStream, and control messages.

use bricks_tools::PermissionLevel;
use bricks_tools::permissions::{PermissionDecision, PermissionRequest};
use bricks_types::*;
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
        compression: Option<bricks_compression::CompressionStats>,
        /// The tool ran. `false` for a call refused, denied or blocked before
        /// it ran (its result says why).
        executed: bool,
        /// How the call ended, as its report says.
        status: bricks_tools::tool_report::ToolStatus,
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
        event: bricks_hooks::HookEvent,
        hook_name: String,
    },
    HookBlocked {
        event: bricks_hooks::HookEvent,
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
    MemoryMaintenanceFinished(std::result::Result<bricks_memory::MaintenanceReport, String>),

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
        files: Vec<bricks_tools::preview::FileChange>,
    },

    // Skills
    /// A skill's instructions are now in this agent's context (`origin`:
    /// `model`, `user`, `profile`, or `restored` with a session).
    /// `agent_id` is set when a sub-agent's event is forwarded.
    SkillActivated {
        agent_id: Option<String>,
        name: String,
        source: String,
        revision: String,
        origin: String,
    },
    /// A skill was not activated (refused, too large, failed), or an
    /// activation was suspended when a session was restored.
    SkillNotActivated {
        agent_id: Option<String>,
        name: String,
        reason: String,
    },

    // Terminal
    /// Lifecycle of a sub-agent started by this run (its own text is never
    /// part of this run's answer).
    SubAgent(crate::agents::SubAgentEvent),
    /// A background job of the session (started, output so far, finished).
    Job(bricks_tools::jobs::JobEvent),
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
    controls: std::sync::Arc<StreamControls>,
}

impl AgentStream {
    pub(crate) fn new(
        rx: mpsc::Receiver<AgentEvent>,
        controls: std::sync::Arc<StreamControls>,
    ) -> Self {
        Self { rx, controls }
    }

    /// Respond to a PermissionRequired event.
    pub fn respond_permission(&self, request_id: String, decision: PermissionDecision) {
        if let Some(reply) = self.controls.inner.lock().pending.remove(&request_id) {
            let _ = reply.send(decision);
        }
    }

    /// Send a cancellation signal.
    pub fn cancel(&self) {
        self.controls.cancel.cancel();
    }

    /// Queue a user message for the next model request, after pending tool results.
    /// Messages submitted after the run has finished are ignored.
    pub fn inject_message(&self, message: String) {
        let mut inner = self.controls.inner.lock();
        if !inner.finished && !message.trim().is_empty() {
            inner.messages.push(message);
        }
    }

    /// Receive the next event.
    pub async fn next(&mut self) -> Option<AgentEvent> {
        self.rx.recv().await
    }

    /// Collect all events and return the final output.
    pub async fn collect(mut self) -> bricks_types::Result<AgentOutput> {
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
    pub async fn collect_text(mut self) -> bricks_types::Result<String> {
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

// Dropping the consumer must not leave tools awaiting permission forever.
impl Drop for AgentStream {
    fn drop(&mut self) {
        // A consumed final answer must not cancel subsequent memory maintenance.
        let mut inner = self.controls.inner.lock();
        if !inner.finished {
            self.controls.cancel.cancel();
        }
        inner.finished = true;
        inner.pending.clear();
    }
}

/// Why a message was not queued for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectRefusal {
    /// Nothing but whitespace.
    Empty,
    /// Over [`MAX_INJECTED_BYTES`].
    TooLarge,
    /// [`MAX_PENDING_MESSAGES`] are already waiting.
    QueueFull,
    /// The run has ended: nothing would read it.
    Finished,
}

/// Messages waiting for the next model request, at most.
pub const MAX_PENDING_MESSAGES: usize = 32;
/// One injected message, at most (UTF-8 bytes).
pub const MAX_INJECTED_BYTES: usize = 16 * 1024;

#[derive(Default)]
struct ControlState {
    messages: Vec<String>,
    pending: std::collections::HashMap<String, tokio::sync::oneshot::Sender<PermissionDecision>>,
    finished: bool,
}

pub(crate) struct StreamControls {
    inner: parking_lot::Mutex<ControlState>,
    cancel: tokio_util::sync::CancellationToken,
}

impl StreamControls {
    pub(crate) fn new(cancel: tokio_util::sync::CancellationToken) -> Self {
        Self {
            inner: Default::default(),
            cancel,
        }
    }

    /// Queue a message for the next model request, or say why not. Checked
    /// under the same lock as the end of the run, so an accepted message is
    /// always read (the run continues for it) and a refused one never is.
    pub(crate) fn try_inject(&self, message: String) -> std::result::Result<(), InjectRefusal> {
        if message.trim().is_empty() {
            return Err(InjectRefusal::Empty);
        }
        if message.len() > MAX_INJECTED_BYTES {
            return Err(InjectRefusal::TooLarge);
        }
        let mut inner = self.inner.lock();
        if inner.finished || self.cancel.is_cancelled() {
            return Err(InjectRefusal::Finished);
        }
        if inner.messages.len() >= MAX_PENDING_MESSAGES {
            return Err(InjectRefusal::QueueFull);
        }
        inner.messages.push(message);
        Ok(())
    }

    /// Messages accepted and not read yet.
    pub(crate) fn pending_messages(&self) -> usize {
        self.inner.lock().messages.len()
    }

    pub(crate) fn drain_messages(&self) -> Vec<String> {
        std::mem::take(&mut self.inner.lock().messages)
    }

    pub(crate) fn finish_if_empty(&self) -> bool {
        let mut inner = self.inner.lock();
        if !inner.messages.is_empty() {
            return false;
        }
        inner.finished = true;
        true
    }

    pub(crate) fn finish(&self) {
        let mut inner = self.inner.lock();
        inner.finished = true;
        inner.pending.clear();
    }
}

/// A run-local binding, also inherited by sub-agents through ToolContext.
pub(crate) struct StreamPermissions {
    controls: std::sync::Arc<StreamControls>,
    tx: mpsc::Sender<AgentEvent>,
    emit: std::sync::Arc<dyn Fn(AgentEvent) + Send + Sync>,
}

impl StreamPermissions {
    pub(crate) fn new(
        controls: std::sync::Arc<StreamControls>,
        tx: mpsc::Sender<AgentEvent>,
        emit: std::sync::Arc<dyn Fn(AgentEvent) + Send + Sync>,
    ) -> Self {
        Self { controls, tx, emit }
    }
}

// Remove abandoned requests on cancellation or when a tool future is dropped.
struct PendingPermission<'a>(&'a StreamControls, String);
impl Drop for PendingPermission<'_> {
    fn drop(&mut self) {
        self.0.inner.lock().pending.remove(&self.1);
    }
}

#[async_trait::async_trait]
impl bricks_tools::permissions::PermissionPolicy for StreamPermissions {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        let denied = || PermissionDecision::Deny("Permission stream closed or cancelled".into());
        let mut request = request.clone();
        // Tool call ids may be reused by children or during a stale-preview retry.
        request.id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut inner = self.controls.inner.lock();
            if inner.finished || self.controls.cancel.is_cancelled() {
                return denied();
            }
            inner.pending.insert(request.id.clone(), tx);
        }
        let _pending = PendingPermission(&self.controls, request.id.clone());
        let event = AgentEvent::PermissionRequired(request);
        tokio::select! {
            biased;
            _ = self.controls.cancel.cancelled() => return denied(),
            result = self.tx.send(event.clone()) => {
                if result.is_err() { return denied(); }
            }
        }
        (self.emit)(event);
        tokio::select! {
            biased;
            _ = self.controls.cancel.cancelled() => denied(),
            result = rx => result.unwrap_or_else(|_| denied()),
        }
    }
}
