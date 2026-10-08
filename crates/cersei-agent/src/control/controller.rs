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
use super::session::{SessionMeta, SessionScope, SessionSummary};
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
    /// Where sub-agent profiles are read (default: the session's
    /// `.bricks/agents` and `~/.bricks/agents`).
    pub agent_profile_sources: Option<crate::agents::ProfileSources>,
    /// Reads the project of the session being opened (its own
    /// `bricks.toml`, system prompt, memory...). Without one, the values
    /// above apply to every session, whatever its folder.
    pub project_loader: Option<Arc<dyn ProjectLoader>>,
}

/// What belongs to a session's project (its workspace): read for the
/// folder of the session actually opened — a new one, or a resumed one with
/// its recorded folder — never inherited from the folder Bricks started in.
#[derive(Clone)]
pub struct ProjectContext {
    /// Canonical folder of the project.
    pub working_dir: PathBuf,
    pub bricks: BricksConfig,
    pub system_prompt: Option<String>,
    pub long_term_memory: Option<Arc<dyn LongTermMemory>>,
    pub memory_space: Option<String>,
    pub mcp_servers: Vec<cersei_mcp::McpServerConfig>,
    pub agent_profile_sources: Option<crate::agents::ProfileSources>,
}

/// Loads a project's context from its folder, with the frontend's loaders.
/// Errors are complete messages; a failed load opens nothing.
pub trait ProjectLoader: Send + Sync {
    fn load(&self, working_dir: &Path) -> Result<ProjectContext, String>;
}

/// The project of `working_dir`: from the loader, or the configuration's
/// own values (an embedder that injects them).
fn load_project(cfg: &EngineConfig, working_dir: &Path) -> Result<ProjectContext, String> {
    if let Some(l) = &cfg.project_loader {
        return l.load(working_dir);
    }
    Ok(ProjectContext {
        working_dir: working_dir.to_path_buf(),
        bricks: cfg.bricks.clone(),
        system_prompt: cfg.system_prompt.clone(),
        long_term_memory: cfg.long_term_memory.clone(),
        memory_space: cfg.memory_space.clone(),
        mcp_servers: cfg.mcp_servers.clone(),
        agent_profile_sources: cfg.agent_profile_sources.clone(),
    })
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
            agent_profile_sources: None,
            project_loader: None,
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
    /// The open session's project (config, prompt, memory, profiles).
    project: parking_lot::RwLock<Arc<ProjectContext>>,
    agent: parking_lot::RwLock<Arc<Agent>>,
    meta: parking_lot::Mutex<SessionMeta>,
    queue: Arc<EventQueue>,
    broker: Arc<ApprovalBroker>,
    activity: parking_lot::Mutex<Activity>,
    maintenance: parking_lot::Mutex<Option<CancellationToken>>,
    idle: tokio::sync::Notify,
    /// Sub-agent profiles of the session's workspace.
    profiles: parking_lot::RwLock<Arc<crate::agents::ProfileCatalog>>,
    /// The session's sub-agent runtime (when `[agents] enabled`).
    spawner: parking_lot::RwLock<Option<Arc<crate::agents::AgentSpawner>>>,
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

/// Stored sessions of `scope`, most recently updated first. Reading only:
/// nothing is created, moved or attributed.
pub async fn list_sessions_in(
    sessions_dir: &Path,
    scope: &SessionScope,
) -> Result<Vec<SessionSummary>, String> {
    let mut all = list_sessions(sessions_dir).await?;
    all.retain(|s| scope.includes(s));
    Ok(all)
}

/// Every stored session, most recently updated first.
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

fn profile_catalog(
    project: &ProjectContext,
    meta: &SessionMeta,
) -> Arc<crate::agents::ProfileCatalog> {
    let sources = project
        .agent_profile_sources
        .clone()
        .unwrap_or_else(|| crate::agents::ProfileSources::standard(&meta.working_dir));
    Arc::new(crate::agents::ProfileCatalog::new(sources))
}

/// The session agent's tools: the configured ones, plus the native `Agent`
/// and `AgentProfiles` tools when `[agents] enabled` (never a second
/// `Agent` next to one already configured).
fn session_tools(
    cfg: &EngineConfig,
    project: &ProjectContext,
    meta: &SessionMeta,
    profiles: &Arc<crate::agents::ProfileCatalog>,
) -> (Vec<Box<dyn Tool>>, Option<Arc<crate::agents::AgentSpawner>>) {
    let mut tools = (cfg.tools)();
    let mut spawner_out = None;
    if project.bricks.agents.enabled && !tools.iter().any(|t| t.name() == "Agent") {
        let factory = Arc::clone(&cfg.tools);
        let spawner = Arc::new(
            crate::agents::AgentSpawner::new(
                Arc::clone(&cfg.catalog),
                Arc::clone(profiles),
                Arc::new(move || factory()),
                project.bricks.clone(),
            )
            .with_artifacts_dir(files_dir(cfg, &meta.id).join("agents"))
            .with_session_id(meta.id.clone()),
        );
        tools.push(Box::new(crate::agents::NativeAgentTool::new(Arc::clone(
            &spawner,
        ))));
        tools.push(Box::new(crate::agents::AgentsTool::new(Arc::clone(
            &spawner,
        ))));
        tools.push(Box::new(crate::agents::AgentControlTool::new(Arc::clone(
            &spawner,
        ))));
        tools.push(Box::new(crate::agents::AgentProfilesTool::new(Arc::clone(
            &spawner,
        ))));
        spawner_out = Some(spawner);
    }
    (tools, spawner_out)
}

fn build_agent(
    cfg: &EngineConfig,
    project: &ProjectContext,
    meta: &SessionMeta,
    provider: Box<dyn Provider>,
    broker: &Arc<ApprovalBroker>,
    profiles: &Arc<crate::agents::ProfileCatalog>,
) -> Result<(Agent, Option<Arc<crate::agents::AgentSpawner>>), String> {
    let settings = &project.bricks.agent;
    let (tools, spawner) = session_tools(cfg, project, meta, profiles);
    let ext = cersei_tools::Extensions::default();
    // The session's background jobs (their logs kept with the session).
    let jobs = cersei_tools::jobs::JobRegistry::new(
        project.bricks.background.clone(),
        Some(files_dir(cfg, &meta.id).join("jobs")),
    );
    ext.insert(cersei_tools::jobs::JobsHandle(jobs));
    if let Some(sp) = &spawner {
        // The session agent records its usage in its root runs' ledger and
        // sees the session's sub-agents.
        ext.insert(crate::agents::RuntimeHandle(Arc::clone(sp.runtime())));
    }
    let mut b = Agent::builder()
        .provider_boxed(provider)
        .tools(tools)
        .extensions(ext)
        .working_dir(&meta.working_dir)
        .model(meta.model.clone())
        .permission_policy(ApprovalGate::new(
            project.bricks.permissions.clone(),
            broker.clone(),
        ))
        .memory(session_store(cfg))
        .session_id(meta.id.clone())
        .bricks_config(project.bricks.clone())
        .max_turns(settings.max_turns.unwrap_or(50));
    if let Some(r) = &meta.reasoning {
        b = b.reasoning_profile(r.clone());
    }
    if let Some(t) = settings.max_tokens {
        b = b.max_tokens(t);
    }
    if let Some(s) = &project.system_prompt {
        b = b.system_prompt(s.clone());
    }
    if let Some(m) = &project.long_term_memory {
        b = b.long_term_memory(m.clone());
    }
    for s in &project.mcp_servers {
        b = b.mcp_server(s.clone());
    }
    let agent = b.build().map_err(|e| e.to_string())?;
    Ok((agent, spawner))
}

/// A session ready to open.
struct Prepared {
    meta: SessionMeta,
    provider: Box<dyn Provider>,
    project: ProjectContext,
    resumed: bool,
    warnings: Vec<String>,
}

/// Resolve the session to open: first the session itself (its metadata,
/// its recorded folder), then the project of that folder, then the
/// provider. Nothing is opened, saved or published here: an error leaves
/// everything as it was. `base_dir`: the folder of a new session, and of
/// an older session that recorded none.
fn prepare(
    cfg: &EngineConfig,
    base_dir: &Path,
    session: &SessionChoice,
    model: Option<&str>,
    reasoning: Option<&str>,
) -> Result<Prepared, String> {
    let mut warnings = Vec::new();
    match session {
        SessionChoice::New => {
            let project = load_project(cfg, base_dir)?;
            let settings = &project.bricks.agent;
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
            let mut meta = SessionMeta::new(&new_session_id(), base_dir, &model, reasoning);
            meta.memory_space = project.memory_space.clone();
            Ok(Prepared {
                meta,
                provider,
                project,
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
            let recorded = SessionMeta::load(&files_dir(cfg, id))?;
            // The session's own folder decides its project; an older
            // session without one resumes in `base_dir`, said explicitly.
            let dir = match &recorded {
                Some(m) => {
                    if !m.working_dir.is_dir() {
                        return Err(format!(
                            "the session's working directory {} no longer exists; nothing was \
                             resumed",
                            m.working_dir.display()
                        ));
                    }
                    std::fs::canonicalize(&m.working_dir).unwrap_or_else(|_| m.working_dir.clone())
                }
                None => base_dir.to_path_buf(),
            };
            let project = load_project(cfg, &dir)?;
            let settings = &project.bricks.agent;
            let mut meta = match recorded {
                Some(m) => m,
                None => {
                    let model = model
                        .map(str::to_string)
                        .or_else(|| settings.model.clone())
                        .ok_or_else(|| {
                            format!("pass --model to resume this session ({})", model_list(cfg))
                        })?;
                    warnings.push(format!(
                        "this session has no stored settings (older session): it resumes in {} \
                         with the model `{model}`",
                        dir.display()
                    ));
                    SessionMeta::new(id, &dir, &model, None)
                }
            };
            if meta.working_dir != base_dir {
                warnings.push(format!(
                    "the session works in {}, not in the current directory; that project's \
                     settings apply",
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
            if meta.memory_space.is_some() && meta.memory_space != project.memory_space {
                warnings.push(format!(
                    "the session used the long-term memory space `{}`; its project's is now {}",
                    meta.memory_space.clone().unwrap_or_default(),
                    project
                        .memory_space
                        .as_deref()
                        .map(|s| format!("`{s}`"))
                        .unwrap_or_else(|| "none".into())
                ));
            }
            let provider = cfg.catalog.build(&meta.model, meta.reasoning.as_deref())?;
            Ok(Prepared {
                meta,
                provider,
                project,
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
            project,
            resumed,
            warnings,
        } = prepare(
            &cfg,
            &cfg.working_dir,
            &opts.session,
            opts.model.as_deref(),
            opts.reasoning.as_deref(),
        )?;
        let broker = ApprovalBroker::new(cfg.interactive);
        let profiles = profile_catalog(&project, &meta);
        let (agent, spawner) = build_agent(&cfg, &project, &meta, provider, &broker, &profiles)?;
        let agent = Arc::new(agent);
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
        let mut all_warnings = project.bricks.diagnostics.clone();
        all_warnings.extend(warnings);
        all_warnings.extend(
            profiles
                .snapshot()
                .diagnostics
                .iter()
                .map(|d| format!("{}: {}", d.source, d.message)),
        );
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
            project: parking_lot::RwLock::new(Arc::new(project)),
            agent: parking_lot::RwLock::new(agent),
            meta: parking_lot::Mutex::new(meta),
            queue: queue.clone(),
            broker,
            activity: parking_lot::Mutex::new(Activity::Idle),
            maintenance: parking_lot::Mutex::new(None),
            idle: tokio::sync::Notify::new(),
            profiles: parking_lot::RwLock::new(profiles),
            spawner: parking_lot::RwLock::new(spawner.clone()),
        });
        if let Some(sp) = &spawner {
            install_session_sink(&queue, sp.runtime());
        }
        install_job_sink(&queue, &inner.agent.read());
        queue.push(None, opened, None).await;
        Ok((Controller { inner }, EventStream { queue }))
    }

    /// The open session's project.
    pub fn project(&self) -> Arc<ProjectContext> {
        self.inner.project.read().clone()
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
                .project()
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

    /// Every stored session.
    pub async fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        list_sessions(&self.inner.cfg.sessions_dir).await
    }

    /// The stored sessions of `scope`.
    pub async fn sessions_in(&self, scope: &SessionScope) -> Result<Vec<SessionSummary>, String> {
        list_sessions_in(&self.inner.cfg.sessions_dir, scope).await
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
            approval_rules: self.project().bricks.permissions.clone(),
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

    /// A person's action on the session's sub-agents (the frontend's
    /// equivalent of `AgentControl`; the session's tree is the destination
    /// of `apply_changes`).
    async fn agent_control(
        &self,
        sp: &Arc<crate::agents::AgentSpawner>,
        action: &str,
        agent_id: Option<String>,
        changeset_id: Option<String>,
    ) -> (bool, String) {
        let rt = sp.runtime();
        let wd = self.agent().working_dir().to_path_buf();
        let rec = |id: &Option<String>| id.as_deref().and_then(|i| rt.record(i));
        match action {
            "list" => {
                let recs = rt.records();
                if recs.is_empty() {
                    return (true, "No sub-agent in this session.".into());
                }
                (
                    true,
                    recs.iter()
                        .map(crate::agents::tool::render_record)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
            "status" => match rec(&agent_id) {
                Some(r) => (true, crate::agents::tool::render_record(&r)),
                None => (false, "unknown sub-agent".into()),
            },
            "result" => match rec(&agent_id) {
                Some(r) => match &r.result {
                    Some(res) => (true, crate::agents::render_result(res)),
                    None => (true, format!("no result yet ({})", r.state.as_str())),
                },
                None => (false, "unknown sub-agent".into()),
            },
            "cancel" => match rec(&agent_id) {
                Some(r) => {
                    let n = rt.cancel(&r.info.agent_id);
                    (true, format!("cancellation requested ({n} running)"))
                }
                None => (false, "unknown sub-agent".into()),
            },
            "inspect_changes" | "apply_changes" | "discard_changes" => {
                let cs = changeset_id.or_else(|| {
                    agent_id
                        .as_deref()
                        .and_then(|a| rt.workspaces.changeset_of(a))
                        .map(|c| c.id)
                });
                let Some(cs) = cs else {
                    return (false, "no ChangeSet given or found".into());
                };
                match action {
                    "inspect_changes" => match rt.workspaces.patch_text(&cs, 200_000) {
                        Ok((p, cut)) => (true, if cut { format!("{p}\n[shortened]") } else { p }),
                        Err(e) => (false, e.to_string()),
                    },
                    "apply_changes" => match rt.workspaces.apply(&cs, &wd).await {
                        Ok(done) => {
                            let files: Vec<String> =
                                done.files.iter().map(|f| f.path.clone()).collect();
                            rt.emit(AgentEvent::SubAgent(
                                crate::agents::SubAgentEvent::ChangesUpdated {
                                    changeset_id: cs.clone(),
                                    state: crate::agents::ChangeSetState::Applied,
                                    files: files.clone(),
                                    detail: None,
                                },
                            ));
                            (true, format!("applied: {}", files.join(", ")))
                        }
                        Err(e) => {
                            if let crate::agents::workspace::WsError::Conflict { files, detail } =
                                &e
                            {
                                rt.emit(AgentEvent::SubAgent(
                                    crate::agents::SubAgentEvent::ChangesUpdated {
                                        changeset_id: cs.clone(),
                                        state: crate::agents::ChangeSetState::Conflict,
                                        files: files.clone(),
                                        detail: Some(detail.clone()),
                                    },
                                ));
                            }
                            (false, e.to_string())
                        }
                    },
                    _ => match rt.workspaces.discard(&cs).await {
                        Ok(_) => {
                            rt.emit(AgentEvent::SubAgent(
                                crate::agents::SubAgentEvent::ChangesUpdated {
                                    changeset_id: cs.clone(),
                                    state: crate::agents::ChangeSetState::Discarded,
                                    files: Vec::new(),
                                    detail: None,
                                },
                            ));
                            (true, "discarded".into())
                        }
                        Err(e) => (false, e.to_string()),
                    },
                }
            }
            other => (false, format!("unknown action `{other}`")),
        }
    }

    /// A person's view of the session's jobs (all of them) and stop.
    async fn job_control(&self, action: &str, job_id: Option<String>) -> (bool, String) {
        let Some(jobs) = self
            .agent()
            .extensions
            .get::<cersei_tools::jobs::JobsHandle>()
        else {
            return (false, "no job registry".into());
        };
        match action {
            "jobs" => {
                let list = jobs.0.list(None);
                if list.is_empty() {
                    return (true, "No background job in this session.".into());
                }
                (
                    true,
                    list.iter()
                        .map(|(id, owner, st)| {
                            format!(
                                "{id} [{}] pid {} · {} · {} · {}",
                                st.state.label(),
                                st.pid,
                                owner.agent_id,
                                cersei_types::duration::display_ms(st.elapsed),
                                st.command
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
            _ => match job_id {
                Some(id) => match jobs.0.stop(&id, None).await {
                    Some(st) => (true, format!("{id}: {}", st.label())),
                    None => (false, format!("no job `{id}`")),
                },
                None => (false, "`stop_job` needs `job_id`".into()),
            },
        }
    }

    /// The session's sub-agent runtime.
    pub fn agent_runtime(&self) -> Option<Arc<crate::agents::AgentRuntime>> {
        self.inner
            .spawner
            .read()
            .as_ref()
            .map(|s| Arc::clone(s.runtime()))
    }

    fn emit_profiles(&self, reg: &crate::agents::ProfileRegistry, query: &str, page: usize) {
        let (profiles, total) = reg.search(query, page, 20);
        let ev = Event::AgentProfiles {
            profiles,
            total,
            page,
            diagnostics: reg.diagnostics.clone(),
        };
        let q = self.inner.queue.clone();
        tokio::spawn(async move { q.push(None, ev, None).await });
    }

    /// The workspace's shared engine (the one the agents' CodeScout uses).
    pub fn semantic_engine(&self) -> Arc<bricks_semantic::SemanticEngine> {
        let wd = self.agent().working_dir().to_path_buf();
        bricks_semantic::SemanticRegistry::global().engine_for(&wd, &self.project().bricks.semantic)
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
                // Idle, with background sub-agents still running: stop them.
                let sp = self.inner.spawner.read().clone();
                if let Some(sp) = sp {
                    let pending = sp.runtime().pending(None);
                    if !pending.is_empty() {
                        for r in pending {
                            sp.runtime().cancel(&r.info.agent_id);
                        }
                        return Ok(());
                    }
                }
                self.reject(&cmd, "nothing to cancel".into())
            }
            Command::ListAgentProfiles { ref query, page } => {
                let reg = self.inner.profiles.read().snapshot();
                self.emit_profiles(&reg, query, page);
                Ok(())
            }
            Command::AgentControl {
                ref action,
                ref agent_id,
                ref changeset_id,
                ref job_id,
            } => {
                if action == "jobs" || action == "stop_job" {
                    let this = self.clone();
                    let (action, job_id) = (action.clone(), job_id.clone());
                    tokio::spawn(async move {
                        let (ok, text) = this.job_control(&action, job_id).await;
                        let ev = Event::AgentControlResult { action, ok, text };
                        this.inner.queue.push(None, ev, None).await;
                    });
                    return Ok(());
                }
                let sp = self.inner.spawner.read().clone();
                let Some(sp) = sp else {
                    return self.reject(
                        &cmd,
                        "sub-agents are disabled ([agents] enabled = false)".into(),
                    );
                };
                let this = self.clone();
                let (action, agent_id, changeset_id) =
                    (action.clone(), agent_id.clone(), changeset_id.clone());
                tokio::spawn(async move {
                    let (ok, text) = this
                        .agent_control(&sp, &action, agent_id, changeset_id)
                        .await;
                    let ev = Event::AgentControlResult { action, ok, text };
                    this.inner.queue.push(None, ev, None).await;
                });
                Ok(())
            }
            Command::ReloadAgentProfiles => {
                let reg = self.inner.profiles.read().reload();
                self.emit_profiles(&reg, "", 0);
                Ok(())
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

    /// Open another stored session in place of this one. Everything the
    /// target needs (its project, provider, agent) is prepared first; on
    /// error the current session stays as it was.
    async fn resume(&self, session_id: &str) -> Result<(), String> {
        let cfg = &self.inner.cfg;
        let base = self.project().working_dir.clone();
        let Prepared {
            meta,
            provider,
            project,
            warnings,
            ..
        } = prepare(
            cfg,
            &base,
            &SessionChoice::Resume(session_id.to_string()),
            None,
            None,
        )?;
        let profiles = profile_catalog(&project, &meta);
        let (agent, spawner) = build_agent(
            cfg,
            &project,
            &meta,
            provider,
            &self.inner.broker,
            &profiles,
        )?;
        let agent = Arc::new(agent);
        meta.save(&files_dir(cfg, &meta.id))?;
        // From here the target replaces the current session. Approvals
        // "for the session" belonged to the previous one.
        self.inner.broker.reset_session();
        *self.inner.profiles.write() = profiles;
        let old_spawner = std::mem::replace(&mut *self.inner.spawner.write(), spawner.clone());
        if let Some(old) = old_spawner {
            old.runtime()
                .shutdown(std::time::Duration::from_secs(10))
                .await;
        }
        if let Some(sp) = &spawner {
            install_session_sink(&self.inner.queue, sp.runtime());
        }
        install_job_sink(&self.inner.queue, &agent);
        let count = session_store(cfg)
            .load(&meta.id)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let old = std::mem::replace(&mut *self.inner.agent.write(), agent);
        // The previous session's background jobs end with it: none is left
        // running without an owner, and none reports as the new session.
        if let Some(j) = old.extensions.get::<cersei_tools::jobs::JobsHandle>() {
            j.0.set_sink(Arc::new(|_| {}));
            j.0.stop_all().await;
        }
        old.close().await;
        let mut all_warnings = project.bricks.diagnostics.clone();
        all_warnings.extend(warnings);
        all_warnings.extend(
            self.inner
                .profiles
                .read()
                .snapshot()
                .diagnostics
                .iter()
                .map(|d| format!("{}: {}", d.source, d.message)),
        );
        *self.inner.project.write() = Arc::new(project);
        self.inner.queue.set_session(&meta.id);
        let ev = Event::SessionOpened {
            working_dir: meta.working_dir.display().to_string(),
            model: meta.model.clone(),
            reasoning: meta.reasoning.clone(),
            resumed: true,
            message_count: count,
            warnings: all_warnings,
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
        // The run's usage with its descendants, just before `run_finished`
        // (a consumer that stops there still gets it): partial while
        // background sub-agents still run, final again when the last ends.
        let sp = self.inner.spawner.read().clone();
        if let (Some(sp), Some(id)) = (sp, agent.extensions.get::<crate::agents::AgentIdentity>()) {
            if matches!(&result, Err(CerseiError::Cancelled)) {
                sp.runtime().cancel_root(&id.root_run_id);
            }
            if let AgentEvent::SubAgent(ev) = sp.runtime().root_finished(&id.root_run_id) {
                q.push(rid, sub_agent_event(ev), Some(&token)).await;
            }
        }
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
        if let Some(j) = self
            .agent()
            .extensions
            .get::<cersei_tools::jobs::JobsHandle>()
        {
            j.0.stop_all().await;
        }
        let sp = self.inner.spawner.read().clone();
        if let Some(sp) = sp {
            sp.runtime()
                .shutdown(std::time::Duration::from_secs(10))
                .await;
        }
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
            AgentEvent::SubAgent(e) => sub_agent_event(e),
            AgentEvent::Job(e) => job_event(e),
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

/// A sub-agent runtime event, as a protocol event.
pub(crate) fn sub_agent_event(e: crate::agents::SubAgentEvent) -> Event {
    use crate::agents::SubAgentEvent as S;
    match e {
        S::Spawned(info) => Event::AgentSpawned { agent: info },
        S::State {
            agent_id,
            state,
            reason,
        } => Event::AgentState {
            agent_id,
            state,
            reason,
        },
        S::ToolStarted {
            agent_id,
            tool_call_id,
            name,
            input,
        } => Event::AgentToolStarted {
            agent_id,
            tool_call_id,
            name,
            input,
        },
        S::ToolFinished {
            agent_id,
            tool_call_id,
            name,
            is_error,
            duration_ms,
        } => Event::AgentToolFinished {
            agent_id,
            tool_call_id,
            name,
            is_error,
            duration_ms,
        },
        S::Finished(result) => Event::AgentFinished { result },
        S::ChangesReady(changeset) => Event::ChangesReady { changeset },
        S::ChangesUpdated {
            changeset_id,
            state,
            files,
            detail,
        } => Event::ChangesUpdated {
            changeset_id,
            state,
            files,
            detail,
        },
        S::RunUsage {
            root_run_id,
            own,
            descendants,
            total,
            pending_agents,
            final_total,
        } => Event::RunUsage {
            root_run_id,
            own,
            descendants,
            total,
            pending_agents,
            final_total,
        },
    }
}

/// Events of the runtime outside runs (background sub-agents, final
/// totals) reach the queue in order, through one forwarder.
fn install_session_sink(q: &Arc<EventQueue>, rt: &Arc<crate::agents::AgentRuntime>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
    let q = q.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let AgentEvent::SubAgent(e) = ev {
                q.push(None, sub_agent_event(e), None).await;
            }
        }
    });
    rt.set_session_sink(Arc::new(move |ev| {
        let _ = tx.send(ev);
    }));
}

/// Job events reach the queue in order, outside runs.
fn install_job_sink(q: &Arc<EventQueue>, agent: &Agent) {
    let Some(jobs) = agent.extensions.get::<cersei_tools::jobs::JobsHandle>() else {
        return;
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<cersei_tools::jobs::JobEvent>();
    let q = q.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            q.push(None, job_event(ev), None).await;
        }
    });
    jobs.0.set_sink(Arc::new(move |ev| {
        let _ = tx.send(ev);
    }));
}

pub(crate) fn job_event(e: cersei_tools::jobs::JobEvent) -> Event {
    use cersei_tools::jobs::JobEvent as J;
    match e {
        J::Started {
            job_id,
            agent_id,
            root_run_id,
            command,
            cwd,
            pid,
        } => Event::JobStarted {
            job_id,
            agent_id,
            root_run_id,
            command,
            cwd,
            pid,
        },
        J::Output {
            job_id,
            stdout_bytes,
            stderr_bytes,
        } => Event::JobOutput {
            job_id,
            stdout_bytes,
            stderr_bytes,
        },
        J::Finished {
            job_id,
            state,
            code,
            signal,
            duration_ms,
            logs,
        } => Event::JobFinished {
            job_id,
            state,
            code,
            signal,
            duration_ms,
            logs,
        },
    }
}
