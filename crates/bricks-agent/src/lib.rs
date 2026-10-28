//! bricks-agent: The high-level Agent API with builder pattern, agentic loop,
//! realtime event streaming, broadcast channels, and reporters.

pub mod agents;
pub mod bricks_config;
pub mod compact;
pub mod context;
pub mod control;
pub mod events;
pub mod permission;
pub mod reporters;
mod request_env;
pub(crate) mod runner;
pub mod skills;
pub mod subagent;
pub mod system_prompt;

// Re-export runner utilities
pub use bricks_config::BricksConfig;
pub use compact::CompactionOutcome;
pub use context::{ContextPolicy, ContextStatus, Provenance};
pub use runner::{apply_tool_result_budget, apply_tool_result_budget_with};

use bricks_hooks::Hook;
use bricks_mcp::McpServerConfig;
use bricks_memory::Memory;
use bricks_provider::Provider;
use bricks_tools::permissions::PermissionPolicy;
use bricks_tools::{CostTracker, Tool};
use bricks_types::*;
use events::AgentEvent;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

// Re-exports
pub use events::{AgentStream, CompactReason, WarningState};
pub use reporters::Reporter;

// ─── User input ──────────────────────────────────────────────────────────────

/// What the user sends for one run: text, then attachments already
/// converted to content blocks (images, documents, captured file contents).
/// The provider checks that the model accepts them before sending.
#[derive(Debug, Clone, Default)]
pub struct UserInput {
    pub text: String,
    pub attachments: Vec<ContentBlock>,
    /// Skills to activate before the text, in order (a person's choice:
    /// `/skill`, `--skill`). Each is checked and placed in the context
    /// before the request; if one cannot be, the run fails before any
    /// request is sent.
    pub skills: Vec<String>,
}

impl UserInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

// ─── Agent output ────────────────────────────────────────────────────────────

/// Why a run that returned stopped. A cancelled or failed run returns an
/// error instead; every variant here except [`Termination::Completed`] is
/// an incomplete run, whose history and partial results are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Termination {
    /// The model gave its final answer with no tool call left to run.
    Completed,
    /// The answer was cut by the output-token limit, and the `continuations`
    /// allowed after a cut ran out.
    OutputTruncated { continuations: u32 },
    /// The same tool calls kept returning the same results: stopped
    /// instead of repeating them.
    NoProgress { repeats: u32 },
    /// The provider stopped the answer (content filter or refusal).
    ContentFiltered,
    /// The model returned neither text nor a tool call.
    EmptyResponse,
    /// A stored end whose `kind` this version does not know, written by an
    /// earlier version and produced by no run any more. It is read as an
    /// interrupted, incomplete end, never as a success, and it keeps the
    /// `kind` it was stored with.
    Historical { kind: String },
}

/// The stored form of a [`Termination`]: an object whose `kind` names the
/// variant. A `kind` this version does not know is read as
/// [`Termination::Historical`].
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredTermination {
    Completed,
    OutputTruncated {
        continuations: u32,
    },
    NoProgress {
        repeats: u32,
    },
    ContentFiltered,
    EmptyResponse,
    /// Any other kind.
    #[serde(other)]
    Other,
}

/// The stored form of [`Termination::Historical`]: its kind only.
#[derive(serde::Serialize)]
struct StoredHistorical<'a> {
    kind: &'a str,
}

impl serde::Serialize for Termination {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let stored = match self {
            Termination::Completed => StoredTermination::Completed,
            Termination::OutputTruncated { continuations } => StoredTermination::OutputTruncated {
                continuations: *continuations,
            },
            Termination::NoProgress { repeats } => {
                StoredTermination::NoProgress { repeats: *repeats }
            }
            Termination::ContentFiltered => StoredTermination::ContentFiltered,
            Termination::EmptyResponse => StoredTermination::EmptyResponse,
            Termination::Historical { kind } => {
                return StoredHistorical { kind }.serialize(serializer);
            }
        };
        stored.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Termination {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(deserializer)?;
        let stored = StoredTermination::deserialize(&value).map_err(D::Error::custom)?;
        Ok(match stored {
            StoredTermination::Completed => Termination::Completed,
            StoredTermination::OutputTruncated { continuations } => {
                Termination::OutputTruncated { continuations }
            }
            StoredTermination::NoProgress { repeats } => Termination::NoProgress { repeats },
            StoredTermination::ContentFiltered => Termination::ContentFiltered,
            StoredTermination::EmptyResponse => Termination::EmptyResponse,
            StoredTermination::Other => Termination::Historical {
                kind: value
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
        })
    }
}

impl Termination {
    /// The task ended with a final answer.
    pub fn is_completed(&self) -> bool {
        matches!(self, Termination::Completed)
    }

    /// One line for people and logs.
    pub fn describe(&self) -> String {
        match self {
            Termination::Completed => "completed".into(),
            Termination::OutputTruncated { continuations } => format!(
                "the answer was cut by the output-token limit ({continuations} continuation(s) allowed)"
            ),
            Termination::NoProgress { repeats } => {
                format!("stopped: the same tool calls returned the same results {repeats} times")
            }
            Termination::ContentFiltered => {
                "the provider stopped the answer (content filter)".into()
            }
            Termination::EmptyResponse => "the model returned an empty response".into(),
            Termination::Historical { kind } => {
                format!("interrupted: stored by an earlier version as `{kind}`, not a final answer")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentOutput {
    pub message: Message,
    pub usage: Usage,
    /// The provider's reason for its last response. How the run ended is
    /// [`AgentOutput::termination`].
    pub stop_reason: StopReason,
    /// Generation turns actually run.
    pub turns: u32,
    pub tool_calls: Vec<ToolCallRecord>,
    pub termination: Termination,
}

impl AgentOutput {
    pub fn text(&self) -> &str {
        self.message.get_text().unwrap_or("")
    }

    /// The run ended with a final answer (not at a limit).
    pub fn is_complete(&self) -> bool {
        self.termination.is_completed()
    }
}

#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub name: String,
    pub id: String,
    pub input: serde_json::Value,
    pub result: String,
    pub is_error: bool,
    pub duration: Duration,
}

/// The active history as it was just before a compaction replaced it.
#[derive(Debug, Clone)]
pub struct CompactionSnapshot {
    /// 1-based number of the compaction in this session.
    pub number: usize,
    pub messages: Vec<Message>,
    /// Memory key the snapshot was stored under, when a memory is configured.
    pub memory_key: Option<String>,
}

/// Callback receiving every event.
type EventHandler = Arc<dyn Fn(&AgentEvent) + Send + Sync>;
/// Predicate deciding which events are emitted.
type EventFilter = Arc<dyn Fn(&AgentEvent) -> bool + Send + Sync>;

// ─── Agent ───────────────────────────────────────────────────────────────────

/// The policy cell and the approver an agent's engine was built from.
type PermissionParts = (permission::PolicyCell, Arc<dyn PermissionPolicy>);

pub struct Agent {
    /// Provider, model label and reasoning profile, changed together by
    /// [`Agent::set_model`] / [`Agent::set_reasoning`]. A run takes one
    /// snapshot per request: a request never combines an old provider with
    /// a new profile.
    selection: parking_lot::RwLock<Selection>,
    tools: Vec<Box<dyn Tool>>,
    system_prompt: Option<String>,
    /// The Bricks prompt, assembled for each request from its announced
    /// tools and its context (wins over `system_prompt`).
    prompt_options: Option<system_prompt::SystemPromptOptions>,
    /// Environment facts of the requests (cached, git read when stale).
    request_env: request_env::RequestEnv,
    /// The system of the last request built (what was measured and sent).
    last_system: parking_lot::Mutex<Option<String>>,
    append_system_prompt: Option<String>,
    max_tokens: u32,
    temperature: Option<f32>,
    working_dir: PathBuf,
    /// Decides every tool call (the session's policy, then the approver).
    permission_policy: Arc<dyn PermissionPolicy>,
    /// The policy and the approver it was built from (absent when a whole
    /// engine was given, as for a sub-agent).
    permission_parts: Option<PermissionParts>,
    memory: Option<Arc<dyn Memory>>,
    /// Long-term memory recalled into the prompt and fed with each exchange.
    /// Read once at the start of each run: [`Agent::set_long_term_memory`]
    /// applies from the next run.
    pub(crate) long_term_memory:
        parking_lot::RwLock<Option<Arc<dyn bricks_memory::LongTermMemory>>>,
    /// Token budget of the recalled block (also capped at a tenth of the
    /// prompt budget).
    pub(crate) memory_recall_tokens: usize,
    /// The block recalled for the current run, appended to the system prompt.
    pub(crate) recalled: parking_lot::Mutex<Option<String>>,
    session_id: Option<String>,
    hooks: Vec<Arc<dyn Hook>>,
    /// MCP servers to connect at the first run.
    mcp_servers: Vec<McpServerConfig>,
    /// Their connections and tools, once connected.
    mcp: tokio::sync::OnceCell<McpState>,
    event_handler: Option<EventHandler>,
    broadcast_tx: Option<broadcast::Sender<AgentEvent>>,
    reporters: Vec<Arc<dyn Reporter>>,
    event_filter: Option<EventFilter>,
    /// Id of this agent's shell session (persistent bash/PowerShell and
    /// background tasks): the session id when one is set, else one per agent.
    pub(crate) shell_session_id: String,
    /// The longest `Retry-After` waited for before a retry (`[agent]
    /// retry_after_max_wait_ms`): a longer one ends the retries.
    pub(crate) retry_after_max_wait: std::time::Duration,
    cost_tracker: Arc<CostTracker>,
    auto_compact: bool,
    /// Set by the builder; `None`: scaled to the model.
    tool_result_budget: Option<usize>,
    /// Occupation, measurements and session totals of the context.
    pub(crate) context: Arc<parking_lot::Mutex<context::ContextManager>>,
    /// Reduces tool outputs and saves their originals.
    pub(crate) compressor: Arc<bricks_compression::Compressor>,
    /// Every message as it happened, tool results unreduced; never compacted.
    pub(crate) raw_history: Arc<parking_lot::Mutex<Vec<Message>>>,
    /// The active history before each compaction.
    pub(crate) snapshots: Arc<parking_lot::Mutex<Vec<CompactionSnapshot>>>,
    /// Saved originals of tool results, by tool-call id.
    pub(crate) raw_refs:
        Arc<parking_lot::Mutex<std::collections::HashMap<String, bricks_compression::RawRef>>>,
    pub(crate) compaction_state: Arc<parking_lot::Mutex<runner::CompactionState>>,
    /// Configuration diagnostics, reported once at the first run.
    pub(crate) config_notes: parking_lot::Mutex<Vec<String>>,
    /// Messages of the stored history loaded when the session opened
    /// ([`Agent::restore_history`]), announced by the first run.
    pub(crate) restored_unannounced: parking_lot::Mutex<Option<usize>>,
    /// Cadence (in turns) at which `HookEvent::TurnsElapsed` fires. Default
    /// 10. Setting to 0 disables the event entirely. Used by the
    /// `SkillNudgeHook` for agent-curated skill review.
    pub(crate) turns_elapsed_cadence: u32,
    pub(crate) compression_level: Arc<parking_lot::Mutex<bricks_compression::CompressionLevel>>,
    pub benchmark_mode: bool,
    messages: Arc<parking_lot::Mutex<Vec<Message>>>,
    cumulative_usage: Arc<parking_lot::Mutex<Usage>>,
    /// Cancels every run of this agent (given to the builder, or its own).
    cancel_token: tokio_util::sync::CancellationToken,
    /// Cancels the current run only: a child of `cancel_token`, replaced at
    /// the start of each run, so a cancelled run does not cancel the next.
    run_cancel: parking_lot::Mutex<tokio_util::sync::CancellationToken>,
    /// Orders this agent's session saves (see `Memory::store_histories`),
    /// under an owner id of its own.
    pub(crate) save_revision: std::sync::atomic::AtomicU64,
    pub(crate) save_owner: u64,
    /// The token of the manual compaction in progress (or the last one):
    /// a child of `cancel_token`, new for each [`Agent::compact`].
    compact_cancel: parking_lot::Mutex<tokio_util::sync::CancellationToken>,
    /// Type-map injected into every `ToolContext` this agent builds. Used by
    /// orchestration layers (e.g. bricks-agentrl) to hand tools a dynamic tool
    /// registry, a sandbox handle, a Mailbox/KvStore, etc. at runtime.
    pub(crate) extensions: bricks_tools::Extensions,
    /// The skills service and this agent's activations (`None`: no skills).
    pub(crate) skills: Option<bricks_tools::skill_tool::SkillSession>,
    /// The withdrawn activations and snapshots the summaries of the active
    /// history were last checked against (a fingerprint).
    pub(crate) skills_checked: parking_lot::Mutex<Option<String>>,
}

impl Agent {
    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    /// Run a prompt through the agentic loop. With a long-term memory, the
    /// memory's maintenance (extraction, embeddings) runs after the answer,
    /// before this returns; [`AgentEvent::MemoryMaintenance`] reports it.
    pub async fn run(&self, prompt: &str) -> bricks_types::Result<AgentOutput> {
        runner::run_agent(self, &UserInput::text(prompt)).await
    }

    /// [`Agent::run`] with attachments (images, documents, captured file
    /// contents) after the text.
    pub async fn run_input(&self, input: &UserInput) -> bricks_types::Result<AgentOutput> {
        runner::run_agent(self, input).await
    }

    /// Run with streaming — returns a stream of AgentEvents.
    /// Takes `Arc<Self>` so the agent can safely outlive the caller in the spawned task.
    pub fn run_stream(self: &Arc<Self>, prompt: &str) -> AgentStream {
        self.run_stream_input(UserInput::text(prompt))
    }

    /// [`Agent::run_stream`] with attachments. The stream ends with
    /// `Complete` or `Error`, then the memory maintenance events, if any.
    pub fn run_stream_input(self: &Arc<Self>, input: UserInput) -> AgentStream {
        let (event_tx, event_rx) = mpsc::channel(512);
        let agent = Arc::clone(self);
        // Make cancellation available before the worker is first polled.
        let cancel = self.begin_run();
        let controls = Arc::new(events::StreamControls::new(cancel.clone()));
        let permissions: Arc<dyn PermissionPolicy> = if self.permission_policy.requires_stream() {
            let stream: Arc<dyn PermissionPolicy> = Arc::new(events::StreamPermissions::new(
                Arc::clone(&controls),
                event_tx.clone(),
                Arc::new(self.emit_handle()),
            ));
            // The stream answers what the policy asks; the policy still
            // decides.
            match &self.permission_parts {
                Some((cell, _)) => {
                    Arc::new(permission::PermissionEngine::new(cell.clone(), stream))
                }
                None => stream,
            }
        } else {
            Arc::clone(&self.permission_policy)
        };
        let run_controls = Arc::clone(&controls);

        tokio::spawn(async move {
            let result = runner::run_prepared_controlled(
                &agent,
                &input,
                event_tx.clone(),
                cancel.clone(),
                Some(run_controls.clone()),
                permissions,
            )
            .await;
            run_controls.finish();
            let ok = result.is_ok();
            match result {
                Ok(output) => {
                    agent.emit(AgentEvent::Complete(output.clone()));
                    let _ = event_tx.send(AgentEvent::Complete(output)).await;
                }
                Err(e) => {
                    agent.emit(AgentEvent::Error(e.to_string()));
                    let _ = event_tx.send(AgentEvent::Error(e.to_string())).await;
                }
            }
            if ok {
                runner::maintain_memory(&agent, &cancel, Some(&event_tx)).await;
            }
        });

        AgentStream::new(event_rx, controls)
    }

    /// Multi-turn: send a follow-up message in the same conversation.
    pub async fn reply(&self, message: &str) -> bricks_types::Result<AgentOutput> {
        runner::run_agent(self, &UserInput::text(message)).await
    }

    /// Access the conversation history.
    pub fn messages(&self) -> Vec<Message> {
        self.messages.lock().clone()
    }

    /// Get cumulative usage/cost.
    pub fn usage(&self) -> Usage {
        self.cumulative_usage.lock().clone()
    }

    /// Occupation of the active context (with its provenance), the model's
    /// window and the session totals, for the next request as it would be
    /// built now.
    pub fn context_status(&self) -> ContextStatus {
        runner::context_status(self)
    }

    /// Every message of the session as it happened, tool results unreduced.
    /// Compaction never shortens it.
    pub fn raw_history(&self) -> Vec<Message> {
        self.raw_history.lock().clone()
    }

    /// The active history before each compaction, oldest first.
    pub fn compaction_snapshots(&self) -> Vec<CompactionSnapshot> {
        self.snapshots.lock().clone()
    }

    /// Compact the history now (outside of the automatic triggers). It has
    /// its own operation token, new for each call: [`Agent::cancel`] stops
    /// it, and a compaction after a cancelled one starts normally.
    pub async fn compact(&self) -> CompactionOutcome {
        let token = self.cancel_token.child_token();
        *self.compact_cancel.lock() = token.clone();
        runner::compact_now(self, &token).await
    }

    /// Cancel the current run (and the memory maintenance that follows
    /// it), and a manual compaction in progress. The next run or
    /// compaction starts normally.
    pub fn cancel(&self) {
        self.run_cancel.lock().cancel();
        self.compact_cancel.lock().cancel();
    }

    /// A fresh cancellation token for a new run.
    pub(crate) fn begin_run(&self) -> tokio_util::sync::CancellationToken {
        let token = self.cancel_token.child_token();
        *self.run_cancel.lock() = token.clone();
        token
    }

    /// The token of the current (or last) run.
    pub fn run_cancellation(&self) -> tokio_util::sync::CancellationToken {
        self.run_cancel.lock().clone()
    }

    /// The model the requests go to (its `provider_id/model_id` when the
    /// provider knows it).
    pub fn model_label(&self) -> String {
        runner::model_label(self)
    }

    /// The reasoning profile requested for this agent (`None`: the model's
    /// default, as the provider configuration declares it).
    pub fn reasoning_profile(&self) -> Option<String> {
        self.selection.read().reasoning.clone()
    }

    /// Switch model and reasoning profile, together. A run in progress uses
    /// them from its next request; the request being streamed (and its
    /// retries) keeps the selection it was sent with. Measurements made with
    /// the previous selection no longer count as measured.
    pub fn set_model(
        &self,
        provider: Box<dyn Provider>,
        label: Option<String>,
        reasoning_profile: Option<String>,
    ) {
        {
            let mut sel = self.selection.write();
            *sel = Selection {
                provider: Arc::from(provider),
                label,
                reasoning: reasoning_profile,
                generation: sel.generation + 1,
            };
        }
        self.context
            .lock()
            .invalidate("the model or reasoning profile changed");
    }

    /// Change only the reasoning profile (`None`: back to the model's
    /// default). Same rules as [`Agent::set_model`].
    pub fn set_reasoning(&self, reasoning_profile: Option<String>) {
        {
            let mut sel = self.selection.write();
            sel.reasoning = reasoning_profile;
            sel.generation += 1;
        }
        self.context
            .lock()
            .invalidate("the reasoning profile changed");
    }

    /// The current provider.
    pub(crate) fn provider(&self) -> Arc<dyn Provider> {
        self.selection.read().provider.clone()
    }

    /// One consistent snapshot of the selection (for one request).
    pub(crate) fn selection(&self) -> Selection {
        self.selection.read().clone()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn working_dir(&self) -> &std::path::Path {
        &self.working_dir
    }

    /// Store the session now (active history and raw history), when a
    /// session memory and id are configured. Runs store it when they end;
    /// this is for interrupted runs and explicit saves.
    pub async fn save_session(&self) -> bricks_types::Result<bool> {
        runner::save_session(self).await
    }

    /// Empty the active context. The raw history keeps every message, and
    /// the cleared history is kept as a snapshot like a compaction's.
    pub async fn clear_context(&self) -> bricks_types::Result<usize> {
        runner::clear_context(self).await
    }

    /// The position, in the active history, of the user message a
    /// `revise` replaces: the `user_index`-th user-role message, whose text
    /// is `original` (compared trimmed). An error says why it cannot be
    /// revised; nothing changes.
    pub fn revision_point(
        &self,
        user_index: usize,
        original: &str,
    ) -> std::result::Result<usize, String> {
        let messages = self.messages.lock();
        let Some((position, message)) = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == Role::User)
            .nth(user_index)
        else {
            return Err(
                "this message is no longer in the active context (it was compacted or cleared)"
                    .into(),
            );
        };
        let is_tool_result = matches!(&message.content, MessageContent::Blocks(blocks)
            if blocks.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. })));
        let original = original.trim();
        let matches = message.get_all_text().trim() == original
            || message.get_text().is_some_and(|t| t.trim() == original);
        if is_tool_result || !matches {
            return Err("this message changed since it was shown: reload the conversation".into());
        }
        Ok(position)
    }

    /// Cut the active history before `position` (see `revision_point`),
    /// keeping it as a snapshot. Returns the messages removed.
    pub async fn revise_context(&self, position: usize) -> bricks_types::Result<usize> {
        runner::revise_context(self, position).await
    }

    /// Load the session's stored history now, before any run, so the
    /// context measured until then is the whole history's (an estimate until
    /// the next server measurement). The first run announces it and checks
    /// the session's skills, as when it loads the history itself. Returns
    /// the number of messages loaded.
    pub async fn restore_history(&self) -> bricks_types::Result<Option<usize>> {
        let loaded = runner::load_session_history(self).await?;
        if loaded.is_some() {
            *self.restored_unannounced.lock() = loaded;
        }
        Ok(loaded)
    }

    /// Whether a long-term memory is attached.
    pub fn has_long_term_memory(&self) -> bool {
        self.long_term_memory.read().is_some()
    }

    /// The long-term memory the next run starts with.
    pub fn long_term_memory(&self) -> Option<Arc<dyn bricks_memory::LongTermMemory>> {
        self.long_term_memory.read().clone()
    }

    /// Replace the long-term memory (`None`: off), without rebuilding the
    /// agent. A run reads it once, at its start: the run in progress keeps
    /// its own, recall, tools, capture and maintenance included.
    pub fn set_long_term_memory(&self, memory: Option<Arc<dyn bricks_memory::LongTermMemory>>) {
        *self.long_term_memory.write() = memory;
    }

    /// The session's permission policy, when this agent was built with one
    /// (not a sub-agent given a whole engine).
    pub(crate) fn policy_cell(&self) -> Option<&permission::PolicyCell> {
        self.permission_parts.as_ref().map(|(cell, _)| cell)
    }

    /// Run the long-term memory's maintenance now (what earlier runs
    /// recorded and did not process). `None` without a long-term memory.
    pub async fn maintain_memory(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Option<bricks_types::Result<bricks_memory::MaintenanceReport>> {
        let ltm = self.long_term_memory()?;
        Some(ltm.maintain(cancel).await)
    }

    /// Get the current tool-output compression level.
    pub fn compression_level(&self) -> bricks_compression::CompressionLevel {
        *self.compression_level.lock()
    }

    /// Change the tool-output compression level at runtime. Takes effect on
    /// the next tool call.
    pub fn set_compression_level(&self, level: bricks_compression::CompressionLevel) {
        *self.compression_level.lock() = level;
    }

    /// Subscribe to the broadcast channel (requires enable_broadcast on builder).
    pub fn subscribe(&self) -> Option<broadcast::Receiver<AgentEvent>> {
        self.broadcast_tx.as_ref().map(|tx| tx.subscribe())
    }

    /// Emit an event to all listeners.
    pub(crate) fn emit(&self, event: AgentEvent) {
        (self.emit_handle())(event)
    }

    /// An owned emitter, for callbacks that outlive a borrow of the agent
    /// (tool progress).
    pub(crate) fn emit_handle(&self) -> impl Fn(AgentEvent) + Send + Sync + 'static + use<> {
        let filter = self.event_filter.clone();
        let handler = self.event_handler.clone();
        let broadcast = self.broadcast_tx.clone();
        let reporters = self.reporters.clone();
        move |event: AgentEvent| {
            if let Some(filter) = &filter
                && !filter(&event)
            {
                return;
            }
            if let Some(handler) = &handler {
                handler(&event);
            }
            if let Some(tx) = &broadcast {
                let _ = tx.send(event.clone());
            }
            for reporter in &reporters {
                let reporter = Arc::clone(reporter);
                let event = event.clone();
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move {
                        reporter.on_event(&event).await;
                    });
                }
            }
        }
    }

    /// Stop this agent's shells and background tasks: graceful, bounded,
    /// awaited. Dropping the agent does the same synchronously (forced).
    pub async fn close(&self) {
        bricks_tools::shell::close_session(&self.shell_session_id).await;
        if let Some(m) = self
            .mcp
            .get()
            .filter(|s| !s.shared)
            .and_then(|s| s.manager.clone())
        {
            m.close().await;
        }
    }

    /// Use another agent's MCP connections (a sub-agent shares its
    /// parent's): their tools are offered, they are not closed with this
    /// agent. No effect once connections exist.
    pub async fn adopt_mcp(&self, manager: Arc<bricks_mcp::McpManager>) {
        let _ = self
            .mcp
            .get_or_init(|| async {
                let tools = bricks_tools::mcp_tool::tools_of(&manager).await;
                McpState {
                    manager: Some(manager),
                    tools,
                    shared: true,
                }
            })
            .await;
    }

    /// The MCP connections, once connected (after the first run starts).
    pub fn mcp_manager(&self) -> Option<Arc<bricks_mcp::McpManager>> {
        self.mcp.get().and_then(|s| s.manager.clone())
    }

    /// Connect the configured MCP servers once; returns what failed.
    pub(crate) async fn connect_mcp(&self) -> Vec<String> {
        if self.mcp_servers.is_empty() || self.mcp.initialized() {
            return Vec::new();
        }
        let mut notes = Vec::new();
        let state = self
            .mcp
            .get_or_init(|| async {
                let manager = Arc::new(bricks_mcp::McpManager::connect(&self.mcp_servers).await);
                let tools = bricks_tools::mcp_tool::tools_of(&manager).await;
                McpState {
                    manager: Some(manager),
                    tools,
                    shared: false,
                }
            })
            .await;
        if let Some(m) = &state.manager {
            for (name, status) in m.statuses().await {
                if let bricks_mcp::McpServerStatus::Error(e) = status {
                    notes.push(format!("MCP server '{name}' unavailable: {e}"));
                }
            }
        }
        notes
    }

    /// The system prompt of the last request built ([`Self::request_system`]),
    /// else the prompt without a request context: what estimates and
    /// compaction measure.
    pub(crate) fn effective_system(&self) -> Option<String> {
        if let Some(s) = self.last_system.lock().clone() {
            return Some(s);
        }
        let names: Vec<String> = self
            .tool_list()
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        let base = match &self.prompt_options {
            Some(o) => {
                let mut o = o.clone();
                o.tools_available = names;
                Some(system_prompt::build_system_prompt(&o))
            }
            None => self.system_prompt.clone(),
        };
        self.join_system(base, None)
    }

    /// The configured or assembled prompt, the catalog of skills (names and
    /// descriptions only), the plan block (raw prompts only), the recalled
    /// long-term memory block, then the appended instructions.
    fn join_system(&self, base: Option<String>, plan: Option<String>) -> Option<String> {
        let parts: Vec<String> = [
            // A preloaded skill withdrawn since leaves the prompt.
            base.map(|b| self.without_withdrawn_skills(b)),
            self.skills_catalog(),
            plan,
            self.recalled.lock().clone(),
            self.append_system_prompt.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    /// The system prompt of one request announcing `tools`, sent with
    /// `selection` and `options`: the conditional guidance follows those
    /// tools, the dynamic part describes this request (date, environment,
    /// git, model, plan). Kept as the last system built.
    pub(crate) async fn request_system(
        &self,
        tools: &[String],
        selection: &Selection,
        options: &bricks_provider::ProviderOptions,
    ) -> Option<String> {
        let has = |n: &str| tools.iter().any(|t| t == n);
        let plan = self
            .plan()
            .filter(|_| has("TodoWrite"))
            .map(|p| p.current())
            .filter(|p| p.revision > 0);
        let (base, raw_plan) = match &self.prompt_options {
            Some(o) => {
                let mut o = o.clone();
                o.tools_available = tools.to_vec();
                o.ask_timer = self
                    .extensions
                    .get::<crate::control::AskTimer>()
                    .map(|t| *t)
                    .unwrap_or_default();
                let replace = o.replace_system_prompt && o.custom_system_prompt.is_some();
                if !replace {
                    o.git_status = self.request_env.git(&self.working_dir).await;
                    let shell = has("Bash").then(|| {
                        let config = self
                            .extensions
                            .get::<bricks_tools::shell::ShellConfig>()
                            .map(|c| (*c).clone())
                            .unwrap_or_default();
                        bricks_tools::shell::tool_shell(&config, &self.extensions)
                    });
                    let identity = selection.provider.request_identity(options);
                    let model = system_prompt::ModelIdentity {
                        selection: Some(selection.model_label()),
                        api_model: identity.as_ref().map(|i| i.api_model.clone()),
                        reasoning: identity
                            .and_then(|i| i.reasoning_profile)
                            .or_else(|| selection.reasoning.clone()),
                    };
                    o.request = Some(request_env::context(
                        &self.request_env,
                        &self.working_dir,
                        shell,
                        has("Bash")
                            .then(|| bricks_tools::shell::session_cwd(&self.shell_session_id))
                            .flatten(),
                        model,
                        plan,
                    ));
                }
                (Some(system_prompt::build_system_prompt(&o)), None)
            }
            None => (
                self.system_prompt.clone(),
                plan.as_ref()
                    .map(|p| system_prompt::render_plan_block(p).trim_start().to_string()),
            ),
        };
        let system = self.join_system(base, raw_plan);
        *self.last_system.lock() = system.clone();
        system
    }

    /// The workspace may have changed: the git state of the next request
    /// is read again.
    pub(crate) fn workspace_may_have_changed(&self) {
        self.request_env.mark_git_stale();
    }

    /// A user request starts: its first request reads the git state again.
    pub(crate) fn request_started(&self) {
        self.request_env.mark_git_stale();
    }

    /// The plan TodoWrite writes to (`None`: this agent has no TodoWrite and
    /// was given no plan).
    pub fn plan(&self) -> Option<Arc<bricks_tools::todo_write::PlanService>> {
        self.extensions
            .get::<bricks_tools::todo_write::PlanHandle>()
            .map(|h| Arc::clone(&h.0))
    }

    /// Built-in tools, then MCP tools.
    pub fn tool_list(&self) -> Vec<&dyn Tool> {
        self.tools
            .iter()
            .map(|t| t.as_ref())
            .chain(
                self.mcp
                    .get()
                    .into_iter()
                    .flat_map(|s| s.tools.iter().map(|t| t.as_ref())),
            )
            .collect()
    }

    pub fn tool_by_name(&self, name: &str) -> Option<&dyn Tool> {
        self.tool_list().into_iter().find(|t| t.name() == name)
    }
}

/// Publishes the plan updates of an agent's own plan as its events.
struct PlanEvents(Box<dyn Fn(AgentEvent) + Send + Sync>);

impl bricks_tools::todo_write::PlanObserver for PlanEvents {
    fn plan_updated(&self, plan: &bricks_tools::todo_write::PlanState, root_run_id: Option<&str>) {
        (self.0)(AgentEvent::PlanUpdated {
            plan: plan.clone(),
            root_run_id: root_run_id.map(str::to_string),
        });
    }
}

/// The model a request goes to: provider, label, reasoning profile, and a
/// generation that changes with every switch.
#[derive(Clone)]
pub(crate) struct Selection {
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) label: Option<String>,
    pub(crate) reasoning: Option<String>,
    pub(crate) generation: u64,
}

impl Selection {
    /// The label of the model (its `provider_id/model_id` when known).
    pub(crate) fn model_label(&self) -> String {
        self.label
            .clone()
            .or_else(|| self.provider.model_info().map(|m| m.selection))
            .unwrap_or_else(|| self.provider.name().to_string())
    }
}

/// Connected MCP servers and their tools.
pub(crate) struct McpState {
    manager: Option<Arc<bricks_mcp::McpManager>>,
    tools: Vec<Box<dyn Tool>>,
    /// Connections owned by another agent (a sub-agent uses its parent's):
    /// not closed with this one.
    shared: bool,
}

impl Drop for Agent {
    fn drop(&mut self) {
        bricks_tools::shell::close_session_now(&self.shell_session_id);
    }
}

// ─── Agent builder ───────────────────────────────────────────────────────────

pub struct AgentBuilder {
    provider: Option<Box<dyn Provider>>,
    tools: Vec<Box<dyn Tool>>,
    system_prompt: Option<String>,
    prompt_options: Option<system_prompt::SystemPromptOptions>,
    append_system_prompt: Option<String>,
    model: Option<String>,
    max_tokens: u32,
    temperature: Option<f32>,
    reasoning_profile: Option<String>,
    seed_usage: Option<Usage>,
    working_dir: Option<PathBuf>,
    permission_approver: Option<Arc<dyn PermissionPolicy>>,
    permission_profile: Option<permission::SessionPolicy>,
    permission_cell: Option<permission::PolicyCell>,
    permission_engine: Option<Arc<dyn PermissionPolicy>>,
    memory: Option<Arc<dyn Memory>>,
    long_term_memory: Option<Arc<dyn bricks_memory::LongTermMemory>>,
    memory_recall_tokens: usize,
    session_id: Option<String>,
    hooks: Vec<Arc<dyn Hook>>,
    mcp_servers: Vec<McpServerConfig>,
    event_handler: Option<EventHandler>,
    broadcast_capacity: Option<usize>,
    reporters: Vec<Arc<dyn Reporter>>,
    event_filter: Option<EventFilter>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    auto_compact: bool,
    compact_threshold: Option<f64>,
    tool_result_budget: Option<usize>,
    turns_elapsed_cadence: u32,
    compression_level: Option<bricks_compression::CompressionLevel>,
    context_policy: Option<context::ContextPolicy>,
    compression_config: Option<bricks_compression::CompressionConfig>,
    compression_rules: Option<Arc<bricks_compression::RuleSet>>,
    raw_output_dir: Option<PathBuf>,
    bricks_config: Option<BricksConfig>,
    web_config: Option<bricks_web::WebConfig>,
    initial_messages: Option<Vec<Message>>,
    benchmark_mode: bool,
    skills: Option<bricks_skills::SkillAccess>,
    preloaded_skills: Vec<(String, bricks_skills::Activation)>,
    extensions: bricks_tools::Extensions,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            provider: None,
            tools: Vec::new(),
            system_prompt: None,
            prompt_options: None,
            append_system_prompt: None,
            model: None,
            max_tokens: 16384,
            temperature: None,
            reasoning_profile: None,
            seed_usage: None,
            working_dir: None,
            permission_approver: None,
            permission_profile: None,
            permission_cell: None,
            permission_engine: None,
            memory: None,
            long_term_memory: None,
            memory_recall_tokens: 1200,
            session_id: None,
            hooks: Vec::new(),
            mcp_servers: Vec::new(),
            event_handler: None,
            broadcast_capacity: None,
            reporters: Vec::new(),
            event_filter: None,
            cancel_token: None,
            auto_compact: true,
            compact_threshold: None,
            tool_result_budget: None,
            turns_elapsed_cadence: 10,
            compression_level: None,
            context_policy: None,
            compression_config: None,
            compression_rules: None,
            raw_output_dir: None,
            bricks_config: None,
            web_config: None,
            initial_messages: None,
            benchmark_mode: false,
            skills: None,
            preloaded_skills: Vec::new(),
            extensions: bricks_tools::Extensions::default(),
        }
    }
}

impl AgentBuilder {
    pub fn provider(mut self, p: impl Provider + 'static) -> Self {
        self.provider = Some(Box::new(p));
        self
    }

    /// Accept a pre-boxed provider. Useful when the caller already has a
    /// `Box<dyn Provider>` (e.g., the delegation primitive, which builds
    /// child providers via a factory closure).
    pub fn provider_boxed(mut self, p: Box<dyn Provider>) -> Self {
        self.provider = Some(p);
        self
    }

    pub fn tool(mut self, t: impl Tool + 'static) -> Self {
        self.tools.push(Box::new(t));
        self
    }

    pub fn tools(mut self, ts: Vec<Box<dyn Tool>>) -> Self {
        self.tools.extend(ts);
        self
    }

    pub fn system_prompt(mut self, s: impl Into<String>) -> Self {
        self.system_prompt = Some(s.into());
        self
    }

    /// The Bricks prompt, assembled for each request: its conditional
    /// guidance follows the tools the request announces, and its dynamic
    /// part describes that request (date, environment, git, model, plan).
    /// `tools_available` and `request` are set per request. Wins over
    /// [`Self::system_prompt`]. Replace mode
    /// (`replace_system_prompt` with a custom prompt) stays the custom text
    /// alone.
    pub fn system_prompt_options(mut self, opts: system_prompt::SystemPromptOptions) -> Self {
        self.prompt_options = Some(opts);
        self
    }

    pub fn append_system_prompt(mut self, s: impl Into<String>) -> Self {
        self.append_system_prompt = Some(s.into());
        self
    }

    pub fn model(mut self, m: impl Into<String>) -> Self {
        self.model = Some(m.into());
        self
    }

    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    /// Select a reasoning profile by id for every request of this agent. The
    /// profiles a model offers are defined in the provider configuration; an id
    /// the model does not define is an error when the request is made.
    pub fn reasoning_profile(mut self, id: impl Into<String>) -> Self {
        self.reasoning_profile = Some(id.into());
        self
    }

    /// Seed the agent's cumulative token/cost usage. Use this when rebuilding an
    /// agent per turn so cumulative totals carry over instead of resetting to zero.
    pub fn with_cumulative_usage(mut self, usage: Usage) -> Self {
        self.seed_usage = Some(usage);
        self
    }

    pub fn working_dir(mut self, p: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(p.into());
        self
    }

    /// The approver: who answers the calls the policy says to ask (a person
    /// through a callback, a stream, a fixed answer). It never decides the
    /// others. Without a policy set, giving an approver means the ASK
    /// profile, so a call that used to be asked is still asked.
    pub fn permission_policy(mut self, p: impl PermissionPolicy + 'static) -> Self {
        self.permission_approver = Some(Arc::new(p));
        self
    }

    /// [`AgentBuilder::permission_policy`] with an approver already shared.
    pub fn permission_policy_arc(mut self, p: Arc<dyn PermissionPolicy>) -> Self {
        self.permission_approver = Some(p);
        self
    }

    /// The agent's permission policy (ASK, EXECUTE or CUSTOM). Default:
    /// EXECUTE without an approver, ASK with one.
    pub fn permission_profile(mut self, policy: permission::SessionPolicy) -> Self {
        self.permission_profile = Some(policy);
        self
    }

    /// A policy shared with other agents of the same session (a change
    /// reaches all of them at their next call).
    pub fn permission_cell(mut self, cell: permission::PolicyCell) -> Self {
        self.permission_cell = Some(cell);
        self
    }

    /// A whole decision path already built — a parent's engine, for a
    /// sub-agent (which never gets more than its parent). Wins over the
    /// approver, profile and cell.
    pub fn permission_engine_arc(mut self, p: Arc<dyn PermissionPolicy>) -> Self {
        self.permission_engine = Some(p);
        self
    }

    /// A long-term memory: what it recalls for the prompt is added to the
    /// system prompt of each run (within the recall budget), and each
    /// finished exchange is recorded into it.
    pub fn long_term_memory(mut self, m: Arc<dyn bricks_memory::LongTermMemory>) -> Self {
        self.long_term_memory = Some(m);
        self
    }

    /// Token budget of recalled memory per run (default 1200; never more
    /// than a tenth of the model's prompt budget).
    pub fn memory_recall_tokens(mut self, tokens: usize) -> Self {
        self.memory_recall_tokens = tokens;
        self
    }

    pub fn memory(mut self, m: impl Memory + 'static) -> Self {
        self.memory = Some(Arc::new(m));
        self
    }

    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn hook(mut self, h: impl Hook + 'static) -> Self {
        self.hooks.push(Arc::new(h));
        self
    }

    pub fn mcp_server(mut self, config: McpServerConfig) -> Self {
        self.mcp_servers.push(config);
        self
    }

    pub fn on_event(mut self, f: impl Fn(&AgentEvent) + Send + Sync + 'static) -> Self {
        self.event_handler = Some(Arc::new(f));
        self
    }

    pub fn enable_broadcast(mut self, capacity: usize) -> Self {
        self.broadcast_capacity = Some(capacity);
        self
    }

    pub fn reporter(mut self, r: impl Reporter + 'static) -> Self {
        self.reporters.push(Arc::new(r));
        self
    }

    pub fn event_filter(mut self, f: impl Fn(&AgentEvent) -> bool + Send + Sync + 'static) -> Self {
        self.event_filter = Some(Arc::new(f));
        self
    }

    pub fn cancel_token(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    pub fn auto_compact(mut self, enabled: bool) -> Self {
        self.auto_compact = enabled;
        self
    }

    /// Fraction of the prompt budget at which the history is compacted after
    /// a turn (overrides the context policy's `compact_threshold`).
    pub fn compact_threshold(mut self, threshold: f64) -> Self {
        self.compact_threshold = Some(threshold);
        self
    }

    /// Thresholds of the context manager (default: [`ContextPolicy::default`]).
    pub fn context_policy(mut self, policy: ContextPolicy) -> Self {
        self.context_policy = Some(policy);
        self
    }

    /// Limits of tool-output compression.
    pub fn compression_config(mut self, config: bricks_compression::CompressionConfig) -> Self {
        self.compression_config = Some(config);
        self
    }

    /// Output rules (default: the built-in rules only; see
    /// [`BricksConfig::load`] for user and project rules).
    pub fn compression_rules(mut self, rules: Arc<bricks_compression::RuleSet>) -> Self {
        self.compression_rules = Some(rules);
        self
    }

    /// Directory where the originals of reduced tool outputs are saved
    /// (default: a per-session directory under the system temp directory).
    pub fn raw_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.raw_output_dir = Some(dir.into());
        self
    }

    /// Apply a loaded `bricks.toml` / user rules configuration. Explicit
    /// builder calls take precedence over it.
    /// Web settings (search providers, fetch limits, passages), overriding
    /// `bricks.toml` `[web]`.
    pub fn web_config(mut self, config: bricks_web::WebConfig) -> Self {
        self.web_config = Some(config);
        self
    }

    pub fn bricks_config(mut self, config: BricksConfig) -> Self {
        self.bricks_config = Some(config);
        self
    }

    pub fn tool_result_budget(mut self, chars: usize) -> Self {
        self.tool_result_budget = Some(chars);
        self
    }

    /// Set the tool-output compression level (default `Off`). Compression is
    /// applied to each tool result before the per-result cap and the overall
    /// tool-result budget run.
    /// How often `HookEvent::TurnsElapsed` fires (default 10). Set to 0 to
    /// disable. Used by skill-nudge hooks for agent-curated skill review.
    pub fn turns_elapsed_cadence(mut self, n: u32) -> Self {
        self.turns_elapsed_cadence = n;
        self
    }

    pub fn compression_level(mut self, level: bricks_compression::CompressionLevel) -> Self {
        self.compression_level = Some(level);
        self
    }

    /// Pre-populate conversation history (for provider switching mid-session).
    pub fn with_messages(mut self, msgs: Vec<Message>) -> Self {
        self.initial_messages = Some(msgs);
        self
    }

    /// Enable benchmark mode (self-verification loop for terminal-bench).
    pub fn benchmark_mode(mut self, enabled: bool) -> Self {
        self.benchmark_mode = enabled;
        self
    }

    /// Give this agent the skills of a project: their catalog in its system
    /// prompt, the `Skill` tool (unless a tool of that name is already
    /// registered), and its own activations. `access` carries the session's
    /// trust; it is shared with sub-agents, the activations are not.
    pub fn skills(mut self, access: bricks_skills::SkillAccess) -> Self {
        self.skills = Some(access);
        self
    }

    /// Skills already validated by the caller (a sub-agent profile's), with
    /// the id their text carries, recorded as active from the start. Their
    /// text is the caller's to place (the profile puts it in the system
    /// prompt).
    pub fn preloaded_skills(mut self, skills: Vec<(String, bricks_skills::Activation)>) -> Self {
        self.preloaded_skills = skills;
        self
    }

    /// Inject a type-map that is cloned into every `ToolContext` this agent
    /// builds, letting tools retrieve runtime-injected handles (dynamic tool
    /// registry, sandbox, Mailbox/KvStore) via `ctx.extensions.get::<T>()`.
    pub fn extensions(mut self, ext: bricks_tools::Extensions) -> Self {
        self.extensions = ext;
        self
    }

    pub fn build(self) -> bricks_types::Result<Agent> {
        let provider = self
            .provider
            .ok_or_else(|| BricksError::Config("Provider is required".into()))?;

        let working_dir = self
            .working_dir
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        // One engine decides every call: a whole one given (a sub-agent's
        // parent), or the policy (shared cell, profile, or the default)
        // with its approver.
        let (permission_policy, permission_parts): (
            Arc<dyn PermissionPolicy>,
            Option<PermissionParts>,
        ) = match self.permission_engine {
            Some(engine) => (engine, None),
            None => {
                let has_approver = self.permission_approver.is_some();
                let approver = self
                    .permission_approver
                    .unwrap_or_else(|| Arc::new(bricks_tools::permissions::DenyAll));
                let cell = self.permission_cell.unwrap_or_else(|| {
                    let policy = self.permission_profile.unwrap_or_else(|| {
                        permission::SessionPolicy::chosen(if has_approver {
                            permission::PermissionProfile::Ask
                        } else {
                            permission::PermissionProfile::Execute
                        })
                    });
                    permission::PolicyCell::new(policy, 0)
                });
                (
                    Arc::new(permission::PermissionEngine::new(
                        cell.clone(),
                        Arc::clone(&approver),
                    )),
                    Some((cell, approver)),
                )
            }
        };

        let broadcast_tx = self.broadcast_capacity.map(|cap| {
            let (tx, _) = broadcast::channel(cap);
            tx
        });

        let bricks = self.bricks_config.unwrap_or_default();
        let retry_after_max_wait_ms = bricks.agent.retry_after_max_wait_ms;
        let mut policy = self.context_policy.unwrap_or(bricks.context);
        if let Some(t) = self.compact_threshold {
            policy.compact_threshold = t;
        }
        policy.validate().map_err(BricksError::Config)?;
        let raw_dir = self.raw_output_dir.or(bricks.raw_output_dir);
        let session_for_store = self
            .session_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // Originals live with the session when it is stored durably, so the
        // references in the history survive a restore and are deleted with
        // it; an explicit directory wins, a temporary one is the last resort.
        let session_dir = match (&self.memory, &self.session_id) {
            (Some(memory), Some(sid)) => memory.session_files_dir(sid),
            _ => None,
        };
        let store = match (raw_dir, session_dir.clone()) {
            (Some(dir), _) => bricks_compression::RawStore::new(dir.join(&session_for_store)),
            (None, Some(dir)) => bricks_compression::RawStore::new(dir),
            (None, None) => bricks_compression::RawStore::for_session(&session_for_store),
        };
        let compressor = bricks_compression::Compressor::new(
            self.compression_config.unwrap_or(bricks.compression),
            self.compression_rules.unwrap_or(bricks.rules),
            Some(store),
        );
        // One web context per session: documents are stored with the
        // session's saved originals, so they are restored and deleted with it.
        if self
            .extensions
            .get::<bricks_tools::web_runtime::WebRuntime>()
            .is_none()
        {
            let web_config = self.web_config.unwrap_or(bricks.web);
            let store = compressor.raw_store().map(|r| {
                Arc::new(bricks_web::store::WebStore::open(
                    r.dir().join("web"),
                    web_config.store.max_session_bytes,
                ))
            });
            let web =
                bricks_web::WebContext::new(web_config, store).map_err(BricksError::Config)?;
            self.extensions
                .insert(bricks_tools::web_runtime::WebRuntime(Arc::new(web)));
        }
        let compression_level = self
            .compression_level
            .or(bricks.compression_level)
            .unwrap_or_default();
        let initial = self.initial_messages.unwrap_or_default();

        let mut own_plan = false;
        // TodoWrite's plan: the given one, else this instance's own — kept
        // with the session's files when it has a folder, in memory otherwise.
        if self.tools.iter().any(|t| t.name() == "TodoWrite")
            && self
                .extensions
                .get::<bricks_tools::todo_write::PlanHandle>()
                .is_none()
        {
            let plan = match &session_dir {
                Some(dir) => bricks_tools::todo_write::PlanService::in_dir(dir),
                None => bricks_tools::todo_write::PlanService::in_memory(),
            };
            self.extensions
                .insert(bricks_tools::todo_write::PlanHandle(plan));
            own_plan = true;
        }

        // The workspace's shared code understanding engine, for CodeScout
        // (and the sub-agents of this workspace, through the registry).
        if self.tools.iter().any(|t| t.name() == "CodeScout")
            && self
                .extensions
                .get::<bricks_tools::code_scout::SemanticHandle>()
                .is_none()
        {
            let engine = bricks_semantic::SemanticRegistry::global()
                .engine_for(&working_dir, &bricks.semantic);
            self.extensions
                .insert(bricks_tools::code_scout::SemanticHandle(engine));
        }

        // Jobs: one registry for the agent. The session's (in the
        // extensions: a session, a sub-agent) takes every tool that works
        // with jobs; otherwise the tools' own one becomes the agent's.
        // Tools on different registries are refused here, before any call.
        let mut tools = assemble_jobs(self.tools, &self.extensions)?;

        // Skills: one session (service + trust, this agent's ledger) and the
        // `Skill` tool, registered once.
        let skills = self.skills.map(|access| {
            let session = bricks_tools::skill_tool::SkillSession::new(access);
            {
                let mut ledger = session.ledger.lock();
                for (id, a) in self.preloaded_skills {
                    ledger.commit(id, a, bricks_skills::Origin::Profile);
                }
            }
            self.extensions.insert(session.clone());
            if !tools.iter().any(|t| t.name() == "Skill") {
                tools.push(Box::new(bricks_tools::skill_tool::SkillTool::new()));
            }
            session
        });

        let cancel_token = self.cancel_token.unwrap_or_default();
        let run_cancel = cancel_token.child_token();
        let compact_cancel = cancel_token.child_token();
        let agent = Agent {
            selection: parking_lot::RwLock::new(Selection {
                provider: Arc::from(provider),
                label: self.model,
                reasoning: self.reasoning_profile,
                generation: 0,
            }),
            tools,
            system_prompt: self.system_prompt,
            prompt_options: self.prompt_options,
            request_env: request_env::RequestEnv::default(),
            last_system: parking_lot::Mutex::new(None),
            append_system_prompt: self.append_system_prompt,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            working_dir,
            permission_policy,
            permission_parts,
            memory: self.memory,
            long_term_memory: parking_lot::RwLock::new(self.long_term_memory),
            memory_recall_tokens: self.memory_recall_tokens,
            recalled: parking_lot::Mutex::new(None),
            session_id: self.session_id,
            hooks: self.hooks,
            mcp_servers: self.mcp_servers,
            mcp: tokio::sync::OnceCell::new(),
            event_handler: self.event_handler,
            shell_session_id: session_for_store.clone(),
            retry_after_max_wait: std::time::Duration::from_millis(retry_after_max_wait_ms),
            broadcast_tx,
            reporters: self.reporters,
            event_filter: self.event_filter,
            cost_tracker: Arc::new(CostTracker::new()),
            auto_compact: self.auto_compact,
            tool_result_budget: self.tool_result_budget,
            context: Arc::new(parking_lot::Mutex::new(context::ContextManager::new(
                policy,
            ))),
            compressor: Arc::new(compressor),
            raw_history: Arc::new(parking_lot::Mutex::new(initial.clone())),
            snapshots: Arc::new(parking_lot::Mutex::new(Vec::new())),
            raw_refs: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            compaction_state: Arc::new(parking_lot::Mutex::new(runner::CompactionState::default())),
            config_notes: parking_lot::Mutex::new(bricks.diagnostics),
            restored_unannounced: parking_lot::Mutex::new(None),
            turns_elapsed_cadence: if self.turns_elapsed_cadence == 0 {
                u32::MAX
            } else {
                self.turns_elapsed_cadence
            },
            compression_level: Arc::new(parking_lot::Mutex::new(compression_level)),
            benchmark_mode: self.benchmark_mode,
            messages: Arc::new(parking_lot::Mutex::new(initial)),
            cumulative_usage: Arc::new(parking_lot::Mutex::new(
                self.seed_usage.unwrap_or_default(),
            )),
            cancel_token,
            run_cancel: parking_lot::Mutex::new(run_cancel),
            compact_cancel: parking_lot::Mutex::new(compact_cancel),
            save_revision: std::sync::atomic::AtomicU64::new(0),
            save_owner: uuid::Uuid::new_v4().as_u64_pair().0,
            extensions: self.extensions,
            skills,
            skills_checked: parking_lot::Mutex::new(None),
        };
        // A plan made here reports to this agent's events (one given by the
        // caller reports wherever the caller decided).
        if own_plan && let Some(plan) = agent.plan() {
            plan.set_observer(Some(Arc::new(PlanEvents(Box::new(agent.emit_handle())))));
        }
        Ok(agent)
    }

    /// Build + run in one shot.
    pub async fn run_with(self, prompt: &str) -> bricks_types::Result<AgentOutput> {
        self.build()?.run(prompt).await
    }
}

/// The tools of an agent on one job registry: the one in `extensions`
/// ([`bricks_tools::jobs::JobsHandle`], a session's or a parent's) when there
/// is one, else the single one the tools carry, then installed in
/// `extensions` for the runner. Tools on two different registries with none
/// in `extensions` are an incomplete assembly: refused.
fn assemble_jobs(
    tools: Vec<Box<dyn Tool>>,
    extensions: &bricks_tools::Extensions,
) -> bricks_types::Result<Vec<Box<dyn Tool>>> {
    if let Some(handle) = extensions.get::<bricks_tools::jobs::JobsHandle>() {
        return Ok(bricks_tools::on_job_registry(tools, &handle.0));
    }
    let mut registry: Option<Arc<bricks_tools::jobs::JobRegistry>> = None;
    for tool in &tools {
        let Some(r) = tool.job_registry() else {
            continue;
        };
        match &registry {
            Some(known) if !Arc::ptr_eq(known, &r) => {
                return Err(BricksError::Config(format!(
                    "`{}` is on another job registry than the agent's other job tools: build \
                     them together with bricks_tools::shell_with_jobs(registry), or give the \
                     agent a JobsHandle",
                    tool.name()
                )));
            }
            Some(_) => {}
            None => registry = Some(r),
        }
    }
    Ok(match registry {
        Some(r) => {
            extensions.insert(bricks_tools::jobs::JobsHandle(Arc::clone(&r)));
            bricks_tools::on_job_registry(tools, &r)
        }
        None => tools,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bricks_provider::{CompletionRequest, CompletionStream, Provider};

    /// Minimal provider that never produces output — enough to `build()` an agent.
    struct StubProvider;

    #[async_trait::async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &str {
            "stub"
        }
        fn context_window(&self, _model: &str) -> u64 {
            1000
        }
        async fn complete(
            &self,
            _request: CompletionRequest,
        ) -> bricks_types::Result<CompletionStream> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(CompletionStream::new(rx))
        }
    }

    #[test]
    fn cumulative_usage_defaults_to_zero() {
        let agent = Agent::builder().provider(StubProvider).build().unwrap();
        assert_eq!(agent.usage().input_tokens, 0);
        assert_eq!(agent.usage().output_tokens, 0);
    }

    #[test]
    fn seeded_cumulative_usage_is_restored() {
        let seed = Usage {
            input_tokens: 1234,
            output_tokens: 567,
            total_tokens: 1801,
            ..Default::default()
        };
        let agent = Agent::builder()
            .provider(StubProvider)
            .with_cumulative_usage(seed.clone())
            .build()
            .unwrap();
        let restored = agent.usage();
        assert_eq!(restored.input_tokens, 1234);
        assert_eq!(restored.output_tokens, 567);
        assert_eq!(restored.total_tokens, 1801);
    }
}

#[cfg(test)]
mod termination_history_tests {
    use super::Termination;

    /// Results stored by an earlier version still load. A kind this version
    /// does not know is read as a historical, interrupted end, never as a
    /// success; the kinds it knows keep their fields.
    #[test]
    fn a_stored_termination_of_an_unknown_kind_is_still_read() {
        // Historical data: a stop written before 0.4.8.
        let t: Termination = serde_json::from_str(r#"{"kind":"max_turns","limit":50}"#).unwrap();
        assert_eq!(
            t,
            Termination::Historical {
                kind: "max_turns".into()
            }
        );
        assert!(!t.is_completed());
        assert!(t.describe().starts_with("interrupted"));
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            r#"{"kind":"max_turns"}"#
        );

        let done: Termination = serde_json::from_str(r#"{"kind":"completed"}"#).unwrap();
        assert!(done.is_completed());
        let cut: Termination =
            serde_json::from_str(r#"{"kind":"no_progress","repeats":5}"#).unwrap();
        assert_eq!(cut, Termination::NoProgress { repeats: 5 });
        assert_eq!(
            serde_json::to_string(&cut).unwrap(),
            r#"{"kind":"no_progress","repeats":5}"#
        );
        assert!(serde_json::from_str::<Termination>(r#"{"kind":"no_progress"}"#).is_err());
        assert!(serde_json::from_str::<Termination>(r#"{"limit":3}"#).is_err());
    }
}
