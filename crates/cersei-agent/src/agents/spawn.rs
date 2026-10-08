//! The shared spawner: validates a request, resolves profile, model and
//! reasoning, admits the child into the workspace, runs it in a fresh
//! context with its parent's policy and tools, and returns a compact
//! result.
//!
//! Everything that can be refused is refused before a provider, a shell or a
//! request exists. A child is a regular [`Agent`] running the regular loop:
//! there is no second agentic loop.

use super::admission::writers_for;
use super::profile::{
    AgentProfile, Isolation, ModelPref, ProfileScope, ProfileSource, ReasoningPref,
};
use super::registry::ProfileCatalog;
use crate::control::{ModelCatalog, ModelChoice};
use crate::events::AgentEvent;
use crate::subagent;
use crate::{Agent, AgentOutput, BricksConfig, Termination};
use cersei_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use cersei_tools::{Tool, ToolContext};
use cersei_types::Usage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

// ─── Settings ────────────────────────────────────────────────────────────────

/// `[agents]` in `bricks.toml`: the runtime's own limits (never a
/// profile's business rule).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DelegationSettings {
    /// Register the `Agent` tool in the agents of the CLI and controller.
    pub enabled: bool,
    /// The model `model: auto` means. Without it, `auto` inherits.
    pub auto_model: Option<String>,
    /// A profile's reasoning preference → an id of the model's catalogue
    /// (`high = "deep"`), used only when the model has no profile with the
    /// preferred id itself.
    pub reasoning_aliases: BTreeMap<String, String>,
    /// Largest explicit `context` a request may pass, in characters.
    pub max_context_chars: usize,
    /// Profiles described in the tool's schema (the rest: `AgentProfiles`).
    pub catalog_size: usize,
    /// Characters of the child's answer returned to the parent (the full
    /// transcript stays on disk).
    pub summary_chars: usize,
    /// Descendants active at once in the session (slots).
    pub max_concurrent: usize,
    /// Deepest generation: the session's agent is 0, its children 1.
    pub max_depth: u32,
    /// Descendants admitted per root run, cumulatively.
    pub max_total_per_run: u32,
    /// `auto` (shared for one foreground child, a worktree for background
    /// or parallel children), `shared` or `worktree`.
    pub default_isolation: String,
    /// Children per `Agents` call.
    pub max_batch: usize,
    /// Children waiting for a slot at most.
    pub max_queued: usize,
    /// How long a child may wait for a slot.
    pub admission_timeout_ms: u64,
    /// Headless: how long to wait for background children after the
    /// answer, before cancelling them.
    pub background_drain_ms: u64,
    /// Bytes of profile skills loaded into a child, in total.
    pub skills_max_bytes: usize,
}

impl Default for DelegationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_model: None,
            reasoning_aliases: BTreeMap::new(),
            max_context_chars: 16_000,
            catalog_size: 30,
            summary_chars: 4_000,
            max_concurrent: 8,
            max_depth: 2,
            max_total_per_run: 32,
            default_isolation: "auto".into(),
            max_batch: 8,
            max_queued: 32,
            admission_timeout_ms: 600_000,
            background_drain_ms: 600_000,
            skills_max_bytes: 24_000,
        }
    }
}

// ─── Context the runner provides ─────────────────────────────────────────────

/// Who an agent is: put in its tool context by the runner (top level) or
/// the spawner (children).
#[derive(Debug, Clone)]
pub struct AgentIdentity {
    pub agent_id: String,
    pub parent_id: Option<String>,
    /// The top-level run this agent works for.
    pub root_run_id: String,
}

/// The parent's model and reasoning profile, as of the current run.
#[derive(Debug, Clone)]
pub struct ParentModel {
    pub selection: String,
    pub reasoning: Option<String>,
}

/// Where a parent's run reports its children's lifecycle.
#[derive(Clone)]
pub struct SubAgentSink(pub Arc<dyn Fn(AgentEvent) + Send + Sync>);

// ─── Requests, instances, results ────────────────────────────────────────────

/// A delegation. Only `task` is required.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSpawnRequest {
    /// The complete, self-contained task. `prompt` is accepted as an alias
    /// (the former `Agent` tool's field).
    #[serde(alias = "prompt")]
    pub task: String,
    /// A short label (3–5 words) for displays.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    /// `inherit`, `auto` or `provider_id/model_id`.
    #[serde(default)]
    pub model: Option<String>,
    /// `inherit` or a reasoning profile id of the chosen model.
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Extra context passed explicitly (bounded).
    #[serde(default)]
    pub context: Option<String>,
    /// Former turn limit (removed in 0.4.8). Never used: a request that
    /// still sends it is refused with a migration message.
    #[doc(hidden)]
    #[serde(default, rename = "max_turns", skip_serializing)]
    pub legacy_max_turns: Option<serde_json::Value>,
    /// Run in the background (a handle is returned at once).
    #[serde(default)]
    pub background: Option<bool>,
    /// `auto` (default), `shared` or `worktree`.
    #[serde(default)]
    pub isolation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    Created,
    /// Waiting for a free slot.
    Queued,
    /// Waiting for the shared workspace's writer lease.
    WaitingAdmission,
    Starting,
    Running,
    Cancelling,
    Completed,
    /// Stopped at a limit before a final answer: not completed.
    Incomplete,
    Failed,
    Cancelled,
    /// The process that ran it ended first (found in the manifest).
    Interrupted,
}

impl InstanceState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Incomplete | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Queued => "queued",
            Self::WaitingAdmission => "waiting_admission",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

/// What was chosen and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub requested: String,
    pub applied: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A started child, as frontends see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnInfo {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub root_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub profile: String,
    pub profile_source: String,
    pub profile_revision: String,
    pub model: Choice,
    pub reasoning: Choice,
    pub workspace: String,
    /// `shared` in this phase.
    pub isolation: String,
    /// The task, shortened for displays.
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// UTC time of creation (RFC 3339), for records only.
    pub created_at: String,
    /// Position in an `Agents` batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_index: Option<usize>,
    #[serde(default)]
    pub background: bool,
    /// 1 for a child of the session's agent.
    #[serde(default)]
    pub depth: u32,
    /// Its worktree's branch (isolated children).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The profile's skills as loaded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillLoad>,
}

/// A command the child actually ran, and how it ended (evidence, not a
/// claim of the model).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRun {
    pub tool: String,
    pub command: String,
    pub is_error: bool,
    pub duration_ms: u64,
}

/// The compact result of a child.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentResult {
    pub agent_id: String,
    pub profile: String,
    /// `completed`, `incomplete`, `failed` or `cancelled`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination: Option<Termination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The child's final (or last partial) answer, bounded.
    pub summary: String,
    pub summary_truncated: bool,
    /// Files written by approved, applied changes.
    pub files_changed: Vec<String>,
    /// Shell commands run, with their outcome.
    pub commands: Vec<CommandRun>,
    pub warnings: Vec<String>,
    pub turns: u32,
    pub tool_calls: usize,
    pub usage: Usage,
    pub duration_ms: u64,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub workspace: String,
    /// The child's full conversation (JSON), when stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// Its ChangeSet (isolated children that changed files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changeset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The profile's skills as loaded (known once the child is built).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillLoad>,
}

impl AgentResult {
    /// A result for a child that never ran (or whose task was lost).
    pub fn lost(info: &SpawnInfo) -> Self {
        Self {
            agent_id: info.agent_id.clone(),
            profile: info.profile.clone(),
            status: "failed".into(),
            termination: None,
            error: Some("the sub-agent's task ended without a result".into()),
            summary: String::new(),
            summary_truncated: false,
            files_changed: Vec::new(),
            commands: Vec::new(),
            warnings: Vec::new(),
            turns: 0,
            tool_calls: 0,
            usage: Usage::default(),
            duration_ms: 0,
            model: info.model.applied.clone(),
            reasoning: None,
            workspace: info.workspace.clone(),
            transcript: None,
            changeset: None,
            branch: info.branch.clone(),
            skills: info.skills.clone(),
        }
    }
}

/// Lifecycle events of children, for the parent's event stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubAgentEvent {
    Spawned(Box<SpawnInfo>),
    State {
        agent_id: String,
        state: InstanceState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    ToolStarted {
        agent_id: String,
        tool_call_id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolFinished {
        agent_id: String,
        tool_call_id: String,
        name: String,
        is_error: bool,
        duration_ms: u64,
    },
    Finished(Box<AgentResult>),
    /// A change was applied to the session's workspace by a child working
    /// in it (`changeset_id: None`), or by applying a ChangeSet.
    EditApplied {
        agent_id: String,
        tool_call_id: String,
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changeset_id: Option<String>,
        files: Vec<crate::control::WrittenFile>,
    },
    /// An isolated child's work is ready for review.
    ChangesReady(Box<super::workspace::ChangeSet>),
    /// A ChangeSet was applied, discarded, or hit a conflict.
    ChangesUpdated {
        changeset_id: String,
        state: super::workspace::ChangeSetState,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Usage of a root run: the session agent's own, its descendants', and
    /// the total; `final_total` once every descendant ended.
    RunUsage {
        root_run_id: String,
        own: Box<Usage>,
        descendants: Box<Usage>,
        total: Box<Usage>,
        pending_agents: usize,
        final_total: bool,
    },
}

/// The children of the current run, so that a cancelled run can wait
/// for their cleanup (put in the tool context by the runner).
#[derive(Clone, Default)]
pub struct LiveChildren(pub Arc<parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>>);

/// Wait (at most `timeout`) for the children of a run to finish.
pub async fn settle_children(ext: &cersei_tools::Extensions, timeout: std::time::Duration) {
    let Some(live) = ext.get::<LiveChildren>() else {
        return;
    };
    let handles: Vec<_> = std::mem::take(&mut *live.0.lock());
    if handles.is_empty() {
        return;
    }
    let _ = tokio::time::timeout(timeout, futures::future::join_all(handles)).await;
}

/// Why nothing was started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    Invalid(String),
    /// Parsed, understood, but not available in this phase.
    FeatureUnavailable(String),
    Refused(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m) => write!(f, "{m} Nothing was started."),
            Self::FeatureUnavailable(m) => {
                write!(f, "not available yet: {m} Nothing was started.")
            }
            Self::Refused(m) => write!(f, "{m} Nothing was started."),
        }
    }
}

// ─── Resolution ──────────────────────────────────────────────────────────────

/// The neutral profile used when none is named.
pub fn neutral_profile() -> Arc<AgentProfile> {
    Arc::new(AgentProfile {
        name: "general".into(),
        description: "A general sub-agent for one self-contained task.".into(),
        model: ModelPref::Inherit,
        reasoning: ReasoningPref::Inherit,
        permissions: "inherit".into(),
        tools: "inherit".into(),
        isolation: Isolation::Auto,
        background: false,
        skills: Vec::new(),
        instructions: String::new(),
        source: ProfileSource {
            scope: ProfileScope::BuiltIn,
            path: None,
            label: "builtin:general (internal default)".into(),
            revision: "internal".into(),
        },
        notes: Vec::new(),
    })
}

/// Model and reasoning chosen for a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub model: Choice,
    pub reasoning: Choice,
    pub warnings: Vec<String>,
}

fn known_models(models: &[ModelChoice]) -> String {
    let v: Vec<&str> = models
        .iter()
        .map(|m| m.selection.as_str())
        .take(20)
        .collect();
    if v.is_empty() {
        "(none configured)".into()
    } else {
        v.join(", ")
    }
}

/// Precedence: request > profile > parent > model default. An explicit
/// request that does not fit is refused; a profile preference that does
/// not fit falls back, with a warning. Reasoning ids are the model's own
/// (free identifiers): no closed list, no "closest level".
pub fn resolve(
    req: &AgentSpawnRequest,
    profile: &AgentProfile,
    parent: &ParentModel,
    models: &[ModelChoice],
    settings: &DelegationSettings,
) -> Result<Resolved, String> {
    let mut warnings = Vec::new();
    let find = |sel: &str| models.iter().find(|m| m.selection == sel);
    let auto = |warnings: &mut Vec<String>| -> (String, Option<String>) {
        match &settings.auto_model {
            Some(m) if find(m).is_some() => (m.clone(), Some("[agents] auto_model".into())),
            Some(m) => {
                warnings.push(format!(
                    "auto_model `{m}` is not configured: the parent's model is used"
                ));
                (
                    parent.selection.clone(),
                    Some("auto: rule invalid, inherited".into()),
                )
            }
            None => {
                warnings.push(
                    "model `auto` without an `[agents] auto_model` rule: the parent's model is used"
                        .into(),
                );
                (
                    parent.selection.clone(),
                    Some("auto: no rule, inherited".into()),
                )
            }
        }
    };

    // Model.
    let (model_requested, model, model_reason) = match req.model.as_deref().map(str::trim) {
        Some(m) if !m.is_empty() => match ModelPref::parse(m) {
            Ok(ModelPref::Inherit) => (
                m.to_string(),
                parent.selection.clone(),
                Some("inherited".into()),
            ),
            Ok(ModelPref::Auto) => {
                let (s, r) = auto(&mut warnings);
                (m.to_string(), s, r)
            }
            Ok(ModelPref::Explicit(sel)) => {
                if find(&sel).is_none() {
                    return Err(format!(
                        "model `{sel}` is not configured; configured models: {}.",
                        known_models(models)
                    ));
                }
                (m.to_string(), sel, Some("requested".into()))
            }
            Err(e) => return Err(format!("{e}.")),
        },
        _ => match &profile.model {
            ModelPref::Inherit => (
                "inherit".into(),
                parent.selection.clone(),
                Some("inherited".into()),
            ),
            ModelPref::Auto => {
                let (s, r) = auto(&mut warnings);
                ("auto".into(), s, r)
            }
            ModelPref::Explicit(sel) => {
                if find(sel).is_some() {
                    (
                        sel.clone(),
                        sel.clone(),
                        Some(format!("profile `{}`", profile.name)),
                    )
                } else {
                    warnings.push(format!(
                        "profile `{}` asks for model `{sel}`, which is not configured: the parent's model is used",
                        profile.name
                    ));
                    (
                        sel.clone(),
                        parent.selection.clone(),
                        Some("profile model unavailable, inherited".into()),
                    )
                }
            }
        },
    };
    let model_info = find(&model);
    let ids: Vec<String> = model_info
        .map(|m| m.profiles.iter().map(|p| p.id.clone()).collect())
        .unwrap_or_default();
    let model_default = model_info.and_then(|m| m.default_profile.clone());
    let has = |id: &str| ids.iter().any(|x| x == id);
    let inherited = || -> (Option<String>, String) {
        match &parent.reasoning {
            Some(r) if model_info.is_none() || has(r) => (Some(r.clone()), "inherited".into()),
            Some(r) => (
                model_default.clone(),
                format!("the parent's `{r}` is not a profile of `{model}`: model default"),
            ),
            None => (model_default.clone(), "model default".into()),
        }
    };

    // Reasoning.
    let (reasoning_requested, reasoning, reasoning_reason) = match req
        .reasoning
        .as_deref()
        .map(str::trim)
    {
        Some(r) if !r.is_empty() => match ReasoningPref::parse(r) {
            Ok(ReasoningPref::Inherit) => {
                let (a, why) = inherited();
                (r.to_string(), a, why)
            }
            Ok(ReasoningPref::Id(id)) => {
                if model_info.is_some() && !has(&id) {
                    return Err(format!(
                        "model `{model}` has no reasoning profile `{id}`; its profiles: {}.",
                        if ids.is_empty() {
                            "(none)".to_string()
                        } else {
                            ids.join(", ")
                        }
                    ));
                }
                (r.to_string(), Some(id), "requested".into())
            }
            Err(e) => return Err(format!("{e}.")),
        },
        _ => match &profile.reasoning {
            ReasoningPref::Inherit => {
                let (a, why) = inherited();
                ("inherit".into(), a, why)
            }
            ReasoningPref::Id(id) if model_info.is_none() || has(id) => (
                id.clone(),
                Some(id.clone()),
                format!("profile `{}`", profile.name),
            ),
            ReasoningPref::Id(id) => match settings.reasoning_aliases.get(id) {
                Some(alias) if has(alias) => (
                    id.clone(),
                    Some(alias.clone()),
                    format!("alias `{id}` → `{alias}` ([agents.reasoning_aliases])"),
                ),
                _ => {
                    let (a, why) = inherited();
                    warnings.push(format!(
                            "profile `{}` prefers reasoning `{id}`, which `{model}` does not have ({}); {why}",
                            profile.name,
                            if ids.is_empty() { "no profiles".to_string() } else { ids.join(", ") }
                        ));
                    (id.clone(), a, format!("`{id}` unavailable: {why}"))
                }
            },
        },
    };

    Ok(Resolved {
        model: Choice {
            requested: model_requested,
            applied: model,
            reason: model_reason,
        },
        reasoning: Choice {
            requested: reasoning_requested,
            applied: reasoning.clone().unwrap_or_else(|| "(none)".into()),
            reason: Some(reasoning_reason),
        },
        warnings,
    })
}

// ─── The child's policy ──────────────────────────────────────────────────────

/// The parent's policy, with the child's identity on every request (so an
/// approval shows which agent asks). Nothing is decided here.
struct ChildPolicy {
    inner: Arc<dyn PermissionPolicy>,
    agent_id: String,
}

#[async_trait::async_trait]
impl PermissionPolicy for ChildPolicy {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        let mut r = request.clone();
        r.agent_id = Some(self.agent_id.clone());
        self.inner.check(&r).await
    }
}

// ─── The spawner ─────────────────────────────────────────────────────────────

/// Builds the tools of a child (the parent's real factory).
pub type ChildTools = Arc<dyn Fn() -> Vec<Box<dyn Tool>> + Send + Sync>;

const CHILD_FRAMING: &str = "\n\n## Your role\n\nYou are a sub-agent working on one task given by another agent, in a fresh context: \
you do not see its conversation. Do that task within its scope, then reply with your result: \
what you did, what you found (with file paths and evidence), what you verified and how, and \
anything left undone. Stop as soon as the task is done; do not widen it.";

/// How a batch runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchOptions {
    pub background: bool,
    pub fail_fast: bool,
    /// Called as `Agents` (children carry their position).
    pub batch: bool,
}

/// What a call gets back.
#[derive(Debug, Clone)]
pub enum Spawned {
    /// Foreground: every result, in the order asked.
    Results(Vec<AgentResult>),
    /// Background: the admitted handles (the work goes on).
    Handles(Vec<SpawnInfo>),
}

/// A request validated and resolved, nothing built yet.
struct Prepared {
    req: AgentSpawnRequest,
    profile: Arc<AgentProfile>,
    resolved: Resolved,
    background: bool,
    isolation: Option<Isolation>,
}

/// What a child task takes from its parent's context.
#[derive(Clone)]
struct ParentCtx {
    working_dir: PathBuf,
    permissions: Arc<dyn PermissionPolicy>,
    mcp_manager: Option<Arc<cersei_mcp::McpManager>>,
    semantic: Option<cersei_tools::code_scout::SemanticHandle>,
    web: Option<cersei_tools::web_runtime::WebRuntime>,
    depth: u32,
    sink: Option<SubAgentSink>,
    jobs: Option<cersei_tools::jobs::JobsHandle>,
}

/// One per session (or embedding): shared by every delegation tool.
pub struct AgentSpawner {
    catalog: Arc<dyn ModelCatalog>,
    profiles: Arc<ProfileCatalog>,
    tools: ChildTools,
    bricks: BricksConfig,
    settings: DelegationSettings,
    /// Where children's transcripts are stored.
    artifacts_dir: Option<PathBuf>,
    session_id: String,
    runtime: Arc<super::runtime::AgentRuntime>,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn shorten(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        (s.to_string(), false)
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        (t, true)
    }
}

fn limits_of(s: &DelegationSettings) -> super::runtime::Limits {
    super::runtime::Limits {
        max_concurrent: s.max_concurrent.max(1),
        max_depth: s.max_depth,
        max_total_per_run: s.max_total_per_run,
        max_queued: s.max_queued,
        admission_timeout: std::time::Duration::from_millis(s.admission_timeout_ms),
        max_batch: s.max_batch.max(1),
        background_drain: std::time::Duration::from_millis(s.background_drain_ms),
    }
}

impl AgentSpawner {
    pub fn new(
        catalog: Arc<dyn ModelCatalog>,
        profiles: Arc<ProfileCatalog>,
        tools: ChildTools,
        bricks: BricksConfig,
    ) -> Self {
        let settings = bricks.agents.clone();
        let runtime = super::runtime::AgentRuntime::new(
            limits_of(&settings),
            Arc::new(super::workspace::WorkspaceManager::new(
                None,
                Default::default(),
            )),
            None,
        );
        Self {
            catalog,
            profiles,
            tools,
            bricks,
            settings,
            artifacts_dir: None,
            session_id: String::new(),
            runtime,
        }
    }

    /// Store children's transcripts, the instance manifest, worktrees,
    /// snapshots and ChangeSets under `dir` (the session's files).
    pub fn with_artifacts_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let ws = Arc::new(super::workspace::WorkspaceManager::new(
            Some(dir.join("workspaces")),
            Default::default(),
        ));
        self.runtime = super::runtime::AgentRuntime::new(
            limits_of(&self.settings),
            ws,
            Some(dir.join("manifest.json")),
        );
        self.artifacts_dir = Some(dir);
        self
    }

    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = id.into();
        self
    }

    pub fn profiles(&self) -> &Arc<ProfileCatalog> {
        &self.profiles
    }

    pub fn settings(&self) -> &DelegationSettings {
        &self.settings
    }

    pub fn runtime(&self) -> &Arc<super::runtime::AgentRuntime> {
        &self.runtime
    }

    /// Children started by this spawner and their last state.
    pub fn instances(&self) -> Vec<(SpawnInfo, InstanceState)> {
        self.runtime
            .records()
            .into_iter()
            .map(|r| (r.info, r.state))
            .collect()
    }

    fn emit(&self, sink: &Option<SubAgentSink>, ev: SubAgentEvent) {
        if self.runtime.has_session_sink() {
            self.runtime.emit(AgentEvent::SubAgent(ev));
        } else if let Some(s) = sink {
            (s.0)(AgentEvent::SubAgent(ev));
        }
    }

    fn set_state(
        &self,
        sink: &Option<SubAgentSink>,
        id: &str,
        state: InstanceState,
        reason: Option<String>,
    ) {
        if self.runtime.set_state(id, state, reason.clone()) {
            self.emit(
                sink,
                SubAgentEvent::State {
                    agent_id: id.to_string(),
                    state,
                    reason,
                },
            );
        }
    }

    /// Everything about one request that can be refused without building
    /// anything (kept for embedders: the 10.1 entry point).
    pub fn check(
        &self,
        req: &AgentSpawnRequest,
        ext: &cersei_tools::Extensions,
    ) -> Result<(Arc<AgentProfile>, Resolved), SpawnError> {
        let p = self.prepare(req, ext, None)?;
        Ok((p.profile, p.resolved))
    }

    fn prepare(
        &self,
        req: &AgentSpawnRequest,
        ext: &cersei_tools::Extensions,
        batch_background: Option<bool>,
    ) -> Result<Prepared, SpawnError> {
        if req.legacy_max_turns.is_some() {
            return Err(SpawnError::Invalid(
                "`max_turns` was removed in 0.4.8: sub-agents have no turn limit. Send the \
                 request again without it."
                    .into(),
            ));
        }
        if subagent::is_blank(&req.task) {
            return Err(SpawnError::Invalid(
                "`task` is empty: give the sub-agent a precise, self-contained task.".into(),
            ));
        }
        let isolation = match req.isolation.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(s) => Some(Isolation::parse(s).map_err(SpawnError::Invalid)?),
        };
        let profile = match req.profile.as_deref().map(str::trim) {
            None | Some("") => neutral_profile(),
            Some(name) => self
                .profiles
                .snapshot()
                .get(name)
                .map_err(|e| SpawnError::Invalid(format!("{e}.")))?,
        };
        if let Some(c) = &req.context {
            let n = c.chars().count();
            if n > self.settings.max_context_chars {
                return Err(SpawnError::Invalid(format!(
                    "`context` has {n} characters; the limit is {} ([agents] max_context_chars).",
                    self.settings.max_context_chars
                )));
            }
        }
        if subagent::run_token(ext).is_some_and(|t| t.is_cancelled()) {
            return Err(SpawnError::Refused("the run was cancelled.".into()));
        }
        let parent = ext.get::<ParentModel>().ok_or_else(|| {
            SpawnError::Refused("the parent's model is unknown (not started by a run).".into())
        })?;
        let resolved = resolve(
            req,
            &profile,
            &parent,
            &self.catalog.models(),
            &self.settings,
        )
        .map_err(SpawnError::Invalid)?;
        let background = batch_background
            .or(req.background)
            .unwrap_or(profile.background);
        let isolation = isolation.or(match profile.isolation {
            Isolation::Auto => None,
            other => Some(other),
        });
        Ok(Prepared {
            req: req.clone(),
            profile,
            resolved,
            background,
            isolation,
        })
    }

    /// Shared or worktree, from the writers that would really coexist.
    fn decide(&self, p: &Prepared, batch_len: usize) -> Isolation {
        let default = Isolation::parse(&self.settings.default_isolation).unwrap_or(Isolation::Auto);
        match p.isolation.unwrap_or(default) {
            Isolation::Shared => Isolation::Shared,
            Isolation::Worktree => Isolation::Worktree,
            Isolation::Auto => {
                if p.background || batch_len > 1 {
                    Isolation::Worktree
                } else {
                    Isolation::Shared
                }
            }
        }
    }

    /// Run one foreground child to its end (the 10.1 entry point).
    pub async fn spawn(
        self: &Arc<Self>,
        req: AgentSpawnRequest,
        ctx: &ToolContext,
        _tool_call_id: Option<String>,
    ) -> Result<AgentResult, SpawnError> {
        let mut req = req;
        req.background = Some(false);
        match self
            .spawn_batch(vec![req], BatchOptions::default(), ctx)
            .await?
        {
            Spawned::Results(mut r) => r
                .pop()
                .ok_or_else(|| SpawnError::Refused("no result".into())),
            Spawned::Handles(_) => Err(SpawnError::Refused("unexpected background start".into())),
        }
    }

    /// Validate the whole batch, reserve it, start every child in its own
    /// supervised task, and wait for the results (foreground) or return the
    /// handles (background).
    pub async fn spawn_batch(
        self: &Arc<Self>,
        reqs: Vec<AgentSpawnRequest>,
        opts: BatchOptions,
        ctx: &ToolContext,
    ) -> Result<Spawned, SpawnError> {
        if reqs.is_empty() {
            return Err(SpawnError::Invalid("`agents` is empty.".into()));
        }
        let max_batch = self.settings.max_batch.max(1);
        if reqs.len() > max_batch {
            return Err(SpawnError::Invalid(format!(
                "{} agents asked; at most {max_batch} per call ([agents] max_batch).",
                reqs.len()
            )));
        }
        let batch_bg = opts.batch.then_some(opts.background);
        let mut prepared = Vec::with_capacity(reqs.len());
        for (i, r) in reqs.iter().enumerate() {
            match self.prepare(r, &ctx.extensions, batch_bg) {
                Ok(p) => prepared.push(p),
                Err(e) if opts.batch => {
                    return Err(match e {
                        SpawnError::Invalid(m) => SpawnError::Invalid(format!("agents[{i}]: {m}")),
                        other => other,
                    })
                }
                Err(e) => return Err(e),
            }
        }
        let parent = ctx.extensions.get::<AgentIdentity>();
        let root = parent
            .as_ref()
            .map(|p| p.root_run_id.clone())
            .unwrap_or_else(|| "detached".into());
        let depth = subagent::depth_of(&ctx.extensions);
        let n = prepared.len() as u32;
        self.runtime
            .scheduler
            .reserve(&root, depth, n)
            .map_err(|e| SpawnError::Refused(format!("{e}.")))?;

        let isolations: Vec<Isolation> = prepared
            .iter()
            .map(|p| self.decide(p, prepared.len()))
            .collect();
        // One capture of the parent's state for every isolated child.
        let snapshot = if isolations.contains(&Isolation::Worktree) {
            Some(
                self.runtime
                    .workspaces
                    .snapshot(&ctx.working_dir)
                    .await
                    .map(Arc::new),
            )
        } else {
            None
        };

        let sink = ctx.extensions.get::<SubAgentSink>().map(|s| (*s).clone());
        let pctx = ParentCtx {
            working_dir: ctx.working_dir.clone(),
            permissions: Arc::clone(&ctx.permissions),
            mcp_manager: ctx.mcp_manager.clone(),
            semantic: ctx
                .extensions
                .get::<cersei_tools::code_scout::SemanticHandle>()
                .map(|h| (*h).clone()),
            web: ctx
                .extensions
                .get::<cersei_tools::web_runtime::WebRuntime>()
                .map(|w| (*w).clone()),
            depth,
            sink: sink.clone(),
            jobs: ctx
                .extensions
                .get::<cersei_tools::jobs::JobsHandle>()
                .map(|j| (*j).clone()),
        };
        let run_token = subagent::run_token(&ctx.extensions).unwrap_or_default();
        let call_id = ctx
            .extensions
            .get::<cersei_tools::CurrentToolCall>()
            .map(|c| c.0.clone());

        let mut receivers = Vec::new();
        let mut handles_info = Vec::new();
        let mut ids = Vec::new();
        for (i, (p, iso)) in prepared.into_iter().zip(isolations).enumerate() {
            let agent_id = format!("agent_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
            let reasoning = (p.resolved.reasoning.applied != "(none)")
                .then(|| p.resolved.reasoning.applied.clone());
            let info = SpawnInfo {
                agent_id: agent_id.clone(),
                parent_id: parent.as_ref().map(|x| x.agent_id.clone()),
                root_run_id: root.clone(),
                tool_call_id: call_id.clone(),
                profile: p.profile.name.clone(),
                profile_source: p.profile.source.label.clone(),
                profile_revision: p.profile.source.revision.clone(),
                model: p.resolved.model.clone(),
                reasoning: p.resolved.reasoning.clone(),
                workspace: ctx.working_dir.display().to_string(),
                isolation: match iso {
                    Isolation::Worktree => "worktree".into(),
                    _ => "shared".into(),
                },
                task: shorten(p.req.task.trim(), 200).0,
                description: p.req.description.clone().filter(|d| !subagent::is_blank(d)),
                created_at: now_rfc3339(),
                batch_index: opts.batch.then_some(i),
                background: p.background,
                depth: depth + 1,
                branch: None,
                skills: Vec::new(),
            };
            // A background child outlives its parent's turn (not its
            // session); a foreground one is cancelled with the parent's run.
            let cancel = if p.background {
                self.runtime.session_token.child_token()
            } else {
                run_token.child_token()
            };
            self.runtime
                .register(info.clone(), p.background, cancel.clone());
            self.emit(&sink, SubAgentEvent::Spawned(Box::new(info.clone())));
            let (tx, rx) = tokio::sync::oneshot::channel();
            let this = Arc::clone(self);
            let snap = snapshot.clone();
            let pc = pctx.clone();
            let child_info = info.clone();
            let handle = tokio::spawn(async move {
                let r = this
                    .run_one(p, child_info, iso, reasoning, snap, pc, cancel)
                    .await;
                let _ = tx.send(r);
            });
            if info.background {
                self.runtime.attach_task(&agent_id, handle);
            } else if let Some(live) = ctx.extensions.get::<LiveChildren>() {
                live.0.lock().push(handle);
            } else {
                self.runtime.attach_task(&agent_id, handle);
            }
            ids.push(agent_id);
            receivers.push(rx);
            handles_info.push(info);
        }

        if opts.background || handles_info.iter().all(|i| i.background) {
            return Ok(Spawned::Handles(handles_info));
        }

        // Foreground: wait without holding this agent's slot or writer lease.
        let runtime = Arc::clone(&self.runtime);
        let fail_fast = opts.fail_fast;
        let results = super::runtime::while_waiting(&ctx.extensions, async move {
            use futures::stream::{FuturesUnordered, StreamExt};
            let mut pending: FuturesUnordered<_> = receivers
                .into_iter()
                .enumerate()
                .map(|(i, rx)| async move { (i, rx.await) })
                .collect();
            let mut out: Vec<Option<AgentResult>> = vec![None; ids.len()];
            let mut cancelled_rest = false;
            while let Some((i, r)) = pending.next().await {
                let r = r.ok();
                if fail_fast
                    && !cancelled_rest
                    && r.as_ref().is_none_or(|r| r.status != "completed")
                {
                    cancelled_rest = true;
                    for (j, id) in ids.iter().enumerate() {
                        if j != i {
                            runtime.cancel(id);
                        }
                    }
                }
                out[i] = r;
            }
            out
        })
        .await;
        let mut final_results = Vec::with_capacity(results.len());
        for (i, r) in results.into_iter().enumerate() {
            final_results.push(r.unwrap_or_else(|| {
                let info = &handles_info[i];
                AgentResult::lost(info)
            }));
        }
        Ok(Spawned::Results(final_results))
    }

    /// One child, from its workspace to its terminal state.
    #[allow(clippy::too_many_arguments)]
    async fn run_one(
        self: Arc<Self>,
        p: Prepared,
        mut info: SpawnInfo,
        iso: Isolation,
        reasoning: Option<String>,
        snapshot: Option<Result<Arc<super::workspace::Snapshot>, super::workspace::WsError>>,
        pc: ParentCtx,
        cancel: CancellationToken,
    ) -> AgentResult {
        let started = Instant::now();
        let id = info.agent_id.clone();
        let sink = pc.sink.clone();
        let fail = |this: &Self, info: &SpawnInfo, status: &str, err: String| -> AgentResult {
            let r = this.finish_without_run(info, &p.resolved, status, Some(err), started);
            let state = if status == "cancelled" {
                InstanceState::Cancelled
            } else {
                InstanceState::Failed
            };
            this.set_state(&sink, &info.agent_id, state, r.error.clone());
            // The result first, then the totals it completes.
            this.emit(&sink, SubAgentEvent::Finished(Box::new(r.clone())));
            this.runtime.finish(&info.agent_id, r.clone());
            r
        };

        // Workspace.
        let mut cwd = pc.working_dir.clone();
        if iso == Isolation::Worktree {
            let snap = match snapshot {
                Some(Ok(s)) => s,
                Some(Err(e)) => return fail(&self, &info, "failed", e.to_string()),
                None => return fail(&self, &info, "failed", "no snapshot".into()),
            };
            match self
                .runtime
                .workspaces
                .create(&snap, &info.root_run_id, &id)
                .await
            {
                Ok(wt) => {
                    cwd = wt.cwd.clone();
                    info.branch = Some(wt.branch.clone());
                    info.workspace = wt.cwd.display().to_string();
                }
                Err(e) => return fail(&self, &info, "failed", e.to_string()),
            }
        }
        if cancel.is_cancelled() {
            return self
                .close_isolated(
                    fail(
                        &self,
                        &info,
                        "cancelled",
                        "cancelled before starting".into(),
                    ),
                    iso,
                )
                .await;
        }

        // Provider, once everything else is ready.
        let provider = match self
            .catalog
            .build(&p.resolved.model.applied, reasoning.as_deref())
        {
            Ok(pr) => pr,
            Err(e) => {
                return self
                    .close_isolated(fail(&self, &info, "failed", e), iso)
                    .await
            }
        };

        // A slot (queued when none is free).
        let s2 = sink.clone();
        let id2 = id.clone();
        let this2 = Arc::clone(&self);
        let permit = match self
            .runtime
            .scheduler
            .acquire(&cancel, move || {
                this2.set_state(
                    &s2,
                    &id2,
                    InstanceState::Queued,
                    Some("waiting for a free slot ([agents] max_concurrent)".into()),
                );
            })
            .await
        {
            Ok(p) => p,
            Err(super::runtime::AdmissionError::Cancelled) => {
                return self
                    .close_isolated(
                        fail(&self, &info, "cancelled", "cancelled while queued".into()),
                        iso,
                    )
                    .await
            }
            Err(e) => {
                return self
                    .close_isolated(fail(&self, &info, "failed", e.to_string()), iso)
                    .await
            }
        };

        // The shared workspace: one writer at a time.
        let mut writer = None;
        if iso == Isolation::Shared {
            self.set_state(&sink, &id, InstanceState::WaitingAdmission, None);
            let s3 = sink.clone();
            let id3 = id.clone();
            let this3 = Arc::clone(&self);
            let g = writers_for(&cwd)
                .acquire(&id, &cancel, move |holder| {
                    this3.emit(
                        &s3,
                        SubAgentEvent::State {
                            agent_id: id3.clone(),
                            state: InstanceState::WaitingAdmission,
                            reason: Some(format!(
                                "another writer is active in this workspace ({holder})"
                            )),
                        },
                    );
                })
                .await;
            match g {
                Some(g) => writer = Some((cwd.clone(), g)),
                None => {
                    drop(permit);
                    return fail(
                        &self,
                        &info,
                        "cancelled",
                        "cancelled while waiting for the workspace".into(),
                    );
                }
            }
        }
        let lease = super::runtime::ActivityLease::new(
            Arc::clone(&self.runtime.scheduler),
            id.clone(),
            permit,
            writer,
            cancel.clone(),
        );
        self.set_state(&sink, &id, InstanceState::Starting, None);

        let (child, skills) = match self
            .build_child(
                &info,
                &p.profile,
                &p.resolved,
                reasoning,
                provider,
                &pc,
                &cwd,
                iso,
                &cancel,
                Arc::clone(&lease),
            )
            .await
        {
            Ok(c) => c,
            Err(e) => {
                lease.release().await;
                return self
                    .close_isolated(fail(&self, &info, "failed", e), iso)
                    .await;
            }
        };
        info.skills = skills;

        let prompt = match p.req.context.as_deref().filter(|c| !subagent::is_blank(c)) {
            Some(c) => format!(
                "{}\n\n## Context given by the parent agent\n\n{}",
                p.req.task.trim(),
                c.trim()
            ),
            None => p.req.task.trim().to_string(),
        };
        let mut r = self
            .run_child(
                child,
                prompt,
                &info,
                &p.resolved,
                sink.clone(),
                cancel.clone(),
                started,
            )
            .await;
        r.skills = info.skills.clone();
        lease.release().await;

        // The work of an isolated child: a ChangeSet (or nothing).
        if iso == Isolation::Worktree {
            match self
                .runtime
                .workspaces
                .finalize(&id, &p.req.task, r.commands.clone())
                .await
            {
                Ok(Some(cs)) => {
                    r.changeset = Some(cs.id.clone());
                    r.files_changed = cs.files.iter().map(|f| f.path.clone()).collect();
                    self.emit(&sink, SubAgentEvent::ChangesReady(Box::new(cs)));
                }
                Ok(None) => {}
                Err(e) => r
                    .warnings
                    .push(format!("changes of {id} not collected: {e}")),
            }
        }
        let state = match r.status.as_str() {
            "completed" => InstanceState::Completed,
            "incomplete" => InstanceState::Incomplete,
            "cancelled" => InstanceState::Cancelled,
            _ => InstanceState::Failed,
        };
        self.set_state(&sink, &id, state, r.error.clone());
        self.emit(&sink, SubAgentEvent::Finished(Box::new(r.clone())));
        self.runtime.finish(&id, r.clone());
        r
    }

    /// After a failure, an isolated child's untouched worktree goes away.
    async fn close_isolated(&self, r: AgentResult, iso: Isolation) -> AgentResult {
        if iso == Isolation::Worktree && self.runtime.workspaces.worktree(&r.agent_id).is_some() {
            let _ = self
                .runtime
                .workspaces
                .finalize(&r.agent_id, "", Vec::new())
                .await;
        }
        r
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_child(
        &self,
        child: Agent,
        prompt: String,
        info: &SpawnInfo,
        resolved: &Resolved,
        sink: Option<SubAgentSink>,
        cancel: CancellationToken,
        started: Instant,
    ) -> AgentResult {
        let agent_id = info.agent_id.clone();
        self.set_state(&sink, &agent_id, InstanceState::Running, None);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
        let fwd_id = agent_id.clone();
        // Writes of a child in the session's workspace are changes of that
        // workspace; a worktree's count only once applied (its ChangeSet).
        let shared = info.isolation == "shared";
        let rt = Arc::clone(&self.runtime);
        let fwd_sink = sink.clone();
        let forward = tokio::spawn(async move {
            let mut files: Vec<String> = Vec::new();
            while let Some(ev) = rx.recv().await {
                let out = match ev {
                    AgentEvent::ToolStart { name, id, input } => Some(SubAgentEvent::ToolStarted {
                        agent_id: fwd_id.clone(),
                        tool_call_id: id,
                        name,
                        input,
                    }),
                    AgentEvent::ToolEnd {
                        name,
                        id,
                        is_error,
                        duration,
                        ..
                    } => Some(SubAgentEvent::ToolFinished {
                        agent_id: fwd_id.clone(),
                        tool_call_id: id,
                        name,
                        is_error,
                        duration_ms: duration.as_millis() as u64,
                    }),
                    AgentEvent::EditApplied {
                        tool_call_id,
                        tool,
                        files: f,
                    } => {
                        for w in &f {
                            if !files.contains(&w.path) {
                                files.push(w.path.clone());
                            }
                        }
                        shared.then(|| SubAgentEvent::EditApplied {
                            agent_id: fwd_id.clone(),
                            tool_call_id,
                            tool,
                            changeset_id: None,
                            files: f
                                .into_iter()
                                .map(|w| crate::control::WrittenFile {
                                    path: w.path,
                                    kind: w.kind,
                                    added: w.added,
                                    removed: w.removed,
                                    binary: false,
                                })
                                .collect(),
                        })
                    }
                    _ => None,
                };
                if let Some(e) = out {
                    if rt.has_session_sink() {
                        rt.emit(AgentEvent::SubAgent(e));
                    } else if let Some(s) = &fwd_sink {
                        (s.0)(AgentEvent::SubAgent(e));
                    }
                }
            }
            files
        });
        let cancel_watch = {
            let token = cancel.clone();
            let rt = Arc::clone(&self.runtime);
            let s = sink.clone();
            let id = agent_id.clone();
            tokio::spawn(async move {
                token.cancelled().await;
                if rt.set_state(
                    &id,
                    InstanceState::Cancelling,
                    Some("cancellation requested".into()),
                ) {
                    let ev = SubAgentEvent::State {
                        agent_id: id.clone(),
                        state: InstanceState::Cancelling,
                        reason: Some("cancellation requested".into()),
                    };
                    if rt.has_session_sink() {
                        rt.emit(AgentEvent::SubAgent(ev));
                    } else if let Some(s) = &s {
                        (s.0)(AgentEvent::SubAgent(ev));
                    }
                }
            })
        };
        let input = crate::UserInput::text(prompt);
        let result = crate::runner::run_agent_streaming(&child, &input, tx).await;
        cancel_watch.abort();
        let partial = subagent::partial_text(&child);
        // What it spent, also when it failed or was cancelled.
        let spent = child.usage();
        let transcript = self.store_transcript(&agent_id, &child);
        // Its jobs end with it (the session agent's live with the session).
        if let Some(j) = child.extensions.get::<cersei_tools::jobs::JobsHandle>() {
            j.0.stop_owned_by(&agent_id).await;
        }
        // Close the child's shell and jobs, then let go of it: its event
        // stream ends with it.
        child.close().await;
        drop(child);
        let files = forward.await.unwrap_or_default();
        self.result_of(
            info,
            resolved,
            result,
            (partial, spent),
            files,
            transcript,
            started,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_child(
        self: &Arc<Self>,
        info: &SpawnInfo,
        profile: &AgentProfile,
        resolved: &Resolved,
        reasoning: Option<String>,
        provider: Box<dyn cersei_provider::Provider>,
        pc: &ParentCtx,
        cwd: &Path,
        iso: Isolation,
        cancel: &CancellationToken,
        lease: Arc<super::runtime::ActivityLease>,
    ) -> Result<(Agent, Vec<SkillLoad>), String> {
        // The parent's tools (rebuilt), and the delegation tools of this
        // runtime: recursion is bounded by the scheduler, not by removing
        // tools. Legacy delegation tools never pass.
        let mut tools = subagent::child_tools((self.tools)(), &[]);
        tools.push(Box::new(super::tool::NativeAgentTool::new(Arc::clone(
            self,
        ))));
        tools.push(Box::new(super::tool::AgentsTool::new(Arc::clone(self))));
        tools.push(Box::new(super::tool::AgentControlTool::new(Arc::clone(
            self,
        ))));
        tools.push(Box::new(super::tool::AgentProfilesTool::new(Arc::clone(
            self,
        ))));
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        let ext = subagent::child_extensions(pc.depth);
        // Services: the code engine only within the same workspace (a
        // worktree gets its own), the web context. The shell is the child's.
        if iso == Isolation::Shared {
            if let Some(h) = &pc.semantic {
                ext.insert(h.clone());
            }
        }
        if let Some(w) = &pc.web {
            ext.insert(w.clone());
        }
        ext.insert(AgentIdentity {
            agent_id: info.agent_id.clone(),
            parent_id: info.parent_id.clone(),
            root_run_id: info.root_run_id.clone(),
        });
        ext.insert(super::runtime::LeaseHandle(lease));
        if let Some(j) = &pc.jobs {
            ext.insert(j.clone());
        }
        ext.insert(super::runtime::RuntimeHandle(Arc::clone(&self.runtime)));
        let (skill_text, skills) =
            load_skills(&profile.skills, cwd, self.settings.skills_max_bytes);
        let mut system = child_system_prompt(profile, &names, cwd);
        system.push_str(&skill_text);
        let mut b = Agent::builder()
            .provider_boxed(provider)
            .tools(tools)
            .model(resolved.model.applied.clone())
            .permission_policy_arc(Arc::new(ChildPolicy {
                inner: Arc::clone(&pc.permissions),
                agent_id: info.agent_id.clone(),
            }))
            .working_dir(cwd)
            .bricks_config(self.bricks.clone())
            .extensions(ext)
            .system_prompt(system)
            .cancel_token(cancel.child_token());
        if let Some(r) = reasoning {
            b = b.reasoning_profile(r);
        }
        let agent = b
            .build()
            .map_err(|e| format!("cannot build the sub-agent: {e}"))?;
        if let Some(m) = &pc.mcp_manager {
            agent.adopt_mcp(Arc::clone(m)).await;
        }
        Ok((agent, skills))
    }

    fn store_transcript(&self, id: &str, child: &Agent) -> Option<String> {
        let dir = self.artifacts_dir.as_ref()?;
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join(format!("{id}.json"));
        let body = serde_json::to_vec_pretty(&child.messages()).ok()?;
        std::fs::write(&path, body).ok()?;
        Some(path.display().to_string())
    }

    fn finish_without_run(
        &self,
        info: &SpawnInfo,
        resolved: &Resolved,
        status: &str,
        error: Option<String>,
        started: Instant,
    ) -> AgentResult {
        let mut r = AgentResult::lost(info);
        r.status = status.into();
        r.error = error;
        r.warnings = resolved.warnings.clone();
        r.duration_ms = started.elapsed().as_millis() as u64;
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn result_of(
        &self,
        info: &SpawnInfo,
        resolved: &Resolved,
        result: cersei_types::Result<AgentOutput>,
        (partial, spent): (String, Usage),
        files: Vec<String>,
        transcript: Option<String>,
        started: Instant,
    ) -> AgentResult {
        let status = subagent::status_of(&result);
        let (text, termination, turns, tool_calls, usage, commands) = match &result {
            Ok(out) => (
                out.text().to_string(),
                Some(out.termination.clone()),
                out.turns,
                out.tool_calls.len(),
                out.usage.clone(),
                commands_of(out),
            ),
            Err(_) => (partial, None, 0, 0, spent, Vec::new()),
        };
        let (summary, truncated) = shorten(text.trim(), self.settings.summary_chars);
        let error = match &status {
            subagent::ChildStatus::Failed(e) => Some(e.clone()),
            subagent::ChildStatus::Incomplete(t) => Some(t.describe()),
            _ => None,
        };
        AgentResult {
            agent_id: info.agent_id.clone(),
            profile: info.profile.clone(),
            status: status.label().into(),
            termination,
            error,
            summary,
            summary_truncated: truncated,
            files_changed: files,
            commands,
            warnings: resolved.warnings.clone(),
            turns,
            tool_calls,
            usage,
            duration_ms: started.elapsed().as_millis() as u64,
            model: resolved.model.applied.clone(),
            reasoning: (resolved.reasoning.applied != "(none)")
                .then(|| resolved.reasoning.applied.clone()),
            workspace: info.workspace.clone(),
            transcript,
            changeset: None,
            branch: info.branch.clone(),
            skills: info.skills.clone(),
        }
    }
}

/// A skill named by a profile, as loaded for a child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLoad {
    pub name: String,
    /// `loaded`, `truncated`, `missing`, `invalid`, `skipped` (budget).
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub bytes: usize,
}

/// Load a profile's skills with Bricks' own skill loader, for the child's
/// workspace, within `max_bytes` in total. Nothing is run.
pub fn load_skills(
    names: &[String],
    workspace: &Path,
    max_bytes: usize,
) -> (String, Vec<SkillLoad>) {
    let mut text = String::new();
    let mut loads = Vec::new();
    let mut used = 0usize;
    for name in names {
        let loaded = cersei_tools::skills::discovery::load_skill(name, Some(workspace), &[]);
        let Some(skill) = loaded else {
            loads.push(SkillLoad {
                name: name.clone(),
                state: "missing".into(),
                source: None,
                revision: None,
                bytes: 0,
            });
            continue;
        };
        let content = skill.content.trim().to_string();
        if subagent::is_blank(&content) {
            loads.push(SkillLoad {
                name: name.clone(),
                state: "invalid".into(),
                source: skill.meta.path.clone(),
                revision: None,
                bytes: 0,
            });
            continue;
        }
        let revision = super::profile::revision_of(&content);
        let source = skill
            .meta
            .path
            .clone()
            .unwrap_or_else(|| format!("bundled:{}", skill.meta.name));
        let room = max_bytes.saturating_sub(used);
        if room < 64 {
            loads.push(SkillLoad {
                name: name.clone(),
                state: "skipped".into(),
                source: Some(source),
                revision: Some(revision),
                bytes: 0,
            });
            continue;
        }
        let (body, state) = if content.len() > room {
            let mut cut = room;
            while !content.is_char_boundary(cut) {
                cut -= 1;
            }
            (
                format!("{}\n[skill truncated at {room} bytes]", &content[..cut]),
                "truncated",
            )
        } else {
            (content.clone(), "loaded")
        };
        used += body.len();
        text.push_str(&format!(
            "\n\n## Skill `{}` ({source}, revision {revision})\n\n{body}",
            skill.meta.name
        ));
        loads.push(SkillLoad {
            name: name.clone(),
            state: state.into(),
            source: Some(source),
            revision: Some(revision),
            bytes: body.len(),
        });
    }
    let missing: Vec<&str> = loads
        .iter()
        .filter(|l| l.state != "loaded" && l.state != "truncated")
        .map(|l| l.name.as_str())
        .collect();
    if !missing.is_empty() {
        text.push_str(&format!(
            "\n\nSkills named by this profile that could not be loaded: {} (your tools are unchanged).",
            missing.join(", ")
        ));
    }
    (text, loads)
}

/// Shell commands the child ran, from its own tool calls.
fn commands_of(out: &AgentOutput) -> Vec<CommandRun> {
    out.tool_calls
        .iter()
        .filter(|c| matches!(c.name.as_str(), "Bash" | "PowerShell"))
        .map(|c| CommandRun {
            tool: c.name.clone(),
            command: shorten(c.input["command"].as_str().unwrap_or(""), 200).0,
            is_error: c.is_error,
            duration_ms: c.duration.as_millis() as u64,
        })
        .take(50)
        .collect()
}

/// A fresh child's system prompt: the engine's prompt for its tools, the
/// profile's instructions, and its role. Rebuilt for each child (never in
/// the history, so no compaction removes it).
pub fn child_system_prompt(profile: &AgentProfile, tools: &[String], wd: &Path) -> String {
    let base =
        crate::system_prompt::build_system_prompt(&crate::system_prompt::SystemPromptOptions {
            is_non_interactive: true,
            working_directory: Some(wd.display().to_string()),
            tools_available: tools.to_vec(),
            has_auto_compact: true,
            ..Default::default()
        });
    let mut s = base;
    s.push_str(CHILD_FRAMING);
    if !subagent::is_blank(&profile.instructions) {
        s.push_str(&format!(
            "\n\n## Profile `{}` ({})\n\n{}",
            profile.name,
            profile.source.label,
            profile.instructions.trim()
        ));
    }
    s
}

/// The text returned to the parent: compact, never the child's transcript.
pub fn render_result(r: &AgentResult) -> String {
    let mut out = format!(
        "Sub-agent {} ({}, model {}{}) — {} in {}, {} turn(s), {} tool call(s)",
        r.agent_id,
        r.profile,
        r.model,
        r.reasoning
            .as_deref()
            .map(|x| format!(", reasoning {x}"))
            .unwrap_or_default(),
        r.status,
        cersei_types::duration::display_ms_u64(r.duration_ms),
        r.turns,
        r.tool_calls
    );
    if let Some(e) = &r.error {
        out.push_str(&format!("\nReason: {e}"));
    }
    if r.summary.is_empty() {
        out.push_str("\n(no answer)");
    } else {
        out.push_str(&format!(
            "\n\n{}{}",
            r.summary,
            if r.summary_truncated {
                "\n[answer shortened; the full transcript is referenced below]"
            } else {
                ""
            }
        ));
    }
    if !r.files_changed.is_empty() {
        out.push_str(&format!(
            "\n\nFiles changed: {}",
            r.files_changed.join(", ")
        ));
    }
    if !r.commands.is_empty() {
        let cmds: Vec<String> = r
            .commands
            .iter()
            .map(|c| {
                format!(
                    "`{}` ({}, {})",
                    c.command,
                    if c.is_error { "failed" } else { "ok" },
                    cersei_types::duration::display_ms_u64(c.duration_ms)
                )
            })
            .collect();
        out.push_str(&format!("\nCommands run: {}", cmds.join("; ")));
    }
    for w in &r.warnings {
        out.push_str(&format!("\nWarning: {w}"));
    }
    if let Some(t) = &r.transcript {
        out.push_str(&format!("\nTranscript: {t}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ProfileChoice;

    fn models() -> Vec<ModelChoice> {
        let m = |sel: &str, profiles: &[&str], default: Option<&str>| ModelChoice {
            selection: sel.into(),
            name: sel.into(),
            provider: "p".into(),
            profiles: profiles
                .iter()
                .map(|p| ProfileChoice {
                    id: p.to_string(),
                    label: p.to_string(),
                })
                .collect(),
            default_profile: default.map(String::from),
            max_input_tokens: 100_000,
            context_window_tokens: None,
            input: vec!["text".into()],
            priced: false,
        };
        vec![
            m("p/fast", &["quick", "deep"], Some("quick")),
            m("p/plain", &[], None),
            m("p/custom", &["réfléchi", "high"], Some("réfléchi")),
        ]
    }

    fn parent(reasoning: Option<&str>) -> ParentModel {
        ParentModel {
            selection: "p/fast".into(),
            reasoning: reasoning.map(String::from),
        }
    }

    fn prof(model: ModelPref, reasoning: ReasoningPref) -> AgentProfile {
        let mut p = (*neutral_profile()).clone();
        p.name = "x".into();
        p.model = model;
        p.reasoning = reasoning;
        p
    }

    #[test]
    fn precedence_request_profile_parent() {
        let s = DelegationSettings::default();
        let p = prof(
            ModelPref::Explicit("p/custom".into()),
            ReasoningPref::Id("high".into()),
        );
        let r = resolve(
            &AgentSpawnRequest::default(),
            &p,
            &parent(Some("deep")),
            &models(),
            &s,
        )
        .unwrap();
        assert_eq!(r.model.applied, "p/custom");
        assert_eq!(r.reasoning.applied, "high");
        let req = AgentSpawnRequest {
            model: Some("inherit".into()),
            ..Default::default()
        };
        let r = resolve(&req, &p, &parent(Some("deep")), &models(), &s).unwrap();
        assert_eq!(r.model.applied, "p/fast");
        // The profile's `high` is not a profile of p/fast: inherited `deep`.
        assert_eq!(r.reasoning.applied, "deep");
        assert!(r.warnings[0].contains("does not have"));
    }

    #[test]
    fn custom_reasoning_ids_and_aliases() {
        let mut s = DelegationSettings::default();
        let p = prof(ModelPref::Inherit, ReasoningPref::Id("high".into()));
        // No alias: the parent's compatible profile.
        let r = resolve(
            &AgentSpawnRequest::default(),
            &p,
            &parent(Some("quick")),
            &models(),
            &s,
        )
        .unwrap();
        assert_eq!(r.reasoning.applied, "quick");
        // With an explicit alias.
        s.reasoning_aliases.insert("high".into(), "deep".into());
        let r = resolve(
            &AgentSpawnRequest::default(),
            &p,
            &parent(Some("quick")),
            &models(),
            &s,
        )
        .unwrap();
        assert_eq!(r.reasoning.applied, "deep");
        assert!(r.reasoning.reason.as_deref().unwrap().contains("alias"));
        // A custom, non-English id is accepted as is.
        let req = AgentSpawnRequest {
            model: Some("p/custom".into()),
            reasoning: Some("réfléchi".into()),
            ..Default::default()
        };
        let r = resolve(&req, &p, &parent(None), &models(), &s).unwrap();
        assert_eq!(r.reasoning.applied, "réfléchi");
    }

    #[test]
    fn inconsistent_explicit_overrides_are_refused() {
        let s = DelegationSettings::default();
        let p = prof(ModelPref::Inherit, ReasoningPref::Inherit);
        let bad_model = AgentSpawnRequest {
            model: Some("q/unknown".into()),
            ..Default::default()
        };
        let e = resolve(&bad_model, &p, &parent(None), &models(), &s).unwrap_err();
        assert!(e.contains("not configured") && e.contains("p/fast"), "{e}");
        let bad_reasoning = AgentSpawnRequest {
            reasoning: Some("high".into()),
            ..Default::default()
        };
        let e = resolve(&bad_reasoning, &p, &parent(None), &models(), &s).unwrap_err();
        assert!(e.contains("quick, deep"), "{e}");
    }

    #[test]
    fn auto_needs_a_rule() {
        let mut s = DelegationSettings::default();
        let p = prof(ModelPref::Auto, ReasoningPref::Inherit);
        let r = resolve(
            &AgentSpawnRequest::default(),
            &p,
            &parent(None),
            &models(),
            &s,
        )
        .unwrap();
        assert_eq!(r.model.applied, "p/fast");
        assert!(r.warnings.iter().any(|w| w.contains("auto_model")));
        s.auto_model = Some("p/plain".into());
        let r = resolve(
            &AgentSpawnRequest::default(),
            &p,
            &parent(Some("quick")),
            &models(),
            &s,
        )
        .unwrap();
        assert_eq!(r.model.applied, "p/plain");
        // p/plain has no reasoning profiles: the parent's id cannot apply.
        assert_eq!(r.reasoning.applied, "(none)");
    }
}
