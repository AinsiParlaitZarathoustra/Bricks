//! Decision, hooks and execution of one call.

use bricks_hooks::{Hook, HookAction, HookContext, HookEvent};
use bricks_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use bricks_tools::preview::{ApprovedState, ChangePreview, is_stale_approval};
use bricks_tools::{ToolContext, ToolResult};
use std::sync::Arc;

/// How many times a call is decided when the file it would change moves on
/// while the decision is pending or after it. Past this, nothing is written.
const MAX_DECISION_ROUNDS: u32 = 3;

/// What a call needs from its round to be decided and run.
pub(super) struct CallSite<'a> {
    pub(super) tool_id: &'a str,
    pub(super) tool_ctx: &'a ToolContext,
    pub(super) permissions: &'a dyn PermissionPolicy,
    pub(super) hooks: &'a [Arc<dyn Hook>],
    pub(super) turn: u32,
    pub(super) cumulative_cost: f64,
    pub(super) message_count: usize,
}

/// What a call came to.
pub(super) struct CallRun {
    pub(super) result: ToolResult,
    /// The tool ran (it may have changed files).
    pub(super) executed: bool,
    /// The change the call was approved for, and executed.
    pub(super) change: Option<ChangePreview>,
}

impl CallRun {
    pub(super) fn not_run(result: ToolResult) -> Self {
        Self {
            result,
            executed: false,
            change: None,
        }
    }
}

/// Decides a call and runs it.
///
/// The decision is taken on the change the call would make, computed without
/// writing. The hooks then see the call. When a hook changes the input and
/// the change differs from the approved one, the new input needs its own
/// decision. The tool itself checks the approval under the file's lock before
/// it writes (see `bricks_tools::file_lock`): a file that moved on since the
/// approval is not written, and the call is decided again on its current
/// state. The change executed is therefore always the change approved.
pub(super) async fn decide_and_run(
    site: &CallSite<'_>,
    tool: &dyn bricks_tools::Tool,
    tool_name: &str,
    input: serde_json::Value,
) -> CallRun {
    let mut input = input;
    // Set once a hook changed the input: that input was decided again and
    // is what runs (the hook is not run a second time on its own output).
    let mut hooked = false;
    for _ in 0..MAX_DECISION_ROUNDS {
        let (decision, approved) = decide(site, tool, tool_name, &input).await;
        if let PermissionDecision::Deny(reason) = decision {
            return CallRun::not_run(ToolResult::error(format!("Permission denied: {reason}")));
        }
        if hooked {
            let mut exec_ctx = site.tool_ctx.clone();
            if let Some(change) = &approved {
                exec_ctx.extensions = exec_ctx.extensions.with_local(ApprovedState::of(change));
            }
            let result = execute_admitted(tool, input.clone(), &exec_ctx).await;
            if is_stale_approval(&result) {
                continue;
            }
            return CallRun {
                result,
                executed: true,
                change: approved,
            };
        }

        let hook_ctx = HookContext {
            event: HookEvent::PreToolUse,
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(input.clone()),
            tool_result: None,
            tool_is_error: None,
            turn: site.turn,
            cumulative_cost_usd: site.cumulative_cost,
            message_count: site.message_count,
        };
        let (effective, modified) = match bricks_hooks::run_hooks(site.hooks, &hook_ctx).await {
            HookAction::Block(reason) => {
                return CallRun::not_run(ToolResult::error(format!("Blocked by hook: {}", reason)));
            }
            HookAction::ModifyInput(new_input) => (new_input, true),
            _ => (input.clone(), false),
        };
        if modified {
            // Other input, other call: decided again, even when its file
            // changes look the same.
            input = effective;
            hooked = true;
            continue;
        }

        let mut exec_ctx = site.tool_ctx.clone();
        if let Some(change) = &approved {
            exec_ctx.extensions = exec_ctx.extensions.with_local(ApprovedState::of(change));
        }
        let result = execute_admitted(tool, effective.clone(), &exec_ctx).await;
        if is_stale_approval(&result) {
            input = effective;
            continue;
        }
        return CallRun {
            result,
            executed: true,
            change: approved,
        };
    }
    CallRun::not_run(ToolResult::error(
        "The file changed on disk while the change was awaiting approval, so nothing was \
         written. Read the file again and retry.",
    ))
}

/// The decision on a call, and the change it was given for (`None` when the
/// call was not allowed, or has no change to show).
///
/// The preview is computed without writing. If the files it describes move on
/// while the decision is pending, the preview is computed again and asked
/// again, up to [`MAX_DECISION_ROUNDS`].
async fn decide(
    site: &CallSite<'_>,
    tool: &dyn bricks_tools::Tool,
    tool_name: &str,
    input: &serde_json::Value,
) -> (PermissionDecision, Option<ChangePreview>) {
    let mut description = format!("Execute tool '{}'", tool_name);
    if let Some(details) = tool.permission_details(input, site.tool_ctx).await {
        description.push('\n');
        description.push_str(&details);
    }
    let mut preview = tool.preview(input, site.tool_ctx).await;
    let mut stale_rounds = 0;
    loop {
        if let Some(reason) = preview.as_ref().and_then(|p| p.refusal.clone()) {
            // The tool would refuse it: nothing to approve.
            return (
                PermissionDecision::Deny(format!("{reason}\nNothing was written.")),
                None,
            );
        }
        let request = PermissionRequest {
            tool_name: tool_name.to_string(),
            tool_input: input.clone(),
            permission_level: tool.permission_level_for(input),
            category: tool.category(),
            description: description.clone(),
            id: site.tool_id.to_string(),
            preview: preview.clone(),
            agent_id: None,
            policy_revision: None,
        };
        let decision = site.permissions.check(&request).await;
        let allowed = matches!(
            decision,
            PermissionDecision::Allow | PermissionDecision::AllowOnce
        );
        match preview.as_ref().map(|p| p.check_current()) {
            Some(Err(changed)) if allowed => {
                stale_rounds += 1;
                if stale_rounds >= MAX_DECISION_ROUNDS {
                    return (
                        PermissionDecision::Deny(format!(
                            "{} changed on disk while the change was awaiting approval; \
                             nothing was written. Read the file again and retry.",
                            changed.join(", ")
                        )),
                        None,
                    );
                }
                preview = tool.preview(input, site.tool_ctx).await;
            }
            _ => {
                let approved = if allowed { preview } else { None };
                return (decision, approved);
            }
        }
    }
}

/// Run a tool, admitted as a writer of the workspace when it may write
/// (level `write`, `execute` or `dangerous`): while a sub-agent works in
/// this checkout, another agent's writing call waits for it. A delegation
/// call holds nothing for its parent (its child does), so a parent waiting
/// for its child cannot deadlock.
async fn execute_admitted(
    tool: &dyn bricks_tools::Tool,
    input: serde_json::Value,
    ctx: &ToolContext,
) -> ToolResult {
    use bricks_tools::PermissionLevel as L;
    let writes = matches!(
        tool.permission_level_for(&input),
        L::Write | L::Execute | L::Dangerous
    ) && !crate::subagent::DELEGATION_TOOLS.contains(&tool.name());
    if !writes {
        return tool.execute(input, ctx).await;
    }
    if let Some(other) = another_agents_worktree(&input, ctx) {
        return ToolResult::error(format!(
            "`{}` is in {other}'s worktree: another agent's worktree is read-only for you \
             (Read, Grep, CodeScout, CodeSearch, CodeStats). Write in your own workspace.",
            other_path(&input, ctx).unwrap_or_default()
        ));
    }
    // A child of the shared workspace takes it at its first writing call
    // and keeps it to its end: its edits are never interleaved with
    // another's. A child that only reads never blocks anyone.
    if let Some(lease) = ctx.extensions.get::<crate::agents::runtime::LeaseHandle>()
        && lease.0.writer_dir() == Some(ctx.working_dir.as_path())
    {
        let timeout = crate::agents::runtime::WRITER_CLAIM_TIMEOUT;
        return match lease.0.claim_writer(timeout).await {
            Ok(true) => tool.execute(input, ctx).await,
            Ok(false) => ToolResult::error("cancelled while waiting for the workspace"),
            Err(holder) => ToolResult::error(format!(
                "the workspace stayed held by another writer ({holder}) for {}: nothing was \
                 written. Go on reading, or try again later.",
                bricks_types::duration::display_ms(timeout)
            )),
        };
    }
    let holder = ctx
        .extensions
        .get::<crate::agents::AgentIdentity>()
        .map(|i| i.agent_id.clone())
        .unwrap_or_else(|| "agent".into());
    let cancel = crate::subagent::run_token(&ctx.extensions).unwrap_or_default();
    let gate = crate::agents::admission::writers_for(&ctx.working_dir);
    match gate.acquire(&holder, &cancel, |_| {}).await {
        Some(_guard) => tool.execute(input, ctx).await,
        None => {
            ToolResult::error("cancelled while waiting for another agent writing in this workspace")
        }
    }
}

/// The paths a writing call names (`file_path`, `path`).
fn named_paths(input: &serde_json::Value, ctx: &ToolContext) -> Vec<std::path::PathBuf> {
    ["file_path", "path"]
        .iter()
        .filter_map(|k| input[*k].as_str())
        .filter(|p| !p.trim().is_empty())
        .map(|p| resolved(&ctx.working_dir.join(p)))
        .collect()
}

/// `path` with its `.` and `..` folded and its existing part's links
/// resolved: `../other/x` names the folder it reaches.
fn resolved(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut folded = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                folded.pop();
            }
            Component::CurDir => {}
            other => folded.push(other),
        }
    }
    // The deepest existing ancestor, through its links; the rest as written.
    let mut existing = folded.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => rest.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut out = std::fs::canonicalize(&existing).unwrap_or(existing);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

/// The other sub-agent whose worktree a writing call targets, if any: those
/// are read-only for everyone else.
fn another_agents_worktree(input: &serde_json::Value, ctx: &ToolContext) -> Option<String> {
    let rt = ctx.extensions.get::<crate::agents::RuntimeHandle>()?;
    let me = ctx
        .extensions
        .get::<crate::agents::AgentIdentity>()
        .map(|i| i.agent_id.clone());
    let paths = named_paths(input, ctx);
    rt.0.records().into_iter().find_map(|r| {
        // A worktree really made (it has its branch), of another agent.
        if r.info.isolation != "worktree"
            || r.info.branch.is_none()
            || Some(&r.info.agent_id) == me.as_ref()
        {
            return None;
        }
        let root = resolved(std::path::Path::new(&r.info.workspace));
        let own = resolved(&ctx.working_dir);
        (!own.starts_with(&root) && paths.iter().any(|p| p.starts_with(&root)))
            .then(|| r.info.agent_id.clone())
    })
}

fn other_path(input: &serde_json::Value, ctx: &ToolContext) -> Option<String> {
    named_paths(input, ctx)
        .first()
        .map(|p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::admission::writers_for;
    use crate::agents::runtime::{
        ActivityLease, AgentRuntime, LeaseHandle, Limits, RuntimeHandle, Scheduler,
        WRITER_CLAIM_TIMEOUT,
    };
    use crate::agents::spawn::{AgentIdentity, SpawnInfo};
    use crate::agents::workspace::WorkspaceManager;
    use async_trait::async_trait;
    use bricks_tools::{CostTracker, Extensions, PermissionLevel, Tool};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    /// A tool that touches no disk: it counts the calls it was let run.
    struct Fake {
        level: PermissionLevel,
        calls: Arc<AtomicUsize>,
    }

    fn fake(level: PermissionLevel) -> (Fake, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Fake {
                level,
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }

    #[async_trait]
    impl Tool for Fake {
        fn name(&self) -> &str {
            "Fake"
        }
        fn description(&self) -> &str {
            "counts its calls"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn permission_level(&self) -> PermissionLevel {
            self.level
        }
        async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::success("done")
        }
    }

    fn limits() -> Limits {
        Limits {
            max_concurrent: 4,
            max_depth: 3,
            max_queued: 4,
            admission_timeout: Duration::from_secs(5),
            background_drain: Duration::from_secs(5),
        }
    }

    /// A sub-agent record of the session, in `isolation` at `workspace`.
    fn record(id: &str, isolation: &str, workspace: &Path) -> SpawnInfo {
        // A worktree made for it carries its branch.
        let branch = (isolation == "worktree").then(|| format!("bricks/{id}"));
        serde_json::from_value(serde_json::json!({
            "branch": branch,
            "agent_id": id, "root_run_id": "run_1", "parent_id": "main_1",
            "profile": "p", "profile_source": "s", "profile_revision": "r",
            "model": {"requested": "inherit", "applied": "m"},
            "reasoning": {"requested": "inherit", "applied": "(none)"},
            "workspace": workspace.display().to_string(), "isolation": isolation,
            "task": "t", "created_at": "now", "depth": 1
        }))
        .unwrap()
    }

    fn runtime(records: &[(&str, &str, &Path)]) -> Arc<AgentRuntime> {
        let ws = Arc::new(WorkspaceManager::new(None, Default::default()));
        let rt = AgentRuntime::new(limits(), ws, None);
        for (id, isolation, dir) in records {
            rt.register(record(id, isolation, dir), false, CancellationToken::new());
        }
        rt
    }

    /// The context of a call by `who` (`None`: the session's agent), in
    /// `dir`, with the lease of a shared-workspace child when given.
    fn ctx(
        rt: &Arc<AgentRuntime>,
        dir: &Path,
        who: Option<&str>,
        lease: Option<Arc<ActivityLease>>,
    ) -> ToolContext {
        let extensions = Extensions::default();
        extensions.insert(RuntimeHandle(Arc::clone(rt)));
        if let Some(id) = who {
            extensions.insert(AgentIdentity {
                agent_id: id.into(),
                parent_id: Some("main_1".into()),
                root_run_id: "run_1".into(),
            });
        }
        if let Some(l) = lease {
            extensions.insert(LeaseHandle(l));
        }
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "main_1".into(),
            permissions: Arc::new(bricks_tools::permissions::AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions,
        }
    }

    fn write_to(path: &Path) -> serde_json::Value {
        serde_json::json!({"file_path": path.display().to_string(), "content": "x"})
    }

    /// A session's folders: the worktrees of `sub_0000` (the caller) and of
    /// `sub_0001`, and a plain folder. Returns the base to keep alive.
    fn layout() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let base = tempfile::tempdir().unwrap();
        let me = base.path().join("wt_sub_0000");
        let other = base.path().join("wt_sub_0001");
        let plain = base.path().join("notes");
        for d in [&me, &other, &plain] {
            std::fs::create_dir_all(d).unwrap();
        }
        (base, me, other, plain)
    }

    #[tokio::test]
    async fn a_write_into_another_agents_worktree_is_refused_and_runs_nothing() {
        let (base, me, other, _) = layout();
        let rt = runtime(&[
            ("sub_0000", "worktree", &me),
            ("sub_0001", "worktree", &other),
        ]);
        let (tool, calls) = fake(PermissionLevel::Write);
        let r = execute_admitted(
            &tool,
            write_to(&other.join("src/lib.rs")),
            &ctx(&rt, &me, Some("sub_0000"), None),
        )
        .await;
        assert!(r.is_error, "{}", r.content);
        assert!(
            r.content.contains("sub_0001's worktree") && r.content.contains("read-only for you"),
            "{}",
            r.content
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the tool did not run");
        // The session's agent, in the plain checkout, is refused the same way.
        let r = execute_admitted(
            &tool,
            write_to(&other.join("src/lib.rs")),
            &ctx(&rt, base.path(), None, None),
        )
        .await;
        assert!(r.is_error, "{}", r.content);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_write_in_its_own_worktree_or_outside_every_worktree_passes() {
        let (base, me, _, plain) = layout();
        let rt = runtime(&[
            ("sub_0000", "worktree", &me),
            ("sub_0001", "worktree", &base.path().join("wt_sub_0001")),
        ]);
        let (tool, calls) = fake(PermissionLevel::Write);
        let own = execute_admitted(
            &tool,
            write_to(&me.join("src/lib.rs")),
            &ctx(&rt, &me, Some("sub_0000"), None),
        )
        .await;
        assert!(!own.is_error, "{}", own.content);
        let outside = execute_admitted(
            &tool,
            write_to(&plain.join("notes.md")),
            &ctx(&rt, &me, Some("sub_0000"), None),
        )
        .await;
        assert!(!outside.is_error, "{}", outside.content);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// Reading another agent's worktree is what it is for: only writes are refused.
    #[tokio::test]
    async fn reading_another_agents_worktree_is_allowed() {
        let (_base, me, other, _) = layout();
        let rt = runtime(&[
            ("sub_0000", "worktree", &me),
            ("sub_0001", "worktree", &other),
        ]);
        let (tool, calls) = fake(PermissionLevel::ReadOnly);
        let r = execute_admitted(
            &tool,
            write_to(&other.join("src/lib.rs")),
            &ctx(&rt, &me, Some("sub_0000"), None),
        )
        .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// A refused write into another worktree claims nothing in the shared
    /// workspace either.
    #[tokio::test]
    async fn a_refused_write_into_another_worktree_claims_no_workspace() {
        let (_base, me, other, _) = layout();
        let rt = runtime(&[
            ("sub_0000", "shared", &me),
            ("sub_0001", "worktree", &other),
        ]);
        let scheduler = Scheduler::new(limits());
        let cancel = CancellationToken::new();
        let slot = scheduler.acquire(&cancel, || {}).await.unwrap();
        let lease = ActivityLease::shared(
            Arc::clone(&scheduler),
            "sub_0000".into(),
            slot,
            me.clone(),
            cancel.clone(),
        );
        let (tool, calls) = fake(PermissionLevel::Write);
        let r = execute_admitted(
            &tool,
            write_to(&other.join("src/lib.rs")),
            &ctx(&rt, &me, Some("sub_0000"), Some(Arc::clone(&lease))),
        )
        .await;
        assert!(r.is_error, "{}", r.content);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(writers_for(&me).holder().is_none(), "nothing claimed");
        lease.release().await;
    }

    /// A shared-workspace child takes the workspace at its first writing call,
    /// not at its first call: reads before it hold nothing.
    #[tokio::test]
    async fn a_shared_child_claims_the_workspace_at_its_first_write() {
        let (_base, me, _, _) = layout();
        let rt = runtime(&[("sub_0000", "shared", &me)]);
        let scheduler = Scheduler::new(limits());
        let cancel = CancellationToken::new();
        let slot = scheduler.acquire(&cancel, || {}).await.unwrap();
        let lease = ActivityLease::shared(
            Arc::clone(&scheduler),
            "sub_0000".into(),
            slot,
            me.clone(),
            cancel.clone(),
        );
        let c = ctx(&rt, &me, Some("sub_0000"), Some(Arc::clone(&lease)));
        let (read, read_calls) = fake(PermissionLevel::ReadOnly);
        let r = execute_admitted(&read, write_to(&me.join("a.rs")), &c).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(read_calls.load(Ordering::SeqCst), 1);
        assert!(writers_for(&me).holder().is_none(), "a read holds nothing");
        let (write, write_calls) = fake(PermissionLevel::Write);
        let r = execute_admitted(&write, write_to(&me.join("a.rs")), &c).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(write_calls.load(Ordering::SeqCst), 1);
        assert_eq!(writers_for(&me).holder().as_deref(), Some("sub_0000"));
        lease.release().await;
        assert!(writers_for(&me).holder().is_none());
    }

    /// Another writer keeps the shared workspace: the write waits the claim
    /// timeout (virtual time here), then is refused with the holder's name and
    /// runs nothing.
    #[tokio::test(start_paused = true)]
    async fn a_write_waits_for_the_shared_workspace_then_is_refused_with_its_holder() {
        let (_base, me, _, _) = layout();
        let rt = runtime(&[("sub_0000", "shared", &me)]);
        let scheduler = Scheduler::new(limits());
        let cancel = CancellationToken::new();
        let slot = scheduler.acquire(&cancel, || {}).await.unwrap();
        let _other = writers_for(&me)
            .acquire("sub_0009", &cancel, |_| {})
            .await
            .unwrap();
        let lease = ActivityLease::shared(
            Arc::clone(&scheduler),
            "sub_0000".into(),
            slot,
            me.clone(),
            cancel.clone(),
        );
        let (write, calls) = fake(PermissionLevel::Write);
        let started = tokio::time::Instant::now();
        let r = execute_admitted(
            &write,
            write_to(&me.join("a.rs")),
            &ctx(&rt, &me, Some("sub_0000"), Some(Arc::clone(&lease))),
        )
        .await;
        assert!(r.is_error, "{}", r.content);
        assert!(
            r.content.contains("sub_0009") && r.content.contains("nothing was written"),
            "{}",
            r.content
        );
        assert_eq!(started.elapsed(), WRITER_CLAIM_TIMEOUT);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        lease.release().await;
    }

    /// A `..` segment from the caller's own worktree into another one is
    /// seen: the paths are compared folded and resolved.
    #[tokio::test]
    async fn a_dotdot_path_into_another_worktree_is_refused_too() {
        let (_base, me, other, _) = layout();
        let rt = runtime(&[
            ("sub_0000", "worktree", &me),
            ("sub_0001", "worktree", &other),
        ]);
        let (tool, calls) = fake(PermissionLevel::Write);
        let escaping = me.join("../wt_sub_0001/src/lib.rs");
        let r = execute_admitted(
            &tool,
            write_to(&escaping),
            &ctx(&rt, &me, Some("sub_0000"), None),
        )
        .await;
        assert!(r.is_error, "{}", r.content);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
