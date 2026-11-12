//! The native tools: `Agent` (one mission or several, each in the
//! background), `AgentControl` (instances, results, ChangeSets) and
//! `AgentProfiles` (list and search the profiles). They are adapters of the
//! session's spawner and runtime; none runs agents on its own.
//!
//! `Agent` is a delegation: the session's policy allows it (EXECUTE, or
//! CUSTOM with `Agent` set to Allow) or refuses it at once (ASK); it never
//! asks. A launched sub-agent asks for nothing either; Deny rules still
//! hold for it.

use super::runtime::{RuntimeHandle, while_waiting};
use super::spawn::{AgentSpawnRequest, AgentSpawner, BatchOptions, Spawned, render_result};
use super::workspace::{ChangeSetState, WsError};
use async_trait::async_trait;
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn item_schema() -> Value {
    json!({
        "task": {
            "type": "string",
            "minLength": 1,
            "description": "The complete task: goal, relevant files or facts, what done means, what to report."
        },
        "description": { "type": "string", "description": "Short label (3-5 words)." },
        "profile": {
            "type": "string",
            "description": "Optional specialist profile. Use AgentProfiles to list or search profiles."
        },
        "model": { "type": "string", "description": "`inherit` (default), `auto`, or `provider_id/model_id`." },
        "reasoning": { "type": "string", "description": "`inherit` (default) or a reasoning profile id of that model." },
        "context": { "type": "string", "description": "Extra context passed explicitly (bounded); the sub-agent sees nothing else of this conversation." },
        "isolation": { "type": "string", "enum": ["auto", "shared", "worktree"], "description": "`auto` (default) or `worktree`: its own git worktree, since sub-agents run in parallel (outside a git repository, choose `shared`); `shared`: this workspace, one writer at a time." }
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
                        "- {} ({}, {}{}): {}",
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
                "{} sub-agent(s) started in the background. Each report reaches you between two turns as it ends; `AgentControl wait` waits for the next one when you have nothing else to do. Messagent writes to them.\n{}",
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
        "Delegate one mission, or several with `missions`, to sub-agents working in the \
         background. Returns their ids at once; each report reaches you as it ends. A sub-agent \
         starts with a fresh context: give it everything it needs."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    fn input_schema(&self) -> Value {
        let mut properties = item_schema();
        properties["missions"] = json!({
            "type": "array",
            "minItems": 1,
            "maxItems": MAX_MISSIONS,
            "description": "Several missions at once (16 at most), instead of `task`: each one starts its own sub-agent.",
            "items": { "type": "object", "properties": item_schema(), "required": ["task"] }
        });
        json!({ "type": "object", "properties": properties })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let requests = match missions(input.clone()) {
            Ok(r) => r,
            Err(e) => {
                return bricks_tools::tool_feedback::invalid_input(
                    self,
                    &input,
                    format!(
                        "{e}. Nothing was started. (`system_prompt` is not accepted: choose a \
                         `profile` instead.)"
                    ),
                );
            }
        };
        let batch = requests.len() > 1;
        match self
            .spawner
            .spawn_batch(
                requests,
                BatchOptions {
                    background: true,
                    fail_fast: false,
                    batch,
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

/// Missions of one `Agent` call, at most: every report then fits the
/// orchestrator's queue.
const MAX_MISSIONS: usize = 16;

/// The missions of an `Agent` call: one (`task` and its options) or a list
/// (`missions`), never both.
fn missions(input: Value) -> Result<Vec<AgentSpawnRequest>, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Many {
        missions: Vec<AgentSpawnRequest>,
    }
    if input.get("missions").is_some() {
        let many: Many = serde_json::from_value(input).map_err(|e| e.to_string())?;
        if many.missions.is_empty() {
            return Err("`missions` is empty".into());
        }
        if many.missions.len() > MAX_MISSIONS {
            return Err(format!(
                "{} missions at once; {MAX_MISSIONS} at most",
                many.missions.len()
            ));
        }
        return Ok(many.missions);
    }
    let one: AgentSpawnRequest = serde_json::from_value(input).map_err(|e| e.to_string())?;
    Ok(vec![one])
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn one_mission_or_a_list_never_both() {
        assert_eq!(
            missions(json!({"task": "Inspect the parser"}))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            missions(json!({"missions": [{"task": "a"}, {"task": "b"}]}))
                .unwrap()
                .len(),
            2
        );
        assert!(missions(json!({"missions": []})).is_err());
        assert!(missions(json!({"missions": [{"task": "a"}], "task": "b"})).is_err());
        assert!(missions(json!({"agents": [{"task": "a"}]})).is_err());
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
    /// `resume`: what the ended sub-agent should do now.
    #[serde(default)]
    prompt: Option<String>,
}

const MAX_WAIT_MS: u64 = 600_000;

/// Viewer: `None` for the session's agent (sees the session), else the
/// sub-agent (sees its descendants).
pub(super) fn viewer(ctx: &ToolContext) -> Option<String> {
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
            bricks_types::duration::display_ms_u64(res.duration_ms)
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
        "Follow and control your sub-agents: `list`, `status`/`result` (agent_id), `wait` (until \
         the next report or message, or agent_id's end; timeout_ms), `resume` (agent_id, prompt: \
         an ended sub-agent continues its own conversation), `cancel` (agent_id); and their \
         ChangeSets, never applied by themselves: `inspect_changes` (files and diff), \
         `check_changes` (would it apply now? nothing written), `apply_changes` (to your working \
         tree; a conflict writes nothing), `discard_changes` (changeset_id or agent_id)."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    /// Reads are read-only; cancel is `execute`; apply/discard write.
    fn permission_level_for(&self, input: &Value) -> PermissionLevel {
        match input["action"].as_str().unwrap_or("") {
            "list" | "status" | "result" | "wait" | "inspect_changes" | "check_changes" => {
                PermissionLevel::ReadOnly
            }
            "cancel" | "resume" => PermissionLevel::Execute,
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
                "action": { "type": "string", "enum": ["list", "status", "result", "wait", "resume", "cancel", "inspect_changes", "check_changes", "apply_changes", "discard_changes"] },
                "agent_id": { "type": "string", "description": "Required for status, result, resume, cancel; for wait, the sub-agent whose end ends it; or to find an agent's ChangeSet." },
                "prompt": { "type": "string", "description": "resume: what the ended sub-agent should do now." },
                "changeset_id": { "type": "string", "description": "For inspect/apply/discard (or give agent_id)." },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": MAX_WAIT_MS, "description": "wait: how long at most (default 60000 ms); a timeout stops nothing." }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let i: ControlInput = match bricks_tools::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
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
                    recs.iter()
                        .map(render_record)
                        .collect::<Vec<_>>()
                        .join("\n")
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
            "wait" => {
                let ids = match &i.agent_id {
                    None => Vec::new(),
                    Some(_) => match need_agent(&i.agent_id) {
                        Err(e) => return e,
                        Ok(id) => vec![id],
                    },
                };
                let t = Duration::from_millis(i.timeout_ms.unwrap_or(60_000).clamp(1, MAX_WAIT_MS));
                let cancel = crate::subagent::run_token(&ctx.extensions).unwrap_or_default();
                let me = who
                    .clone()
                    .unwrap_or_else(|| super::messagent::MAIN.to_string());
                let outcome =
                    while_waiting(&ctx.extensions, rt.wait_event(&me, &ids, t, &cancel)).await;
                use super::runtime::WaitOutcome as W;
                match outcome {
                    W::Delivered(n) => ToolResult::success(format!(
                        "{n} new delivery(ies) for you (reports or messages): you read them now."
                    )),
                    W::Ended(id) => ToolResult::success(format!(
                        "{id} has ended; AgentControl result {id} gives its report."
                    )),
                    W::TimedOut => ToolResult::success(format!(
                        "nothing new after {} (nothing was stopped).",
                        bricks_types::duration::display_ms(t)
                    )),
                    W::Cancelled => ToolResult::error("the wait was cancelled."),
                }
            }
            "resume" => match need_agent(&i.agent_id) {
                Err(e) => e,
                Ok(id) => {
                    let prompt = i.prompt.clone().unwrap_or_default();
                    match self.spawner.resume(&id, &prompt, ctx, who.as_deref()).await {
                        Ok(info) => ToolResult::success(format!(
                            "{id} resumed (execution {}): it continues its own conversation in \
                             parallel. Use wait for its result.",
                            rt.execution(&id).unwrap_or(1)
                        ))
                        .with_metadata(serde_json::to_value(&info).unwrap_or(Value::Null)),
                        Err(e) => ToolResult::error(e.to_string()),
                    }
                }
            },
            "cancel" => match need_agent(&i.agent_id) {
                Err(e) => e,
                Ok(id) => {
                    rt.mark_stopped(&id);
                    let n = rt.cancel(&id);
                    ToolResult::success(format!(
                        "cancellation requested for {id} and its descendants ({n} running)."
                    ))
                }
            },
            "inspect_changes" | "check_changes" | "apply_changes" | "discard_changes" => {
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
                                (Some(a), Some(r)) => {
                                    format!("{} {} (+{a} −{r})", f.status, f.path)
                                }
                                _ => format!("{} {} (binary)", f.status, f.path),
                            })
                            .collect();
                        let (patch, cut) =
                            rt.workspaces.patch_text(&cs_id, 20_000).unwrap_or_default();
                        ToolResult::success(format!(
                            "ChangeSet {} of {} · {:?} · branch {} · base {}\nFiles:\n{}\n\n{}{}",
                            cs.id,
                            cs.agent_id,
                            cs.state,
                            cs.branch,
                            &cs.base_sha[..cs.base_sha.len().min(12)],
                            files.join("\n"),
                            patch,
                            if cut {
                                "\n[patch shortened; the full patch is in the ChangeSet file]"
                            } else {
                                ""
                            }
                        ))
                    }
                    "check_changes" => match rt.workspaces.check(&cs_id, &ctx.working_dir).await {
                        Ok(()) => ToolResult::success(format!(
                            "ChangeSet {cs_id} applies cleanly to your working tree now; nothing was written. Review its diff before applying it."
                        )),
                        Err(WsError::Conflict { files, detail }) => ToolResult::error(format!(
                            "ChangeSet {cs_id} would conflict in: {}. Nothing was written.\n{detail}",
                            files.join(", ")
                        )),
                        Err(e) => ToolResult::error(e.to_string()),
                    },
                    "apply_changes" => match rt.workspaces.apply(&cs_id, &ctx.working_dir).await {
                        Ok(done) => {
                            rt.changeset_applied(&done);
                            ToolResult::success(format!(
                                "ChangeSet {cs_id} applied to your working tree ({} file(s)): {}. Nothing was committed.",
                                done.files.len(),
                                done.files
                                    .iter()
                                    .map(|f| f.path.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
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
                "unknown action `{other}`: list, status, result, wait, resume, cancel, inspect_changes, check_changes, apply_changes, discard_changes"
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
        "List or search the sub-agent profiles usable with Agent: name, description, \
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
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            #[serde(default)]
            query: String,
            #[serde(default)]
            page: usize,
        }
        let Input { query, page } = match bricks_tools::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let reg = self.spawner.profiles().snapshot();
        let (items, total) = reg.search(&query, page, 20);
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
        if page.saturating_add(1).saturating_mul(20) < total {
            out.push_str(&format!("(next: page {})\n", page + 1));
        }
        ToolResult::success(out)
    }
}
