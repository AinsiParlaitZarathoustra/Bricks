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
