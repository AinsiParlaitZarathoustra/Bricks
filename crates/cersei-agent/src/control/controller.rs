//! The engine's narrow entry point for frontends.
//!
//! A [`Controller`] owns one session's agent and turns [`Command`]s into
//! engine calls, and the agent's events into [`Envelope`]s on an
//! [`EventStream`]. It does not run the agentic loop itself: runs, tools,
//! compaction, memory recall and maintenance stay in the agent.
//!
//! Rules while a run is active:
//! * `cancel` is always accepted and reaches the run at once;
//! * `set_model` is accepted and applies from the run's next turn
//!   (`model_changed` says so);
//! * `submit`, `resume`, `compact` and `clear_context` are refused with a
//!   `command_rejected` event: a second prompt is never started silently.

use super::approval::{ApprovalBroker, ApprovalGate, ApprovalRequest};
use super::attach::{self, AttachLimits};
use super::protocol::*;
use super::queue::EventQueue;
use super::session::{SessionMeta, SessionSummary};
use crate::events::AgentEvent;
use crate::{Agent, BricksConfig, CompactReason};
use cersei_memory::{JsonlMemory, LongTermMemory, Memory};
use cersei_provider::Provider;
use cersei_tools::Tool;
use cersei_types::{CerseiError, Message, Usage};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ─── Models ──────────────────────────────────────────────────────────────────

/// A selectable reasoning profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileChoice {
    pub id: String,
    pub label: String,
}

/// A selectable model, as the configuration declares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelChoice {
    /// `provider_id/model_id`.
    pub selection: String,
    pub name: String,
    pub provider: String,
    pub profiles: Vec<ProfileChoice>,
    pub default_profile: Option<String>,
    pub max_input_tokens: u64,
    pub context_window_tokens: Option<u64>,
    /// Input modalities the model declares (`text`, `image`, ...).
    pub input: Vec<String>,
    /// A price is configured (otherwise costs are unknown).
    pub priced: bool,
}

/// Where models come from: the provider registry (`providers.toml`), or a
/// test double.
pub trait ModelCatalog: Send + Sync {
    fn models(&self) -> Vec<ModelChoice>;
    /// The provider of `selection`, with `reasoning` checked against the
    /// model's profiles.
    fn build(&self, selection: &str, reasoning: Option<&str>) -> Result<Box<dyn Provider>, String>;
}

impl ModelCatalog for cersei_provider::ProviderRegistry {
    fn models(&self) -> Vec<ModelChoice> {
        self.models()
            .iter()
            .filter_map(|m| {
                let r = self.resolve(&m.selection()).ok()?;
                Some(ModelChoice {
                    selection: r.selection(),
                    name: r.model_name.clone(),
                    provider: r.provider_name.clone(),
                    profiles: r
                        .reasoning
                        .profiles
                        .iter()
                        .map(|p| ProfileChoice {
                            id: p.id.clone(),
                            label: p.label().to_string(),
                        })
                        .collect(),
                    default_profile: r.reasoning.default.clone(),
                    max_input_tokens: r.limits.max_input_tokens,
                    context_window_tokens: r.limits.context_window_tokens,
                    input: r.declared.input.iter().map(|m| m.to_string()).collect(),
                    priced: r.pricing.is_some(),
                })
            })
            .collect()
    }

    fn build(&self, selection: &str, reasoning: Option<&str>) -> Result<Box<dyn Provider>, String> {
        let r = self.resolve(selection).map_err(|e| e.to_string())?;
        if let Some(p) = reasoning {
            if !r.reasoning.profiles.iter().any(|x| x.id == p) {
                let known: Vec<&str> = r.reasoning.profiles.iter().map(|x| x.id.as_str()).collect();
                return Err(format!(
                    "model `{selection}` has no reasoning profile `{p}`; its profiles: {}",
                    if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    }
                ));
            }
        }
        Ok(Box::new(r.build_provider().map_err(|e| e.to_string())?))
    }
}

// ─── Configuration ───────────────────────────────────────────────────────────

/// Builds the tools of each agent.
pub type ToolFactory = Arc<dyn Fn() -> Vec<Box<dyn Tool>> + Send + Sync>;

/// What a controller needs, assembled by the frontend from the existing
/// loaders (`ProviderRegistry::load`, `BricksConfig::load`).
#[derive(Clone)]
pub struct EngineConfig {
    /// Working directory of new sessions (resumed sessions keep theirs).
    pub working_dir: PathBuf,
    pub catalog: Arc<dyn ModelCatalog>,
    pub bricks: BricksConfig,
    /// Session store directory (`JsonlMemory`).
    pub sessions_dir: PathBuf,
    /// Whether a person can answer approval requests.
    pub interactive: bool,
    pub tools: ToolFactory,
    pub system_prompt: Option<String>,
    pub long_term_memory: Option<Arc<dyn LongTermMemory>>,
    /// Space of that memory, recorded with the session.
    pub memory_space: Option<String>,
    pub mcp_servers: Vec<cersei_mcp::McpServerConfig>,
    pub queue_capacity: usize,
    pub attach_limits: AttachLimits,
}

impl EngineConfig {
    pub fn new(
        working_dir: impl Into<PathBuf>,
        catalog: Arc<dyn ModelCatalog>,
        bricks: BricksConfig,
        sessions_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            working_dir: working_dir.into(),
            catalog,
            bricks,
            sessions_dir: sessions_dir.into(),
            interactive: true,
            tools: Arc::new(cersei_tools::coding),
            system_prompt: None,
            long_term_memory: None,
            memory_space: None,
            mcp_servers: Vec::new(),
            queue_capacity: 256,
            attach_limits: AttachLimits::default(),
        }
    }
}

/// Which session to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionChoice {
    New,
    Resume(String),
}

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub session: SessionChoice,
    /// Overrides the session's (or the configuration's) model.
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

// ─── Views ───────────────────────────────────────────────────────────────────

/// What the controller is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Activity {
    Idle,
    Running {
        run_id: String,
    },
    /// A command that rewrites the session (compaction, resume) is in
    /// progress.
    Busy {
        what: String,
    },
}

/// A read-only picture of the session for inspectors and status bars.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub session_id: String,
    pub title: String,
    pub working_dir: PathBuf,
    pub model: String,
    pub reasoning: Option<String>,
    pub activity: Activity,
    pub maintenance_running: bool,
    pub interactive: bool,
    pub context: crate::context::ContextStatus,
    pub usage: Usage,
    pub pending_approvals: Vec<ApprovalRequest>,
    pub allowed_for_session: Vec<String>,
    pub memory_space: Option<String>,
    pub approval_rules: super::settings::ApprovalRules,
}

/// A tool as inspectors show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub level: String,
}

// ─── Controller ──────────────────────────────────────────────────────────────

struct Inner {
    cfg: EngineConfig,
    agent: parking_lot::RwLock<Arc<Agent>>,
    meta: parking_lot::Mutex<SessionMeta>,
    queue: Arc<EventQueue>,
    broker: Arc<ApprovalBroker>,
    activity: parking_lot::Mutex<Activity>,
    maintenance: parking_lot::Mutex<Option<CancellationToken>>,
    idle: tokio::sync::Notify,
}

/// One session, driven by commands. Cheap to clone.
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Inner>,
}

/// The events of a controller, for one consumer.
pub struct EventStream {
    queue: Arc<EventQueue>,
}

impl EventStream {
    pub async fn next(&mut self) -> Option<Envelope> {
        self.queue.recv().await
    }

    pub fn try_next(&mut self) -> Option<Envelope> {
        self.queue.try_recv()
    }
}

impl Drop for EventStream {
    /// The consumer is gone: producers stop waiting for it.
    fn drop(&mut self) {
        self.queue.close();
    }
}

fn session_store(cfg: &EngineConfig) -> JsonlMemory {
    JsonlMemory::new(&cfg.sessions_dir)
}

fn files_dir(cfg: &EngineConfig, session_id: &str) -> PathBuf {
    session_store(cfg)
        .session_files_dir(session_id)
        .unwrap_or_else(|| cfg.sessions_dir.join(format!("{session_id}.files")))
}

fn model_list(cfg: &EngineConfig) -> String {
    let models: Vec<String> = cfg
        .catalog
        .models()
        .into_iter()
        .map(|m| m.selection)
        .collect();
    if models.is_empty() {
        "no model is configured (providers.toml)".to_string()
    } else {
        format!("configured models: {}", models.join(", "))
    }
}

/// Stored sessions, most recently updated first.
pub async fn list_sessions(sessions_dir: &Path) -> Result<Vec<SessionSummary>, String> {
    let store = JsonlMemory::new(sessions_dir);
    let mut out = Vec::new();
    for s in store.sessions().await.map_err(|e| e.to_string())? {
        let meta = store
            .session_files_dir(&s.id)
            .and_then(|d| SessionMeta::load(&d).ok().flatten());
        out.push(SessionSummary {
            title: meta.as_ref().map(|m| m.title.clone()).unwrap_or_default(),
            created_at: meta
                .as_ref()
                .map(|m| m.created_at)
                .unwrap_or_else(|| s.created_at.timestamp_millis()),
            updated_at: meta
                .as_ref()
                .map(|m| m.updated_at)
                .unwrap_or_else(|| s.created_at.timestamp_millis()),
            message_count: s.message_count,
            working_dir: meta.as_ref().map(|m| m.working_dir.clone()),
            model: meta.as_ref().map(|m| m.model.clone()),
            reasoning: meta.as_ref().and_then(|m| m.reasoning.clone()),
            id: s.id,
        });
    }
    out.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

fn new_session_id() -> String {
    let now = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let rand = uuid::Uuid::new_v4().simple().to_string();
    format!("{now}-{}", &rand[..6])
}

fn build_agent(
    cfg: &EngineConfig,
    meta: &SessionMeta,
    provider: Box<dyn Provider>,
    broker: &Arc<ApprovalBroker>,
) -> Result<Agent, String> {
    let settings = &cfg.bricks.agent;
    let mut b = Agent::builder()
        .provider_boxed(provider)
        .tools((cfg.tools)())
        .working_dir(&meta.working_dir)
        .model(meta.model.clone())
        .permission_policy(ApprovalGate::new(
            cfg.bricks.permissions.clone(),
            broker.clone(),
        ))
        .memory(session_store(cfg))
        .session_id(meta.id.clone())
        .bricks_config(cfg.bricks.clone())
        .max_turns(settings.max_turns.unwrap_or(50));
    if let Some(r) = &meta.reasoning {
        b = b.reasoning_profile(r.clone());
    }
    if let Some(t) = settings.max_tokens {
        b = b.max_tokens(t);
    }
    if let Some(s) = &cfg.system_prompt {
        b = b.system_prompt(s.clone());
    }
    if let Some(m) = &cfg.long_term_memory {
        b = b.long_term_memory(m.clone());
    }
    for s in &cfg.mcp_servers {
        b = b.mcp_server(s.clone());
    }
    b.build().map_err(|e| e.to_string())
}

/// A session ready to open.
struct Prepared {
    meta: SessionMeta,
    provider: Box<dyn Provider>,
    resumed: bool,
    warnings: Vec<String>,
}

/// Resolve the session to open: its metadata (checked) and provider.
fn prepare(
    cfg: &EngineConfig,
    session: &SessionChoice,
    model: Option<&str>,
    reasoning: Option<&str>,
) -> Result<Prepared, String> {
    let mut warnings = Vec::new();
    let settings = &cfg.bricks.agent;
    match session {
        SessionChoice::New => {
            let model = model
                .map(str::to_string)
                .or_else(|| settings.model.clone())
                .ok_or_else(|| {
                    format!(
                        "no model selected: pass --model or set [agent] model in bricks.toml ({})",
                        model_list(cfg)
                    )
                })?;
            let reasoning = reasoning.map(str::to_string).or_else(|| {
                (settings.model.as_deref() == Some(model.as_str()))
                    .then(|| settings.reasoning.clone())
                    .flatten()
            });
            let provider = cfg
                .catalog
                .build(&model, reasoning.as_deref())
                .map_err(|e| format!("{e} ({})", model_list(cfg)))?;
            let mut meta = SessionMeta::new(&new_session_id(), &cfg.working_dir, &model, reasoning);
            meta.memory_space = cfg.memory_space.clone();
            Ok(Prepared {
                meta,
                provider,
                resumed: false,
                warnings,
            })
        }
        SessionChoice::Resume(id) => {
            let store_file = cfg.sessions_dir.join(format!("{id}.jsonl"));
            if !store_file.exists() {
                return Err(format!(
                    "no stored session `{id}` in {}",
                    cfg.sessions_dir.display()
                ));
            }
            let mut meta = match SessionMeta::load(&files_dir(cfg, id))? {
                Some(m) => m,
                None => {
                    warnings.push(
                        "this session has no stored settings (older session): it resumes in the \
                         current directory with the selected model"
                            .into(),
                    );
                    let model = model
                        .map(str::to_string)
                        .or_else(|| settings.model.clone())
                        .ok_or_else(|| {
                            format!("pass --model to resume this session ({})", model_list(cfg))
                        })?;
                    SessionMeta::new(id, &cfg.working_dir, &model, None)
                }
            };
            if !meta.working_dir.is_dir() {
                return Err(format!(
                    "the session's working directory {} no longer exists; nothing was resumed",
                    meta.working_dir.display()
                ));
            }
            if meta.working_dir != cfg.working_dir {
                warnings.push(format!(
                    "the session works in {}, not in the current directory",
                    meta.working_dir.display()
                ));
            }
            if let Some(m) = model {
                if m != meta.model {
                    warnings.push(format!("model changed from `{}` to `{m}`", meta.model));
                    meta.model = m.to_string();
                    meta.reasoning = None;
                }
            } else if !cfg
                .catalog
                .models()
                .iter()
                .any(|c| c.selection == meta.model)
            {
                return Err(format!(
                    "the session's model `{}` is no longer configured; choose one with --model \
                     ({}); none is substituted",
                    meta.model,
                    model_list(cfg)
                ));
            }
            if let Some(r) = reasoning {
                meta.reasoning = Some(r.to_string());
            }
            if meta.memory_space.is_some() && meta.memory_space != cfg.memory_space {
                warnings.push(format!(
                    "the session used the long-term memory space `{}`; the current one is {}",
                    meta.memory_space.clone().unwrap_or_default(),
                    cfg.memory_space
                        .as_deref()
                        .map(|s| format!("`{s}`"))
                        .unwrap_or_else(|| "none".into())
                ));
            }
            let provider = cfg.catalog.build(&meta.model, meta.reasoning.as_deref())?;
            Ok(Prepared {
                meta,
                provider,
                resumed: true,
                warnings,
            })
        }
    }
}

impl Controller {
    /// Open a session and its event stream. Errors are complete messages
    /// (missing model, missing directory, unknown session).
    pub async fn open(
        cfg: EngineConfig,
        opts: OpenOptions,
    ) -> Result<(Controller, EventStream), String> {
        let Prepared {
            meta,
            provider,
            resumed,
            warnings,
        } = prepare(
            &cfg,
            &opts.session,
            opts.model.as_deref(),
            opts.reasoning.as_deref(),
        )?;
        let broker = ApprovalBroker::new(cfg.interactive);
        let agent = Arc::new(build_agent(&cfg, &meta, provider, &broker)?);
        meta.save(&files_dir(&cfg, &meta.id))?;
        let queue = Arc::new(EventQueue::new(&meta.id, cfg.queue_capacity));
        let message_count = if resumed {
            session_store(&cfg)
                .load(&meta.id)
                .await
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };
        let mut all_warnings = cfg.bricks.diagnostics.clone();
        all_warnings.extend(warnings);
        let opened = Event::SessionOpened {
            working_dir: meta.working_dir.display().to_string(),
            model: meta.model.clone(),
            reasoning: meta.reasoning.clone(),
            resumed,
            message_count,
            warnings: all_warnings,
        };
        let inner = Arc::new(Inner {
            cfg,
            agent: parking_lot::RwLock::new(agent),
            meta: parking_lot::Mutex::new(meta),
            queue: queue.clone(),
            broker,
            activity: parking_lot::Mutex::new(Activity::Idle),
            maintenance: parking_lot::Mutex::new(None),
            idle: tokio::sync::Notify::new(),
        });
        queue.push(None, opened, None).await;
        Ok((Controller { inner }, EventStream { queue }))
    }

    pub fn session_id(&self) -> String {
        self.inner.meta.lock().id.clone()
    }

    /// The current agent (read-only uses: history, context, tools).
    pub fn agent(&self) -> Arc<Agent> {
        self.inner.agent.read().clone()
    }

    pub fn models(&self) -> Vec<ModelChoice> {
        self.inner.cfg.catalog.models()
    }

    pub fn tools(&self) -> Vec<ToolInfo> {
        self.agent()
            .tool_list()
            .iter()
            .map(|t| ToolInfo {
                name: t.name().to_string(),
                description: t.description().to_string(),
                level: format!("{:?}", t.permission_level()).to_lowercase(),
            })
            .collect()
    }

    /// MCP servers and their state (after the first run connected them).
    pub async fn mcp_statuses(&self) -> Vec<(String, String)> {
        let Some(m) = self.agent().mcp_manager() else {
            return self
                .inner
                .cfg
                .mcp_servers
                .iter()
                .map(|s| {
                    (
                        s.name.clone(),
                        "not connected yet (connects at the first run)".into(),
                    )
                })
                .collect();
        };
        m.statuses()
            .await
            .into_iter()
            .map(|(n, s)| (n, format!("{s:?}")))
            .collect()
    }

    /// The conversation as stored (the authority), or as held in memory
    /// once a run has loaded it.
    pub async fn history(&self) -> Vec<Message> {
        let agent = self.agent();
        let held = agent.messages();
        if !held.is_empty() {
            return held;
        }
        let id = self.session_id();
        session_store(&self.inner.cfg)
            .load(&id)
            .await
            .unwrap_or_default()
    }

    pub async fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        list_sessions(&self.inner.cfg.sessions_dir).await
    }

    pub fn snapshot(&self) -> Snapshot {
        let agent = self.agent();
        let meta = self.inner.meta.lock().clone();
        Snapshot {
            session_id: meta.id.clone(),
            title: meta.title.clone(),
            working_dir: meta.working_dir.clone(),
            model: meta.model.clone(),
            reasoning: meta.reasoning.clone(),
            activity: self.inner.activity.lock().clone(),
            maintenance_running: self.inner.maintenance.lock().is_some(),
            interactive: self.inner.broker.is_interactive(),
            context: agent.context_status(),
            usage: agent.usage(),
            pending_approvals: self.inner.broker.pending(),
            allowed_for_session: self.inner.broker.session_allowed(),
            memory_space: meta.memory_space.clone(),
            approval_rules: self.inner.cfg.bricks.permissions.clone(),
        }
    }

    pub fn activity(&self) -> Activity {
        self.inner.activity.lock().clone()
    }

    /// Wait until no run or session command is in progress (maintenance
    /// may continue).
    pub async fn wait_idle(&self) {
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if *self.inner.activity.lock() == Activity::Idle {
                return;
            }
            notified.await;
        }
    }

    /// Wait until the memory maintenance (if any) is over.
    pub async fn wait_maintenance(&self) {
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.maintenance.lock().is_none()
                && *self.inner.activity.lock() == Activity::Idle
            {
                return;
            }
            notified.await;
        }
    }

    fn reject(&self, cmd: &Command, reason: String) -> Result<(), String> {
        let q = self.inner.queue.clone();
        let event = Event::CommandRejected {
            command: cmd.name().to_string(),
            reason: reason.clone(),
        };
        tokio::spawn(async move { q.push(None, event, None).await });
        Err(reason)
    }

    /// The workspace's shared engine (the one the agents' CodeScout uses).
    pub fn semantic_engine(&self) -> Arc<bricks_semantic::SemanticEngine> {
        let wd = self.agent().working_dir().to_path_buf();
        bricks_semantic::SemanticRegistry::global().engine_for(&wd, &self.inner.cfg.bricks.semantic)
    }

    /// Text search on the shared engine, answered by `search_results`. It
    /// never starts a language server and runs beside an active run.
    fn search(&self, text: String, regex: bool) {
        use bricks_semantic::{CodeQuery, ContextPolicy, Detail, Intent, MatchMode, Requester};
        let engine = self.semantic_engine();
        let wd = self.agent().working_dir().to_path_buf();
        let q = self.inner.queue.clone();
        tokio::spawn(async move {
            let mut query = CodeQuery::text(text.clone())
                .intent(Intent::TextSearch)
                .context(ContextPolicy::None)
                .detail(Detail::Compact);
            if regex {
                query.mode = MatchMode::Regex;
            }
            query.limits.budget_tokens = Some(engine.config().max_budget_tokens);
            let r = engine
                .query(query, &Requester::new("frontend", &wd).without_lsp_start())
                .await;
            let hits = r
                .items
                .iter()
                .map(|i| {
                    let col = i
                        .line_text
                        .get(..i.range.start.col as usize)
                        .map(|p| p.chars().count())
                        .unwrap_or(0);
                    SearchHit {
                        path: i.path.clone(),
                        line: i.range.start.line + 1,
                        column: col as u32 + 1,
                        text: i.line_text.clone(),
                    }
                })
                .collect();
            let mut omitted = 0;
            let mut notes: Vec<String> = Vec::new();
            for o in &r.omissions {
                if let bricks_semantic::Omission::ItemsOmitted { count, .. } = o {
                    omitted += count;
                }
                notes.push(bricks_semantic::render::omission_text(o));
            }
            if let bricks_semantic::ResultStatus::Error { message } = &r.status {
                notes.push(message.clone());
            }
            let ev = Event::SearchResults {
                query: text,
                status: r.status.label().to_string(),
                hits,
                omitted,
                notes,
                elapsed_ms: r.metrics.elapsed_ms,
            };
            q.push(None, ev, None).await;
        });
    }

    fn set_activity(&self, a: Activity) {
        *self.inner.activity.lock() = a;
        self.inner.idle.notify_waiters();
    }

    /// Send a command. Refused commands return the reason (also emitted as
    /// `command_rejected`); accepted ones report through events.
    pub fn send(&self, cmd: Command) -> Result<(), String> {
        match cmd {
            Command::Cancel => {
                let activity = self.inner.activity.lock().clone();
                if matches!(activity, Activity::Running { .. }) {
                    self.agent().cancel();
                    return Ok(());
                }
                if let Some(t) = self.inner.maintenance.lock().as_ref() {
                    t.cancel();
                    return Ok(());
                }
                self.reject(&cmd, "nothing to cancel".into())
            }
            Command::Search { ref text, regex } => {
                if text.trim().is_empty() {
                    return self.reject(&cmd, "nothing to search for".into());
                }
                self.search(text.clone(), regex);
                Ok(())
            }
            Command::Approve {
                ref approval_id,
                decision,
                ref reason,
            } => match self
                .inner
                .broker
                .respond(approval_id, decision, reason.clone())
            {
                Ok(()) => Ok(()),
                Err(e) => self.reject(&cmd, e),
            },
            Command::SetModel {
                ref model,
                ref reasoning,
            } => {
                let (cur_model, cur_reasoning) = {
                    let m = self.inner.meta.lock();
                    (m.model.clone(), m.reasoning.clone())
                };
                let new_model = model.clone().unwrap_or(cur_model.clone());
                // A new model starts from its own default profile unless one
                // is named.
                let new_reasoning = match (model, reasoning) {
                    (_, Some(r)) => Some(r.clone()),
                    (Some(m), None) if *m != cur_model => None,
                    _ => cur_reasoning,
                };
                let provider = match self
                    .inner
                    .cfg
                    .catalog
                    .build(&new_model, new_reasoning.as_deref())
                {
                    Ok(p) => p,
                    Err(e) => return self.reject(&cmd, e),
                };
                let running = matches!(*self.inner.activity.lock(), Activity::Running { .. });
                self.agent()
                    .set_model(provider, Some(new_model.clone()), new_reasoning.clone());
                {
                    let mut m = self.inner.meta.lock();
                    m.model = new_model.clone();
                    m.reasoning = new_reasoning.clone();
                    m.touch();
                    let _ = m.save(&files_dir(&self.inner.cfg, &m.id));
                }
                let q = self.inner.queue.clone();
                let ev = Event::ModelChanged {
                    model: new_model,
                    reasoning: new_reasoning,
                    applies: if running { "next_turn" } else { "next_run" }.into(),
                };
                tokio::spawn(async move { q.push(None, ev, None).await });
                Ok(())
            }
            Command::Submit { ref prompt } => {
                let run_id = format!("run_{}", uuid::Uuid::new_v4().simple());
                {
                    let mut a = self.inner.activity.lock();
                    match &*a {
                        Activity::Idle => {}
                        Activity::Running { .. } => {
                            drop(a);
                            return self.reject(
                                &cmd,
                                "a run is in progress: wait for it or cancel it first".into(),
                            );
                        }
                        Activity::Busy { what } => {
                            let what = what.clone();
                            drop(a);
                            return self.reject(&cmd, format!("{what} is in progress"));
                        }
                    }
                    *a = Activity::Running {
                        run_id: run_id.clone(),
                    };
                }
                let wd = self.agent().working_dir().to_path_buf();
                let converted = match attach::convert(prompt, &wd, self.inner.cfg.attach_limits) {
                    Ok(c) => c,
                    Err(e) => {
                        self.set_activity(Activity::Idle);
                        return self.reject(&cmd, e);
                    }
                };
                let this = self.clone();
                tokio::spawn(async move { this.run(run_id, converted).await });
                Ok(())
            }
            Command::Compact | Command::ClearContext | Command::Resume { .. } => {
                let what = match &cmd {
                    Command::Compact => "a compaction",
                    Command::ClearContext => "clearing the context",
                    _ => "a resume",
                };
                {
                    let mut a = self.inner.activity.lock();
                    if *a != Activity::Idle {
                        let reason = match &*a {
                            Activity::Running { .. } => format!(
                                "{what} cannot happen during a run: wait for it or cancel it first"
                            ),
                            Activity::Busy { what: other } => format!("{other} is in progress"),
                            Activity::Idle => unreachable!(),
                        };
                        drop(a);
                        return self.reject(&cmd, reason);
                    }
                    *a = Activity::Busy {
                        what: what.to_string(),
                    };
                }
                let this = self.clone();
                tokio::spawn(async move {
                    this.session_command(cmd).await;
                    this.set_activity(Activity::Idle);
                });
                Ok(())
            }
        }
    }

    async fn session_command(&self, cmd: Command) {
        let q = self.inner.queue.clone();
        match cmd {
            Command::Compact => {
                let outcome = self.agent().compact().await;
                q.push(
                    None,
                    Event::Compaction {
                        reason: format!("{:?}", CompactReason::ManualTrigger),
                        compacted: outcome.is_compacted(),
                        outcome: outcome.to_string(),
                    },
                    None,
                )
                .await;
                q.push(
                    None,
                    Event::Context {
                        status: Box::new(self.agent().context_status()),
                    },
                    None,
                )
                .await;
            }
            Command::ClearContext => match self.agent().clear_context().await {
                Ok(n) => {
                    q.push(
                        None,
                        Event::ContextCleared {
                            messages_removed: n,
                        },
                        None,
                    )
                    .await
                }
                Err(e) => {
                    q.push(
                        None,
                        Event::CommandRejected {
                            command: "clear_context".into(),
                            reason: e.to_string(),
                        },
                        None,
                    )
                    .await
                }
            },
            Command::Resume { session_id } => {
                if let Err(e) = self.resume(&session_id).await {
                    q.push(
                        None,
                        Event::CommandRejected {
                            command: "resume".into(),
                            reason: e,
                        },
                        None,
                    )
                    .await;
                }
            }
            _ => {}
        }
    }

    async fn resume(&self, session_id: &str) -> Result<(), String> {
        let cfg = &self.inner.cfg;
        let Prepared {
            meta,
            provider,
            warnings,
            ..
        } = prepare(
            cfg,
            &SessionChoice::Resume(session_id.to_string()),
            None,
            None,
        )?;
        let agent = Arc::new(build_agent(cfg, &meta, provider, &self.inner.broker)?);
        let count = session_store(cfg)
            .load(&meta.id)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let old = std::mem::replace(&mut *self.inner.agent.write(), agent);
        old.close().await;
        self.inner.queue.set_session(&meta.id);
        let ev = Event::SessionOpened {
            working_dir: meta.working_dir.display().to_string(),
            model: meta.model.clone(),
            reasoning: meta.reasoning.clone(),
            resumed: true,
            message_count: count,
            warnings,
        };
        *self.inner.meta.lock() = meta;
        self.inner.queue.push(None, ev, None).await;
        Ok(())
    }

    async fn run(&self, run_id: String, converted: attach::Converted) {
        let agent = self.agent();
        let q = self.inner.queue.clone();
        let rid = Some(run_id.as_str());
        let prompt_text = converted.input.text.clone();
        {
            let mut m = self.inner.meta.lock();
            m.title_from(&prompt_text);
            m.touch();
            let _ = m.save(&files_dir(&self.inner.cfg, &m.id));
        }
        q.push(
            rid,
            Event::RunStarted {
                prompt: prompt_text,
                attachments: converted.attachments,
                model: agent.model_label(),
                reasoning: agent.reasoning_profile(),
            },
            None,
        )
        .await;

        let token = agent.begin_run();
        // Small: the run feels the consumer's pace through the bounded
        // queue instead of filling a large buffer first.
        let (tx, rx) = mpsc::channel::<AgentEvent>(16);
        self.inner.broker.begin_run(tx.clone(), token.clone());
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let translator = tokio::spawn(translate(
            rx,
            done_rx,
            q.clone(),
            run_id.clone(),
            agent.clone(),
            token.clone(),
        ));
        let result = crate::runner::run_prepared(&agent, &converted.input, tx, token.clone()).await;
        self.inner.broker.end_run();
        // Every event of the run is in the channel now (the agent keeps a
        // sender for tool progress, so the channel itself never closes).
        let _ = done_tx.send(());
        let (streamed, turns_done) = translator.await.unwrap_or_default();
        let unsatisfied = self.inner.broker.unsatisfied();

        let finished = match &result {
            Ok(out) => Event::RunFinished {
                outcome: if out.is_complete() {
                    RunOutcome::Succeeded
                } else {
                    RunOutcome::Incomplete
                },
                failure: None,
                error: (!out.is_complete()).then(|| out.termination.describe()),
                termination: Some(out.termination.clone()),
                text: out.text().to_string(),
                turns: out.turns,
                approvals_unsatisfied: Vec::new(),
            },
            Err(CerseiError::Cancelled) if !unsatisfied.is_empty() => Event::RunFinished {
                outcome: RunOutcome::Failed,
                failure: Some(FailureKind::ApprovalRequired),
                error: Some(format!(
                    "stopped: {} call(s) needed an approval and the run is non-interactive",
                    unsatisfied.len()
                )),
                termination: None,
                text: streamed,
                turns: turns_done,
                approvals_unsatisfied: unsatisfied,
            },
            Err(CerseiError::Cancelled) => Event::RunFinished {
                outcome: RunOutcome::Cancelled,
                failure: None,
                error: None,
                termination: None,
                text: streamed,
                turns: turns_done,
                approvals_unsatisfied: Vec::new(),
            },
            Err(e) => Event::RunFinished {
                outcome: RunOutcome::Failed,
                failure: Some(FailureKind::Error),
                error: Some(e.to_string()),
                termination: None,
                text: streamed,
                turns: turns_done,
                approvals_unsatisfied: Vec::new(),
            },
        };
        q.push(rid, finished, Some(&token)).await;
        {
            let mut m = self.inner.meta.lock();
            m.touch();
            let _ = m.save(&files_dir(&self.inner.cfg, &m.id));
        }

        let maintain = result.is_ok() && agent.has_long_term_memory();
        let maintenance = CancellationToken::new();
        if maintain {
            *self.inner.maintenance.lock() = Some(maintenance.clone());
        }
        // The answer is delivered: the next prompt may be submitted while
        // the memory is maintained.
        self.set_activity(Activity::Idle);
        if maintain {
            q.push(rid, Event::MemoryMaintenanceStarted, None).await;
            let ev = match agent.maintain_memory(&maintenance).await {
                Some(Ok(report)) => Event::MemoryMaintenanceFinished {
                    outcome: if report.cancelled {
                        MaintenanceOutcome::Cancelled
                    } else if report.failed > 0 || report.embedding_errors > 0 {
                        MaintenanceOutcome::Failed
                    } else {
                        MaintenanceOutcome::Completed
                    },
                    report: Some(report),
                    error: None,
                },
                Some(Err(e)) => Event::MemoryMaintenanceFinished {
                    outcome: MaintenanceOutcome::Failed,
                    report: None,
                    error: Some(e.to_string()),
                },
                None => Event::MemoryMaintenanceFinished {
                    outcome: MaintenanceOutcome::Completed,
                    report: None,
                    error: None,
                },
            };
            q.push(rid, ev, None).await;
            *self.inner.maintenance.lock() = None;
            self.inner.idle.notify_waiters();
        }
    }

    /// End the session: cancel what runs, close the agent's shells and MCP
    /// connections. The session itself stays stored.
    pub async fn close(&self) {
        self.agent().cancel();
        if let Some(t) = self.inner.maintenance.lock().as_ref() {
            t.cancel();
        }
        self.agent().close().await;
    }
}

/// Engine events → protocol events, in order. Returns the streamed answer
/// text (for a run that ends without a final message).
async fn translate(
    mut rx: mpsc::Receiver<AgentEvent>,
    mut done: tokio::sync::oneshot::Receiver<()>,
    q: Arc<EventQueue>,
    run_id: String,
    agent: Arc<Agent>,
    token: CancellationToken,
) -> (String, u32) {
    let rid = Some(run_id.as_str());
    let mut text = String::new();
    // Turns that got a response (reported when the run does not return).
    let mut turns = 0u32;
    let mut finished = false;
    loop {
        let e = if finished {
            match rx.try_recv() {
                Ok(e) => e,
                Err(_) => break,
            }
        } else {
            tokio::select! {
                biased;
                e = rx.recv() => match e {
                    Some(e) => e,
                    None => break,
                },
                _ = &mut done => {
                    finished = true;
                    continue;
                }
            }
        };
        let ev = match e {
            AgentEvent::TextDelta(t) => {
                text.push_str(&t);
                Event::TextDelta { text: t }
            }
            AgentEvent::ThinkingDelta(t) => Event::ThinkingDelta { text: t },
            AgentEvent::ToolStart { name, id, input } => {
                // A new answer follows the tools: the partial text is the
                // text after the last tool call.
                text.clear();
                Event::ToolStarted {
                    tool_call_id: id,
                    name,
                    input,
                }
            }
            AgentEvent::ToolProgress { name, message } => Event::ToolProgress {
                tool_call_id: None,
                name,
                message,
            },
            AgentEvent::ToolEnd {
                name,
                id,
                result,
                is_error,
                duration,
                ..
            } => Event::ToolFinished {
                tool_call_id: id,
                name,
                is_error,
                duration_ms: duration.as_millis() as u64,
                output: result,
            },
            AgentEvent::TurnComplete { usage, .. } => {
                turns += 1;
                Event::Usage {
                    turn: Box::new(usage),
                    total: Box::new(agent.usage()),
                }
            }
            AgentEvent::TokenWarning { pct_used, .. } => Event::Notice {
                message: format!("context {:.0}% full", pct_used * 100.0),
            },
            AgentEvent::CompactionResult { reason, outcome } => Event::Compaction {
                reason: format!("{reason:?}"),
                compacted: outcome.is_compacted(),
                outcome: outcome.to_string(),
            },
            AgentEvent::ContextUpdate(status) => Event::Context {
                status: Box::new(status),
            },
            AgentEvent::SessionSaved { .. } => Event::SessionSaved,
            AgentEvent::SubAgentSpawned { agent_id, .. } => Event::Notice {
                message: format!("sub-agent {agent_id} started"),
            },
            AgentEvent::SubAgentComplete { agent_id, .. } => Event::Notice {
                message: format!("sub-agent {agent_id} finished"),
            },
            AgentEvent::HookBlocked {
                hook_name, reason, ..
            } => Event::Notice {
                message: format!("hook {hook_name} blocked: {reason}"),
            },
            AgentEvent::Status(s) => Event::Notice { message: s },
            AgentEvent::MemoryRecalled {
                items,
                tokens,
                omitted,
                budget,
            } => Event::MemoryRecalled {
                items,
                tokens,
                omitted,
                budget,
            },
            AgentEvent::ApprovalRequested(r) => Event::ApprovalRequested { approval: r },
            AgentEvent::ApprovalResolved {
                approval_id,
                tool_call_id,
                decision,
                by,
            } => Event::ApprovalResolved {
                approval_id,
                tool_call_id,
                decision,
                by,
            },
            AgentEvent::EditApplied {
                tool_call_id,
                tool,
                files,
            } => Event::EditApplied {
                tool_call_id,
                tool,
                files: files
                    .into_iter()
                    .map(|f| WrittenFile {
                        path: f.path,
                        kind: f.kind,
                        added: f.added,
                        removed: f.removed,
                    })
                    .collect(),
            },
            // Internal or reported otherwise (`run_finished`, `usage`).
            AgentEvent::ToolPermissionCheck { .. }
            | AgentEvent::PermissionRequired(_)
            | AgentEvent::TurnStart { .. }
            | AgentEvent::ModelRequestStart { .. }
            | AgentEvent::ModelResponseStart { .. }
            | AgentEvent::CompactStart { .. }
            | AgentEvent::CompactEnd { .. }
            | AgentEvent::SessionLoaded { .. }
            | AgentEvent::CostUpdate { .. }
            | AgentEvent::HookFired { .. }
            | AgentEvent::MemoryMaintenanceStarted
            | AgentEvent::MemoryMaintenanceFinished(_)
            | AgentEvent::Error(_)
            | AgentEvent::Complete(_) => continue,
        };
        q.push(rid, ev, Some(&token)).await;
    }
    (text, turns)
}
