//! cersei-agent: The high-level Agent API with builder pattern, agentic loop,
//! realtime event streaming, broadcast channels, and reporters.

pub mod agent_tool;
pub mod auto_dream;
pub mod bricks_config;
pub mod compact;
pub mod context;
pub mod context_analyzer;
pub mod control;
pub mod coordinator;
pub mod delegate;
pub mod delegate_tool;
pub mod events;
pub mod reporters;
pub(crate) mod runner;
pub mod session_memory;
pub mod subagent;
pub mod system_prompt;

// Re-export runner utilities
pub use bricks_config::BricksConfig;
pub use compact::CompactionOutcome;
pub use context::{ContextPolicy, ContextStatus, Provenance};
pub use runner::{apply_tool_result_budget, apply_tool_result_budget_with};

use cersei_hooks::Hook;
use cersei_mcp::McpServerConfig;
use cersei_memory::Memory;
use cersei_provider::Provider;
use cersei_tools::permissions::{AllowAll, PermissionPolicy};
use cersei_tools::{CostTracker, Tool};
use cersei_types::*;
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
}

impl UserInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
        }
    }
}

// ─── Agent output ────────────────────────────────────────────────────────────

/// Why a run that returned stopped. A cancelled or failed run returns an
/// error instead; every variant here except [`Termination::Completed`] is
/// an incomplete run, whose history and partial results are kept.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Termination {
    /// The model gave its final answer with no tool call left to run.
    Completed,
    /// `limit` generation turns ran and the model still wanted to continue.
    MaxTurns { limit: u32 },
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
            Termination::MaxTurns { limit } => {
                format!("stopped at the turn limit ({limit}) before a final answer")
            }
            Termination::OutputTruncated { continuations } => format!(
                "the answer was cut by the output-token limit ({continuations} continuation(s) allowed)"
            ),
            Termination::NoProgress { repeats } => format!(
                "stopped: the same tool calls returned the same results {repeats} times"
            ),
            Termination::ContentFiltered => "the provider stopped the answer (content filter)".into(),
            Termination::EmptyResponse => "the model returned an empty response".into(),
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
    /// Generation turns actually run (never more than `max_turns`).
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

#[allow(dead_code)]
pub struct Agent {
    /// The model's provider. Replaced by [`Agent::set_model`]; a run reads
    /// it at the start of each turn.
    provider: parking_lot::RwLock<Arc<dyn Provider>>,
    tools: Vec<Box<dyn Tool>>,
    system_prompt: Option<String>,
    append_system_prompt: Option<String>,
    model: parking_lot::Mutex<Option<String>>,
    max_turns: u32,
    max_tokens: u32,
    temperature: Option<f32>,
    reasoning_profile: parking_lot::Mutex<Option<String>>,
    working_dir: PathBuf,
    permission_policy: Arc<dyn PermissionPolicy>,
    memory: Option<Arc<dyn Memory>>,
    /// Long-term memory recalled into the prompt and fed with each exchange.
    pub(crate) long_term_memory: Option<Arc<dyn cersei_memory::LongTermMemory>>,
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
    cost_tracker: Arc<CostTracker>,
    auto_compact: bool,
    tool_result_budget: usize,
    /// Occupation, measurements and session totals of the context.
    pub(crate) context: Arc<parking_lot::Mutex<context::ContextManager>>,
    /// Reduces tool outputs and saves their originals.
    pub(crate) compressor: Arc<cersei_compression::Compressor>,
    /// Every message as it happened, tool results unreduced; never compacted.
    pub(crate) raw_history: Arc<parking_lot::Mutex<Vec<Message>>>,
    /// The active history before each compaction.
    pub(crate) snapshots: Arc<parking_lot::Mutex<Vec<CompactionSnapshot>>>,
    /// Saved originals of tool results, by tool-call id.
    pub(crate) raw_refs:
        Arc<parking_lot::Mutex<std::collections::HashMap<String, cersei_compression::RawRef>>>,
    pub(crate) compaction_state: Arc<parking_lot::Mutex<runner::CompactionState>>,
    /// Configuration diagnostics, reported once at the first run.
    pub(crate) config_notes: parking_lot::Mutex<Vec<String>>,
    /// Cadence (in turns) at which `HookEvent::TurnsElapsed` fires. Default
    /// 10. Setting to 0 disables the event entirely. Used by the
    /// `SkillNudgeHook` for agent-curated skill review.
    pub(crate) turns_elapsed_cadence: u32,
    pub(crate) compression_level: Arc<parking_lot::Mutex<cersei_compression::CompressionLevel>>,
    pub benchmark_mode: bool,
    messages: Arc<parking_lot::Mutex<Vec<Message>>>,
    cumulative_usage: Arc<parking_lot::Mutex<Usage>>,
    /// Cancels every run of this agent (given to the builder, or its own).
    cancel_token: tokio_util::sync::CancellationToken,
    /// Cancels the current run only: a child of `cancel_token`, replaced at
    /// the start of each run, so a cancelled run does not cancel the next.
    run_cancel: parking_lot::Mutex<tokio_util::sync::CancellationToken>,
    /// Type-map injected into every `ToolContext` this agent builds. Used by
    /// orchestration layers (e.g. cersei-agentrl) to hand tools a dynamic tool
    /// registry, a sandbox handle, a Mailbox/KvStore, etc. at runtime.
    pub(crate) extensions: cersei_tools::Extensions,
}

impl Agent {
    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    /// Run a prompt through the agentic loop. With a long-term memory, the
    /// memory's maintenance (extraction, embeddings) runs after the answer,
    /// before this returns; [`AgentEvent::MemoryMaintenance`] reports it.
    pub async fn run(&self, prompt: &str) -> cersei_types::Result<AgentOutput> {
        runner::run_agent(self, &UserInput::text(prompt)).await
    }

    /// [`Agent::run`] with attachments (images, documents, captured file
    /// contents) after the text.
    pub async fn run_input(&self, input: &UserInput) -> cersei_types::Result<AgentOutput> {
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
        let (control_tx, mut control_rx) = mpsc::channel(64);
        let agent = Arc::clone(self);

        // Controls: cancellation reaches the run immediately, whatever the
        // event consumer is doing.
        let controlled = Arc::clone(self);
        let controls = tokio::spawn(async move {
            while let Some(c) = control_rx.recv().await {
                match c {
                    events::AgentControl::Cancel => controlled.cancel(),
                    events::AgentControl::PermissionResponse { .. }
                    | events::AgentControl::InjectMessage(_) => {}
                }
            }
        });

        tokio::spawn(async move {
            let result = runner::run_agent_streaming(&agent, &input, event_tx.clone()).await;
            let ok = result.is_ok();
            match result {
                Ok(output) => {
                    let _ = event_tx.send(AgentEvent::Complete(output)).await;
                }
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::Error(e.to_string())).await;
                }
            }
            if ok {
                let cancel = agent.run_cancel.lock().clone();
                runner::maintain_memory(&agent, &cancel, Some(&event_tx)).await;
            }
            controls.abort();
        });

        AgentStream::new(event_rx, control_tx)
    }

    /// Multi-turn: send a follow-up message in the same conversation.
    pub async fn reply(&self, message: &str) -> cersei_types::Result<AgentOutput> {
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

    /// Compact the history now (outside of the automatic triggers).
    pub async fn compact(&self) -> CompactionOutcome {
        runner::compact_now(self).await
    }

    /// Cancel the current run (and the memory maintenance that follows
    /// it). The next run starts normally.
    pub fn cancel(&self) {
        self.run_cancel.lock().cancel();
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

    /// The selected reasoning profile, if any.
    pub fn reasoning_profile(&self) -> Option<String> {
        self.reasoning_profile.lock().clone()
    }

    /// Switch model and reasoning profile. A run in progress uses them from
    /// its next turn; measurements made with the previous model no longer
    /// count as measured.
    pub fn set_model(
        &self,
        provider: Box<dyn Provider>,
        label: Option<String>,
        reasoning_profile: Option<String>,
    ) {
        *self.provider.write() = Arc::from(provider);
        *self.model.lock() = label;
        *self.reasoning_profile.lock() = reasoning_profile;
        self.context
            .lock()
            .invalidate("the model or reasoning profile changed");
    }

    /// The current provider.
    pub(crate) fn provider(&self) -> Arc<dyn Provider> {
        self.provider.read().clone()
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
    pub async fn save_session(&self) -> cersei_types::Result<bool> {
        runner::save_session(self).await
    }

    /// Empty the active context. The raw history keeps every message, and
    /// the cleared history is kept as a snapshot like a compaction's.
    pub async fn clear_context(&self) -> cersei_types::Result<usize> {
        runner::clear_context(self).await
    }

    /// Whether a long-term memory is attached.
    pub fn has_long_term_memory(&self) -> bool {
        self.long_term_memory.is_some()
    }

    /// Run the long-term memory's maintenance now (what earlier runs
    /// recorded and did not process). `None` without a long-term memory.
    pub async fn maintain_memory(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Option<cersei_types::Result<cersei_memory::MaintenanceReport>> {
        let ltm = self.long_term_memory.as_ref()?;
        Some(ltm.maintain(cancel).await)
    }

    /// Get the current tool-output compression level.
    pub fn compression_level(&self) -> cersei_compression::CompressionLevel {
        *self.compression_level.lock()
    }

    /// Change the tool-output compression level at runtime. Takes effect on
    /// the next tool call.
    pub fn set_compression_level(&self, level: cersei_compression::CompressionLevel) {
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
    pub(crate) fn emit_handle(&self) -> impl Fn(AgentEvent) + Send + Sync + 'static {
        let filter = self.event_filter.clone();
        let handler = self.event_handler.clone();
        let broadcast = self.broadcast_tx.clone();
        let reporters = self.reporters.clone();
        move |event: AgentEvent| {
            if let Some(filter) = &filter {
                if !filter(&event) {
                    return;
                }
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
        cersei_tools::shell::close_session(&self.shell_session_id).await;
        if let Some(m) = self.mcp.get().and_then(|s| s.manager.clone()) {
            m.close().await;
        }
    }

    /// The MCP connections, once connected (after the first run starts).
    pub fn mcp_manager(&self) -> Option<Arc<cersei_mcp::McpManager>> {
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
                let manager = Arc::new(cersei_mcp::McpManager::connect(&self.mcp_servers).await);
                let tools = cersei_tools::mcp_tool::tools_of(&manager).await;
                McpState {
                    manager: Some(manager),
                    tools,
                }
            })
            .await;
        if let Some(m) = &state.manager {
            for (name, status) in m.statuses().await {
                if let cersei_mcp::McpServerStatus::Error(e) = status {
                    notes.push(format!("MCP server '{name}' unavailable: {e}"));
                }
            }
        }
        notes
    }

    /// The system prompt of the requests of the current run: the configured
    /// prompt, then the recalled long-term memory block (if any).
    pub(crate) fn effective_system(&self) -> Option<String> {
        let recalled = self.recalled.lock().clone();
        match (&self.system_prompt, recalled) {
            (Some(s), Some(r)) => Some(format!("{s}\n\n{r}")),
            (Some(s), None) => Some(s.clone()),
            (None, Some(r)) => Some(r),
            (None, None) => None,
        }
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

/// Connected MCP servers and their tools.
pub(crate) struct McpState {
    manager: Option<Arc<cersei_mcp::McpManager>>,
    tools: Vec<Box<dyn Tool>>,
}

impl Drop for Agent {
    fn drop(&mut self) {
        cersei_tools::shell::close_session_now(&self.shell_session_id);
    }
}

// ─── Agent builder ───────────────────────────────────────────────────────────

pub struct AgentBuilder {
    provider: Option<Box<dyn Provider>>,
    tools: Vec<Box<dyn Tool>>,
    system_prompt: Option<String>,
    append_system_prompt: Option<String>,
    model: Option<String>,
    max_turns: u32,
    max_tokens: u32,
    temperature: Option<f32>,
    reasoning_profile: Option<String>,
    seed_usage: Option<Usage>,
    working_dir: Option<PathBuf>,
    permission_policy: Option<Arc<dyn PermissionPolicy>>,
    memory: Option<Arc<dyn Memory>>,
    long_term_memory: Option<Arc<dyn cersei_memory::LongTermMemory>>,
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
    tool_result_budget: usize,
    turns_elapsed_cadence: u32,
    compression_level: Option<cersei_compression::CompressionLevel>,
    context_policy: Option<context::ContextPolicy>,
    compression_config: Option<cersei_compression::CompressionConfig>,
    compression_rules: Option<Arc<cersei_compression::RuleSet>>,
    raw_output_dir: Option<PathBuf>,
    bricks_config: Option<BricksConfig>,
    web_config: Option<cersei_web::WebConfig>,
    initial_messages: Option<Vec<Message>>,
    benchmark_mode: bool,
    extensions: cersei_tools::Extensions,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            provider: None,
            tools: Vec::new(),
            system_prompt: None,
            append_system_prompt: None,
            model: None,
            max_turns: 10,
            max_tokens: 16384,
            temperature: None,
            reasoning_profile: None,
            seed_usage: None,
            working_dir: None,
            permission_policy: None,
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
            tool_result_budget: 50_000,
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
            extensions: cersei_tools::Extensions::default(),
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

    pub fn append_system_prompt(mut self, s: impl Into<String>) -> Self {
        self.append_system_prompt = Some(s.into());
        self
    }

    pub fn model(mut self, m: impl Into<String>) -> Self {
        self.model = Some(m.into());
        self
    }

    pub fn max_turns(mut self, n: u32) -> Self {
        self.max_turns = n;
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

    pub fn permission_policy(mut self, p: impl PermissionPolicy + 'static) -> Self {
        self.permission_policy = Some(Arc::new(p));
        self
    }

    /// [`AgentBuilder::permission_policy`] with a policy already shared, such
    /// as a parent's (a sub-agent never gets more than its parent).
    pub fn permission_policy_arc(mut self, p: Arc<dyn PermissionPolicy>) -> Self {
        self.permission_policy = Some(p);
        self
    }

    /// A long-term memory: what it recalls for the prompt is added to the
    /// system prompt of each run (within the recall budget), and each
    /// finished exchange is recorded into it.
    pub fn long_term_memory(mut self, m: Arc<dyn cersei_memory::LongTermMemory>) -> Self {
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
    pub fn compression_config(mut self, config: cersei_compression::CompressionConfig) -> Self {
        self.compression_config = Some(config);
        self
    }

    /// Output rules (default: the built-in rules only; see
    /// [`BricksConfig::load`] for user and project rules).
    pub fn compression_rules(mut self, rules: Arc<cersei_compression::RuleSet>) -> Self {
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
    pub fn web_config(mut self, config: cersei_web::WebConfig) -> Self {
        self.web_config = Some(config);
        self
    }

    pub fn bricks_config(mut self, config: BricksConfig) -> Self {
        self.bricks_config = Some(config);
        self
    }

    pub fn tool_result_budget(mut self, chars: usize) -> Self {
        self.tool_result_budget = chars;
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

    pub fn compression_level(mut self, level: cersei_compression::CompressionLevel) -> Self {
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

    /// Inject a type-map that is cloned into every `ToolContext` this agent
    /// builds, letting tools retrieve runtime-injected handles (dynamic tool
    /// registry, sandbox, Mailbox/KvStore) via `ctx.extensions.get::<T>()`.
    pub fn extensions(mut self, ext: cersei_tools::Extensions) -> Self {
        self.extensions = ext;
        self
    }

    pub fn build(self) -> cersei_types::Result<Agent> {
        let provider = self
            .provider
            .ok_or_else(|| CerseiError::Config("Provider is required".into()))?;

        let working_dir = self
            .working_dir
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let broadcast_tx = self.broadcast_capacity.map(|cap| {
            let (tx, _) = broadcast::channel(cap);
            tx
        });

        let bricks = self.bricks_config.unwrap_or_default();
        let mut policy = self.context_policy.unwrap_or(bricks.context);
        if let Some(t) = self.compact_threshold {
            policy.compact_threshold = t;
        }
        policy.validate().map_err(CerseiError::Config)?;
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
        let store = match (raw_dir, session_dir) {
            (Some(dir), _) => cersei_compression::RawStore::new(dir.join(&session_for_store)),
            (None, Some(dir)) => cersei_compression::RawStore::new(dir),
            (None, None) => cersei_compression::RawStore::for_session(&session_for_store),
        };
        let compressor = cersei_compression::Compressor::new(
            self.compression_config.unwrap_or(bricks.compression),
            self.compression_rules.unwrap_or(bricks.rules),
            Some(store),
        );
        // One web context per session: documents are stored with the
        // session's saved originals, so they are restored and deleted with it.
        if self
            .extensions
            .get::<cersei_tools::web_runtime::WebRuntime>()
            .is_none()
        {
            let web_config = self.web_config.unwrap_or(bricks.web);
            let store = compressor.raw_store().map(|r| {
                Arc::new(cersei_web::store::WebStore::open(
                    r.dir().join("web"),
                    web_config.store.max_session_bytes,
                ))
            });
            let web =
                cersei_web::WebContext::new(web_config, store).map_err(CerseiError::Config)?;
            self.extensions
                .insert(cersei_tools::web_runtime::WebRuntime(Arc::new(web)));
        }
        let compression_level = self
            .compression_level
            .or(bricks.compression_level)
            .unwrap_or_default();
        let initial = self.initial_messages.unwrap_or_default();

        let cancel_token = self.cancel_token.unwrap_or_default();
        let run_cancel = cancel_token.child_token();
        Ok(Agent {
            provider: parking_lot::RwLock::new(Arc::from(provider)),
            tools: self.tools,
            system_prompt: self.system_prompt,
            append_system_prompt: self.append_system_prompt,
            model: parking_lot::Mutex::new(self.model),
            max_turns: self.max_turns,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            reasoning_profile: parking_lot::Mutex::new(self.reasoning_profile),
            working_dir,
            permission_policy: self.permission_policy.unwrap_or_else(|| Arc::new(AllowAll)),
            memory: self.memory,
            long_term_memory: self.long_term_memory,
            memory_recall_tokens: self.memory_recall_tokens,
            recalled: parking_lot::Mutex::new(None),
            session_id: self.session_id,
            hooks: self.hooks,
            mcp_servers: self.mcp_servers,
            mcp: tokio::sync::OnceCell::new(),
            event_handler: self.event_handler,
            shell_session_id: session_for_store.clone(),
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
            extensions: self.extensions,
        })
    }

    /// Build + run in one shot.
    pub async fn run_with(self, prompt: &str) -> cersei_types::Result<AgentOutput> {
        self.build()?.run(prompt).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cersei_provider::{CompletionRequest, CompletionStream, Provider};

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
        ) -> cersei_types::Result<CompletionStream> {
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
