//! Tool calls of a round: running or finished, published, and their results.

use super::dispatch::{CallRun, CallSite, decide_and_run};
use super::history::push_message;
use super::send;
use crate::Agent;
use crate::events::AgentEvent;
use bricks_tools::permissions::PermissionPolicy;
use bricks_tools::{ToolContext, ToolResult};
use bricks_types::*;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// Consecutive failures of one tool after which the advice stops being gentle.
pub(super) const MAX_TOOL_ERRORS_PER_TOOL: u32 = 3;

/// Advice appended to a failing tool result, escalating with the streak.
///
/// This is steering, not a budget. The old text counted down "N attempts
/// remaining" while nothing anywhere compared against a limit, so the countdown
/// ran past zero into negative numbers and promised an intervention that never
/// came. Rather than invent that intervention — refusing a tool outright can
/// leave a turn with no way forward, which is the failure mode this work exists
/// to remove — the claim is dropped and the wording says only what is true: the
/// same call keeps failing, so try a different one.
///
/// [`MAX_TOOL_ERRORS_PER_TOOL`] is the point at which the advice turns blunt.
pub(super) fn error_budget_note(tool_name: &str, count: u32) -> String {
    if count >= MAX_TOOL_ERRORS_PER_TOOL {
        format!(
            "[Tool '{tool_name}' has now failed {count} times in a row. Do not call it again \
             with a variation of this input — that has not worked {count} times. Use a \
             different tool, or tell the user what is blocking you and ask how to proceed.]"
        )
    } else {
        format!(
            "[Tool '{tool_name}' has failed {count} time(s) in a row. Read the error above and \
             change your approach — do not resend the same call.]"
        )
    }
}

/// A turn stopped while its answer streamed (cancelled, failed or closed
/// early). The calls already started are kept, after the finished blocks of
/// the answer: a finished call with its real result, published once, a call
/// still running stopped and answered as interrupted. A call never started
/// (incomplete, or not run early) is not recorded, and nothing runs after
/// the stop. The end of the run saves the session.
#[allow(clippy::too_many_arguments)]
pub(super) async fn keep_interrupted_answer(
    agent: &Agent,
    event_tx: &mpsc::Sender<AgentEvent>,
    tool_ctx: &ToolContext,
    finished: Vec<ContentBlock>,
    running: futures::stream::FuturesUnordered<futures::future::BoxFuture<'_, FinishedCall>>,
    mut done: Vec<FinishedCall>,
    started: &std::collections::HashSet<String>,
    why: &str,
) {
    // The calls still running stop here (their guards end shells).
    drop(running);
    let blocks: Vec<ContentBlock> = finished
        .into_iter()
        .filter(|b| match b {
            ContentBlock::ToolUse { id, .. } => started.contains(id),
            _ => true,
        })
        .collect();
    let calls: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        return;
    }
    push_message(agent, Message::assistant_blocks(blocks));
    agent
        .context
        .lock()
        .invalidate("an interrupted answer was kept");
    let mut active = Vec::new();
    let mut raw = Vec::new();
    for id in calls {
        match done.iter().position(|c| c.id == id) {
            Some(at) => {
                let call = done.swap_remove(at);
                let p = publish_call(agent, event_tx, tool_ctx, call, false).await;
                let (capped, full) = p.render();
                active.push(tool_result_block(&p.id, capped, p.result.is_error));
                raw.push(tool_result_block(&p.id, full, p.result.is_error));
            }
            None => {
                let text = interrupted_text(why);
                active.push(tool_result_block(&id, text.clone(), true));
                raw.push(tool_result_block(&id, text, true));
            }
        }
    }
    agent.raw_history.lock().push(Message::user_blocks(raw));
    agent.messages.lock().push(Message::user_blocks(active));
}

/// The result of a tool call stopped by the end of its run.
pub(super) fn interrupted_text(why: &str) -> String {
    format!(
        "[the run was {why} before this tool call finished; any effect it already \
         had remains]"
    )
}

/// The result of a call that shows images, as the model can receive them:
/// inside the result, after the round's results in the same user turn (the
/// returned blocks), or not at all, with a note saying so.
pub(super) fn tool_result_with_images(
    transport: bricks_provider::ImageTransport,
    id: &str,
    text: String,
    is_error: bool,
    images: &[bricks_tools::ToolImage],
) -> (ContentBlock, Vec<ContentBlock>) {
    use bricks_provider::ImageTransport;
    if images.is_empty() {
        return (tool_result_block(id, text, is_error), Vec::new());
    }
    let image_block =
        |i: &bricks_tools::ToolImage| ContentBlock::image_base64(&i.media_type, &i.data_base64);
    match transport {
        ImageTransport::ToolResult => {
            let mut blocks = vec![ContentBlock::Text { text }];
            blocks.extend(images.iter().map(image_block));
            let block = ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Blocks(blocks),
                is_error: Some(is_error),
            };
            (block, Vec::new())
        }
        ImageTransport::UserTurn => {
            let mut extra = Vec::new();
            for image in images {
                extra.push(ContentBlock::Text {
                    text: bricks_tools::tool_image_label(id, image),
                });
                extra.push(image_block(image));
            }
            (tool_result_block(id, text, is_error), extra)
        }
        ImageTransport::None => {
            let note = "[the image was not shown: the current model does not accept images]";
            (tool_result_block(id, format!("{text}\n{note}"), is_error), Vec::new())
        }
    }
}

pub(super) fn tool_result_block(id: &str, text: String, is_error: bool) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.to_string(),
        content: ToolResultContent::Text(text),
        is_error: Some(is_error),
    }
}

/// A tool call of a round, as it finished.
pub(super) struct FinishedCall {
    /// Its position in the response's calls.
    pub(super) index: usize,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: serde_json::Value,
    pub(super) result: ToolResult,
    /// Measured when the call ended.
    pub(super) duration: std::time::Duration,
    /// The change an approval was given for.
    pub(super) change: Option<bricks_tools::preview::ChangePreview>,
    /// The tool itself ran (refusals and denials did not).
    pub(super) executed: bool,
}

/// Position of a call started while its answer still streams, until the
/// answer is complete.
pub(super) const UNPLACED: usize = usize::MAX;

/// One call of an answer, decided then run. `index` is its position in the
/// answer's calls, or [`UNPLACED`] for a call started while the answer still
/// streams.
#[allow(clippy::too_many_arguments)]
pub(super) fn call_future<'a>(
    agent: &'a Agent,
    tool_ctx: &ToolContext,
    permissions: &Arc<dyn PermissionPolicy>,
    registered_tool_names: &[String],
    index: usize,
    (tool_id, tool_name, tool_input): (String, String, serde_json::Value),
    turn: u32,
    cumulative_cost: f64,
    message_count: usize,
) -> futures::future::BoxFuture<'a, FinishedCall> {
    // Its own view: the call id never leaks to a sibling.
    let tool_ctx = tool_ctx.for_call(&tool_id);
    let permission_policy = Arc::clone(permissions);
    let hooks = agent.hooks.clone();
    let registered_tool_names = registered_tool_names.to_vec();
    let tool_ref = agent.tool_by_name(&tool_name);
    Box::pin(async move {
        let start = Instant::now();
        let run = match tool_ref {
            Some(tool) => {
                let site = CallSite {
                    tool_id: &tool_id,
                    tool_ctx: &tool_ctx,
                    permissions: permission_policy.as_ref(),
                    hooks: &hooks,
                    turn,
                    cumulative_cost,
                    message_count,
                };
                decide_and_run(&site, tool, &tool_name, tool_input.clone()).await
            }
            // F-A15: weak models hallucinate tool names constantly, and
            // "Unknown tool: X" gave them nothing to correct toward.
            None => CallRun::not_run(bricks_tools::tool_feedback::not_found(
                "tool",
                &tool_name,
                &registered_tool_names,
                "Use one of the available tools above, with its exact name.",
            )),
        };
        // Measured when the call ends, not when it is published.
        let duration = start.elapsed();
        FinishedCall {
            index,
            id: tool_id,
            name: tool_name,
            input: tool_input,
            result: run.result,
            duration,
            change: run.change,
            executed: run.executed,
        }
    })
}

/// A finished call already published, waiting for the round's assembly.
pub(super) struct PublishedCall {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: serde_json::Value,
    pub(super) result: ToolResult,
    pub(super) duration: std::time::Duration,
    /// The tool's own output, before the engine adds notes to it.
    pub(super) own_output: String,
    pub(super) report: bricks_tools::tool_report::ToolReport,
    /// The output as the active context shows it (possibly reduced).
    pub(super) context_text: String,
}

impl PublishedCall {
    /// (active context text, raw history text).
    pub(super) fn render(&self) -> (String, String) {
        (
            self.report
                .render(&self.name, self.duration, Some(&self.context_text)),
            self.report.render(&self.name, self.duration, None),
        )
    }
}

pub(super) enum CallSlot {
    Published(PublishedCall),
    /// A `Skill` proposal, published once its activation is decided.
    Deferred(FinishedCall),
}

/// Publish a finished call: the change it applied, the files the model now
/// knows, its output reduced for the active context (the original kept in
/// the raw history and, when reduced, in the output store) and its tool end.
/// Order-dependent parts (failure streaks, history) are left to the round.
pub(super) async fn publish_call(
    agent: &Agent,
    event_tx: &mpsc::Sender<AgentEvent>,
    tool_ctx: &ToolContext,
    call: FinishedCall,
    verified_skill: bool,
) -> PublishedCall {
    let FinishedCall {
        id,
        name,
        input,
        result,
        duration,
        change,
        executed,
        ..
    } = call;
    // Anything but a read may have changed files: cached code-understanding
    // answers are dropped. A call refused or denied before running changed
    // nothing.
    if executed
        && agent.tools.iter().any(|t| {
            t.name() == name && t.permission_level() != bricks_tools::PermissionLevel::ReadOnly
        })
    {
        bricks_tools::code_scout::notify_workspace_changed(tool_ctx);
    }
    // The same, for the git state of the next request (a plan update
    // changes no file).
    if executed
        && agent.tool_list().iter().any(|t| {
            t.name() == name
                && !matches!(
                    t.permission_level(),
                    bricks_tools::PermissionLevel::ReadOnly | bricks_tools::PermissionLevel::None
                )
        })
    {
        agent.workspace_may_have_changed();
    }
    let own_output = result.content.clone();
    if let Some(change) = change.filter(|c| !result.is_error && !c.files.is_empty()) {
        send(
            agent,
            Some(event_tx),
            AgentEvent::EditApplied {
                tool_call_id: id.clone(),
                tool: name.clone(),
                files: change.files,
            },
        )
        .await;
    }
    // One rendering for every tool: a header (status, duration, code), then
    // the output, notes, suggestion. Only the output is reduced for the
    // active context (errors too: diagnostics are kept first).
    let report = result.to_report();
    let output = report.render_output();
    let (context_text, compression) = if verified_skill {
        (output, None)
    } else {
        let level = *agent.compression_level.lock();
        let compressor = Arc::clone(&agent.compressor);
        let (tool, call_input, call_id) = (name.clone(), input.clone(), id.clone());
        let (is_error, exit_code) = (report.status.is_error(), report.exit_code);
        // CPU work and the original's write: off the async threads.
        let processed = tokio::task::spawn_blocking(move || {
            compressor.process(
                &bricks_compression::ToolOutput {
                    tool: &tool,
                    input: &call_input,
                    content: &output,
                    is_error,
                    call_id: &call_id,
                    exit_code,
                },
                level,
            )
        })
        .await
        .expect("output reduction does not panic");
        if let Some(r) = &processed.raw {
            agent.raw_refs.lock().insert(id.clone(), r.clone());
        }
        (processed.text, Some(processed.stats))
    };
    send(
        agent,
        Some(event_tx),
        AgentEvent::ToolEnd {
            name: name.clone(),
            id: id.clone(),
            result: result.content.clone(),
            is_error: result.is_error,
            duration,
            compression,
            executed,
            status: report.status,
        },
    )
    .await;
    PublishedCall {
        id,
        name,
        input,
        result,
        duration,
        own_output,
        report,
        context_text,
    }
}
