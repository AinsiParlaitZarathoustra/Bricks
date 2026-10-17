//! Agent runner: the core agentic loop.

use crate::compact;
use crate::context::{BudgetDecision, ContextStatus, ModelView, RequestView};
use crate::events::{AgentEvent, CompactReason};
use crate::{Agent, AgentOutput, ToolCallRecord, UserInput};
use cersei_hooks::{HookAction, HookContext, HookEvent};
use cersei_provider::{CompletionRequest, ProviderOptions, StreamAccumulator};
use cersei_tools::permissions::{PermissionDecision, PermissionRequest};
use cersei_tools::{ToolContext, ToolResult};
use cersei_types::*;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

// ─── Retry jitter ────────────────────────────────────────────────────────────

/// Simple pseudo-random jitter for retry delays (no external crate needed).
fn rand_jitter() -> u64 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    seed ^ (seed >> 16) ^ (seed << 7)
}

// ─── Retry delay and notice ──────────────────────────────────────────────────

/// The local backoff of retry `retry` (1-based): 1000 × 2^(retry−1) ms,
/// capped at 30000 ms, plus `jitter % max(base / 4, 1)` ms.
fn local_backoff(retry: u32, jitter: u64) -> std::time::Duration {
    let factor = 1u64
        .checked_shl(retry.saturating_sub(1))
        .unwrap_or(u64::MAX);
    let base = 1000u64.saturating_mul(factor).min(30_000);
    std::time::Duration::from_millis(base + jitter % (base / 4).max(1))
}

/// The delay before retry `retry`: the local backoff, or the server's
/// `Retry-After` when that is longer (never shorter: a zero or past value
/// keeps the backoff; a long one is not capped). Computed once per retry;
/// the wait, the notice and the log all use it.
pub(crate) fn retry_delay(
    retry: u32,
    jitter: u64,
    server: Option<std::time::Duration>,
) -> std::time::Duration {
    let local = local_backoff(retry, jitter);
    server.map_or(local, |s| s.max(local))
}

/// What kind of transient failure this is, for people: "Rate limited" only
/// for HTTP 429; the status when there is one, none for a transport error.
/// Built from the status and the error class only — never from the error's
/// text, headers or body.
pub(crate) fn retry_label(e: &CerseiError) -> String {
    match e.http_status() {
        Some(429) => "Rate limited (HTTP 429)".into(),
        Some(503) => "Service unavailable (HTTP 503)".into(),
        Some(529) => "Provider overloaded (HTTP 529)".into(),
        Some(504) => "Gateway timeout (HTTP 504)".into(),
        Some(s) => format!("Temporary provider error (HTTP {s})"),
        None if e.is_timeout() => "Temporary connection error (timeout)".into(),
        None => "Temporary connection error".into(),
    }
}

/// The one notice of a retry, sent on every existing channel.
pub(crate) fn retry_notice(
    e: &CerseiError,
    retry: u32,
    max: u32,
    delay: std::time::Duration,
) -> String {
    format!(
        "{}. Retrying in {} ms... (retry {retry}/{max})",
        retry_label(e),
        delay.as_millis()
    )
}

// ─── Read-before-edit guard (F-11) ───────────────────────────────────────────

/// Paths a call would write to, as the model named them.
///
/// `ApplyPatch` is the awkward one: its targets are not a parameter, they are
/// inside the patch body. They are read out of the `+++ ` headers and put
/// through the *same* normalisation `apply_patch.rs` applies before it joins
/// against the working directory — timestamp stripped, git-style `b/` prefix
/// stripped.
///
/// Keeping the two in step is load-bearing, not tidiness. A guard that resolves
/// a target differently from the tool does not merely mis-report: for
/// `+++ b/a.rs` it would look up `<wd>/b/a.rs`, find nothing, conclude the file
/// is new and needs no prior read, and wave through an overwrite of an unread
/// `<wd>/a.rs`. The failure is silent and in the unsafe direction.
fn write_targets(tool_name: &str, tool_input: &serde_json::Value) -> Vec<String> {
    let named = || {
        tool_input
            .get("file_path")
            .and_then(serde_json::Value::as_str)
            .map(|s| vec![s.to_string()])
            .unwrap_or_default()
    };
    match tool_name {
        "Write" | "write" | "Edit" | "edit" | "MultiEdit" | "multi_edit" | "NotebookEdit"
        | "notebook_edit" => named(),
        "ApplyPatch" | "apply_patch" => tool_input
            .get("patch")
            .and_then(serde_json::Value::as_str)
            .map(cersei_tools::apply_patch::patch_targets)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// One spelling for one file, so the read side and the write side can be
/// compared at all.
///
/// The set of seen files is keyed by this. Comparing raw strings meant
/// `Read("src/x.rs")` followed by `Edit("/wd/src/x.rs")` looked like two
/// different files and the edit was refused, and it made `./x` and `x`
/// distinct. Relative paths are resolved against the tool context's working
/// directory; `canonicalize` then resolves symlinks and `..`, and is expected
/// to fail for a path that does not exist yet — the lexical form is the right
/// answer there.
fn resolve_path(working_dir: &std::path::Path, p: &str) -> String {
    let joined = if std::path::Path::new(p).is_absolute() {
        std::path::PathBuf::from(p)
    } else {
        working_dir.join(p)
    };
    std::fs::canonicalize(&joined)
        .unwrap_or(joined)
        .to_string_lossy()
        .to_string()
}

/// Refuse a blind overwrite: writing a file that exists but was never read.
///
/// Returns the message to hand back *instead of* running the tool. This must be
/// consulted before dispatch. It used to be applied to the returned
/// `ToolResult` after `execute` had already completed, which meant the file was
/// modified and then the model was told the edit had been blocked — the worst
/// of both, since it left disk and conversation disagreeing about what
/// happened.
///
/// A path that does not exist yet is a creation, not an overwrite, and needs no
/// prior read.
fn read_before_edit_block(
    tool_name: &str,
    tool_input: &serde_json::Value,
    files_read: &std::collections::HashSet<String>,
    working_dir: &std::path::Path,
) -> Option<String> {
    for target in write_targets(tool_name, tool_input) {
        // Both sides go through `resolve_path`, so a file counts as seen no
        // matter which spelling the model used for the read and the write.
        let resolved = resolve_path(working_dir, &target);
        if files_read.contains(&resolved) {
            continue;
        }
        if !std::path::Path::new(&resolved).exists() {
            continue;
        }
        return Some(format!(
            "{tool_name} was not run: '{target}' already exists and you have not read it in \
             this session, so this call would overwrite content you have never seen. Call Read \
             with file_path='{target}' first, then send this {tool_name} call again. Nothing \
             was written."
        ));
    }
    None
}

/// Decide which calls in a parallel batch must be refused, before any of them
/// runs.
///
/// Taking the whole batch is what makes the ordering enforceable rather than
/// merely intended: the result is computed from `tool_use_blocks` and then
/// captured by the dispatch closures, so there is no way to build the futures
/// without having decided the refusals first.
///
/// Note the concurrency semantics this fixes in place: `files_read` is the set
/// as of the *start* of the batch. A model that issues `Read(f)` and `Edit(f)`
/// in the same parallel batch still has the edit refused, because the read has
/// not completed when the batch is dispatched and nothing orders the two.
fn refusals_for_batch(
    calls: &[(String, String, serde_json::Value)],
    files_read: &std::collections::HashSet<String>,
    working_dir: &std::path::Path,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for (id, name, input) in calls {
        if let Some(msg) = read_before_edit_block(name, input, files_read, working_dir) {
            out.insert(id.clone(), msg);
        }
    }
    out
}

// ─── Repeated-failure steering (F-06) ────────────────────────────────────────

/// Consecutive failures of one tool after which the advice stops being gentle.
const MAX_TOOL_ERRORS_PER_TOOL: u32 = 3;

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
fn error_budget_note(tool_name: &str, count: u32) -> String {
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

// ─── Tool result size management ─────────────────────────────────────────────

fn tool_result_len(content: &ToolResultContent) -> usize {
    match content {
        ToolResultContent::Text(t) => t.len(),
        ToolResultContent::Blocks(b) => b
            .iter()
            .map(|bb| {
                if let ContentBlock::Text { text } = bb {
                    text.len()
                } else {
                    0
                }
            })
            .sum(),
    }
}

/// Remove the oldest tool results from the active history when their total
/// size exceeds `budget_chars`, replacing each with a placeholder.
/// Returns whether anything was removed. See
/// [`apply_tool_result_budget_with`] to say where the removed text can be
/// read back.
pub fn apply_tool_result_budget(messages: &mut [Message], budget_chars: usize) -> bool {
    apply_tool_result_budget_with(messages, budget_chars, |_, len| {
        format!(
            "[tool result removed from the active context to save space ({len} chars); it was \
             not saved — run the tool again if you need it]"
        )
    })
}

/// Like [`apply_tool_result_budget`]; `placeholder(tool_use_id, len)` writes
/// the replacement text (normally naming the saved original). The six most
/// recent messages are never touched.
pub fn apply_tool_result_budget_with(
    messages: &mut [Message],
    budget_chars: usize,
    placeholder: impl FnMut(&str, usize) -> String,
) -> bool {
    apply_tool_result_budget_protected(messages, budget_chars, &[], placeholder)
}

/// A tool result carrying one of `protected` (the markers of active skill
/// activations) is neither counted nor removed: those instructions stay.
fn is_protected(content: &ToolResultContent, protected: &[String]) -> bool {
    !protected.is_empty()
        && match content {
            ToolResultContent::Text(t) => protected.iter().any(|p| t.contains(p.as_str())),
            ToolResultContent::Blocks(b) => b.iter().any(|bb| {
                matches!(bb, ContentBlock::Text { text } if protected.iter().any(|p| text.contains(p.as_str())))
            }),
        }
}

/// [`apply_tool_result_budget_with`], keeping the results that carry an
/// active skill activation.
pub(crate) fn apply_tool_result_budget_protected(
    messages: &mut [Message],
    budget_chars: usize,
    protected: &[String],
    mut placeholder: impl FnMut(&str, usize) -> String,
) -> bool {
    let total: usize = messages
        .iter()
        .flat_map(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolResult { content, .. }
                        if !is_protected(content, protected) =>
                    {
                        Some(tool_result_len(content))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .sum();
    if total <= budget_chars {
        return false;
    }

    let keep_recent = 6;
    let truncatable_end = messages.len().saturating_sub(keep_recent);
    let mut freed = 0usize;
    let target_free = total - budget_chars;
    let mut changed = false;

    for msg in messages[..truncatable_end].iter_mut() {
        if freed >= target_free {
            break;
        }
        if let MessageContent::Blocks(blocks) = &mut msg.content {
            for block in blocks.iter_mut() {
                if freed >= target_free {
                    break;
                }
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                {
                    let size = tool_result_len(content);
                    let already = matches!(content, ToolResultContent::Text(t) if t.starts_with("[tool result removed"));
                    if size > 200 && !already && !is_protected(content, protected) {
                        freed += size;
                        *content = ToolResultContent::Text(placeholder(tool_use_id, size));
                        changed = true;
                    }
                }
            }
        }
    }
    changed
}

/// The unreduced text of a tool result in the raw history.
fn raw_tool_result(raw: &[Message], id: &str) -> Option<String> {
    raw.iter().rev().find_map(|m| match &m.content {
        MessageContent::Blocks(bs) => bs.iter().find_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content: ToolResultContent::Text(t),
                ..
            } if tool_use_id == id => Some(t.clone()),
            _ => None,
        }),
        _ => None,
    })
}

/// Append to the active history and to the raw history.
fn push_message(agent: &Agent, msg: Message) {
    agent.raw_history.lock().push(msg.clone());
    agent.messages.lock().push(msg);
}

/// Preserve injected input in both histories at a valid conversation boundary.
fn inject_pending(agent: &Agent, controls: Option<&crate::events::StreamControls>) {
    if let Some(controls) = controls {
        for message in controls.drain_messages() {
            push_message(agent, Message::user(message));
            agent
                .context
                .lock()
                .invalidate("a user message was injected");
        }
    }
}

/// Run the agent without streaming (blocking until complete), then the
/// long-term memory's maintenance.
pub async fn run_agent(agent: &Agent, input: &UserInput) -> Result<AgentOutput> {
    // Nobody reads this stream (listeners use `on_event`, broadcast or
    // reporters): with its receiver gone, sends return at once instead of
    // blocking the run once a buffer fills.
    let (event_tx, event_rx) = mpsc::channel(1);
    drop(event_rx);

    let result = run_agent_streaming(agent, input, event_tx).await;

    match result {
        Ok(output) => {
            agent.emit(AgentEvent::Complete(output.clone()));
            let cancel = agent.run_cancellation();
            maintain_memory(agent, &cancel, None).await;
            Ok(output)
        }
        Err(e) => {
            agent.emit(AgentEvent::Error(e.to_string()));
            Err(e)
        }
    }
}

/// One run: the agentic loop. A run that fails or is cancelled leaves a
/// history that can be continued — tool calls without results get one
/// saying they were interrupted — and the session is stored.
pub async fn run_agent_streaming(
    agent: &Agent,
    input: &UserInput,
    event_tx: mpsc::Sender<AgentEvent>,
) -> Result<AgentOutput> {
    let cancel = agent.begin_run();
    run_prepared(agent, input, event_tx, cancel).await
}

/// [`run_agent_streaming`] with a run token the caller already made current
/// (`Agent::begin_run`), so it can cancel the run from the first instant.
pub async fn run_prepared(
    agent: &Agent,
    input: &UserInput,
    event_tx: mpsc::Sender<AgentEvent>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<AgentOutput> {
    run_prepared_controlled(
        agent,
        input,
        event_tx,
        cancel,
        None,
        Arc::clone(&agent.permission_policy),
    )
    .await
}

pub(crate) async fn run_prepared_controlled(
    agent: &Agent,
    input: &UserInput,
    event_tx: mpsc::Sender<AgentEvent>,
    cancel: tokio_util::sync::CancellationToken,
    controls: Option<Arc<crate::events::StreamControls>>,
    permissions: Arc<dyn cersei_tools::permissions::PermissionPolicy>,
) -> Result<AgentOutput> {
    let result = run_loop(
        agent,
        input,
        &event_tx,
        &cancel,
        controls.as_deref(),
        permissions,
    )
    .await;
    if let Some(s) = agent.skill_session() {
        s.ledger.lock().clear_pending();
    }
    if let Err(e) = &result {
        let why = if matches!(e, CerseiError::Cancelled) {
            "cancelled by the user"
        } else {
            "interrupted by an error"
        };
        if let Some(controls) = controls.as_deref() {
            controls.finish();
        }
        repair_interrupted(agent, why);
        inject_pending(agent, controls.as_deref());
        match save_session(agent).await {
            Ok(true) => {
                if let Some(session_id) = &agent.session_id {
                    send(
                        agent,
                        Some(&event_tx),
                        AgentEvent::SessionSaved {
                            session_id: session_id.clone(),
                        },
                    )
                    .await;
                }
            }
            Ok(false) => {}
            Err(err) => {
                send(
                    agent,
                    Some(&event_tx),
                    AgentEvent::Status(format!("The session could not be saved: {err}")),
                )
                .await;
            }
        }
    }
    result
}

/// Answer every tool call left without a result (a run stopped while tools
/// were running), so the history stays valid for the next request. Effects
/// the tools already had are not undone; the result says so.
fn repair_interrupted(agent: &Agent, why: &str) {
    let unanswered = compact::find_unanswered_tool_uses(&agent.messages.lock());
    if unanswered.is_empty() {
        return;
    }
    let blocks: Vec<ContentBlock> = unanswered
        .into_iter()
        .map(|id| tool_result_block(&id, interrupted_text(why), true))
        .collect();
    push_message(agent, Message::user_blocks(blocks));
}

/// The result of a tool call stopped by the end of its run.
fn interrupted_text(why: &str) -> String {
    format!(
        "[the run was {why} before this tool call finished; any effect it already \
         had remains]"
    )
}

fn tool_result_block(id: &str, text: String, is_error: bool) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id.to_string(),
        content: ToolResultContent::Text(text),
        is_error: Some(is_error),
    }
}

/// A tool call of a round, as it finished.
struct FinishedCall {
    /// Its position in the response's calls.
    index: usize,
    id: String,
    name: String,
    input: serde_json::Value,
    result: ToolResult,
    /// Measured when the call ended.
    duration: std::time::Duration,
    /// The change an approval was given for.
    change: Option<cersei_tools::preview::ChangePreview>,
    /// The tool itself ran (refusals and denials did not).
    executed: bool,
}

/// A finished call already published, waiting for the round's assembly.
struct PublishedCall {
    id: String,
    name: String,
    input: serde_json::Value,
    result: ToolResult,
    duration: std::time::Duration,
    /// The tool's own output, before the engine adds notes to it.
    own_output: String,
    report: cersei_tools::tool_report::ToolReport,
    /// The output as the active context shows it (possibly reduced).
    context_text: String,
}

impl PublishedCall {
    /// (active context text, raw history text).
    fn render(&self) -> (String, String) {
        (
            self.report
                .render(&self.name, self.duration, Some(&self.context_text)),
            self.report.render(&self.name, self.duration, None),
        )
    }
}

enum CallSlot {
    Published(PublishedCall),
    /// A `Skill` proposal, published once its activation is decided.
    Deferred(FinishedCall),
}

/// A successful `Skill` result proposing an activation (whether it is one
/// of this agent's own is checked, once, when it is committed).
fn is_skill_proposal(agent: &Agent, call: &FinishedCall) -> bool {
    call.name == "Skill"
        && !call.result.is_error
        && agent.skill_session().is_some()
        && call
            .result
            .metadata
            .as_ref()
            .and_then(|m| m.get(cersei_tools::skill_tool::ACTIVATION_KEY))
            .is_some()
}

/// Publish a finished call: the change it applied, the files the model now
/// knows, its output reduced for the active context (the original kept in
/// the raw history and, when reduced, in the output store) and its tool end.
/// Order-dependent parts (failure streaks, history) are left to the round.
async fn publish_call(
    agent: &Agent,
    event_tx: &mpsc::Sender<AgentEvent>,
    tool_ctx: &ToolContext,
    files_read: &mut std::collections::HashSet<String>,
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
            t.name() == name && t.permission_level() != cersei_tools::PermissionLevel::ReadOnly
        })
    {
        cersei_tools::code_scout::notify_workspace_changed(tool_ctx);
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
    // ── Bookkeeping for the read-before-edit guard ──
    // The refusal itself happens before dispatch (`refusals_for_batch`);
    // what remains is recording the files whose contents the model can be
    // said to know. A successful *write* counts as much as a read: the model
    // supplied that content, so writing it again is not a blind overwrite.
    if !result.is_error {
        for target in write_targets(&name, &input) {
            files_read.insert(resolve_path(&tool_ctx.working_dir, &target));
        }
    }

    // One rendering for every tool: a header (status, duration, code), then
    // the output, notes, suggestion. Only the output is reduced for the
    // active context (errors too: diagnostics are kept first).
    let report = result.to_report();
    let output = report.render_output();
    let (context_text, partial_view, compression) = if verified_skill {
        (output, false, None)
    } else {
        let level = *agent.compression_level.lock();
        let compressor = Arc::clone(&agent.compressor);
        let (tool, call_input, call_id) = (name.clone(), input.clone(), id.clone());
        let (is_error, exit_code) = (report.status.is_error(), report.exit_code);
        // CPU work and the original's write: off the async threads.
        let processed = tokio::task::spawn_blocking(move || {
            compressor.process(
                &cersei_compression::ToolOutput {
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
        (
            processed.text,
            processed.partial_view,
            Some(processed.stats),
        )
    };
    // A file counts as read only when the model saw its exact text: a
    // skeleton or a summary is not enough to edit it.
    if (name == "Read" || name == "read")
        && !result.is_error
        && !partial_view
        && let Some(path) = input.get("file_path").and_then(|v| v.as_str())
    {
        files_read.insert(resolve_path(&tool_ctx.working_dir, path));
    }
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

/// Store the active and raw histories under the session id, as one pair
/// (one checkpoint for backends that publish pairs atomically, see
/// `Memory::histories_are_atomic`). Both are copied together, then the
/// locks are released before any serialization or I/O. Returns once the
/// backend has published the pair (or found it already published).
pub(crate) async fn save_session(agent: &Agent) -> Result<bool> {
    let (Some(memory), Some(session_id)) = (&agent.memory, &agent.session_id) else {
        return Ok(false);
    };
    let (histories, revision) = {
        let active = agent.messages.lock();
        let raw = agent.raw_history.lock();
        (
            cersei_memory::SessionHistories {
                active: active.clone(),
                raw: raw.clone(),
            },
            cersei_memory::SaveRevision {
                owner: agent.save_owner,
                seq: agent
                    .save_revision
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1,
            },
        )
    };
    memory
        .store_histories(session_id, histories, revision)
        .await?;
    Ok(true)
}

/// Empty the active history, keeping it as a snapshot (stored with the
/// session) and in the raw history.
pub(crate) async fn clear_context(agent: &Agent) -> Result<usize> {
    let cleared = std::mem::take(&mut *agent.messages.lock());
    let n = cleared.len();
    if n == 0 {
        return Ok(0);
    }
    let number = agent.snapshots.lock().len() + 1;
    let memory_key = match (&agent.memory, &agent.session_id) {
        (Some(memory), Some(session_id)) => {
            let key = cersei_memory::session_keys::snapshot(session_id, number);
            memory.store(&key, &cleared).await?;
            Some(key)
        }
        _ => None,
    };
    agent.snapshots.lock().push(crate::CompactionSnapshot {
        number,
        messages: cleared,
        memory_key,
    });
    agent.compaction_state.lock().applied = number;
    // The instructions of active skills left with the context.
    agent.clear_skills();
    agent
        .context
        .lock()
        .invalidate("the active context was cleared");
    save_session(agent).await?;
    Ok(n)
}

/// The long-term memory's maintenance after a run, reported as its own
/// phase: started, then finished (completed, cancelled or failed). The
/// answer was already delivered; a failure here never changes it.
pub(crate) async fn maintain_memory(
    agent: &Agent,
    cancel: &tokio_util::sync::CancellationToken,
    tx: Option<&mpsc::Sender<AgentEvent>>,
) {
    let Some(ltm) = &agent.long_term_memory else {
        return;
    };
    send(agent, tx, AgentEvent::MemoryMaintenanceStarted).await;
    let result = ltm.maintain(cancel).await.map_err(|e| e.to_string());
    send(agent, tx, AgentEvent::MemoryMaintenanceFinished(result)).await;
}

/// The agentic loop of one run.
async fn run_loop(
    agent: &Agent,
    input: &UserInput,
    event_tx: &mpsc::Sender<AgentEvent>,
    cancel: &tokio_util::sync::CancellationToken,
    controls: Option<&crate::events::StreamControls>,
    permissions: Arc<dyn cersei_tools::permissions::PermissionPolicy>,
) -> Result<AgentOutput> {
    let prompt = input.text.as_str();
    // Nothing to do: refused before the session is loaded or a request sent.
    if crate::subagent::is_blank(prompt) && input.attachments.is_empty() {
        return Err(CerseiError::InvalidInput(
            "the prompt is empty and has no attachment".into(),
        ));
    }
    let event_tx = event_tx.clone();
    // Load session history (skip if messages were pre-populated via with_messages)
    if agent.messages.lock().is_empty()
        && let (Some(memory), Some(session_id)) = (&agent.memory, &agent.session_id)
    {
        // Both histories from one stored pair (one generation).
        let stored = memory.load_histories(session_id).await?;
        let history = stored.active;
        if !history.is_empty() {
            let count = history.len();
            // An older session without a raw history starts from the
            // active one.
            let raw = if stored.raw.is_empty() {
                history.clone()
            } else {
                stored.raw
            };
            *agent.raw_history.lock() = raw;
            // Pre-compaction snapshots, numbered from 1 without gaps; later
            // compactions continue the numbering instead of overwriting.
            let mut snapshots = Vec::new();
            loop {
                let number = snapshots.len() + 1;
                let key = cersei_memory::session_keys::snapshot(session_id, number);
                match memory.load(&key).await {
                    Ok(messages) if !messages.is_empty() => {
                        snapshots.push(crate::CompactionSnapshot {
                            number,
                            messages,
                            memory_key: Some(key),
                        })
                    }
                    _ => break,
                }
            }
            agent.compaction_state.lock().applied = snapshots.len();
            *agent.snapshots.lock() = snapshots;
            agent.messages.lock().extend(history);
            agent
                .context
                .lock()
                .invalidate("the session was restored from storage");
            let _ = event_tx
                .send(AgentEvent::SessionLoaded {
                    session_id: session_id.clone(),
                    message_count: count,
                })
                .await;
            agent.emit(AgentEvent::SessionLoaded {
                session_id: session_id.clone(),
                message_count: count,
            });
            // The session's skills: checked against their sources before
            // anything of them is used again.
            for ev in agent.restore_skills() {
                send(agent, Some(&event_tx), ev).await;
            }
            agent.readd_missing_skills();
        }
    } // end session load guard

    let mut notes = std::mem::take(&mut *agent.config_notes.lock());
    notes.extend(agent.connect_mcp().await);
    // Long-term memory: recall for this prompt, within its budget (at most
    // a tenth of the prompt budget), appended to the system prompt.
    *agent.recalled.lock() = None;
    if let Some(ltm) = &agent.long_term_memory {
        let budget = (agent.memory_recall_tokens as u64)
            .min(context_status(agent).input_limit / 10)
            .max(1) as usize;
        match ltm.recall_context(prompt, budget).await {
            Ok(Some(r)) => {
                send(
                    agent,
                    Some(&event_tx),
                    AgentEvent::MemoryRecalled {
                        items: r.items,
                        tokens: r.tokens,
                        omitted: r.omitted,
                        budget,
                    },
                )
                .await;
                *agent.recalled.lock() = Some(r.text);
            }
            Ok(None) => {
                send(
                    agent,
                    Some(&event_tx),
                    AgentEvent::MemoryRecalled {
                        items: 0,
                        tokens: 0,
                        omitted: 0,
                        budget,
                    },
                )
                .await;
            }
            Err(e) => notes.push(format!("long-term memory unavailable for this run: {e}")),
        }
    }
    for note in notes {
        send(
            agent,
            Some(&event_tx),
            AgentEvent::Status(format!("Configuration: {note}")),
        )
        .await;
    }

    // Skills chosen by the person, before their request: all checked and
    // placed, or none and the request is not sent.
    activate_user_skills(agent, &input.skills, &event_tx, cancel).await?;

    if input.attachments.is_empty() {
        push_message(agent, Message::user(prompt));
    } else {
        // An attachment-only prompt carries no empty text block.
        let mut blocks = Vec::new();
        if !crate::subagent::is_blank(prompt) {
            blocks.push(ContentBlock::Text {
                text: prompt.to_string(),
            });
        }
        blocks.extend(input.attachments.iter().cloned());
        push_message(agent, Message::user_blocks(blocks));
    }
    let mut overflow_recoveries: u32 = 0;

    let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
    let mut turn: u32 = 0;
    let mut last_stop_reason: StopReason;
    let mut _last_usage = Usage::default();
    // Continuations after an answer cut by the output-token limit (each one
    // is a turn of its own).
    let mut max_tokens_retries: u32 = 0;
    const MAX_TOKENS_RETRY_LIMIT: u32 = 3;
    // Why the loop stopped: every `break` sets it.
    let termination: crate::Termination;
    // Benchmark mode only (explicit, bounded): verification nudges.
    let mut benchmark_retries: u32 = 0;
    let mut completion_verified = false;
    let mut progress = ProgressTracker::default();

    // Runtime guards
    let mut files_read: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut tool_error_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();

    // Build tool context
    // Progress of long tool calls (shell commands) reaches the event stream.
    // Weak: the agent's extensions outlive the run, the stream must not.
    {
        let tx = event_tx.downgrade();
        let reporters_emit = agent.emit_handle();
        agent
            .extensions
            .insert(cersei_tools::shell::ProgressSink(Arc::new(
                move |tool: &str, message: &str| {
                    let event = AgentEvent::ToolProgress {
                        name: tool.to_string(),
                        message: message.to_string(),
                    };
                    if let Some(tx) = tx.upgrade() {
                        let _ = tx.try_send(event.clone());
                    }
                    reporters_emit(event);
                },
            )));
    }
    // Sub-agents started by this run are cancelled with it.
    agent
        .extensions
        .insert(crate::subagent::RunCancellation(cancel.clone()));
    // Who this agent is, its model as of this run (a sub-agent inherits
    // it), and where its sub-agents report.
    {
        let current = agent.extensions.get::<crate::agents::AgentIdentity>();
        let run_id = format!("run_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let identity = match current {
            Some(i) if i.parent_id.is_some() => (*i).clone(),
            Some(i) => crate::agents::AgentIdentity {
                agent_id: i.agent_id.clone(),
                parent_id: None,
                root_run_id: run_id,
            },
            None => crate::agents::AgentIdentity {
                agent_id: format!("main_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
                parent_id: None,
                root_run_id: run_id,
            },
        };
        agent.extensions.insert(cersei_tools::jobs::JobOwner {
            agent_id: identity.agent_id.clone(),
            root_run_id: identity.root_run_id.clone(),
            workspace: agent.working_dir.clone(),
        });
        agent.extensions.insert(identity);
        agent.extensions.insert(crate::agents::ParentModel {
            selection: agent.model_label(),
            reasoning: agent.reasoning_profile(),
        });
        // In order, through one forwarder; listeners see them too. The
        // forwarder only holds a weak reference to this run's stream: it
        // never keeps the stream open after the run.
        let (sub_tx, mut sub_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let tx = event_tx.downgrade();
        let reporters_emit = agent.emit_handle();
        tokio::spawn(async move {
            while let Some(ev) = sub_rx.recv().await {
                reporters_emit(ev.clone());
                let Some(tx) = tx.upgrade() else { break };
                if tx.send(ev).await.is_err() {
                    break;
                }
            }
        });
        agent
            .extensions
            .insert(crate::agents::SubAgentSink(Arc::new(
                move |ev: AgentEvent| {
                    let _ = sub_tx.send(ev);
                },
            )));
    }
    agent
        .extensions
        .insert(crate::agents::spawn::LiveChildren::default());
    // So are code-understanding queries.
    agent
        .extensions
        .insert(cersei_tools::code_scout::RunCancel(cancel.clone()));
    let tool_ctx = ToolContext {
        working_dir: agent.working_dir.clone(),
        // One shell session per agent (not per run): state persists across
        // turns and `reply` calls; sub-agents have their own.
        session_id: agent.shell_session_id.clone(),
        permissions: Arc::clone(&permissions),
        cost_tracker: Arc::clone(&agent.cost_tracker),
        mcp_manager: agent.mcp_manager(),
        extensions: agent.extensions.clone(),
    };

    // Agentic loop
    loop {
        // No limit on the number of turns: the run goes on while the model
        // asks to (tool results to read, a continuation) and stops by the
        // engine's other rules — a final answer, a cancellation, a definitive
        // error, no progress, a refusal, an empty or truncated answer. The
        // counter is statistics only: it saturates, it never stops a run.
        turn = turn.saturating_add(1);

        // Check cancellation
        if cancel.is_cancelled() {
            return Err(CerseiError::Cancelled);
        }

        let _ = event_tx.send(AgentEvent::TurnStart { turn }).await;
        agent.emit(AgentEvent::TurnStart { turn });

        // Apply tool result budget to keep context manageable. Removed
        // results name their saved original, and the measurement of the old
        // history no longer applies.
        let removed = {
            let mut msgs = agent.messages.lock();
            let refs = agent.raw_refs.lock();
            let raw = agent.raw_history.lock();
            let protected = agent.active_skill_markers();
            apply_tool_result_budget_protected(
                &mut msgs,
                agent.tool_result_budget,
                &protected,
                |id, len| {
                    let saved = refs.get(id).cloned().or_else(|| {
                        raw_tool_result(&raw, id).and_then(|text| {
                            agent
                                .compressor
                                .raw_store()?
                                .put(&format!("removed-{id}"), &text)
                                .ok()
                        })
                    });
                    match saved {
                        Some(r) => format!(
                            "[tool result removed from the active context to save space ({len} chars);                          full output: {}]",
                            r.hint()
                        ),
                        None => format!(
                            "[tool result removed from the active context to save space ({len} chars);                          it could not be saved — run the tool again if you need it]"
                        ),
                    }
                },
            )
        };
        if removed {
            agent
                .context
                .lock()
                .invalidate("old tool results were removed from the active context");
        }

        // Inject only between complete rounds, never between tool uses and results.
        inject_pending(agent, controls);

        // Build completion request
        let messages = agent.messages.lock().clone();
        let tool_defs: Vec<ToolDefinition> = agent
            .tool_list()
            .iter()
            .map(|t| t.to_definition())
            .collect();

        // The provider is bound to one configured model and ignores this
        // string; it is only a label for events and metadata.
        // Read once per turn: a model change applies from the next turn.
        // One snapshot for this request: provider, label and profile go
        // together; a change made meanwhile applies to the next request.
        let selection = agent.selection();
        let provider = selection.provider.clone();
        let model = selection.model_label();

        let mut options = ProviderOptions::default();
        if let Some(profile) = selection.reasoning.clone() {
            options.set(cersei_provider::REASONING_PROFILE_OPTION, &profile);
        }

        // Todo nudge: on turns > 2, remind model about incomplete todos
        let system_with_nudge = if turn > 2 {
            // The key TodoWrite writes under: the tool context's session.
            let todos = cersei_tools::todo_write::get_todos(&tool_ctx.session_id);
            let incomplete = todos
                .iter()
                .filter(|t| t.status != cersei_tools::todo_write::TodoStatus::Completed)
                .count();
            if incomplete > 0 {
                let nudge = format!(
                    "\n\n[system reminder: You have {} incomplete task{} in your TodoWrite list. Finish the ones the request still needs; mark done ones completed and drop the ones that are no longer needed.]",
                    incomplete,
                    if incomplete == 1 { "" } else { "s" }
                );
                agent.effective_system().map(|s| format!("{s}{nudge}"))
            } else {
                agent.effective_system()
            }
        } else {
            agent.effective_system()
        };

        // F-04: last line of defence. Compaction is the known way to sever a
        // tool_use/tool_result pair, but anything that rewrites history can do
        // it, and the provider's answer is always a 400 that the retry loop
        // cannot rescue. Report it here, naming the ids, so the cause is in the
        // log next to the request that carried it rather than inferred later
        // from an opaque provider error.
        let orphaned = compact::find_orphaned_tool_results(&messages);
        if !orphaned.is_empty() {
            tracing::error!(
                orphaned_tool_use_ids = ?orphaned,
                message_count = messages.len(),
                "request carries tool_result blocks with no matching tool_use; \
                 the provider will reject this with a 400"
            );
        }
        // §10.5 #3, the mirror rule: an assistant tool_use with no tool_result
        // anywhere in the request is the same unretryable 400 from the other
        // direction. Every request this loop builds ends with a user message,
        // so nothing is legitimately unanswered here.
        let unanswered = compact::find_unanswered_tool_uses(&messages);
        if !unanswered.is_empty() {
            tracing::error!(
                unanswered_tool_use_ids = ?unanswered,
                message_count = messages.len(),
                "request carries tool_use blocks with no matching tool_result; \
                 the provider will reject this with a 400"
            );
        }

        let mut request = CompletionRequest {
            model: model.clone(),
            messages: messages.clone(),
            system: system_with_nudge,
            tools: tool_defs,
            max_tokens: agent.max_tokens,
            temperature: agent.temperature,
            stop_sequences: Vec::new(),
            options,
            output_modalities: Vec::new(),
        };

        let status = ensure_budget(agent, &mut request, &event_tx, cancel).await?;
        // The context this request describes: its usage measures it only if
        // nothing (a switch, a rewrite) changed it before the response.
        let mut sent_version = agent.context.lock().version();
        let _ = event_tx
            .send(AgentEvent::ModelRequestStart {
                turn,
                message_count: request.messages.len(),
                token_estimate: status.context_used.tokens,
            })
            .await;

        // Send to provider with automatic retry on transient errors
        let mut retry_count = 0u32;
        const MAX_RETRIES: u32 = 5;

        let (mut rx, mut accumulator) = loop {
            let req_clone = request.clone();
            // `complete()` now awaits the provider's response headers before it
            // returns (F-02) — that is what lets a 429 come back as a retryable
            // `Err` instead of a stream event the retry loop can't see. But it
            // also means this loop, not the stream loop below, is where the
            // request spends its time-to-first-byte, and this loop is outside
            // the `select!` that watches `cancel_token`. No provider configures
            // a client timeout, so without this branch a cancel is ignored
            // until the first byte arrives — forever, against a server that
            // accepts the connection and then goes quiet.
            let outcome = tokio::select! {
                result = provider.complete(req_clone) => result,
                _ = cancel.cancelled() => return Err(CerseiError::Cancelled),
            };
            match outcome {
                Ok(stream) => {
                    break (stream.into_receiver(), StreamAccumulator::new());
                }
                Err(e)
                    if e.is_context_overflow()
                        && overflow_recoveries
                            < agent.context.lock().policy().max_overflow_recoveries =>
                {
                    // The server says it does not fit: shrink, then resend.
                    // Nothing ran yet for this turn, so no tool effect repeats.
                    overflow_recoveries += 1;
                    send(
                        agent,
                        Some(&event_tx),
                        AgentEvent::Status(format!(
                            "The server refused the request as too long ({e}); compacting."
                        )),
                    )
                    .await;
                    let outcome = run_compaction(
                        agent,
                        CompactReason::ContextOverflow,
                        true,
                        Some(&event_tx),
                        cancel,
                    )
                    .await;
                    if outcome == crate::compact::CompactionOutcome::Cancelled {
                        return Err(CerseiError::Cancelled);
                    }
                    if !outcome.is_compacted() {
                        return Err(e);
                    }
                    request.messages = agent.messages.lock().clone();
                    sent_version = agent.context.lock().version();
                    continue;
                }
                Err(e) if e.is_retryable() && retry_count < MAX_RETRIES => {
                    // MAX_RETRIES retries after the first call: six calls at
                    // most. The delay is chosen once (local backoff 1, 2, 4,
                    // 8, 16 s + jitter, or the server's longer Retry-After).
                    retry_count += 1;
                    let delay = retry_delay(retry_count, rand_jitter(), e.retry_after());
                    let notice = retry_notice(&e, retry_count, MAX_RETRIES, delay);
                    tracing::warn!(
                        "{}: retry {}/{} in {} ms",
                        retry_label(&e),
                        retry_count,
                        MAX_RETRIES,
                        delay.as_millis()
                    );
                    let _ = event_tx.send(AgentEvent::Status(notice.clone())).await;
                    agent.emit(AgentEvent::Status(notice));
                    // Same reasoning as the `complete()` await above, and newly
                    // load-bearing: until F-02 this sleep was unreachable, so
                    // its uncancellability never showed. Five local retries
                    // are up to ~39 s of it; a server's Retry-After may be more.
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancel.cancelled() => return Err(CerseiError::Cancelled),
                    }
                    continue;
                }
                Err(e) => return Err(e),
            }
        };

        let _ = event_tx
            .send(AgentEvent::ModelResponseStart {
                turn,
                model: model.clone(),
            })
            .await;

        // Process stream events (with cancellation support)
        let mut stream_error: Option<CerseiError> = None;
        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Some(event) => {
                            match &event {
                                StreamEvent::TextDelta { text, .. } => {
                                    let _ = event_tx.send(AgentEvent::TextDelta(text.clone())).await;
                                    agent.emit(AgentEvent::TextDelta(text.clone()));
                                }
                                StreamEvent::ThinkingDelta { thinking, .. } => {
                                    let _ = event_tx
                                        .send(AgentEvent::ThinkingDelta(thinking.clone()))
                                        .await;
                                    agent.emit(AgentEvent::ThinkingDelta(thinking.clone()));
                                }
                                StreamEvent::Error { message } => {
                                    stream_error = Some(CerseiError::Provider(message.clone()));
                                    break;
                                }
                                _ => {}
                            }
                            accumulator.process_event(event);
                        }
                        None => break, // Stream ended
                    }
                }
                _ = cancel.cancelled() => {
                    return Err(CerseiError::Cancelled);
                }
            }
        }

        if let Some(e) = stream_error {
            let allowed = agent.context.lock().policy().max_overflow_recoveries;
            if e.is_context_overflow() && overflow_recoveries < allowed {
                // Refused mid-stream before any tool ran: compact and redo the turn.
                overflow_recoveries += 1;
                let outcome = run_compaction(
                    agent,
                    CompactReason::ContextOverflow,
                    true,
                    Some(&event_tx),
                    cancel,
                )
                .await;
                if outcome == crate::compact::CompactionOutcome::Cancelled {
                    return Err(CerseiError::Cancelled);
                }
                if outcome.is_compacted() {
                    // The same turn again: it produced nothing.
                    turn = turn.saturating_sub(1);
                    continue;
                }
            }
            return Err(e);
        }

        // Convert accumulated response
        let response = accumulator.into_response()?;
        overflow_recoveries = 0;
        last_stop_reason = response.stop_reason.clone();
        _last_usage = response.usage.clone();

        // Update cumulative usage
        agent.cumulative_usage.lock().merge(&response.usage);
        agent.cost_tracker.add(&response.usage);
        record_run_usage(agent, &response.usage);

        // Emit cost update
        let cumulative = agent.cumulative_usage.lock().clone();
        let _ = event_tx
            .send(AgentEvent::CostUpdate {
                turn_cost: response.usage.cost_usd.unwrap_or(0.0),
                cumulative_cost: cumulative.cost_usd.unwrap_or(0.0),
                input_tokens: cumulative.input_tokens,
                output_tokens: cumulative.output_tokens,
            })
            .await;
        agent.emit(AgentEvent::CostUpdate {
            turn_cost: response.usage.cost_usd.unwrap_or(0.0),
            cumulative_cost: cumulative.cost_usd.unwrap_or(0.0),
            input_tokens: cumulative.input_tokens,
            output_tokens: cumulative.output_tokens,
        });

        // Add assistant message to history, and record what the usage
        // measured: the request just executed, with this model.
        let assistant_index = request.messages.len();
        push_message(agent, response.message.clone());
        // Recorded against the selection the request was sent with.
        let sent_model = ModelView::of(provider.as_ref(), &model);
        let status = {
            let mut ctx = agent.context.lock();
            let view = RequestView::of(&request);
            ctx.record_response_sent(
                &sent_model,
                &view,
                &response.usage,
                Some((assistant_index, &response.message)),
                sent_version,
            );
            let msgs = agent.messages.lock().clone();
            let next = RequestView {
                messages: &msgs,
                ..RequestView::of(&request)
            };
            ctx.status(&model_view(agent), &next)
        };
        send(agent, Some(&event_tx), AgentEvent::ContextUpdate(status)).await;

        // Fire PostModelTurn hooks
        let hook_ctx = HookContext {
            event: HookEvent::PostModelTurn,
            tool_name: None,
            tool_input: None,
            tool_result: None,
            tool_is_error: None,
            turn,
            cumulative_cost_usd: cumulative.cost_usd.unwrap_or(0.0),
            message_count: agent.messages.lock().len(),
        };
        let hook_action = cersei_hooks::run_hooks(&agent.hooks, &hook_ctx).await;
        if let HookAction::Block(reason) = hook_action {
            return Err(CerseiError::Provider(format!(
                "Blocked by hook: {}",
                reason
            )));
        }

        // Fire TurnsElapsed every `turns_elapsed_cadence` turns (default 10).
        // Callers can register a SkillNudgeHook here for agent-curated skill
        // creation without blocking the agent loop.
        if turn > 0 && turn.is_multiple_of(agent.turns_elapsed_cadence) {
            let cadence_ctx = HookContext {
                event: HookEvent::TurnsElapsed,
                tool_name: None,
                tool_input: None,
                tool_result: None,
                tool_is_error: None,
                turn,
                cumulative_cost_usd: cumulative.cost_usd.unwrap_or(0.0),
                message_count: agent.messages.lock().len(),
            };
            // Don't block on TurnsElapsed hooks — best-effort, fire and forget.
            let _ = cersei_hooks::run_hooks(&agent.hooks, &cadence_ctx).await;
        }

        let _ = event_tx
            .send(AgentEvent::TurnComplete {
                turn,
                stop_reason: response.stop_reason.clone(),
                usage: response.usage.clone(),
            })
            .await;
        agent.emit(AgentEvent::TurnComplete {
            turn,
            stop_reason: response.stop_reason.clone(),
            usage: response.usage.clone(),
        });

        // What the response asks for is decided by what it contains: its
        // tool calls are run whatever stop reason the provider gave, and a
        // "tool use" stop without any call is an answer, never an empty
        // round. Only an answer cut by the output-token limit is not run:
        // its calls may be cut mid-arguments.
        let tool_use_blocks: Vec<(String, String, serde_json::Value)> = response
            .message
            .content_blocks()
            .into_iter()
            .filter_map(|b| {
                if let ContentBlock::ToolUse { id, name, input } = b {
                    Some((id, name, input))
                } else {
                    None
                }
            })
            .collect();

        if response.stop_reason == StopReason::MaxTokens {
            max_tokens_retries += 1;
            // Calls in a cut answer are answered, never run, so the history
            // stays valid whatever happens next.
            let mut blocks: Vec<ContentBlock> = tool_use_blocks
                .iter()
                .map(|(id, name, _)| ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: ToolResultContent::Text(format!(
                        "{name} was not run: your answer was cut by the output-token limit, \
                         possibly inside this call. Send it again if you still need it."
                    )),
                    is_error: Some(true),
                })
                .collect();
            if max_tokens_retries > MAX_TOKENS_RETRY_LIMIT {
                if !blocks.is_empty() {
                    push_message(agent, Message::user_blocks(blocks));
                }
                termination = crate::Termination::OutputTruncated {
                    continuations: MAX_TOKENS_RETRY_LIMIT,
                };
                break;
            }
            blocks.push(ContentBlock::Text {
                text: "[system] Your answer was cut by the output-token limit. Continue \
                       from exactly where you stopped."
                    .into(),
            });
            push_message(agent, Message::user_blocks(blocks));
            send(
                agent,
                Some(&event_tx),
                AgentEvent::Status(format!(
                    "The answer was cut by the output-token limit; continuing \
                     ({max_tokens_retries}/{MAX_TOKENS_RETRY_LIMIT})"
                )),
            )
            .await;
        } else if tool_use_blocks.is_empty() {
            if response.stop_reason == StopReason::ContentFilter {
                termination = crate::Termination::ContentFiltered;
                break;
            }
            if !has_visible_text(&response.message) {
                termination = crate::Termination::EmptyResponse;
                break;
            }
            // A final answer ends the run. Only the explicit benchmark mode
            // may ask for a verification first, a bounded number of times.
            if agent.benchmark_mode
                && let Some((message, status)) = benchmark_nudge(
                    prompt,
                    &tool_calls,
                    turn,
                    &mut completion_verified,
                    &mut benchmark_retries,
                )
            {
                push_message(agent, Message::user(message));
                send(agent, Some(&event_tx), AgentEvent::Status(status)).await;
                continue;
            }
            // A message received during generation still deserves a response.
            // Closing admission and checking the queue are atomic.
            if controls.is_some_and(|c| !c.finish_if_empty()) {
                continue;
            }
            termination = crate::Termination::Completed;
            break;
        } else {
            max_tokens_retries = 0;
            // Phase 1: Emit ToolStart events for all tools
            for (tool_id, tool_name, tool_input) in &tool_use_blocks {
                let _ = event_tx
                    .send(AgentEvent::ToolStart {
                        name: tool_name.clone(),
                        id: tool_id.clone(),
                        input: tool_input.clone(),
                    })
                    .await;
                agent.emit(AgentEvent::ToolStart {
                    name: tool_name.clone(),
                    id: tool_id.clone(),
                    input: tool_input.clone(),
                });
            }

            // Phase 2: Execute all tools in parallel. Each call is published
            // (edit applied, tool end) as soon as it finishes, without
            // waiting for slower siblings; the conversation is assembled
            // afterwards in the order of the calls.
            let msg_count = agent.messages.lock().len();
            // Built once, outside the per-call closure: this is the same
            // `agent.tools` the lookup below uses (so MCP-injected tools
            // stay consistent), and building it inside the closure would
            // re-allocate every tool name for every parallel call (F-A15).
            let registered_tool_names: Vec<String> = agent
                .tool_list()
                .iter()
                .map(|t| t.name().to_string())
                .collect();

            // ── Guard: read-before-edit, decided BEFORE dispatch (F-11) ──
            // This used to run over the *returned* ToolResult, which meant
            // the file had already been written by the time the model was
            // told the edit was blocked: disk and conversation disagreed
            // about whether the edit happened.
            let refusals = refusals_for_batch(&tool_use_blocks, &files_read, &tool_ctx.working_dir);

            let mut pending: futures::stream::FuturesUnordered<_> = tool_use_blocks
                .iter()
                .enumerate()
                .map(|(index, (tool_id, tool_name, tool_input))| {
                    let tool_name = tool_name.clone();
                    let registered_tool_names = registered_tool_names.clone();
                    let refusal = refusals.get(tool_id).cloned();
                    let tool_id = tool_id.clone();
                    let tool_input = tool_input.clone();
                    // Its own view: the call id never leaks to a sibling.
                    let tool_ctx = tool_ctx.for_call(&tool_id);
                    let permission_policy = Arc::clone(&permissions);
                    let hooks = agent.hooks.clone();
                    let cumulative_cost = cumulative.cost_usd.unwrap_or(0.0);

                    // Find tool reference by name
                    let tool_ref = agent.tool_by_name(&tool_name);

                    async move {
                        let start = Instant::now();
                        // The change an approval was given for (written
                        // if the call succeeds).
                        let mut approved_change: Option<cersei_tools::preview::ChangePreview> = None;
                        // The tool itself ran (it may have changed files).
                        let mut executed = false;

                        let result = if let Some(msg) = refusal {
                            // Refused before dispatch: the tool never runs,
                            // so nothing reaches disk.
                            ToolResult::error(msg)
                        } else if let Some(tool) = tool_ref {
                            // Check permissions. Session state a shell
                            // command depends on (aliases, functions) is
                            // shown to the policy, so it cannot hide what
                            // will actually run.
                            let mut description = format!("Execute tool '{}'", tool_name);
                            if let Some(details) = tool.permission_details(&tool_input, &tool_ctx).await {
                                description.push('\n');
                                description.push_str(&details);
                            }
                            // What the call would change, computed
                            // without writing, so the decision is taken
                            // on it. If a file changes while the decision
                            // is pending, the change is recomputed and
                            // asked again: an approval never applies to
                            // a state it was not given for.
                            let mut preview = tool.preview(&tool_input, &tool_ctx).await;
                            let mut stale_rounds = 0;
                            let decision = loop {
                                if let Some(reason) = preview.as_ref().and_then(|p| p.refusal.clone()) {
                                    // The tool would refuse it: nothing to approve.
                                    break PermissionDecision::Deny(format!("{reason}\nNothing was written."));
                                }
                                let perm_req = PermissionRequest {
                                    tool_name: tool_name.clone(),
                                    tool_input: tool_input.clone(),
                                    permission_level: tool.permission_level_for(&tool_input),
                                    description: description.clone(),
                                    id: tool_id.clone(),
                                    preview: preview.clone(),
                                    agent_id: None,
                                };
                                let d = permission_policy.check(&perm_req).await;
                                let allowed = matches!(
                                    d,
                                    PermissionDecision::Allow
                                        | PermissionDecision::AllowOnce
                                        | PermissionDecision::AllowForSession
                                );
                                match preview.as_ref().map(|p| p.check_current()) {
                                    Some(Err(changed)) if allowed => {
                                        stale_rounds += 1;
                                        if stale_rounds >= 3 {
                                            break PermissionDecision::Deny(format!(
                                                "{} changed on disk while the change was awaiting approval; \
                                                 nothing was written. Read the file again and retry.",
                                                changed.join(", ")
                                            ));
                                        }
                                        preview = tool.preview(&tool_input, &tool_ctx).await;
                                    }
                                    _ => {
                                        if allowed {
                                            approved_change = preview.clone();
                                        }
                                        break d;
                                    }
                                }
                            };

                            match decision {
                                PermissionDecision::Allow
                                | PermissionDecision::AllowOnce
                                | PermissionDecision::AllowForSession => {
                                    let hook_ctx = HookContext {
                                        event: HookEvent::PreToolUse,
                                        tool_name: Some(tool_name.clone()),
                                        tool_input: Some(tool_input.clone()),
                                        tool_result: None,
                                        tool_is_error: None,
                                        turn,
                                        cumulative_cost_usd: cumulative_cost,
                                        message_count: msg_count,
                                    };
                                    let hook_action =
                                        cersei_hooks::run_hooks(&hooks, &hook_ctx).await;

                                    match hook_action {
                                        HookAction::Block(reason) => ToolResult::error(
                                            format!("Blocked by hook: {}", reason),
                                        ),
                                        HookAction::ModifyInput(new_input) => {
                                            // What is written is the new
                                            // input's change, not the one
                                            // previewed for the old input.
                                            if approved_change.is_some() {
                                                approved_change = tool
                                                    .preview(&new_input, &tool_ctx)
                                                    .await
                                                    .filter(|p| p.refusal.is_none());
                                            }
                                            executed = true;
                                            execute_admitted(tool, new_input, &tool_ctx).await
                                        }
                                        _ => {
                                            executed = true;
                                            execute_admitted(tool, tool_input.clone(), &tool_ctx)
                                                .await
                                        }
                                    }
                                }
                                PermissionDecision::Deny(reason) => {
                                    ToolResult::error(format!("Permission denied: {}", reason))
                                }
                            }
                        } else {
                            // F-A15: weak models hallucinate tool names
                            // constantly, and "Unknown tool: X" gave them
                            // nothing to correct toward.
                            cersei_tools::tool_feedback::not_found(
                                "tool",
                                &tool_name,
                                &registered_tool_names,
                                "Call ToolSearch with a keyword to find the right tool, then call that tool by its exact name.",
                            )
                        };

                        // Measured when the call ends, not when it is published.
                        let duration = start.elapsed();
                        FinishedCall {
                            index,
                            id: tool_id,
                            name: tool_name,
                            input: tool_input,
                            result,
                            duration,
                            change: approved_change,
                            executed,
                        }
                    }
                })
                .collect();

            // One slot per call, in the order of the calls.
            let mut slots: Vec<Option<CallSlot>> =
                (0..tool_use_blocks.len()).map(|_| None).collect();
            let mut cancelled = false;
            loop {
                // Cancelling the turn drops the running tool futures: a
                // shell command is then interrupted by its own guard instead
                // of running to its timeout.
                let next = tokio::select! {
                    n = futures::StreamExt::next(&mut pending) => n,
                    _ = cancel.cancelled() => {
                        cancelled = true;
                        break;
                    }
                };
                let Some(call) = next else { break };
                let index = call.index;
                // A skill activation proposed by this agent's own `Skill`
                // tool is announced only once committed (it may need a
                // compaction): that happens below, in the order of the calls.
                slots[index] = Some(if is_skill_proposal(agent, &call) {
                    CallSlot::Deferred(call)
                } else {
                    CallSlot::Published(
                        publish_call(agent, &event_tx, &tool_ctx, &mut files_read, call, false)
                            .await,
                    )
                });
            }
            // The calls still running stop here (their guards end shells).
            drop(pending);

            if cancelled {
                // Results already finished are kept, with the effects they
                // reported; the calls stopped get the interruption result
                // `repair_interrupted` would give them. Nothing is invented
                // for an effect that did not happen.
                let mut active = Vec::new();
                let mut raw = Vec::new();
                for (index, slot) in slots.into_iter().enumerate() {
                    let id = tool_use_blocks[index].0.clone();
                    match slot {
                        Some(CallSlot::Published(p)) => {
                            let (capped, full) = p.render();
                            active.push(tool_result_block(&p.id, capped, p.result.is_error));
                            raw.push(tool_result_block(&p.id, full, p.result.is_error));
                        }
                        Some(CallSlot::Deferred(call)) => {
                            // Finished, never admitted to the context.
                            let text = format!(
                                "Skill `{}` was not activated: the run was cancelled by the user.",
                                call.input
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                            );
                            let mut failed = call;
                            failed.result = ToolResult::error(text);
                            let p = publish_call(
                                agent,
                                &event_tx,
                                &tool_ctx,
                                &mut files_read,
                                failed,
                                false,
                            )
                            .await;
                            let (capped, full) = p.render();
                            active.push(tool_result_block(&p.id, capped, true));
                            raw.push(tool_result_block(&p.id, full, true));
                        }
                        None => {
                            let text = interrupted_text("cancelled by the user");
                            active.push(tool_result_block(&id, text.clone(), true));
                            raw.push(tool_result_block(&id, text, true));
                        }
                    }
                }
                agent.raw_history.lock().push(Message::user_blocks(raw));
                agent.messages.lock().push(Message::user_blocks(active));
                // Sub-agents stop with the run's token; wait for their
                // cleanup (shells, terminal state) rather than drop them.
                crate::agents::spawn::settle_children(
                    &agent.extensions,
                    std::time::Duration::from_secs(10),
                )
                .await;
                return Err(CerseiError::Cancelled);
            }

            // Phase 3: assemble the round in the order of the calls (the
            // history, the progress check and the failure streaks never
            // depend on which call finished first).
            let mut result_blocks: Vec<ContentBlock> = Vec::new();
            // (name, input, is_error, output) of each call, for the
            // progress check.
            let mut round: Vec<(String, String, bool, String)> = Vec::new();
            let mut raw_blocks: Vec<ContentBlock> = Vec::new();

            for slot in slots.into_iter().flatten() {
                let mut p = match slot {
                    CallSlot::Published(p) => p,
                    CallSlot::Deferred(mut call) => {
                        // Committed once it fits, refused whole otherwise.
                        // Its text is never reduced.
                        let verified =
                            commit_skill_activation(agent, &call.name, &mut call.result, &event_tx)
                                .await;
                        publish_call(agent, &event_tx, &tool_ctx, &mut files_read, call, verified)
                            .await
                    }
                };

                // ── Guard: Per-tool error counter with reflection (F-06) ──
                if p.result.is_error {
                    let count = tool_error_counts.entry(p.name.clone()).or_insert(0);
                    *count += 1;
                    p.report.notes.push(error_budget_note(&p.name, *count));
                } else {
                    tool_error_counts.remove(&p.name);
                }

                let (capped_content, full_text) = p.render();
                raw_blocks.push(tool_result_block(
                    &p.id,
                    full_text.clone(),
                    p.result.is_error,
                ));
                round.push((
                    p.name.clone(),
                    p.input.to_string(),
                    p.result.is_error,
                    p.own_output.clone(),
                ));
                tool_calls.push(ToolCallRecord {
                    name: p.name,
                    id: p.id.clone(),
                    input: p.input,
                    result: full_text,
                    is_error: p.result.is_error,
                    duration: p.duration,
                });
                result_blocks.push(tool_result_block(&p.id, capped_content, p.result.is_error));
            }

            // Add tool results as user message: reduced in the active
            // history, unreduced in the raw history.
            agent
                .raw_history
                .lock()
                .push(Message::user_blocks(raw_blocks));
            // ── Progress check ──
            // A round that repeats one of the last rounds exactly (same
            // calls, same arguments, same results) brings nothing new.
            // Different reads, or a test run again after an edit, are
            // progress. After a few such rounds the model is told once; if
            // it keeps going, the run stops, incomplete.
            let repeats = progress.record(round_signature(&round));
            if repeats == NO_PROGRESS_WARN {
                result_blocks.push(ContentBlock::Text {
                    text: format!(
                        "[system] Your last {repeats} rounds of tool calls repeated earlier \
                         ones with the same arguments and got the same results. Repeating \
                         them again will not help: change the approach, or stop and say \
                         what is blocking you."
                    ),
                });
                send(
                    agent,
                    Some(&event_tx),
                    AgentEvent::Status(format!(
                        "No progress: the same tool calls returned the same results {repeats} times"
                    )),
                )
                .await;
            }
            agent
                .messages
                .lock()
                .push(Message::user_blocks(result_blocks));
            if repeats >= NO_PROGRESS_STOP {
                termination = crate::Termination::NoProgress { repeats };
                break;
            }
        }
        // After the turn: warn near the limit, and compact proactively once
        // the occupation crosses the policy's threshold.
        if agent.auto_compact {
            let status = context_status(agent);
            let pct = status.fraction_used();
            if pct >= compact::WARNING_PCT {
                use crate::events::WarningState;
                let state = if pct >= compact::CRITICAL_PCT {
                    WarningState::Critical
                } else {
                    WarningState::Warning
                };
                send(
                    agent,
                    Some(&event_tx),
                    AgentEvent::TokenWarning {
                        pct_used: pct,
                        state,
                    },
                )
                .await;
            }
            if agent.context.lock().should_compact(&status)
                && run_compaction(
                    agent,
                    CompactReason::ThresholdExceeded,
                    false,
                    Some(&event_tx),
                    cancel,
                )
                .await
                    == crate::compact::CompactionOutcome::Cancelled
            {
                return Err(CerseiError::Cancelled);
            }
        }
    }

    // Persist session: the active history, and next to it the raw history.
    if let Some(controls) = controls {
        controls.finish();
    }
    inject_pending(agent, controls);
    if save_session(agent).await? {
        let session_id = agent.session_id.clone().unwrap_or_default();
        let _ = event_tx
            .send(AgentEvent::SessionSaved {
                session_id: session_id.clone(),
            })
            .await;
        agent.emit(AgentEvent::SessionSaved {
            session_id: session_id.clone(),
        });
    }

    // Build output
    let last_message = agent
        .messages
        .lock()
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        .cloned()
        .unwrap_or_else(|| Message::assistant(""));

    // Remember the exchange durably (failures are reported, never fatal).
    // Processing it — extraction, embeddings — is the maintenance phase
    // that follows the answer.
    if let Some(ltm) = &agent.long_term_memory {
        let now = chrono::Utc::now().timestamp_millis();
        let mut turns = vec![cersei_memory::MemoryTurn {
            role: "user".into(),
            content: prompt.to_string(),
            at: Some(now),
        }];
        if let Some(text) = last_message.get_text().filter(|t| !t.trim().is_empty()) {
            turns.push(cersei_memory::MemoryTurn {
                role: "assistant".into(),
                content: text.to_string(),
                at: Some(now),
            });
        }
        if let Err(e) = ltm.record_turns(agent.session_id.as_deref(), &turns).await {
            send(
                agent,
                Some(&event_tx),
                AgentEvent::Status(format!("Long-term memory not updated: {e}")),
            )
            .await;
        }
    }

    let output = AgentOutput {
        message: last_message,
        usage: agent.cumulative_usage.lock().clone(),
        stop_reason: last_stop_reason,
        turns: turn,
        tool_calls,
        termination,
    };

    // Notify reporters
    for reporter in &agent.reporters {
        reporter.on_complete(&output).await;
    }

    Ok(output)
}

// ─── Context management ──────────────────────────────────────────────────────

/// Automatic compaction bookkeeping for one agent.
#[derive(Debug, Default)]
pub(crate) struct CompactionState {
    failures: u32,
    disabled: bool,
    /// (context version, message count) of the last attempt that did not
    /// compact: retrying on the same history cannot make progress.
    last_unsuccessful: Option<(u64, usize)>,
    /// Compactions applied so far (numbers the snapshots).
    applied: usize,
}

pub(crate) fn model_label(agent: &Agent) -> String {
    agent.selection().model_label()
}

fn tool_definitions(agent: &Agent) -> Vec<ToolDefinition> {
    agent
        .tool_list()
        .iter()
        .map(|t| t.to_definition())
        .collect()
}

fn model_view(agent: &Agent) -> ModelView {
    let sel = agent.selection();
    ModelView::of(sel.provider.as_ref(), &sel.model_label())
}

/// Status of the next request as it would be built now.
pub(crate) fn context_status(agent: &Agent) -> ContextStatus {
    let messages = agent.messages.lock().clone();
    let tools = tool_definitions(agent);
    let system = agent.effective_system();
    let view = RequestView {
        system: system.as_deref(),
        tools: &tools,
        messages: &messages,
        max_output: agent.max_tokens,
    };
    agent.context.lock().status(&model_view(agent), &view)
}

async fn send(agent: &Agent, tx: Option<&mpsc::Sender<AgentEvent>>, event: AgentEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event.clone()).await;
    }
    agent.emit(event);
}

pub(crate) async fn compact_now(
    agent: &Agent,
    cancel: &tokio_util::sync::CancellationToken,
) -> crate::compact::CompactionOutcome {
    run_compaction(agent, CompactReason::ManualTrigger, true, None, cancel).await
}

/// Run one compaction attempt and apply it if it succeeds. `force` skips the
/// no-progress guard (manual request, or a request that cannot be sent).
async fn run_compaction(
    agent: &Agent,
    reason: CompactReason,
    force: bool,
    tx: Option<&mpsc::Sender<AgentEvent>>,
    cancel: &tokio_util::sync::CancellationToken,
) -> crate::compact::CompactionOutcome {
    use crate::compact::{
        COMPACTION_ATTEMPT_TIMEOUT, CompactionOutcome, CompactionPlan, compact_history_with,
    };

    let messages = agent.messages.lock().clone();
    let version = agent.context.lock().version();
    let skip = {
        let state = agent.compaction_state.lock();
        if state.disabled && reason != CompactReason::ManualTrigger {
            Some(format!(
                "automatic compaction is off for this session after {} unsuccessful attempts",
                state.failures
            ))
        } else if !force && state.last_unsuccessful == Some((version, messages.len())) {
            Some("the history has not changed since the last unsuccessful attempt".to_string())
        } else {
            None
        }
    };
    if let Some(reason_text) = skip {
        let outcome = CompactionOutcome::Skipped {
            reason: reason_text,
        };
        send(
            agent,
            tx,
            AgentEvent::CompactionResult {
                reason,
                outcome: outcome.clone(),
            },
        )
        .await;
        return outcome;
    }

    send(
        agent,
        tx,
        AgentEvent::CompactStart {
            reason,
            messages_before: messages.len(),
        },
    )
    .await;

    let model = model_view(agent);
    let (est, policy) = {
        let ctx = agent.context.lock();
        (ctx.estimator(&model), ctx.policy().clone())
    };
    let tools = tool_definitions(agent);
    let frame_tokens = agent
        .effective_system()
        .as_deref()
        .map(|s| est.text(s).tokens)
        .unwrap_or(0)
        + est.tools(&tools).tokens;
    let input_limit = model
        .limits
        .input_budget(policy.summary_max_tokens as u64)
        .max(1);
    let plan = CompactionPlan {
        keep_recent_messages: policy.keep_recent_messages,
        max_recent_tokens: (input_limit as f64 * policy.max_recent_ratio) as u64,
        input_limit,
        summary_max_tokens: policy.summary_max_tokens,
        min_gain: policy.min_compaction_gain,
        frame_tokens,
        instructions: None,
    };
    let run = compact_history_with(
        agent.provider().as_ref(),
        &messages,
        &est,
        &plan,
        cancel,
        tokio::time::Instant::now() + COMPACTION_ATTEMPT_TIMEOUT,
    )
    .await;

    // The summary call counts once in the session totals, whatever its fate.
    if let Some(u) = &run.usage {
        agent.context.lock().record_compaction_usage(u);
        agent.cumulative_usage.lock().merge(u);
        agent.cost_tracker.add(u);
        record_run_usage(agent, u);
    }

    match (&run.outcome, run.messages) {
        (
            CompactionOutcome::Compacted {
                messages_after,
                tokens_before,
                tokens_after,
                ..
            },
            Some(mut new),
        ) => {
            // Active skills whose text was summarised away come back whole,
            // after the summary (never twice: only the missing ones).
            let readd = agent.skills_to_readd(&new);
            if !readd.is_empty()
                && let Some(first) = new.first_mut()
            {
                let mut blocks = first.content_blocks();
                let at = blocks
                    .iter()
                    .position(|b| !matches!(b, ContentBlock::Text { .. }))
                    .unwrap_or(blocks.len())
                    .min(1);
                for (i, text) in readd.into_iter().enumerate() {
                    blocks.insert(at + i, ContentBlock::Text { text });
                }
                first.content = MessageContent::Blocks(blocks);
            }
            let number = {
                let mut st = agent.compaction_state.lock();
                st.failures = 0;
                st.last_unsuccessful = None;
                st.applied += 1;
                st.applied
            };
            let memory_key = match (&agent.memory, &agent.session_id) {
                (Some(memory), Some(sid)) => {
                    let key = cersei_memory::session_keys::snapshot(sid, number);
                    match memory.store(&key, &messages).await {
                        Ok(()) => Some(key),
                        Err(e) => {
                            tracing::warn!("could not store the pre-compaction snapshot: {e}");
                            None
                        }
                    }
                }
                _ => None,
            };
            agent.snapshots.lock().push(crate::CompactionSnapshot {
                number,
                messages,
                memory_key,
            });
            *agent.messages.lock() = new;
            agent
                .context
                .lock()
                .invalidate(format!("compaction #{number} rewrote the history"));
            // Persist now, so the session, its raw history and the snapshot
            // stay consistent even if the run stops before its end.
            if let Err(e) = save_session(agent).await {
                tracing::warn!("could not store the compacted session: {e}");
            }
            send(
                agent,
                tx,
                AgentEvent::CompactEnd {
                    messages_after: *messages_after,
                    tokens_freed: tokens_before.saturating_sub(*tokens_after),
                },
            )
            .await;
        }
        // A cancellation is not an attempt on this history: the next
        // compaction may try it again, and it never counts as a failure.
        (CompactionOutcome::Cancelled, _) => {}
        (outcome, _) => {
            let mut st = agent.compaction_state.lock();
            st.last_unsuccessful = Some((version, messages.len()));
            if matches!(
                outcome,
                CompactionOutcome::Failed { .. } | CompactionOutcome::InsufficientGain { .. }
            ) {
                st.failures += 1;
                if st.failures >= policy.max_compaction_failures {
                    st.disabled = true;
                }
            }
        }
    }
    tracing::info!(?reason, outcome = %run.outcome, "compaction");
    send(
        agent,
        tx,
        AgentEvent::CompactionResult {
            reason,
            outcome: run.outcome.clone(),
        },
    )
    .await;
    run.outcome
}

/// Whether `text` can be added to the context now, compacting once (by
/// the existing rules) when it cannot. `Err` says why it cannot.
async fn admit_skill_text(
    agent: &Agent,
    text: &str,
    tx: Option<&mpsc::Sender<AgentEvent>>,
) -> std::result::Result<(), String> {
    let check = |agent: &Agent| {
        let status = context_status(agent);
        let est = agent
            .context
            .lock()
            .estimator(&model_view(agent))
            .text(text)
            .upper;
        let limit = status.input_limit.saturating_sub(status.margin);
        let left = limit.saturating_sub(status.context_used.upper_bound);
        (est <= left, est, left)
    };
    if check(agent).0 {
        return Ok(());
    }
    // Within the current run: its cancellation stops the compaction.
    let cancel = agent.run_cancellation();
    let outcome = run_compaction(agent, CompactReason::BudgetExceeded, true, tx, &cancel).await;
    if outcome == crate::compact::CompactionOutcome::Cancelled {
        return Err("the run was cancelled".into());
    }
    let (ok, est, left) = check(agent);
    if ok {
        return Ok(());
    }
    Err(format!(
        "its instructions (about {est} tokens) do not fit in the context ({left} tokens left{})",
        if outcome.is_compacted() {
            " after a compaction"
        } else {
            "; a compaction could not free enough"
        }
    ))
}

/// Commit the activation a `Skill` result proposes, if it is one of this
/// agent's own proposals. Returns whether an activation was committed (its
/// text then bypasses output reduction).
async fn commit_skill_activation(
    agent: &Agent,
    tool_name: &str,
    result: &mut ToolResult,
    tx: &mpsc::Sender<AgentEvent>,
) -> bool {
    if tool_name != "Skill" || result.is_error {
        return false;
    }
    let Some(session) = agent.skill_session() else {
        return false;
    };
    let Some(id) = result
        .metadata
        .as_ref()
        .and_then(|m| m.get(cersei_tools::skill_tool::ACTIVATION_KEY))
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return false;
    };
    // Only a proposal this agent's tool made counts; anything else is an
    // ordinary output.
    let Some((activation, origin)) = session.ledger.lock().take_pending(&id) else {
        return false;
    };
    match admit_skill_text(agent, &result.content, Some(tx)).await {
        Ok(()) => {
            send(
                agent,
                Some(tx),
                AgentEvent::SkillActivated {
                    agent_id: None,
                    name: activation.name.clone(),
                    source: activation.source.clone(),
                    revision: activation.revision.clone(),
                    origin: origin.as_str().into(),
                },
            )
            .await;
            session.ledger.lock().commit(id, activation, origin);
            agent.save_skills();
            true
        }
        Err(why) => {
            send(
                agent,
                Some(tx),
                AgentEvent::SkillNotActivated {
                    agent_id: None,
                    name: activation.name.clone(),
                    reason: why.clone(),
                },
            )
            .await;
            *result = ToolResult::error(format!(
                "Skill `{}` was not activated: {why}. Its instructions were not shortened; \
                 continue without them, or ask the user to free context (`/compact`, `/clear`).",
                activation.name
            ));
            false
        }
    }
}

async fn user_skill_failed(
    agent: &Agent,
    tx: &mpsc::Sender<AgentEvent>,
    name: &str,
    why: String,
) -> CerseiError {
    send(
        agent,
        Some(tx),
        AgentEvent::SkillNotActivated {
            agent_id: None,
            name: name.to_string(),
            reason: why.clone(),
        },
    )
    .await;
    CerseiError::InvalidInput(format!(
        "skill `{name}` was not activated: {why}; the request was not sent"
    ))
}

/// Activate the skills a person chose for this run, before their request.
async fn activate_user_skills(
    agent: &Agent,
    names: &[String],
    tx: &mpsc::Sender<AgentEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let Some(session) = agent.skill_session() else {
        return Err(CerseiError::InvalidInput(
            "skills are not configured for this session; the request was not sent".into(),
        ));
    };
    // Everything is read and checked first.
    let mut todo = Vec::new();
    for name in names {
        let a = match session.access.activate(name) {
            Ok(a) => a,
            Err(e) => return Err(user_skill_failed(agent, tx, name, e.to_string()).await),
        };
        let already = session
            .ledger
            .lock()
            .active_revision(&a.source, &a.revision)
            .is_some();
        if !already
            && !todo
                .iter()
                .any(|(_, t): &(String, bricks_skills::Activation)| {
                    t.same_revision(&a.source, &a.revision)
                })
        {
            todo.push((bricks_skills::Ledger::new_id(), a));
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    let texts: Vec<String> = todo
        .iter()
        .map(|(id, a)| agent.render_skill(a, id))
        .collect();
    if let Err(why) = admit_skill_text(agent, &texts.join("\n\n"), Some(tx)).await {
        let names: Vec<&str> = todo.iter().map(|(_, a)| a.name.as_str()).collect();
        return Err(user_skill_failed(agent, tx, &names.join(", "), why).await);
    }
    if cancel.is_cancelled() {
        return Err(CerseiError::Cancelled);
    }
    let blocks: Vec<ContentBlock> = texts
        .into_iter()
        .map(|text| ContentBlock::Text { text })
        .collect();
    push_message(agent, Message::user_blocks(blocks));
    for (id, a) in todo {
        send(
            agent,
            Some(tx),
            AgentEvent::SkillActivated {
                agent_id: None,
                name: a.name.clone(),
                source: a.source.clone(),
                revision: a.revision.clone(),
                origin: "user".into(),
            },
        )
        .await;
        session
            .ledger
            .lock()
            .commit(id, a, bricks_skills::Origin::User);
    }
    agent.save_skills();
    Ok(())
}

/// Check the budget of `request` before it is sent, compacting once if it
/// manifestly does not fit. A request that still does not fit is not sent.
async fn ensure_budget(
    agent: &Agent,
    request: &mut CompletionRequest,
    tx: &mpsc::Sender<AgentEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<ContextStatus> {
    for attempt in 0..2 {
        let model = model_view(agent);
        let (status, decision) = {
            let ctx = agent.context.lock();
            let status = ctx.status(&model, &RequestView::of(request));
            let decision = ctx.decide(&status);
            (status, decision)
        };
        let fits = match decision {
            BudgetDecision::Fits => return Ok(status),
            BudgetDecision::Preflight(why) => {
                match preflight(agent, &model, request, &status, &why, tx, cancel).await? {
                    Some(st) => return Ok(st),
                    None => false,
                }
            }
            BudgetDecision::Exceeded => false,
        };
        debug_assert!(!fits);
        if attempt == 0 && agent.auto_compact {
            let outcome =
                run_compaction(agent, CompactReason::BudgetExceeded, true, Some(tx), cancel).await;
            if outcome == crate::compact::CompactionOutcome::Cancelled {
                return Err(CerseiError::Cancelled);
            }
            if outcome.is_compacted() {
                request.messages = agent.messages.lock().clone();
                continue;
            }
        }
        let used = status.context_used.tokens;
        let limit = status.input_limit.saturating_sub(status.margin);
        send(
            agent,
            Some(tx),
            AgentEvent::Status(format!(
                "Request not sent: the context (~{used} tokens, {:?}) exceeds the budget of {limit} \
                 tokens ({} input limit − {} margin, {} reserved for output) and compaction could \
                 not reduce it.",
                status.context_used.provenance, status.input_limit, status.margin, status.reserved_output
            )),
        )
        .await;
        return Err(CerseiError::ContextOverflow { used, limit });
    }
    unreachable!("the second attempt always returns")
}

/// Absolute limit of one call to a configured token-counting endpoint.
/// Past it the local estimate is used, as for any counting failure.
pub(crate) const TOKEN_COUNT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A precise check of the complete request: the configured counting endpoint
/// when there is one, else a full local estimate. `Some(status)` when the
/// request fits. A cancellation during the count stops here, without the
/// fallback.
async fn preflight(
    agent: &Agent,
    model: &ModelView,
    request: &CompletionRequest,
    status: &ContextStatus,
    why: &str,
    tx: &mpsc::Sender<AgentEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Option<ContextStatus>> {
    let limit = status.input_limit.saturating_sub(status.margin);
    let mut note = None;
    if model.token_counting {
        let provider = agent.provider();
        let counted = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(CerseiError::Cancelled),
            r = tokio::time::timeout(TOKEN_COUNT_TIMEOUT, provider.count_request_tokens(request)) => r,
        };
        let counted = counted.unwrap_or_else(|_| {
            Err(CerseiError::Provider(format!(
                "no answer within {} s",
                TOKEN_COUNT_TIMEOUT.as_secs()
            )))
        });
        match counted {
            Ok(Some(n)) => {
                let st = {
                    let mut ctx = agent.context.lock();
                    ctx.record_count(model, &RequestView::of(request), n);
                    ctx.status(model, &RequestView::of(request))
                };
                send(
                    agent,
                    Some(tx),
                    AgentEvent::Status(format!(
                        "Pre-flight ({why}): counted {n} input tokens, budget {limit}."
                    )),
                )
                .await;
                return Ok((n <= limit).then_some(st));
            }
            Ok(None) => {}
            Err(e) => {
                note = Some(format!(
                    "counting endpoint unavailable ({e}); local estimate used"
                ))
            }
        }
    }
    // Local fallback: a full, conservative estimate of the request.
    let est = agent.context.lock().estimator(model);
    let full = est.request(request.system.as_deref(), &request.tools, &request.messages);
    let central = full.tokens.max(status.context_used.tokens);
    send(
        agent,
        Some(tx),
        AgentEvent::Status(format!(
            "Pre-flight ({why}): estimated ~{central} input tokens (up to ~{}), budget {limit}{}.",
            full.upper.max(status.context_used.upper_bound),
            note.map(|n| format!("; {n}")).unwrap_or_default()
        )),
    )
    .await;
    Ok((central <= limit).then(|| status.clone()))
}

// ─── Progress check ──────────────────────────────────────────────────────────

/// Consecutive rounds that repeat a recent round exactly before the model
/// is told, once.
const NO_PROGRESS_WARN: u32 = 3;
/// Consecutive repeated rounds after which the run stops, incomplete.
const NO_PROGRESS_STOP: u32 = 5;
/// Rounds remembered: repeating any of them (an A, B, A, B cycle too)
/// counts.
const PROGRESS_WINDOW: usize = 4;

/// The rounds of tool calls of one run, as signatures.
#[derive(Default)]
struct ProgressTracker {
    recent: std::collections::VecDeque<u64>,
    repeats: u32,
}

impl ProgressTracker {
    /// Record a round; returns how many consecutive rounds, this one
    /// included, repeated one of the rounds before them.
    fn record(&mut self, signature: u64) -> u32 {
        if self.recent.contains(&signature) {
            self.repeats += 1;
        } else {
            self.repeats = 0;
        }
        self.recent.push_back(signature);
        if self.recent.len() > PROGRESS_WINDOW {
            self.recent.pop_front();
        }
        self.repeats
    }
}

/// A round's calls (name, arguments, error flag, output), in a stable
/// order: the same calls with the same results give the same signature.
fn round_signature(round: &[(String, String, bool, String)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut calls: Vec<&(String, String, bool, String)> = round.iter().collect();
    calls.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    calls.hash(&mut h);
    h.finish()
}

/// The message has a text block with something visible in it.
fn has_visible_text(message: &Message) -> bool {
    match &message.content {
        MessageContent::Text(t) => !crate::subagent::is_blank(t),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text } if !crate::subagent::is_blank(text))),
    }
}

/// Benchmark mode only: the verification asked for before a final answer
/// is accepted, as (message, status). Bounded: one completion check, then
/// at most four nudges tied to the instruction's own
/// test command.
fn benchmark_nudge(
    prompt: &str,
    tool_calls: &[ToolCallRecord],
    turn: u32,
    completion_verified: &mut bool,
    benchmark_retries: &mut u32,
) -> Option<(String, String)> {
    const BENCHMARK_MAX_RETRIES: u32 = 4;
    if !*completion_verified && turn >= 3 {
        let recent_has_verify = tool_calls.iter().rev().take(5).any(|tc| {
            let cmd = tc
                .input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            cmd.contains("cat ")
                || cmd.contains("python ")
                || cmd.contains("test")
                || cmd.contains("verify")
                || cmd.contains("node ")
                || cmd.contains("./")
                || cmd.contains("check")
        });
        if !recent_has_verify {
            *completion_verified = true;
            return Some((
                "[system] Before finishing, verify your solution is correct:\n\
                 1. Check that all expected output files exist and have correct content\n\
                 2. Run your solution to confirm it produces the right output\n\
                 3. Re-read the original instruction — did you satisfy EVERY requirement?"
                    .into(),
                "Benchmark: nudging the agent to verify before completion".into(),
            ));
        }
    }
    // In TB 2.0 tests are run externally by the verifier after the agent
    // finishes: only a test command named in the instruction is enforced.
    if *benchmark_retries >= BENCHMARK_MAX_RETRIES {
        return None;
    }
    let has_instruction_tests = [
        "test_outputs.py",
        "run_tests",
        "run-tests",
        "pytest",
        "verify.py",
        "check.py",
        "npm test",
        "cargo test",
        "make test",
    ]
    .iter()
    .any(|k| prompt.contains(k));
    if !has_instruction_tests {
        return None;
    }
    match benchmark_check_tests(tool_calls) {
        BenchmarkVerification::NotRun if *benchmark_retries == 0 => {
            *benchmark_retries += 1;
            Some((
                "[system] The task instruction mentions a verification command. Run it now \
                 to check your solution. Look at the instruction again for the exact command."
                    .into(),
                "Benchmark: nudge to run the instruction's test command".into(),
            ))
        }
        BenchmarkVerification::Failed(test_output) => {
            *benchmark_retries += 1;
            let truncated: String = test_output.chars().take(3000).collect();
            Some((
                format!(
                    "[system] Verification FAILED (attempt {}/{}).\n\nOutput:\n```\n{}\n```\n\n\
                     Try a COMPLETELY DIFFERENT approach. Do NOT patch — rewrite.",
                    benchmark_retries, BENCHMARK_MAX_RETRIES, truncated
                ),
                format!(
                    "Benchmark: retry {}/{}",
                    benchmark_retries, BENCHMARK_MAX_RETRIES
                ),
            ))
        }
        _ => None,
    }
}

// ─── Benchmark self-verification helpers ────────────────────────────────────

#[derive(Debug)]
enum BenchmarkVerification {
    NotRun,
    Failed(String), // carries the test output for retry feedback
    Passed,
}

/// Analyze tool call history to determine if tests were run and whether they passed.
fn benchmark_check_tests(tool_calls: &[ToolCallRecord]) -> BenchmarkVerification {
    let test_patterns = [
        "run-tests",
        "run_tests",
        "pytest",
        "python -m pytest",
        "bash run-tests.sh",
        "npm test",
        "cargo test",
        "go test",
        "make test",
        "jest",
        "mocha",
        "unittest",
    ];

    let mut found_test_run = false;
    let mut last_test_failed = false;
    let mut last_test_output = String::new();

    // Check the most recent tool calls (last 30) for test execution
    for tc in tool_calls.iter().rev().take(30) {
        if tc.name != "Bash" && tc.name != "bash" {
            continue;
        }

        let cmd = tc
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let is_test_cmd = test_patterns.iter().any(|p| cmd.contains(p));
        if !is_test_cmd {
            continue;
        }

        found_test_run = true;
        last_test_output = tc.result.clone();

        // Primary signal: exit code (most reliable)
        if tc.is_error {
            last_test_failed = true;
            break;
        }

        // Secondary: parse output for pass/fail indicators
        let result_lower = tc.result.to_lowercase();

        let has_pass = result_lower.contains("passed")
            || result_lower.contains("success")
            || result_lower.contains("all tests")
            || result_lower.contains("exit code 0")
            || tc.result.contains("PASSED")
            || tc.result.contains("PASS")
            || (result_lower.contains(" ok") && !result_lower.contains("not ok"));

        let has_failure = result_lower.contains("failed")
            || result_lower.contains("failure")
            || result_lower.contains("traceback")
            || result_lower.contains("not ok")
            || result_lower.contains("assertion")
            || (result_lower.contains("error")
                && !result_lower.contains("error handling")
                && !result_lower.contains("error_"));

        last_test_failed = has_failure && !has_pass;
        break; // Only care about the most recent test run
    }

    if !found_test_run {
        BenchmarkVerification::NotRun
    } else if last_test_failed {
        BenchmarkVerification::Failed(last_test_output)
    } else {
        BenchmarkVerification::Passed
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// One response's usage, in its root run's ledger (once, as this agent's
/// own: the session agent's or a descendant's).
fn record_run_usage(agent: &Agent, usage: &cersei_types::Usage) {
    if let (Some(rt), Some(id)) = (
        agent.extensions.get::<crate::agents::RuntimeHandle>(),
        agent.extensions.get::<crate::agents::AgentIdentity>(),
    ) {
        rt.0.add_usage(&id.root_run_id, id.parent_id.is_none(), usage);
    }
}

/// Run a tool, admitted as a writer of the workspace when it may write
/// (level `write`, `execute` or `dangerous`): while a sub-agent works in
/// this checkout, another agent's writing call waits for it. A delegation
/// call holds nothing for its parent (its child does), so a parent waiting
/// for its child cannot deadlock.
async fn execute_admitted(
    tool: &dyn cersei_tools::Tool,
    input: serde_json::Value,
    ctx: &ToolContext,
) -> ToolResult {
    use cersei_tools::PermissionLevel as L;
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

#[cfg(test)]
mod guard_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    /// The seen-file set, built the way the runner builds it: every path
    /// normalised through `resolve_path`, never stored raw.
    fn seen(wd: &std::path::Path, paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|p| resolve_path(wd, p)).collect()
    }

    /// The core of F-11: the refusal has to be decidable *before* dispatch.
    ///
    /// The old guard ran over the returned `ToolResult`, so by the time it
    /// replaced the content with "you must Read first" the write had already
    /// landed. Deciding from (name, input, files_read) alone is what makes it
    /// possible to refuse without running the tool.
    #[test]
    fn existing_but_unread_file_is_refused_for_every_writing_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.rs");
        std::fs::write(&f, "existing\n").unwrap();
        let p = f.to_str().unwrap();
        let none = seen(tmp.path(), &[]);

        for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
            let block = read_before_edit_block(tool, &json!({ "file_path": p }), &none, tmp.path());
            assert!(
                block.is_some(),
                "{tool} may not overwrite an unread file that already exists"
            );
            let msg = block.unwrap();
            assert!(msg.contains(p), "{tool}: message must name the file: {msg}");
            assert!(
                msg.contains("Read"),
                "{tool}: message must say what to do: {msg}"
            );
        }
    }

    #[test]
    fn reading_the_file_first_lifts_the_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.rs");
        std::fs::write(&f, "existing\n").unwrap();
        let p = f.to_str().unwrap();

        for tool in ["Edit", "Write", "MultiEdit", "NotebookEdit"] {
            assert!(
                read_before_edit_block(
                    tool,
                    &json!({ "file_path": p }),
                    &seen(tmp.path(), &[p]),
                    tmp.path()
                )
                .is_none(),
                "{tool} must run once the file has been read"
            );
        }
    }

    /// Creating a file is not overwriting one. Requiring a Read of something
    /// that does not exist would be unsatisfiable.
    #[test]
    fn creating_a_new_file_needs_no_prior_read() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("brand_new.rs");
        assert!(
            read_before_edit_block(
                "Write",
                &json!({ "file_path": p.to_str().unwrap() }),
                &seen(tmp.path(), &[]),
                tmp.path()
            )
            .is_none()
        );
    }

    #[test]
    fn read_only_tools_are_never_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.rs");
        std::fs::write(&f, "x\n").unwrap();
        for tool in ["Read", "Grep", "Glob", "Bash", "CodeSearch"] {
            assert!(
                read_before_edit_block(
                    tool,
                    &json!({ "file_path": f.to_str().unwrap() }),
                    &seen(tmp.path(), &[]),
                    tmp.path()
                )
                .is_none()
            );
        }
    }

    /// ApplyPatch hides its targets in the patch body, and names them relative
    /// to the working directory while Read is given absolute paths. Both
    /// spellings must resolve to the same file, or every patch following a read
    /// would be refused.
    #[test]
    fn apply_patch_targets_come_from_the_patch_body() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x\n").unwrap();
        let patch = json!({
            "patch": "--- a/a.rs\n+++ a.rs\n@@ -1 +1 @@\n-x\n+y\n"
        });

        assert_eq!(
            write_targets("ApplyPatch", &patch),
            vec!["a.rs".to_string()]
        );
        assert!(
            read_before_edit_block("ApplyPatch", &patch, &seen(tmp.path(), &[]), tmp.path())
                .is_some(),
            "an unread patched file must be refused"
        );
        let abs = tmp.path().join("a.rs").to_string_lossy().to_string();
        assert!(
            read_before_edit_block("ApplyPatch", &patch, &seen(tmp.path(), &[&abs]), tmp.path())
                .is_none(),
            "reading the absolute path must satisfy a relative patch target"
        );
    }

    /// A `Read` and an `Edit` of the same file in ONE parallel batch: the edit
    /// is still refused.
    ///
    /// Nothing orders the two calls — they are dispatched together via
    /// `join_all` — so the read has not completed when the edit would run.
    /// Letting it through because a read "is in flight" would reintroduce
    /// exactly the blind overwrite the guard exists to prevent.
    #[test]
    fn a_read_in_the_same_batch_does_not_unlock_the_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.rs");
        std::fs::write(&f, "existing\n").unwrap();
        let p = f.to_str().unwrap().to_string();

        let batch = vec![
            (
                "id_read".to_string(),
                "Read".to_string(),
                json!({ "file_path": p }),
            ),
            (
                "id_edit".to_string(),
                "Edit".to_string(),
                json!({ "file_path": p }),
            ),
        ];

        let refusals = refusals_for_batch(&batch, &seen(tmp.path(), &[]), tmp.path());
        assert!(
            refusals.contains_key("id_edit"),
            "the edit must be refused: a concurrent read has not landed yet"
        );
        assert!(!refusals.contains_key("id_read"), "reads are never refused");
    }

    /// Refusals are keyed by tool_use id, so one bad call in a batch cannot
    /// suppress its siblings.
    #[test]
    fn refusal_is_per_call_not_per_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let known = tmp.path().join("known.rs");
        let unknown = tmp.path().join("unknown.rs");
        std::fs::write(&known, "a\n").unwrap();
        std::fs::write(&unknown, "b\n").unwrap();
        let known_p = known.to_str().unwrap().to_string();

        let batch = vec![
            (
                "ok".to_string(),
                "Edit".to_string(),
                json!({ "file_path": known_p }),
            ),
            (
                "bad".to_string(),
                "Edit".to_string(),
                json!({ "file_path": unknown.to_str().unwrap() }),
            ),
        ];

        let refusals = refusals_for_batch(&batch, &seen(tmp.path(), &[&known_p]), tmp.path());
        assert_eq!(refusals.len(), 1, "only the unread target may be refused");
        assert!(refusals.contains_key("bad"));
    }

    /// The guard must resolve a patch target to the SAME path `apply_patch.rs`
    /// will write, or it silently fails open.
    ///
    /// `apply_patch.rs` strips a tab-separated timestamp and a git-style `b/`
    /// prefix before joining against the working directory. A guard that
    /// skipped those normalisations looked up `<wd>/b/a.rs`, found nothing,
    /// concluded "new file, no read required", and waved through an overwrite
    /// of an unread `<wd>/a.rs`.
    #[test]
    fn patch_targets_are_normalised_the_same_way_apply_patch_normalises_them() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x\n").unwrap();

        for header in [
            "+++ b/a.rs",                      // git-style prefix
            "+++ a.rs\t2024-01-01 00:00:00",   // trailing timestamp
            "+++ b/a.rs\t2024-01-01 00:00:00", // both
        ] {
            let patch = json!({ "patch": format!("--- a/a.rs\n{header}\n@@ -1 +1 @@\n-x\n+y\n") });
            assert_eq!(
                write_targets("ApplyPatch", &patch),
                vec!["a.rs".to_string()],
                "header {header:?} must resolve to the path apply_patch writes"
            );
            assert!(
                read_before_edit_block("ApplyPatch", &patch, &seen(tmp.path(), &[]), tmp.path())
                    .is_some(),
                "header {header:?}: guard failed open on an unread file"
            );
        }
    }

    /// A file the model just wrote is a file the model knows.
    ///
    /// Recording only reads meant `Write` could create a file and then never
    /// touch it again: it existed on disk, was absent from the seen set, and
    /// every later write was refused as a blind overwrite of content the model
    /// had authored itself one turn earlier.
    #[test]
    fn a_successful_write_counts_as_having_seen_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("new.rs");
        let ps = p.to_str().unwrap().to_string();

        // Turn 1: creating it is allowed.
        assert!(
            read_before_edit_block(
                "Write",
                &json!({ "file_path": ps }),
                &seen(tmp.path(), &[]),
                tmp.path()
            )
            .is_none()
        );
        std::fs::write(&p, "v1\n").unwrap();

        // The runner records write targets the same way it records reads.
        let mut have = seen(tmp.path(), &[]);
        for t in write_targets("Write", &json!({ "file_path": ps })) {
            have.insert(resolve_path(tmp.path(), &t));
        }

        // Turn 2: revising it must not be refused.
        for tool in ["Write", "Edit", "MultiEdit"] {
            assert!(
                read_before_edit_block(tool, &json!({ "file_path": ps }), &have, tmp.path())
                    .is_none(),
                "{tool} refused a file this session created"
            );
        }
    }

    /// The seen-set is keyed by resolved path, so the spelling used for the
    /// read need not match the spelling used for the write.
    #[test]
    fn path_spelling_does_not_decide_whether_a_file_counts_as_read() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("m.rs"), "x\n").unwrap();
        let abs = tmp.path().join("m.rs").to_string_lossy().to_string();

        // Read relative, write absolute.
        let have = seen(tmp.path(), &["m.rs"]);
        assert!(
            read_before_edit_block("Edit", &json!({ "file_path": abs }), &have, tmp.path())
                .is_none(),
            "a relative Read must satisfy an absolute Edit of the same file"
        );

        // Read absolute, write relative — and via a redundant './'.
        let have = seen(tmp.path(), &[&abs]);
        for spelling in ["m.rs", "./m.rs"] {
            assert!(
                read_before_edit_block(
                    "Edit",
                    &json!({ "file_path": spelling }),
                    &have,
                    tmp.path()
                )
                .is_none(),
                "an absolute Read must satisfy {spelling:?}"
            );
        }
    }

    /// F-06: the advice may not promise an intervention the runtime does not
    /// perform. Nothing blocks a tool on repeated failure, so nothing may say
    /// it will.
    #[test]
    fn repeated_failure_advice_never_claims_a_limit_it_cannot_enforce() {
        for count in 1..=(MAX_TOOL_ERRORS_PER_TOOL + 4) {
            let note = error_budget_note("Bash", count);
            assert!(note.contains(&count.to_string()), "{note}");
            assert!(
                !note.contains("remaining") && !note.contains("left"),
                "counting down to a limit that never binds: {note}"
            );
            assert!(
                !note.contains("attempts remaining"),
                "the removed claim came back: {note}"
            );
        }
        // It does get blunter once the streak is long.
        assert!(error_budget_note("Bash", MAX_TOOL_ERRORS_PER_TOOL).contains("different tool"));
    }

    /// Old tool results removed for space name where their full text is.
    #[test]
    fn removed_tool_results_name_their_original() {
        let mut msgs = vec![Message::user("go")];
        for i in 0..10 {
            msgs.push(Message::assistant_blocks(vec![ContentBlock::ToolUse {
                id: format!("t{i}"),
                name: "Read".into(),
                input: json!({}),
            }]));
            msgs.push(Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: format!("t{i}"),
                content: ToolResultContent::Text("x".repeat(5_000)),
                is_error: Some(false),
            }]));
        }
        let mut seen = Vec::new();
        let changed = apply_tool_result_budget_with(&mut msgs, 20_000, |id, len| {
            seen.push(id.to_string());
            format!("[tool result removed ({len} chars); full output: /raw/{id}]")
        });
        assert!(changed);
        assert_eq!(seen[0], "t0", "oldest first");
        assert!(matches!(&msgs[2].content, MessageContent::Blocks(b)
            if matches!(&b[0], ContentBlock::ToolResult { content: ToolResultContent::Text(t), .. } if t.contains("/raw/t0"))));
        // Nothing left to remove: no change, no double placeholder.
        assert!(!apply_tool_result_budget_with(
            &mut msgs,
            1_000_000,
            |_, _| unreachable!()
        ));
    }
}

#[cfg(test)]
mod retry_delay_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_local_backoff_is_unchanged() {
        // Without jitter: 1, 2, 4, 8, 16 s, then capped at 30 s.
        let nominal: Vec<u128> = (1..=7).map(|n| local_backoff(n, 0).as_millis()).collect();
        assert_eq!(nominal, vec![1000, 2000, 4000, 8000, 16000, 30000, 30000]);
        // Jitter below a quarter of the base.
        assert_eq!(local_backoff(1, 249).as_millis(), 1249);
        assert_eq!(local_backoff(1, 250).as_millis(), 1000);
        assert_eq!(local_backoff(3, 1999).as_millis(), 4999);
        // Never overflows.
        let far = local_backoff(200, u64::MAX).as_millis();
        assert!((30_000..37_500).contains(&far), "{far}");
    }

    #[test]
    fn the_longer_of_local_and_server_delays_wins() {
        assert_eq!(retry_delay(1, 100, None), Duration::from_millis(1100));
        assert_eq!(
            retry_delay(1, 100, Some(Duration::from_secs(12))),
            Duration::from_secs(12),
            "server longer"
        );
        assert_eq!(
            retry_delay(3, 100, Some(Duration::from_secs(1))),
            Duration::from_millis(4100),
            "server shorter: local kept"
        );
        assert_eq!(
            retry_delay(2, 0, Some(Duration::ZERO)),
            Duration::from_secs(2)
        );
        // A long server delay is not capped at the backoff's ceiling.
        assert_eq!(
            retry_delay(5, 0, Some(Duration::from_secs(120))),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn notices_say_what_happened_and_nothing_secret() {
        let d = Duration::from_millis(12000);
        let e = |code| CerseiError::from_http_status(code, None, "Authorization: Bearer sk-SECRET");
        assert_eq!(
            retry_notice(&e(429), 1, 5, d),
            "Rate limited (HTTP 429). Retrying in 12000 ms... (retry 1/5)"
        );
        assert_eq!(
            retry_notice(&e(503), 1, 5, d),
            "Service unavailable (HTTP 503). Retrying in 12000 ms... (retry 1/5)"
        );
        assert_eq!(
            retry_notice(&e(502), 2, 5, d),
            "Temporary provider error (HTTP 502). Retrying in 12000 ms... (retry 2/5)"
        );
        // 429 sent as a plain status is still a rate limit; nothing else is.
        let as_status = CerseiError::ProviderStatus {
            status: 429,
            message: "x".into(),
            retry_after: None,
        };
        assert!(retry_notice(&as_status, 1, 5, d).starts_with("Rate limited (HTTP 429)"));
        for code in [500, 502, 503, 504, 529] {
            let n = retry_notice(&e(code), 1, 5, d);
            assert!(!n.contains("Rate limited"), "{n}");
            assert!(n.contains(&format!("HTTP {code}")), "{n}");
            assert!(!n.contains("SECRET") && !n.contains("Bearer"), "{n}");
        }
    }
}

#[cfg(test)]
mod compaction_cancel_tests {
    use super::*;

    struct Unreachable;

    #[async_trait::async_trait]
    impl cersei_provider::Provider for Unreachable {
        fn name(&self) -> &str {
            "unreachable"
        }
        fn context_window(&self, _: &str) -> u64 {
            100_000
        }
        async fn complete(
            &self,
            _: CompletionRequest,
        ) -> Result<cersei_provider::CompletionStream> {
            panic!("a cancelled compaction sends nothing");
        }
    }

    /// A cancellation is not an unsuccessful attempt: it never counts
    /// towards disabling compaction, and the next automatic compaction may
    /// try the same history.
    #[tokio::test]
    async fn a_cancelled_compaction_poisons_nothing() {
        let mut history = Vec::new();
        for i in 0..12 {
            history.push(Message::user(format!(
                "step {i}: {}",
                "detail ".repeat(300)
            )));
            history.push(Message::assistant(format!("done {i}")));
        }
        let agent = Agent::builder()
            .provider(Unreachable)
            .with_messages(history)
            .working_dir(std::env::temp_dir())
            .build()
            .unwrap();
        let cancelled = tokio_util::sync::CancellationToken::new();
        cancelled.cancel();
        for _ in 0..5 {
            let outcome = run_compaction(
                &agent,
                CompactReason::ThresholdExceeded,
                false,
                None,
                &cancelled,
            )
            .await;
            assert_eq!(outcome, crate::compact::CompactionOutcome::Cancelled);
        }
        let st = agent.compaction_state.lock();
        assert_eq!(st.failures, 0);
        assert!(!st.disabled);
        assert_eq!(st.last_unsuccessful, None);
        assert_eq!(st.applied, 0);
    }
}
