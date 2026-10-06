//! Auto-compact: context window management for long conversations.
//!
//! When the conversation approaches the context window limit, older messages
//! are summarized to free space while preserving essential context.

use cersei_provider::Provider;
use cersei_types::*;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Fraction of context window that triggers auto-compact.
pub const AUTOCOMPACT_TRIGGER_FRACTION: f64 = 0.90;
/// Number of recent messages to always preserve (never compacted).
pub const KEEP_RECENT_MESSAGES: usize = 10;
/// Max consecutive failures before disabling auto-compact.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;
/// Warning threshold (80% of context window).
pub const WARNING_PCT: f64 = 0.80;
/// Critical threshold (95% of context window).
pub const CRITICAL_PCT: f64 = 0.95;

// ─── Types ───────────────────────────────────────────────────────────────────

/// Session-level compaction tracking.
#[derive(Debug, Clone, Default)]
pub struct AutoCompactState {
    pub compaction_count: u32,
    pub consecutive_failures: u32,
    pub disabled: bool,
}

impl AutoCompactState {
    pub fn on_success(&mut self) {
        self.compaction_count += 1;
        self.consecutive_failures = 0;
    }

    pub fn on_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            self.disabled = true;
        }
    }
}

/// Context window fullness level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenWarningState {
    /// Below 80% — no action needed.
    Ok,
    /// 80-95% — warn user, consider compacting.
    Warning,
    /// Above 95% — critical, must compact or will fail.
    Critical,
}

/// A semantically coherent group of messages for summarization.
#[derive(Debug, Clone)]
pub struct MessageGroup {
    pub messages: Vec<Message>,
    pub topic_hint: Option<String>,
    pub token_estimate: usize,
}

/// Result of a compaction operation.
#[derive(Debug, Clone)]
pub struct CompactResult {
    pub messages_before: usize,
    pub messages_after: usize,
    pub tokens_freed_estimate: u64,
    pub summary: String,
}

/// What triggered the compaction.
#[derive(Debug, Clone, Copy)]
pub enum CompactTrigger {
    AutoThreshold,
    Manual,
    ContextOverflow,
}

// ─── Token estimation ────────────────────────────────────────────────────────

/// Local token estimate of a text (see [`cersei_types::tokens`]): an
/// estimate, never a measurement.
pub fn estimate_tokens(text: &str) -> u64 {
    cersei_types::tokens::estimate_text(text).tokens
}

/// Local token estimate of a list of messages, every block included.
pub fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    crate::context::Estimator::new(&crate::context::ContextPolicy::default(), true, 1.0)
        .messages(messages)
        .tokens
}

// ─── Warning state ───────────────────────────────────────────────────────────

/// Calculate the token warning state given current usage.
pub fn calculate_token_warning_state(tokens_used: u64, context_limit: u64) -> TokenWarningState {
    if context_limit == 0 {
        return TokenWarningState::Ok;
    }
    let pct = tokens_used as f64 / context_limit as f64;
    if pct >= CRITICAL_PCT {
        TokenWarningState::Critical
    } else if pct >= WARNING_PCT {
        TokenWarningState::Warning
    } else {
        TokenWarningState::Ok
    }
}

// ─── Should compact ──────────────────────────────────────────────────────────

/// Check if compaction should trigger.
pub fn should_compact(tokens_used: u64, context_limit: u64) -> bool {
    if context_limit == 0 {
        return false;
    }
    (tokens_used as f64 / context_limit as f64) >= AUTOCOMPACT_TRIGGER_FRACTION
}

/// Check if auto-compact should run (considering state/circuit breaker).
pub fn should_auto_compact(tokens_used: u64, context_limit: u64, state: &AutoCompactState) -> bool {
    if state.disabled {
        return false;
    }
    should_compact(tokens_used, context_limit)
}

/// Check if context collapse is needed (emergency, >98%).
pub fn should_context_collapse(tokens_used: u64, context_limit: u64) -> bool {
    if context_limit == 0 {
        return false;
    }
    (tokens_used as f64 / context_limit as f64) >= 0.98
}

// ─── Message grouping ────────────────────────────────────────────────────────

/// Extract a topic hint from messages (first file path or tool name).
fn extract_topic_hint(messages: &[Message]) -> Option<String> {
    for msg in messages {
        for block in msg.content_blocks() {
            if let ContentBlock::ToolUse { name, input, .. } = &block {
                if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                    return Some(path.to_string());
                }
                return Some(name.clone());
            }
        }
    }
    None
}

/// Group messages into semantically coherent chunks at API-round boundaries.
/// Each group = one assistant response + its tool results.
pub fn group_messages_for_compact(messages: &[Message]) -> Vec<MessageGroup> {
    let mut groups: Vec<MessageGroup> = Vec::new();
    let mut current: Vec<Message> = Vec::new();

    for msg in messages {
        current.push(msg.clone());
        // End group at assistant messages that don't have tool use (end of a "round")
        if msg.role == Role::Assistant && !msg.has_tool_use() {
            let token_est = current.iter().map(|m| m.get_all_text().len() / 4).sum();
            let hint = extract_topic_hint(&current);
            groups.push(MessageGroup {
                messages: std::mem::take(&mut current),
                topic_hint: hint,
                token_estimate: token_est,
            });
        }
    }
    // Leftover messages
    if !current.is_empty() {
        let token_est = current.iter().map(|m| m.get_all_text().len() / 4).sum();
        let hint = extract_topic_hint(&current);
        groups.push(MessageGroup {
            messages: current,
            topic_hint: hint,
            token_estimate: token_est,
        });
    }
    groups
}

// ─── Tool-pair-aware splitting (F-04) ────────────────────────────────────────

/// `tool_result` ids in `msg` that no earlier `tool_use` in `msgs` answers.
///
/// This is the same check every provider runs server-side. A `tool_result`
/// without its `tool_use` in the *same request* is a 400, and the runner maps a
/// 400 to `CerseiError::Provider`, which `is_retryable()` does not match — so
/// the conversation is wedged for good rather than retried.
pub fn find_orphaned_tool_results(msgs: &[Message]) -> Vec<String> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut orphaned = Vec::new();
    for m in msgs {
        let MessageContent::Blocks(blocks) = &m.content else {
            continue;
        };
        // Same message first: a tool_use and its result never share a message
        // in practice, but scanning uses before results keeps that from
        // mattering.
        for b in blocks {
            if let ContentBlock::ToolUse { id, .. } = b {
                seen.insert(id);
            }
        }
        for b in blocks {
            if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                if !seen.contains(tool_use_id.as_str()) {
                    orphaned.push(tool_use_id.clone());
                }
            }
        }
    }
    orphaned
}

/// The mirror rule (§10.5 #3): `tool_use` ids in `msgs` that no `tool_result`
/// anywhere in `msgs` answers.
///
/// Providers enforce this direction too — an assistant `tool_use` with no
/// `tool_result` in the following user message is the same unretryable 400 as
/// an orphaned result. Every request the runner builds ends with a user
/// message, so at request time there is no legitimately-unanswered call.
pub fn find_unanswered_tool_uses(msgs: &[Message]) -> Vec<String> {
    let mut answered: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for m in msgs {
        let MessageContent::Blocks(blocks) = &m.content else {
            continue;
        };
        for b in blocks {
            if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                answered.insert(tool_use_id);
            }
        }
    }
    let mut unanswered = Vec::new();
    for m in msgs {
        let MessageContent::Blocks(blocks) = &m.content else {
            continue;
        };
        for b in blocks {
            if let ContentBlock::ToolUse { id, .. } = b {
                if !answered.contains(id.as_str()) {
                    unanswered.push(id.clone());
                }
            }
        }
    }
    unanswered
}

/// True if `msg` carries any `tool_result` block.
fn carries_tool_result(msg: &Message) -> bool {
    match &msg.content {
        MessageContent::Blocks(b) => b
            .iter()
            .any(|x| matches!(x, ContentBlock::ToolResult { .. })),
        _ => false,
    }
}

/// Where to cut a history so that `msgs[split..]` is a valid request.
///
/// The naive `len - keep_n` is wrong half the time. The runner builds a
/// strictly alternating history (idx 0 `user`, odd `assistant[tool_use]`, even
/// `user[tool_result]`), and `KEEP_RECENT_MESSAGES` is even, so
/// `parity(split) == parity(len)`: for every even-length conversation the cut
/// lands on a `user[tool_result]` and discards the `tool_use` that answers it.
///
/// Backing off one message reaches the `assistant[tool_use]`, which repairs the
/// pair and, for the summary path, also removes the `user(summary)` /
/// `user(tool_result)` adjacency that Gemini and most local chat templates
/// reject. It only ever keeps *more* context than asked, never less.
pub fn pair_aware_split(msgs: &[Message], keep_n: usize) -> usize {
    let mut split = msgs.len().saturating_sub(keep_n);
    while split > 0 && carries_tool_result(&msgs[split]) {
        split -= 1;
    }
    split
}

// ─── Snip compact (simple truncation) ────────────────────────────────────────

/// Stands in for the turns the snip fallback dropped, so the history still
/// opens with a `user` message. Kept short: it is pure overhead on a path taken
/// precisely when context is scarce.
pub const SNIP_TRUNCATION_NOTICE: &str =
    "[earlier turns were dropped to free context; continue from the messages below]";

/// Remove oldest messages, keeping the newest `keep_n` — plus one more when
/// that boundary would sever a `tool_use`/`tool_result` pair (see
/// [`pair_aware_split`]). `keep_n` is a floor, not an exact count.
///
/// Returns (remaining messages, estimated tokens freed).
pub fn snip_compact(messages: Vec<Message>, keep_n: usize) -> (Vec<Message>, u64) {
    if messages.len() <= keep_n {
        return (messages, 0);
    }
    let split = pair_aware_split(&messages, keep_n);
    let freed = estimate_messages_tokens(&messages[..split]);
    let mut kept = messages[split..].to_vec();

    // Backing off to keep a tool pair intact lands on the `assistant[tool_use]`,
    // so the surviving history opens with an assistant turn. The summary path
    // gets away with that because it prepends `user(summary)`; this path
    // prepends nothing, and Anthropic rejects any request whose first message is
    // not `user`. Fixing only the orphan would have swapped one guaranteed 400
    // for another — so the marker goes in here, where it also tells the model
    // why its history starts mid-task.
    if !matches!(kept.first().map(|m| m.role), Some(Role::User) | None) {
        kept.insert(0, Message::user(SNIP_TRUNCATION_NOTICE));
    }
    (kept, freed)
}

/// Calculate how many messages to keep given a token budget.
pub fn calculate_messages_to_keep_index(messages: &[Message], token_budget: u64) -> usize {
    let mut total: u64 = 0;
    for (i, msg) in messages.iter().rev().enumerate() {
        total += estimate_tokens(&msg.get_all_text());
        if total > token_budget {
            return messages.len() - i;
        }
    }
    0 // keep all
}

// ─── Collapse strategies ─────────────────────────────────────────────────────

/// Collapse repeated file read results: if the same file is read multiple
/// times, only keep the latest result.
pub fn collapse_read_tool_results(messages: Vec<Message>) -> Vec<Message> {
    let mut seen_files: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result: Vec<Message> = Vec::new();

    // Process in reverse to keep latest reads
    for msg in messages.into_iter().rev() {
        let dominated = match &msg.content {
            MessageContent::Blocks(blocks) => {
                blocks.iter().all(|b| {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = b
                    {
                        // Check if this is a file read result we've already seen
                        if let ToolResultContent::Text(text) = content {
                            if text.contains('\t') {
                                // Line-numbered output = file read
                                let key = tool_use_id.clone();
                                if seen_files.contains(&key) {
                                    return true; // dominated, skip
                                }
                                seen_files.insert(key);
                            }
                        }
                        false
                    } else {
                        false
                    }
                })
            }
            _ => false,
        };

        if !dominated {
            result.push(msg);
        }
    }

    result.reverse();
    result
}

// ─── Full compaction (requires provider call) ────────────────────────────────

/// Parameters of one compaction.
#[derive(Debug, Clone)]
pub struct CompactionPlan {
    /// Messages kept verbatim at the end (a floor that may grow to keep a
    /// tool exchange whole).
    pub keep_recent_messages: usize,
    /// The kept tail may not exceed this many tokens; a larger tail is
    /// shortened down to the last message group.
    pub max_recent_tokens: u64,
    /// Prompt budget of the model.
    pub input_limit: u64,
    /// Output reserved for the summary.
    pub summary_max_tokens: u32,
    /// Required relative gain of the active context.
    pub min_gain: f64,
    /// Tokens of the instructions and tool definitions sent with every request.
    pub frame_tokens: u64,
    /// Extra instructions for the summariser.
    pub instructions: Option<String>,
}

/// What a compaction did. Anything but `Compacted` leaves the history as it
/// was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionOutcome {
    Compacted {
        messages_before: usize,
        messages_after: usize,
        tokens_before: u64,
        tokens_after: u64,
    },
    /// The summary would not shrink the context enough: history kept.
    InsufficientGain {
        tokens_before: u64,
        tokens_after: u64,
    },
    /// The summary call failed or produced nothing usable: history kept.
    Failed { reason: String },
    /// Nothing to do (history too short, compaction disabled, no progress).
    Skipped { reason: String },
}

impl CompactionOutcome {
    pub fn is_compacted(&self) -> bool {
        matches!(self, CompactionOutcome::Compacted { .. })
    }
}

impl std::fmt::Display for CompactionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactionOutcome::Compacted { messages_before, messages_after, tokens_before, tokens_after } => write!(
                f,
                "compacted: {messages_before} → {messages_after} messages, ~{tokens_before} → ~{tokens_after} tokens"
            ),
            CompactionOutcome::InsufficientGain { tokens_before, tokens_after } => write!(
                f,
                "not applied: the summary would only bring ~{tokens_before} to ~{tokens_after} tokens"
            ),
            CompactionOutcome::Failed { reason } => write!(f, "failed: {reason}"),
            CompactionOutcome::Skipped { reason } => write!(f, "skipped: {reason}"),
        }
    }
}

/// Result of [`compact_history`].
#[derive(Debug, Clone)]
pub struct CompactionRun {
    pub outcome: CompactionOutcome,
    /// The new active history, only when compacted.
    pub messages: Option<Vec<Message>>,
    /// Usage of the summary call, when one was made (it counts in the
    /// session totals whatever the outcome).
    pub usage: Option<Usage>,
}

impl CompactionRun {
    fn without_call(outcome: CompactionOutcome) -> Self {
        Self {
            outcome,
            messages: None,
            usage: None,
        }
    }
}

/// System prompt of the summary call.
pub const SUMMARY_SYSTEM_PROMPT: &str =
    "You compact the history of a coding agent's session so the \
agent can continue the same work with less context. Be factual and complete; never invent.";

/// Opening tag of a compaction summary in the history.
pub const SUMMARY_OPEN: &str = "<context_summary>";

/// Build the compaction prompt for the LLM.
pub fn get_compact_prompt(custom_instructions: Option<&str>) -> String {
    let mut prompt = String::from(
        "Summarise the transcript above for the agent that will continue this session. \
         Use these sections, in this order, with bullet points:\n\
         1. Objective — what the user wants, in their terms.\n\
         2. Active instructions and constraints — every rule, preference and limit the user \
            or the system set that still applies (quote exact wording where it matters).\n\
         3. Decisions — what was decided and why.\n\
         4. Files — every file read, created or modified, with its path verbatim and what \
            changed.\n\
         5. Actions done — commands run and their outcomes (tests passing/failing, builds).\n\
         6. Open problems and next steps — what is unresolved, failing, or still to do.\n\
         7. Key facts — identifiers, values, error messages and versions needed later, verbatim.\n\
         Omit nothing actionable. Do not add advice that was not in the transcript.",
    );
    if let Some(instructions) = custom_instructions {
        prompt.push_str("\n\nAdditional instructions: ");
        prompt.push_str(instructions);
    }
    prompt
}

/// Format raw compact output into a summary message.
pub fn format_compact_summary(raw: &str) -> String {
    format!(
        "{SUMMARY_OPEN}\nThe following is a summary of the conversation so far:\n\n{}\n</context_summary>",
        raw.trim()
    )
}

/// Where to cut so that the tail keeps at least `keep_recent` messages, stays
/// within `max_tokens` when possible, and never separates a tool call from
/// its result. `0` means nothing can be summarised.
pub fn choose_split(
    messages: &[Message],
    keep_recent: usize,
    max_tokens: u64,
    est: &crate::context::Estimator,
) -> usize {
    let mut split = pair_aware_split(messages, keep_recent);
    let tail = |from: usize| est.messages(&messages[from..]).tokens;
    while split + 1 < messages.len() && tail(split) > max_tokens {
        // Next boundary that does not open on a tool result.
        let mut next = split + 1;
        while next < messages.len() && carries_tool_result(&messages[next]) {
            next += 1;
        }
        if next >= messages.len() {
            break;
        }
        split = next;
    }
    split
}

fn shorten(text: &str, head: usize, tail: usize) -> String {
    let n = text.chars().count();
    if n <= head + tail + 20 {
        return text.to_string();
    }
    let h: String = text.chars().take(head).collect();
    let t: String = text.chars().skip(n - tail).collect();
    format!("{h} … [{} characters omitted] … {t}", n - head - tail)
}

fn is_engine_nudge(m: &Message) -> bool {
    m.role == Role::User
        && m.get_text().is_some_and(|t| {
            t.starts_with("[system]") || t.starts_with("Continue from exactly where you stopped")
        })
}

/// Text of one message for the summary transcript, at a detail `level`
/// (0 = most detailed).
fn render_message(m: &Message, level: u8) -> String {
    let (result_head, result_tail, text_cap) = match level {
        0 => (1_200, 400, 20_000),
        1 => (300, 150, 4_000),
        _ => (120, 60, 1_200),
    };
    let role = match m.role {
        Role::User => "User",
        Role::Assistant => "Assistant",
        Role::System => "System",
    };
    let mut out = format!("### {role}\n");
    for b in m.content_blocks() {
        match b {
            ContentBlock::Text { text } => {
                // User words are kept whole at every level (they carry the
                // instructions); long assistant prose is shortened.
                if m.role == Role::User {
                    out.push_str(&shorten(&text, text_cap * 2, text_cap / 2));
                } else {
                    out.push_str(&shorten(&text, text_cap, text_cap / 4));
                }
                out.push('\n');
            }
            ContentBlock::ToolUse { name, input, .. } => {
                out.push_str(&format!(
                    "→ {name}({})\n",
                    shorten(&input.to_string(), 400, 100)
                ));
            }
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                let text = match content {
                    ToolResultContent::Text(t) => t,
                    ToolResultContent::Blocks(bs) => bs
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                let tag = if is_error == Some(true) {
                    "← error"
                } else {
                    "← result"
                };
                out.push_str(&format!(
                    "{tag}: {}\n",
                    shorten(&text, result_head, result_tail)
                ));
            }
            ContentBlock::Image { .. } => out.push_str("[image]\n"),
            ContentBlock::Document { title, .. } => out.push_str(&format!(
                "[document{}]\n",
                title.map(|t| format!(": {t}")).unwrap_or_default()
            )),
            ContentBlock::Audio { .. } => out.push_str("[audio]\n"),
            ContentBlock::Video { .. } => out.push_str("[video]\n"),
            // Reasoning and opaque protocol data are not part of the record.
            _ => {}
        }
    }
    out
}

/// The transcript sent to the summariser, shrunk until it fits `budget`.
pub fn render_for_summary(old: &[Message], budget: u64, est: &crate::context::Estimator) -> String {
    for level in 0..3u8 {
        let text: String = old
            .iter()
            .filter(|m| !is_engine_nudge(m))
            .map(|m| render_message(m, level))
            .collect::<Vec<_>>()
            .join("\n");
        if est.text(&text).upper <= budget {
            return text;
        }
    }
    // Still too large: keep the beginning (the objective) and the most
    // recent part, and say what was left out.
    let rendered: Vec<String> = old
        .iter()
        .filter(|m| !is_engine_nudge(m))
        .map(|m| render_message(m, 2))
        .collect();
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let mut used = 0u64;
    let (mut i, mut j) = (0usize, rendered.len());
    let mut take_head = true;
    while i < j {
        let candidate = if take_head {
            &rendered[i]
        } else {
            &rendered[j - 1]
        };
        let cost = est.text(candidate).upper;
        if used + cost > budget.saturating_sub(64) {
            break;
        }
        used += cost;
        if take_head {
            head.push(candidate.clone());
            i += 1;
        } else {
            tail.push(candidate.clone());
            j -= 1;
        }
        // One part from the start for every three from the end.
        take_head = head.len() * 3 < tail.len() + 1;
    }
    tail.reverse();
    let omitted = j - i;
    let mut out = head.join("\n");
    if omitted > 0 {
        out.push_str(&format!(
            "\n[… {omitted} messages omitted from this transcript to fit the summariser's budget …]\n"
        ));
    }
    out.push_str(&tail.join("\n"));
    out
}

/// The user's own messages, verbatim (long ones shortened), so instructions
/// and constraints survive even if the summary misses them.
fn user_messages_section(old: &[Message], budget: u64, est: &crate::context::Estimator) -> String {
    let texts: Vec<String> = old
        .iter()
        .filter(|m| m.role == Role::User && !is_engine_nudge(m))
        .filter_map(|m| {
            let t: String = m
                .content_blocks()
                .into_iter()
                .filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let t = t.trim().to_string();
            (!t.is_empty() && !t.starts_with(SUMMARY_OPEN)).then_some(t)
        })
        .collect();
    if texts.is_empty() {
        return String::new();
    }
    let mut items: Vec<String> = texts.iter().map(|t| shorten(t, 800, 200)).collect();
    // Over budget: keep the first message (the objective) and the latest
    // ones, and say how many were left out.
    let mut dropped = 0;
    while items.len() > 1 && est.text(&items.join("\n")).upper > budget {
        items.remove(1);
        dropped += 1;
    }
    let mut out =
        String::from("## User messages before this point (verbatim, long ones shortened)\n");
    for (k, it) in items.iter().enumerate() {
        if k == 1 && dropped > 0 {
            out.push_str(&format!(
                "- [… {dropped} more user messages are in the session's raw history]\n"
            ));
        }
        out.push_str(&format!("- {}\n", it.replace('\n', "\n  ")));
    }
    out
}

/// Summarise the older part of `messages`, keeping a recent tail verbatim.
///
/// The summary call is budgeted from the model's prompt budget, so it can run
/// before the context saturates. The result is checked: unless the new
/// history is smaller by `plan.min_gain`, nothing is replaced. The kept tail
/// is never altered — tool calls stay paired and reasoning blocks, signatures
/// and opaque protocol items are copied as they are.
pub async fn compact_history(
    provider: &dyn Provider,
    messages: &[Message],
    est: &crate::context::Estimator,
    plan: &CompactionPlan,
) -> CompactionRun {
    let split = choose_split(
        messages,
        plan.keep_recent_messages,
        plan.max_recent_tokens,
        est,
    );
    if split == 0 {
        return CompactionRun::without_call(CompactionOutcome::Skipped {
            reason: "no older messages to summarise".into(),
        });
    }
    let (old, recent) = messages.split_at(split);
    let tokens_before = est.messages(messages).tokens + plan.frame_tokens;

    // Budget of the summary call: the prompt budget minus its reserved
    // output, its own instructions and a margin.
    let overhead = est
        .text(&get_compact_prompt(plan.instructions.as_deref()))
        .upper
        + est.text(SUMMARY_SYSTEM_PROMPT).upper
        + 256;
    let call_budget = plan
        .input_limit
        .saturating_sub(plan.summary_max_tokens as u64)
        .saturating_sub(overhead)
        .saturating_mul(9)
        / 10;
    if call_budget < 512 {
        return CompactionRun::without_call(CompactionOutcome::Skipped {
            reason: format!(
                "the model's prompt budget ({}) leaves no room for a summary call",
                plan.input_limit
            ),
        });
    }
    let transcript = render_for_summary(old, call_budget, est);
    let request = cersei_provider::CompletionRequest {
        model: String::new(),
        messages: vec![Message::user(format!(
            "Transcript of the session so far:\n\n{transcript}\n\n{}",
            get_compact_prompt(plan.instructions.as_deref())
        ))],
        system: Some(SUMMARY_SYSTEM_PROMPT.into()),
        tools: Vec::new(),
        max_tokens: plan.summary_max_tokens,
        temperature: None,
        stop_sequences: Vec::new(),
        options: cersei_provider::ProviderOptions::default(),
        output_modalities: Vec::new(),
    };

    let response = match provider.complete(request).await {
        Ok(stream) => {
            let mut rx = stream.into_receiver();
            let mut acc = cersei_provider::StreamAccumulator::new();
            let mut stream_error = None;
            while let Some(event) = rx.recv().await {
                if let StreamEvent::Error { message } = &event {
                    stream_error = Some(message.clone());
                }
                acc.process_event(event);
            }
            match (stream_error, acc.into_response()) {
                (Some(e), _) => Err(e),
                (None, Ok(r)) => Ok(r),
                (None, Err(e)) => Err(e.to_string()),
            }
        }
        Err(e) => Err(e.to_string()),
    };
    let response = match response {
        Ok(r) => r,
        Err(reason) => {
            return CompactionRun::without_call(CompactionOutcome::Failed {
                reason: format!("summary call failed: {reason}"),
            })
        }
    };
    let usage = Some(response.usage.clone());
    let fail = |reason: String| CompactionRun {
        outcome: CompactionOutcome::Failed { reason },
        messages: None,
        usage: usage.clone(),
    };
    if response.stop_reason == StopReason::MaxTokens {
        return fail(format!(
            "the summary hit its {}-token output limit and would be incomplete",
            plan.summary_max_tokens
        ));
    }
    let summary_text = response.message.get_all_text();
    if summary_text.trim().is_empty() {
        return fail("the summary call returned no text".into());
    }

    // Bounded by the prompt budget and by a quarter of what is summarised, so
    // it can never cancel the gain of the compaction.
    let verbatim_budget =
        ((plan.input_limit as f64 * 0.10) as u64).min(est.messages(old).tokens / 4);
    let verbatim = user_messages_section(old, verbatim_budget, est);
    let summary = format!(
        "{SUMMARY_OPEN}\nThis session was compacted: the {} earlier messages are replaced by the summary \
         below. They are kept in the session's raw history and in the pre-compaction snapshot.\n\n\
         ## Summary\n{}\n\n{verbatim}</context_summary>",
        old.len(),
        summary_text.trim()
    );

    let mut new_messages = Vec::with_capacity(recent.len() + 1);
    match recent.first() {
        // A tail that opens with the user's own message: one user turn
        // carrying the summary first, so roles still alternate.
        Some(first) if first.role == Role::User => {
            let mut blocks = vec![ContentBlock::Text { text: summary }];
            blocks.extend(first.content_blocks());
            let mut merged = first.clone();
            merged.content = MessageContent::Blocks(blocks);
            new_messages.push(merged);
            new_messages.extend_from_slice(&recent[1..]);
        }
        _ => {
            new_messages.push(Message::user(summary));
            new_messages.extend_from_slice(recent);
        }
    }

    let tokens_after = est.messages(&new_messages).tokens + plan.frame_tokens;
    let enough = (tokens_after as f64) <= tokens_before as f64 * (1.0 - plan.min_gain);
    if !enough || tokens_after >= plan.input_limit {
        return CompactionRun {
            outcome: CompactionOutcome::InsufficientGain {
                tokens_before,
                tokens_after,
            },
            messages: None,
            usage,
        };
    }
    CompactionRun {
        outcome: CompactionOutcome::Compacted {
            messages_before: messages.len(),
            messages_after: new_messages.len(),
            tokens_before,
            tokens_after,
        },
        messages: Some(new_messages),
        usage,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_messages(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                if i % 2 == 0 {
                    Message::user(format!("User message {}", i))
                } else {
                    Message::assistant(format!("Assistant response {} with some longer text to simulate real content that takes up tokens in the context window.", i))
                }
            })
            .collect()
    }

    #[test]
    fn test_token_warning_ok() {
        assert_eq!(
            calculate_token_warning_state(50_000, 200_000),
            TokenWarningState::Ok
        );
    }

    #[test]
    fn test_token_warning_warning() {
        assert_eq!(
            calculate_token_warning_state(170_000, 200_000),
            TokenWarningState::Warning
        );
    }

    #[test]
    fn test_token_warning_critical() {
        assert_eq!(
            calculate_token_warning_state(196_000, 200_000),
            TokenWarningState::Critical
        );
    }

    #[test]
    fn test_should_compact() {
        assert!(!should_compact(100_000, 200_000)); // 50%
        assert!(!should_compact(170_000, 200_000)); // 85%
        assert!(should_compact(185_000, 200_000)); // 92.5%
        assert!(should_compact(195_000, 200_000)); // 97.5%
    }

    #[test]
    fn test_should_auto_compact_disabled() {
        let state = AutoCompactState {
            disabled: true,
            ..Default::default()
        };
        assert!(!should_auto_compact(195_000, 200_000, &state));
    }

    #[test]
    fn test_circuit_breaker() {
        let mut state = AutoCompactState::default();
        state.on_failure();
        state.on_failure();
        assert!(!state.disabled);
        state.on_failure(); // 3rd failure
        assert!(state.disabled);
    }

    #[test]
    fn test_snip_compact() {
        let messages = make_messages(20);
        let (kept, freed) = snip_compact(messages, 10);
        assert_eq!(kept.len(), 10);
        assert!(freed > 0);
    }

    #[test]
    fn test_snip_compact_already_small() {
        let messages = make_messages(5);
        let (kept, freed) = snip_compact(messages, 10);
        assert_eq!(kept.len(), 5);
        assert_eq!(freed, 0);
    }

    #[test]
    fn test_group_messages() {
        let messages = vec![
            Message::user("Read file A"),
            Message::assistant("Contents of A"),
            Message::user("Now edit B"),
            Message::assistant("Edited B"),
        ];

        let groups = group_messages_for_compact(&messages);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn test_estimate_tokens() {
        assert!((2..=4).contains(&estimate_tokens("hello world")));
        assert_eq!(estimate_tokens(""), 0);
        assert!(estimate_tokens(&"x".repeat(1000)) > 200);
    }

    /// §10.5 #3, the mirror rule: a tool_use with no answering tool_result
    /// anywhere in the request is the provider 400 from the other direction.
    #[test]
    fn unanswered_tool_uses_are_found_and_answered_ones_are_not() {
        let msgs = vec![
            Message::user("go"),
            Message::assistant_blocks(vec![
                ContentBlock::ToolUse {
                    id: "answered".into(),
                    name: "Read".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "ghost".into(),
                    name: "Grep".into(),
                    input: serde_json::json!({}),
                },
            ]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "answered".into(),
                content: ToolResultContent::Text("ok".into()),
                is_error: Some(false),
            }]),
        ];
        assert_eq!(find_unanswered_tool_uses(&msgs), vec!["ghost".to_string()]);

        // Fully-paired history is clean in both directions.
        let paired = vec![
            Message::assistant_blocks(vec![ContentBlock::ToolUse {
                id: "a".into(),
                name: "Read".into(),
                input: serde_json::json!({}),
            }]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "a".into(),
                content: ToolResultContent::Text("ok".into()),
                is_error: Some(false),
            }]),
        ];
        assert!(find_unanswered_tool_uses(&paired).is_empty());
        assert!(find_orphaned_tool_results(&paired).is_empty());
    }

    #[test]
    fn test_compact_prompt_with_instructions() {
        let prompt = get_compact_prompt(Some("Focus on API changes"));
        assert!(prompt.contains("Focus on API changes"));
        assert!(prompt.contains("Active instructions and constraints"));
    }

    #[test]
    fn test_format_compact_summary() {
        let summary = format_compact_summary("- Did X\n- Did Y");
        assert!(summary.contains("<context_summary>"));
        assert!(summary.contains("- Did X"));
    }

    #[test]
    fn test_calculate_messages_to_keep_index() {
        let messages = make_messages(20);
        let idx = calculate_messages_to_keep_index(&messages, 100);
        assert!(idx > 0);
        assert!(idx < 20);
    }

    #[test]
    fn test_messages_to_keep_all_fit() {
        let messages = make_messages(3);
        let idx = calculate_messages_to_keep_index(&messages, 100_000);
        assert_eq!(idx, 0); // keep all
    }

    // ── compact_history ──

    use crate::context::{ContextPolicy, Estimator};
    use cersei_provider::{CompletionRequest, CompletionStream};
    use std::sync::{Arc, Mutex};

    /// Answers every request with a fixed text (or an error) and records it.
    struct Scripted {
        reply: std::result::Result<String, String>,
        stop: StopReason,
        seen: Arc<Mutex<Vec<CompletionRequest>>>,
    }

    #[async_trait::async_trait]
    impl Provider for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }
        fn context_window(&self, _: &str) -> u64 {
            50_000
        }
        async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream> {
            self.seen.lock().unwrap().push(request);
            let text = self.reply.clone().map_err(CerseiError::Provider)?;
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            let stop = self.stop.clone();
            tokio::spawn(async move {
                let _ = tx
                    .send(StreamEvent::MessageStart {
                        id: "m".into(),
                        model: "x".into(),
                        usage: None,
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index: 0,
                        block_type: "text".into(),
                        id: None,
                        name: None,
                    })
                    .await;
                let _ = tx.send(StreamEvent::TextDelta { index: 0, text }).await;
                let _ = tx.send(StreamEvent::ContentBlockStop { index: 0 }).await;
                let usage = Usage {
                    input_tokens: 900,
                    output_tokens: 120,
                    ..Default::default()
                };
                let _ = tx
                    .send(StreamEvent::MessageDelta {
                        stop_reason: Some(stop),
                        usage: Some(usage),
                    })
                    .await;
                let _ = tx.send(StreamEvent::MessageStop).await;
            });
            Ok(CompletionStream::new(rx))
        }
    }

    fn scripted(
        reply: std::result::Result<&str, &str>,
    ) -> (Scripted, Arc<Mutex<Vec<CompletionRequest>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Scripted {
                reply: reply.map(str::to_string).map_err(str::to_string),
                stop: StopReason::EndTurn,
                seen: seen.clone(),
            },
            seen,
        )
    }

    fn est() -> Estimator {
        Estimator::new(&ContextPolicy::default(), true, 1.0)
    }

    fn plan() -> CompactionPlan {
        CompactionPlan {
            keep_recent_messages: 4,
            max_recent_tokens: 20_000,
            input_limit: 50_000,
            summary_max_tokens: 1_000,
            min_gain: 0.2,
            frame_tokens: 300,
            instructions: None,
        }
    }

    /// A long session: a constraint up front, then tool rounds with big results.
    fn session() -> Vec<Message> {
        let mut v = vec![Message::user(
            "Refactor the parser. Constraint: never touch src/legacy/, and keep the public API stable.",
        )];
        for i in 0..12 {
            let id = format!("t{i}");
            v.push(Message::assistant_blocks(vec![
                ContentBlock::Thinking {
                    thinking: format!("plan {i}"),
                    signature: format!("sig-{i}"),
                },
                ContentBlock::ToolUse {
                    id: id.clone(),
                    name: "Read".into(),
                    input: serde_json::json!({"file_path": format!("src/p{i}.rs")}),
                },
            ]));
            v.push(Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: id,
                content: ToolResultContent::Text(format!("fn f{i}() {{}}\n").repeat(300)),
                is_error: Some(false),
            }]));
        }
        v
    }

    #[tokio::test]
    async fn successful_compaction_keeps_constraints_pairs_and_signatures() {
        let (p, seen) = scripted(Ok(
            "- Objective: refactor the parser\n- Files: src/p0.rs … src/p11.rs",
        ));
        let msgs = session();
        let run = compact_history(&p, &msgs, &est(), &plan()).await;
        let CompactionOutcome::Compacted {
            tokens_before,
            tokens_after,
            ..
        } = run.outcome
        else {
            panic!("{:?}", run.outcome)
        };
        assert!(tokens_after < tokens_before / 2);
        let new = run.messages.unwrap();
        let summary = new[0].get_all_text();
        // The user's constraint survives verbatim, independently of the LLM.
        assert!(summary.contains("never touch src/legacy/"), "{summary}");
        assert!(summary.contains("refactor the parser"));
        // No orphan in either direction, and the kept tail is untouched.
        assert!(find_orphaned_tool_results(&new).is_empty());
        assert!(find_unanswered_tool_uses(&new).is_empty());
        let tail_sig = new.iter().flat_map(|m| m.content_blocks()).any(
            |b| matches!(b, ContentBlock::Thinking { signature, .. } if signature == "sig-11"),
        );
        assert!(
            tail_sig,
            "the kept assistant turn keeps its signed reasoning"
        );
        // The summary call itself was budgeted and carried no tools.
        let req = &seen.lock().unwrap()[0];
        assert!(req.tools.is_empty());
        assert_eq!(req.max_tokens, 1_000);
        assert!(est().messages(&req.messages).upper < 50_000);
        assert!(run.usage.is_some());
    }

    #[tokio::test]
    async fn a_failed_summary_call_keeps_the_history() {
        let (p, _) = scripted(Err("503 overloaded"));
        let run = compact_history(&p, &session(), &est(), &plan()).await;
        assert!(
            matches!(run.outcome, CompactionOutcome::Failed { ref reason } if reason.contains("503"))
        );
        assert!(run.messages.is_none());
    }

    #[tokio::test]
    async fn a_truncated_or_empty_summary_is_a_failure() {
        let (mut p, _) = scripted(Ok("partial"));
        p.stop = StopReason::MaxTokens;
        let run = compact_history(&p, &session(), &est(), &plan()).await;
        assert!(matches!(run.outcome, CompactionOutcome::Failed { .. }));
        assert!(
            run.usage.is_some(),
            "the call happened and must be accounted"
        );
        let (p, _) = scripted(Ok("   "));
        assert!(matches!(
            compact_history(&p, &session(), &est(), &plan())
                .await
                .outcome,
            CompactionOutcome::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn insufficient_gain_keeps_the_history() {
        // A tiny old part and a huge summary: no real gain.
        let mut msgs = vec![Message::user("short task"), Message::assistant("ok")];
        msgs.extend((0..4).map(|i| {
            if i % 2 == 0 {
                Message::user("go on")
            } else {
                Message::assistant("done")
            }
        }));
        let (p, _) = scripted(Ok(&"verbose summary ".repeat(400)));
        let run = compact_history(
            &p,
            &msgs,
            &est(),
            &CompactionPlan {
                keep_recent_messages: 2,
                ..plan()
            },
        )
        .await;
        assert!(
            matches!(run.outcome, CompactionOutcome::InsufficientGain { .. }),
            "{:?}",
            run.outcome
        );
        assert!(run.messages.is_none());
    }

    #[tokio::test]
    async fn nothing_old_enough_is_skipped_without_a_call() {
        let (p, seen) = scripted(Ok("x"));
        let msgs = vec![Message::user("a"), Message::assistant("b")];
        let run = compact_history(&p, &msgs, &est(), &plan()).await;
        assert!(matches!(run.outcome, CompactionOutcome::Skipped { .. }));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn the_summary_transcript_fits_its_budget() {
        let mut msgs = session();
        for _ in 0..5 {
            msgs.extend(session());
        }
        let text = render_for_summary(&msgs, 3_000, &est());
        assert!(
            est().text(&text).upper <= 3_000,
            "{}",
            est().text(&text).upper
        );
        assert!(
            text.contains("never touch src/legacy/"),
            "the objective is kept first"
        );
        assert!(text.contains("messages omitted"));
    }

    #[test]
    fn a_huge_recent_tail_is_shortened_but_never_split_from_its_call() {
        let msgs = session();
        let split = choose_split(&msgs, 10, 2_000, &est());
        assert!(split > msgs.len() - 10);
        assert!(!carries_tool_result(&msgs[split]));
    }
}
