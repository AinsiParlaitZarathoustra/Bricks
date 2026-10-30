//! Agent runner: the core agentic loop.

use crate::compact;
use crate::context::{ModelView, RequestView};
use crate::events::{AgentEvent, CompactReason};
use crate::{Agent, AgentOutput, ToolCallRecord, UserInput};
use bricks_hooks::{HookAction, HookContext, HookEvent};
use bricks_provider::{CompletionRequest, ProviderOptions, StreamAccumulator};
use bricks_tools::{ToolContext, ToolResult};
use bricks_types::*;
use std::sync::Arc;
use tokio::sync::mpsc;

mod context;
mod dispatch;
mod history;
mod skills;
mod stream;
mod tools;

#[cfg(test)]
use context::MIN_TOOL_RESULT_BUDGET;
pub use context::{apply_tool_result_budget, apply_tool_result_budget_with};
use context::{
    apply_tool_result_budget_protected, ensure_budget, model_view, raw_tool_result, run_compaction,
    tool_definitions, tool_result_budget,
};
pub(crate) use context::{compact_now, context_status};
pub(crate) use history::{clear_context, load_session_history, revise_context, save_session};
use history::{inject_pending, push_message, repair_interrupted};
use skills::{activate_user_skills, commit_skill_activation, is_skill_proposal};
#[cfg(test)]
use tools::MAX_TOOL_ERRORS_PER_TOOL;
use tools::{
    CallSlot, FinishedCall, UNPLACED, call_future, error_budget_note, interrupted_text,
    keep_interrupted_answer, publish_call, tool_result_block, tool_result_with_images,
};

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
pub(crate) fn retry_label(e: &BricksError) -> String {
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

/// The error of a retry the server's `Retry-After` rules out: the wait it
/// asks for is longer than the budget, so no retry is sent.
pub(crate) fn retry_budget_refusal(
    e: &BricksError,
    asked: std::time::Duration,
    budget: std::time::Duration,
) -> BricksError {
    BricksError::Provider(format!(
        "{} and the server asks to wait {} ms before retrying, more than [agent] \
         retry_after_max_wait_ms = {}: no retry was sent. Try again later.",
        retry_label(e),
        asked.as_millis(),
        budget.as_millis()
    ))
}

/// The one notice of a retry, sent on every existing channel.
pub(crate) fn retry_notice(
    e: &BricksError,
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

/// The id the next run of the session agent takes as its root run id (set by
/// the controller before each run).
pub(crate) struct RootRunHint(pub String);

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
    permissions: Arc<dyn bricks_tools::permissions::PermissionPolicy>,
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
    end_root_run_commands(agent).await;
    if let Some(s) = agent.skill_session() {
        s.ledger.lock().clear_pending();
    }
    if let Err(e) = &result {
        let why = if matches!(e, BricksError::Cancelled) {
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

/// The end of a root run, whether it completed, failed or was cancelled: the
/// commands of its agent that were handed over as tasks (a foreground command
/// past `yield_time_ms`, not started with `background: true`) end with it.
/// Explicit background jobs outlive the run and end with the session. A
/// sub-agent's jobs end with the sub-agent (see `agents::spawn`).
async fn end_root_run_commands(agent: &Agent) {
    let Some(identity) = agent.extensions.get::<crate::agents::AgentIdentity>() else {
        return;
    };
    if identity.parent_id.is_some() {
        return;
    }
    if let Some(jobs) = agent.extensions.get::<bricks_tools::jobs::JobsHandle>() {
        jobs.0.stop_ordinary_owned_by(&identity.agent_id).await;
    }
}

/// The long-term memory's maintenance after a run, reported as its own
/// phase: started, then finished (completed, cancelled or failed). The
/// answer was already delivered; a failure here never changes it.
pub(crate) async fn maintain_memory(
    agent: &Agent,
    cancel: &tokio_util::sync::CancellationToken,
    tx: Option<&mpsc::Sender<AgentEvent>>,
) {
    let Some(ltm) = agent.long_term_memory() else {
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
    permissions: Arc<dyn bricks_tools::permissions::PermissionPolicy>,
) -> Result<AgentOutput> {
    let prompt = input.text.as_str();
    // Nothing to do: refused before the session is loaded or a request sent.
    if crate::subagent::is_blank(prompt) && input.attachments.is_empty() {
        return Err(BricksError::InvalidInput(
            "the prompt is empty and has no attachment".into(),
        ));
    }
    let event_tx = event_tx.clone();
    // The long-term memory of this run, read once: recall, the memory
    // tools and the capture below use it, whatever changes meanwhile.
    let long_term = agent.long_term_memory();
    agent
        .extensions
        .insert(bricks_tools::memory_tools::RunMemory(long_term.clone()));
    // Load session history (skip if messages were pre-populated via
    // with_messages), or announce the one restored when the session opened.
    let loaded = if agent.messages.lock().is_empty() {
        load_session_history(agent).await?
    } else {
        agent.restored_unannounced.lock().take()
    };
    if let (Some(count), Some(session_id)) = (loaded, &agent.session_id) {
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

    let mut notes = std::mem::take(&mut *agent.config_notes.lock());
    notes.extend(agent.connect_mcp().await);
    agent.refresh_lent_tools();
    // Long-term memory: recall for this prompt, within its budget (at most
    // a tenth of the prompt budget), appended to the system prompt.
    *agent.recalled.lock() = None;
    if let Some(ltm) = &long_term {
        let budget = (agent.memory_recall_tokens as u64)
            .min(context_status(agent).input_limit / 10)
            .max(1) as usize;
        match ltm.recall_context(prompt, budget).await {
            Ok(Some(r)) if r.items == 0 => {
                // Nothing recalled, and part of the memory unavailable.
                notes.extend(r.warnings);
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
            Ok(Some(r)) => {
                notes.extend(r.warnings);
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
            .insert(bricks_tools::shell::ProgressSink(Arc::new(
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
        // The run's id as its starter knows it (the controller's run id), so
        // usage totals, sub-agents and records name the same run; a fresh one
        // otherwise.
        let run_id = agent
            .extensions
            .get::<RootRunHint>()
            .map(|h| h.0.clone())
            .unwrap_or_else(|| format!("run_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]));
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
        agent.extensions.insert(bricks_tools::jobs::JobOwner {
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
        .insert(bricks_tools::code_scout::RunCancel(cancel.clone()));
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
            return Err(BricksError::Cancelled);
        }

        let _ = event_tx.send(AgentEvent::TurnStart { turn }).await;
        agent.emit(AgentEvent::TurnStart { turn });

        // Skills removed, installed again or changed since they were
        // activated: withdrawn before anything is sent (a tool call in
        // progress already ended: this is between two requests).
        for ev in agent.reconcile_skills() {
            send(agent, Some(&event_tx), ev).await;
        }

        // Apply tool result budget to keep context manageable. Removed
        // results name their saved original, and the measurement of the old
        // history no longer applies.
        let tool_result_chars = tool_result_budget(agent.tool_result_budget, &model_view(agent));
        let removed = {
            let mut msgs = agent.messages.lock();
            let refs = agent.raw_refs.lock();
            let raw = agent.raw_history.lock();
            let protected = agent.active_skill_markers();
            apply_tool_result_budget_protected(
                &mut msgs,
                tool_result_chars,
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
        let tool_defs = tool_definitions(agent);

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
            options.set(bricks_provider::REASONING_PROFILE_OPTION, &profile);
        }

        // The system of this request: guidance for the tools it announces,
        // its own date, environment, git state, model and plan (the plan's
        // complete state replaces any reminder), built before the budget
        // measures it.
        if turn == 1 {
            agent.request_started();
        }
        let announced: Vec<String> = tool_defs.iter().map(|d| d.name.clone()).collect();
        let system_with_nudge = agent.request_system(&announced, &selection, &options).await;

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
                _ = cancel.cancelled() => return Err(BricksError::Cancelled),
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
                        return Err(BricksError::Cancelled);
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
                    let server = e.retry_after();
                    // A server's Retry-After beyond the budget is not waited
                    // for, and no retry is sent: the run says why, at once.
                    if let Some(asked) = server
                        && asked > agent.retry_after_max_wait
                    {
                        let refusal = retry_budget_refusal(&e, asked, agent.retry_after_max_wait);
                        tracing::warn!("{refusal}");
                        return Err(refusal);
                    }
                    let delay = retry_delay(retry_count, rand_jitter(), server);
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
                        _ = cancel.cancelled() => return Err(BricksError::Cancelled),
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

        let ((mut early, mut early_done, early_ids), received) = stream::receive(
            agent,
            &event_tx,
            &tool_ctx,
            &permissions,
            cancel,
            turn,
            provider.async_tool_calls(),
            &mut rx,
            &mut accumulator,
        )
        .await;

        if let stream::Received::Cancelled = received {
            keep_interrupted_answer(
                agent,
                &event_tx,
                &tool_ctx,
                accumulator.finished_blocks(),
                early,
                early_done,
                &early_ids,
                "cancelled by the user",
            )
            .await;
            crate::agents::spawn::settle_children(
                &agent.extensions,
                std::time::Duration::from_secs(10),
            )
            .await;
            return Err(BricksError::Cancelled);
        }

        if let stream::Received::Failed(e) = received {
            let allowed = agent.context.lock().policy().max_overflow_recoveries;
            // A call already started may have had effects: the turn is not
            // redone behind it.
            if e.is_context_overflow() && overflow_recoveries < allowed && early_ids.is_empty() {
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
                    return Err(BricksError::Cancelled);
                }
                if outcome.is_compacted() {
                    // The same turn again: it produced nothing.
                    turn = turn.saturating_sub(1);
                    continue;
                }
            }
            keep_interrupted_answer(
                agent,
                &event_tx,
                &tool_ctx,
                accumulator.finished_blocks(),
                early,
                early_done,
                &early_ids,
                "interrupted by an error",
            )
            .await;
            return Err(e);
        }

        // Convert accumulated response (a stream closed before its end is
        // an error, after which the calls started are kept as above).
        let finished = (!early_ids.is_empty()).then(|| accumulator.finished_blocks());
        let response = match accumulator.into_response() {
            Ok(response) => response,
            Err(e) => {
                keep_interrupted_answer(
                    agent,
                    &event_tx,
                    &tool_ctx,
                    finished.unwrap_or_default(),
                    early,
                    early_done,
                    &early_ids,
                    "interrupted by an error",
                )
                .await;
                return Err(e);
            }
        };
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
        let hook_action = bricks_hooks::run_hooks(&agent.hooks, &hook_ctx).await;
        if let HookAction::Block(reason) = hook_action {
            return Err(BricksError::Provider(format!(
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
            let _ = bricks_hooks::run_hooks(&agent.hooks, &cadence_ctx).await;
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
            // stays valid whatever happens next. Those already started while
            // it streamed were complete: they end and keep their results.
            let mut started: Vec<FinishedCall> = std::mem::take(&mut early_done);
            loop {
                let next = tokio::select! {
                    n = futures::StreamExt::next(&mut early) => n,
                    _ = cancel.cancelled() => {
                        drop(early);
                        let blocks: Vec<ContentBlock> = tool_use_blocks
                            .iter()
                            .map(|(id, _, _)| {
                                tool_result_block(id, interrupted_text("cancelled by the user"), true)
                            })
                            .collect();
                        push_message(agent, Message::user_blocks(blocks));
                        return Err(BricksError::Cancelled);
                    }
                };
                let Some(call) = next else { break };
                started.push(call);
            }
            let mut blocks: Vec<ContentBlock> = Vec::new();
            for (id, name, _) in &tool_use_blocks {
                let block = match started.iter().position(|c| &c.id == id) {
                    Some(at) => {
                        let call = started.swap_remove(at);
                        let p = publish_call(agent, &event_tx, &tool_ctx, call, false).await;
                        let (capped, _) = p.render();
                        tool_result_block(&p.id, capped, p.result.is_error)
                    }
                    None => ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: ToolResultContent::Text(format!(
                            "{name} was not run: your answer was cut by the output-token limit, \
                             possibly inside this call. Send it again if you still need it."
                        )),
                        is_error: Some(true),
                    },
                };
                blocks.push(block);
            }
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
            // Phase 1: Emit ToolStart events for the tools not started yet
            for (tool_id, tool_name, tool_input) in &tool_use_blocks {
                if early_ids.contains(tool_id) {
                    continue;
                }
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

            // The calls started while the answer streamed go on; the others
            // start now. Those already finished are placed first, so a
            // cancellation never loses their results.
            let mut pending = std::mem::take(&mut early);
            let mut finished_early = std::mem::take(&mut early_done).into_iter();
            let cumulative_cost = cumulative.cost_usd.unwrap_or(0.0);
            for (index, (tool_id, tool_name, tool_input)) in tool_use_blocks.iter().enumerate() {
                if early_ids.contains(tool_id) {
                    continue;
                }
                pending.push(call_future(
                    agent,
                    &tool_ctx,
                    &permissions,
                    &registered_tool_names,
                    index,
                    (tool_id.clone(), tool_name.clone(), tool_input.clone()),
                    turn,
                    cumulative_cost,
                    msg_count,
                ));
            }

            // One slot per call, in the order of the calls.
            let mut slots: Vec<Option<CallSlot>> =
                (0..tool_use_blocks.len()).map(|_| None).collect();
            let mut cancelled = false;
            loop {
                // Cancelling the turn drops the running tool futures: a
                // shell command is then interrupted by its own guard instead
                // of running to its timeout. No call starts after it.
                let next = match finished_early.next() {
                    Some(call) => Some(call),
                    None => tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            cancelled = true;
                            break;
                        }
                        n = futures::StreamExt::next(&mut pending) => n,
                    },
                };
                let Some(mut call) = next else { break };
                if call.index == UNPLACED {
                    // Every call started early is in the complete answer.
                    let Some(at) = tool_use_blocks.iter().position(|(id, _, _)| *id == call.id)
                    else {
                        continue;
                    };
                    call.index = at;
                }
                let index = call.index;
                // A skill activation proposed by this agent's own `Skill`
                // tool is announced only once committed (it may need a
                // compaction): that happens below, in the order of the calls.
                slots[index] = Some(if is_skill_proposal(agent, &call) {
                    CallSlot::Deferred(call)
                } else {
                    CallSlot::Published(
                        publish_call(agent, &event_tx, &tool_ctx, call, false).await,
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
                            let p = publish_call(agent, &event_tx, &tool_ctx, failed, false).await;
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
                return Err(BricksError::Cancelled);
            }

            // Phase 3: assemble the round in the order of the calls (the
            // history, the progress check and the failure streaks never
            // depend on which call finished first).
            let mut result_blocks: Vec<ContentBlock> = Vec::new();
            // (name, input, is_error, output) of each call, for the
            // progress check.
            let mut round: Vec<(String, String, bool, String)> = Vec::new();
            let mut raw_blocks: Vec<ContentBlock> = Vec::new();
            // Images the calls showed, when they travel after the results.
            let mut image_blocks: Vec<ContentBlock> = Vec::new();
            let mut raw_image_blocks: Vec<ContentBlock> = Vec::new();
            let transport = agent.provider().image_transport();

            for slot in slots.into_iter().flatten() {
                let mut p = match slot {
                    CallSlot::Published(p) => p,
                    CallSlot::Deferred(mut call) => {
                        // Committed once it fits, refused whole otherwise.
                        // Its text is never reduced.
                        let verified =
                            commit_skill_activation(agent, &call.name, &mut call.result, &event_tx)
                                .await;
                        publish_call(agent, &event_tx, &tool_ctx, call, verified).await
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
                let images = std::mem::take(&mut p.result.images);
                let (raw_block, raw_extra) = tool_result_with_images(
                    transport,
                    &p.id,
                    full_text.clone(),
                    p.result.is_error,
                    &images,
                );
                raw_blocks.push(raw_block);
                raw_image_blocks.extend(raw_extra);
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
                let (block, extra) = tool_result_with_images(
                    transport,
                    &p.id,
                    capped_content,
                    p.result.is_error,
                    &images,
                );
                result_blocks.push(block);
                image_blocks.extend(extra);
            }
            // Images after every result: a protocol that puts results in their
            // own messages needs them to follow the calls' turn directly.
            result_blocks.extend(image_blocks);
            raw_blocks.extend(raw_image_blocks);

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
                return Err(BricksError::Cancelled);
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
    if let Some(ltm) = &long_term {
        let now = chrono::Utc::now().timestamp_millis();
        let mut turns = vec![bricks_memory::MemoryTurn {
            role: "user".into(),
            content: prompt.to_string(),
            at: Some(now),
        }];
        if let Some(text) = last_message.get_text().filter(|t| !t.trim().is_empty()) {
            turns.push(bricks_memory::MemoryTurn {
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

async fn send(agent: &Agent, tx: Option<&mpsc::Sender<AgentEvent>>, event: AgentEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event.clone()).await;
    }
    agent.emit(event);
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
fn record_run_usage(agent: &Agent, usage: &bricks_types::Usage) {
    if let (Some(rt), Some(id)) = (
        agent.extensions.get::<crate::agents::RuntimeHandle>(),
        agent.extensions.get::<crate::agents::AgentIdentity>(),
    ) {
        rt.0.add_usage(&id.root_run_id, id.parent_id.is_none(), usage);
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use serde_json::json;

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

    #[test]
    fn the_tool_result_budget_follows_the_model_unless_set() {
        let model = |input| ModelView {
            selection: "m".into(),
            limits: bricks_provider::ModelLimits {
                max_input_tokens: input,
                max_output_tokens: 0,
            },
            reasoning_resent: false,
            token_counting: false,
            limits_reported: true,
        };
        assert_eq!(tool_result_budget(None, &model(1_000_000)), 1_000_000);
        assert_eq!(
            tool_result_budget(None, &model(8_000)),
            MIN_TOOL_RESULT_BUDGET
        );
        assert_eq!(tool_result_budget(None, &model(0)), MIN_TOOL_RESULT_BUDGET);
        assert_eq!(tool_result_budget(Some(20_000), &model(1_000_000)), 20_000);
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
        let e = |code| BricksError::from_http_status(code, None, "Authorization: Bearer sk-SECRET");
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
        let as_status = BricksError::ProviderStatus {
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
    impl bricks_provider::Provider for Unreachable {
        fn name(&self) -> &str {
            "unreachable"
        }
        fn context_window(&self, _: &str) -> u64 {
            100_000
        }
        async fn complete(
            &self,
            _: CompletionRequest,
        ) -> Result<bricks_provider::CompletionStream> {
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
