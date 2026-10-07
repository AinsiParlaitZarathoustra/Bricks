//! The native tools: `Agent` (one sub-agent), `Agents` (several, at once),
//! `AgentControl` (instances, results, ChangeSets) and `AgentProfiles`
//! (list and search the profiles). They are adapters of the session's
//! spawner and runtime; none runs agents on its own.
//!
//! `Agent` and `Agents` ask permission at the `execute` level through the
//! normal policy; allowing them does not allow the children's own tool
//! calls, which go through the same policy one by one.

use super::runtime::{while_waiting, RuntimeHandle};
use super::spawn::{render_result, AgentSpawnRequest, AgentSpawner, BatchOptions, Spawned};
use super::workspace::{ChangeSetState, WsError};
use async_trait::async_trait;
use cersei_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

fn catalog(spawner: &AgentSpawner) -> String {
    let reg = spawner.profiles().snapshot();
    let max = spawner.settings().catalog_size.max(1);
    let usable: Vec<_> = reg.list().into_iter().filter(|l| l.valid).collect();
    let mut lines: Vec<String> = usable
        .iter()
        .take(max)
        .map(|l| {
            let d: String = l.description.chars().take(140).collect();
            format!("{}: {d}", l.name)
        })
        .collect();
    if usable.len() > max {
        lines.push(format!(
            "… {} more: use AgentProfiles to list or search them",
            usable.len() - max
        ));
    }
    lines.join("\n")
}

fn item_schema(spawner: &AgentSpawner) -> Value {
    json!({
        "task": {
            "type": "string",
            "minLength": 1,
            "description": "The complete task: goal, relevant files or facts, what done means, what to report."
        },
        "description": { "type": "string", "description": "Short label (3-5 words)." },
        "profile": {
            "type": "string",
            "description": format!("A specialist profile (optional). Available:\n{}", catalog(spawner))
        },
        "model": { "type": "string", "description": "`inherit` (default), `auto`, or `provider_id/model_id`." },
        "reasoning": { "type": "string", "description": "`inherit` (default) or a reasoning profile id of that model." },
        "context": { "type": "string", "description": "Extra context passed explicitly (bounded); the sub-agent sees nothing else of this conversation." },
        "max_turns": { "type": "integer", "minimum": 1, "maximum": spawner.settings().max_turns_cap },
        "isolation": { "type": "string", "enum": ["auto", "shared", "worktree"], "description": "`auto` (default): the shared workspace for one foreground sub-agent, a git worktree for background or parallel ones." }
    })
}

fn render_spawned(s: Spawned) -> ToolResult {
    match s {
        Spawned::Results(rs) => {
            let n = rs.len();
            let all_ok = rs.iter().all(|r| r.status == "completed");
            let text = if n == 1 {
                render_result(&rs[0])
            } else {
                rs.iter()
                    .enumerate()
                    .map(|(i, r)| format!("### agents[{i}]\n{}", render_result(r)))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            let meta = serde_json::to_value(&rs).unwrap_or(Value::Null);
            if all_ok {
                ToolResult::success(text).with_metadata(meta)
            } else {
                ToolResult::error(text).with_metadata(meta)
            }
        }
        Spawned::Handles(hs) => {
            let lines: Vec<String> = hs
                .iter()
                .map(|h| {
                    format!(
                        "- {} ({}, {}{}) admitted in the background: {}",
                        h.agent_id,
                        h.profile,
                        h.isolation,
                        h.branch
                            .as_deref()
                            .map(|b| format!(", branch {b}"))
                            .unwrap_or_default(),
                        h.task
                    )
                })
                .collect();
            ToolResult::success(format!(
                "{} sub-agent(s) started in the background. Follow them with AgentControl (status, wait, result, cancel); their ChangeSets with inspect_changes / apply_changes.\n{}",
                hs.len(),
                lines.join("\n")
            ))
            .with_metadata(serde_json::to_value(&hs).unwrap_or(Value::Null))
        }
    }
}

/// `Agent`: one sub-agent.
pub struct NativeAgentTool {
    spawner: Arc<AgentSpawner>,
}

impl NativeAgentTool {
    pub fn new(spawner: Arc<AgentSpawner>) -> Self {
        Self { spawner }
    }
}

#[async_trait]
impl Tool for NativeAgentTool {
    fn name(&self) -> &str {
        "Agent"
    }

    fn description(&self) -> &str {
        "Delegate one self-contained task to a sub-agent that works in a fresh context (it does \
         not see this conversation) with your permissions and tools, optionally as a specialist \
         profile, model or reasoning profile. Foreground (default): you wait for its compact \
         result. `background: true`: you get a handle at once (AgentControl to follow it). Use it \
         when a task is substantial and separable; do small tasks yourself."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    fn input_schema(&self) -> Value {
        let mut props = item_schema(&self.spawner);
        props["background"] = json!({ "type": "boolean", "description": "Return a handle at once; the sub-agent goes on (default false)." });
        json!({ "type": "object", "properties": props, "required": ["task"] })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let req: AgentSpawnRequest = match serde_json::from_value(input) {
            Ok(r) => r,
            Err(e) => {
                return ToolResult::error(format!(
                    "Invalid input: {e}. Nothing was started. (`system_prompt` is not accepted: \
                     choose a `profile` instead.)"
                ))
            }
        };
        let background = req.background.unwrap_or(false);
        match self
            .spawner
            .spawn_batch(
                vec![req],
                BatchOptions {
                    background,
                    fail_fast: false,
                    batch: false,
                },
                ctx,
            )
            .await
        {
            Err(e) => ToolResult::error(e.to_string()),
            Ok(s) => render_spawned(s),
        }
    }
}

/// `Agents`: several sub-agents at once.
pub struct AgentsTool {
    spawner: Arc<AgentSpawner>,
}

impl AgentsTool {
    pub fn new(spawner: Arc<AgentSpawner>) -> Self {
        Self { spawner }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentsInput {
    agents: Vec<AgentSpawnRequest>,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    fail_fast: bool,
}

#[async_trait]
impl Tool for AgentsTool {
    fn name(&self) -> &str {
        "Agents"
    }

    fn description(&self) -> &str {
        "Delegate several independent, self-contained tasks to sub-agents that run at the same \
         time (as capacity allows), each in a fresh context with your permissions and tools; \
         parallel writers each get their own git worktree, and their changes come back as \
         ChangeSets to inspect and apply. Foreground (default): all results, in the order asked. \
         `background: true`: handles at once. `fail_fast`: the first failure cancels the others."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agents": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": self.spawner.settings().max_batch,
                    "items": { "type": "object", "properties": item_schema(&self.spawner), "required": ["task"] }
                },
                "background": { "type": "boolean", "description": "Return handles at once (default false)." },
                "fail_fast": { "type": "boolean", "description": "Cancel the others when one fails (default false)." }
            },
            "required": ["agents"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let i: AgentsInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => {
                return ToolResult::error(format!("Invalid input: {e}. Nothing was started."))
            }
        };
        if i.agents.iter().any(|a| a.background.is_some()) {
            return ToolResult::error(
                "`background` is set for the whole batch, not per agent (use Agent for one \
                 background sub-agent). Nothing was started.",
            );
        }
        match self
            .spawner
            .spawn_batch(
                i.agents,
                BatchOptions {
                    background: i.background,
                    fail_fast: i.fail_fast,
                    batch: true,
                },
                ctx,
            )
            .await
        {
            Err(e) => ToolResult::error(e.to_string()),
            Ok(s) => render_spawned(s),
        }
    }
}

/// `AgentControl`: instances, results and ChangeSets.
pub struct AgentControlTool {
    spawner: Arc<AgentSpawner>,
}

impl AgentControlTool {
    pub fn new(spawner: Arc<AgentSpawner>) -> Self {
        Self { spawner }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlInput {
    action: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    changeset_id: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

const MAX_WAIT_MS: u64 = 600_000;

/// Viewer: `None` for the session's agent (sees the session), else the
/// sub-agent (sees its descendants).
fn viewer(ctx: &ToolContext) -> Option<String> {
    ctx.extensions
        .get::<super::spawn::AgentIdentity>()
        .filter(|i| i.parent_id.is_some())
        .map(|i| i.agent_id.clone())
}

pub fn render_record(r: &super::runtime::InstanceRecord) -> String {
    let mut s = format!(
        "{} · {} · {}{} · {} · depth {} · {}",
        r.info.agent_id,
        r.info.profile,
        r.state.as_str(),
        r.reason
            .as_deref()
            .map(|x| format!(" ({x})"))
            .unwrap_or_default(),
        if r.background {
            "background"
        } else {
            "foreground"
        },
        r.info.depth,
        r.info.isolation
    );
    if let Some(b) = &r.info.branch {
        s.push_str(&format!(" · branch {b}"));
    }
    if let Some(res) = &r.result {
        s.push_str(&format!(
            " · {} in {}",
            res.status,
            cersei_types::duration::display_ms_u64(res.duration_ms)
        ));
        if let Some(cs) = &res.changeset {
            s.push_str(&format!(" · ChangeSet {cs}"));
        }
    }
    s.push_str(&format!("\n  task: {}", r.info.task));
    s
}

#[async_trait]
impl Tool for AgentControlTool {
    fn name(&self) -> &str {
        "AgentControl"
    }

    fn description(&self) -> &str {
        "Follow and control your sub-agents: `list`, `status`/`result` (agent_id), `wait` \
         (agent_id, timeout_ms), `cancel` (agent_id, with its descendants); and their ChangeSets: \
         `inspect_changes`, `apply_changes` (to your working tree; a conflict writes nothing), \
         `discard_changes` (changeset_id or agent_id). Reading never starts a model request."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    /// Reads are read-only; cancel is `execute`; apply/discard write.
    fn permission_level_for(&self, input: &Value) -> PermissionLevel {
        match input["action"].as_str().unwrap_or("") {
            "list" | "status" | "result" | "wait" | "inspect_changes" => PermissionLevel::ReadOnly,
            "cancel" => PermissionLevel::Execute,
            _ => PermissionLevel::Write,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "status", "result", "wait", "cancel", "inspect_changes", "apply_changes", "discard_changes"] },
                "agent_id": { "type": "string", "description": "Required for status, result, wait, cancel; or to find an agent's ChangeSet." },
                "changeset_id": { "type": "string", "description": "For inspect/apply/discard (or give agent_id)." },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": MAX_WAIT_MS, "description": "wait: how long (default 60000 ms); a timeout stops nothing." }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let i: ControlInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolResult::error(format!("Invalid input: {e}")),
        };
        let rt = ctx
            .extensions
            .get::<RuntimeHandle>()
            .map(|h| Arc::clone(&h.0))
            .unwrap_or_else(|| Arc::clone(self.spawner.runtime()));
        let who = viewer(ctx);
        let need_agent = |id: &Option<String>| -> Result<String, ToolResult> {
            let id = id
                .clone()
                .ok_or_else(|| ToolResult::error(format!("`{}` needs `agent_id`", i.action)))?;
            if rt.record(&id).is_none() || !rt.visible_to(&id, who.as_deref()) {
                return Err(ToolResult::error(format!(
                    "no sub-agent `{id}` in your scope (only your own sub-agents and their descendants are visible)"
                )));
            }
            Ok(id)
        };
        match i.action.as_str() {
            "list" => {
                let recs: Vec<_> = rt
                    .records()
                    .into_iter()
                    .filter(|r| rt.visible_to(&r.info.agent_id, who.as_deref()))
                    .collect();
                if recs.is_empty() {
                    return ToolResult::success("No sub-agent in your scope.");
                }
                let st = rt.scheduler.stats();
                ToolResult::success(format!(
                    "{} sub-agent(s); slots {}/{} active, {} queued.\n{}",
                    recs.len(),
                    st.active,
                    st.max_concurrent,
                    st.queued,
                    recs.iter().map(render_record).collect::<Vec<_>>().join("\n")
                ))
            }
            "status" => match need_agent(&i.agent_id) {
                Err(e) => e,
                Ok(id) => ToolResult::success(render_record(&rt.record(&id).expect("checked"))),
            },
            "result" => match need_agent(&i.agent_id) {
                Err(e) => e,
                Ok(id) => {
                    let r = rt.record(&id).expect("checked");
                    match &r.result {
                        Some(res) => ToolResult::success(render_result(res))
                            .with_metadata(serde_json::to_value(res).unwrap_or(Value::Null)),
                        None => ToolResult::success(format!(
                            "{id} has no result yet ({}). Use wait.",
                            r.state.as_str()
                        )),
                    }
                }
            },
            "wait" => match need_agent(&i.agent_id) {
                Err(e) => e,
                Ok(id) => {
                    let t = Duration::from_millis(i.timeout_ms.unwrap_or(60_000).clamp(1, MAX_WAIT_MS));
                    let cancel = crate::subagent::run_token(&ctx.extensions).unwrap_or_default();
                    let r = while_waiting(&ctx.extensions, rt.wait(&id, t, &cancel)).await;
                    match r {
                        Some(rec) if rec.result.is_some() => {
                            ToolResult::success(render_result(rec.result.as_ref().expect("some")))
                        }
                        Some(rec) => ToolResult::success(format!(
                            "{id} is still {} after {} (nothing was stopped).",
                            rec.state.as_str(),
                            cersei_types::duration::display_ms(t)
                        )),
                        None => ToolResult::error(format!("{id} disappeared")),
                    }
                }
            },
            "cancel" => match need_agent(&i.agent_id) {
                Err(e) => e,
                Ok(id) => {
                    let n = rt.cancel(&id);
                    ToolResult::success(format!(
                        "cancellation requested for {id} and its descendants ({n} running)."
                    ))
                }
            },
            "inspect_changes" | "apply_changes" | "discard_changes" => {
                let cs_id = match (&i.changeset_id, &i.agent_id) {
                    (Some(c), _) => c.clone(),
                    (None, Some(_)) => match need_agent(&i.agent_id) {
                        Err(e) => return e,
                        Ok(a) => match rt.workspaces.changeset_of(&a) {
                            Some(c) => c.id,
                            None => return ToolResult::error(format!("{a} has no ChangeSet")),
                        },
                    },
                    (None, None) => return ToolResult::error("give `changeset_id` or `agent_id`"),
                };
                let Some(cs) = rt.workspaces.changeset(&cs_id) else {
                    return ToolResult::error(format!("no ChangeSet `{cs_id}`"));
                };
                if !rt.visible_to(&cs.agent_id, who.as_deref()) {
                    return ToolResult::error(format!("no ChangeSet `{cs_id}` in your scope"));
                }
                match i.action.as_str() {
                    "inspect_changes" => {
                        let files: Vec<String> = cs
                            .files
                            .iter()
                            .map(|f| match (f.added, f.removed) {
                                (Some(a), Some(r)) => format!("{} {} (+{a} −{r})", f.status, f.path),
                                _ => format!("{} {} (binary)", f.status, f.path),
                            })
                            .collect();
                        let (patch, cut) = rt.workspaces.patch_text(&cs_id, 20_000).unwrap_or_default();
                        ToolResult::success(format!(
                            "ChangeSet {} of {} · {:?} · branch {} · base {}\nFiles:\n{}\n\n{}{}",
                            cs.id,
                            cs.agent_id,
                            cs.state,
                            cs.branch,
                            &cs.base_sha[..cs.base_sha.len().min(12)],
                            files.join("\n"),
                            patch,
                            if cut { "\n[patch shortened; the full patch is in the ChangeSet file]" } else { "" }
                        ))
                    }
                    "apply_changes" => match rt.workspaces.apply(&cs_id, &ctx.working_dir).await {
                        Ok(done) => {
                            rt.emit(crate::events::AgentEvent::SubAgent(
                                super::spawn::SubAgentEvent::ChangesUpdated {
                                    changeset_id: cs_id.clone(),
                                    state: ChangeSetState::Applied,
                                    files: done.files.iter().map(|f| f.path.clone()).collect(),
                                    detail: None,
                                },
                            ));
                            ToolResult::success(format!(
                                "ChangeSet {cs_id} applied to your working tree ({} file(s)): {}. Nothing was committed.",
                                done.files.len(),
                                done.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>().join(", ")
                            ))
                        }
                        Err(WsError::Conflict { files, detail }) => {
                            rt.emit(crate::events::AgentEvent::SubAgent(
                                super::spawn::SubAgentEvent::ChangesUpdated {
                                    changeset_id: cs_id.clone(),
                                    state: ChangeSetState::Conflict,
                                    files: files.clone(),
                                    detail: Some(detail.clone()),
                                },
                            ));
                            ToolResult::error(
                                WsError::Conflict { files, detail }.to_string()
                                    + " Both versions are kept: your working tree, and the ChangeSet.",
                            )
                        }
                        Err(e) => ToolResult::error(e.to_string()),
                    },
                    _ => match rt.workspaces.discard(&cs_id).await {
                        Ok(_) => {
                            rt.emit(crate::events::AgentEvent::SubAgent(
                                super::spawn::SubAgentEvent::ChangesUpdated {
                                    changeset_id: cs_id.clone(),
                                    state: ChangeSetState::Discarded,
                                    files: Vec::new(),
                                    detail: None,
                                },
                            ));
                            ToolResult::success(format!(
                                "ChangeSet {cs_id} discarded: its worktree is removed; its patch stays with the session as a record."
                            ))
                        }
                        Err(e) => ToolResult::error(e.to_string()),
                    },
                }
            }
            other => ToolResult::error(format!(
                "unknown action `{other}`: list, status, result, wait, cancel, inspect_changes, apply_changes, discard_changes"
            )),
        }
    }
}

/// `AgentProfiles`: list or search the profiles (paged).
pub struct AgentProfilesTool {
    spawner: Arc<AgentSpawner>,
}

impl AgentProfilesTool {
    pub fn new(spawner: Arc<AgentSpawner>) -> Self {
        Self { spawner }
    }
}

#[async_trait]
impl Tool for AgentProfilesTool {
    fn name(&self) -> &str {
        "AgentProfiles"
    }

    fn description(&self) -> &str {
        "List or search the sub-agent profiles usable with Agent and Agents: name, description, \
         source (project, user, built-in) and any invalid profile with its error."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Words of the name or description (empty: all)." },
                "page": { "type": "integer", "minimum": 0, "description": "Page (20 per page), from 0." }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        let page = input["page"].as_u64().unwrap_or(0) as usize;
        let reg = self.spawner.profiles().snapshot();
        let (items, total) = reg.search(query, page, 20);
        let mut out = format!("{total} profile(s) match; page {page}:\n");
        for l in &items {
            if l.valid {
                out.push_str(&format!(
                    "- {} [{}]: {}\n",
                    l.name,
                    l.scope.as_str(),
                    l.description
                ));
            } else {
                out.push_str(&format!(
                    "- {} [{}] INVALID ({}): {}\n",
                    l.name,
                    l.scope.as_str(),
                    l.source,
                    l.error.as_deref().unwrap_or("")
                ));
            }
        }
        if (page + 1) * 20 < total {
            out.push_str(&format!("(next: page {})\n", page + 1));
        }
        ToolResult::success(out)
    }
}
