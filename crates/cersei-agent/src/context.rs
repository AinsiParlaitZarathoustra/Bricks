//! Context manager: how full the active context is, and whether the next
//! request fits.
//!
//! Three numbers are kept apart:
//!
//! * **context used** — the occupation of the active context (what the next
//!   request would carry), with its provenance: *measured* from the `usage`
//!   the server reported for the last request, *counted* by a configured
//!   counting endpoint, *estimated* locally, or *mixed* (a measured base plus
//!   an estimate of what was appended since);
//! * **context window** — the limits of the selected model, from its
//!   configuration (nothing is inferred when a value is missing);
//! * **total tokens** — what the session consumed, cumulated over every
//!   request including compaction calls; it never decreases.
//!
//! A `usage` measures the request that was executed, not the next one. It is
//! recorded with the model identity and the version of the context it was
//! measured on. Additions kept afterwards (the response as it will be sent
//! back, tool results, new messages) are estimated on top of it. Rewriting the
//! history (compaction, removal of tool results, restoring a session) or
//! switching model invalidates the measurement, and the context is estimated
//! again until the next one.

use cersei_provider::{CompletionRequest, ModelInfo, ModelLimits, Provider};
use cersei_types::tokens::{self, TokenEstimate};
use cersei_types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

// ─── Policy ──────────────────────────────────────────────────────────────────

/// Thresholds of the context manager. The defaults are starting points for
/// models with 100k–200k-token windows, not universal values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ContextPolicy {
    /// Minimum margin kept free below the input limit, in tokens.
    pub safety_margin_tokens: u64,
    /// Margin as a fraction of the input limit (the larger margin wins).
    pub safety_margin_ratio: f64,
    /// Content added since the last measurement above this size triggers a
    /// precise pre-flight check.
    pub large_injection_tokens: u64,
    /// Occupation (upper bound) above this fraction of the input limit
    /// triggers a precise pre-flight check.
    pub preflight_ratio: f64,
    /// Occupation above this fraction of the input limit triggers a
    /// compaction after the turn.
    pub compact_threshold: f64,
    /// Messages always kept verbatim at the end of the history.
    pub keep_recent_messages: usize,
    /// The kept tail may use at most this fraction of the input limit; a
    /// larger tail is shortened (never below the last message group).
    pub max_recent_ratio: f64,
    /// Output reserved for the summary of a compaction.
    pub summary_max_tokens: u32,
    /// A compaction must shrink the context by at least this fraction.
    pub min_compaction_gain: f64,
    /// Failed or useless compactions in a row before automatic compaction
    /// stops for the session.
    pub max_compaction_failures: u32,
    /// Compactions attempted per turn after the server refused a request for
    /// exceeding the context.
    pub max_overflow_recoveries: u32,
    /// Estimated cost of one image (the real cost depends on its size).
    pub image_tokens: u64,
    /// Estimated cost of one document (PDF pages are not countable locally).
    pub document_tokens: u64,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            safety_margin_tokens: 1024,
            safety_margin_ratio: 0.02,
            large_injection_tokens: 8_000,
            preflight_ratio: 0.80,
            compact_threshold: 0.85,
            keep_recent_messages: 10,
            max_recent_ratio: 0.30,
            summary_max_tokens: 4096,
            min_compaction_gain: 0.20,
            max_compaction_failures: 3,
            max_overflow_recoveries: 1,
            image_tokens: 1_600,
            document_tokens: 3_000,
        }
    }
}

impl ContextPolicy {
    /// Check the values are usable.
    pub fn validate(&self) -> std::result::Result<(), String> {
        for (name, v) in [
            ("safety_margin_ratio", self.safety_margin_ratio),
            ("preflight_ratio", self.preflight_ratio),
            ("compact_threshold", self.compact_threshold),
            ("max_recent_ratio", self.max_recent_ratio),
            ("min_compaction_gain", self.min_compaction_gain),
        ] {
            if !(0.0..=1.0).contains(&v) || v.is_nan() {
                return Err(format!("[context] `{name}` must be between 0 and 1"));
            }
        }
        if self.compact_threshold == 0.0 {
            return Err("[context] `compact_threshold` must be greater than 0".into());
        }
        if self.summary_max_tokens == 0 {
            return Err("[context] `summary_max_tokens` must be greater than 0".into());
        }
        Ok(())
    }

    fn margin(&self, input_limit: u64) -> u64 {
        self.safety_margin_tokens
            .max((input_limit as f64 * self.safety_margin_ratio).ceil() as u64)
    }
}

// ─── Reported values ─────────────────────────────────────────────────────────

/// Where an occupation figure comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// The server's `usage` for the request that carried exactly this context.
    Measured,
    /// A configured counting endpoint, for exactly this context.
    Counted,
    /// A measured (or counted) base plus a local estimate of what was added.
    Mixed,
    /// A local estimate only.
    Estimated,
}

/// Occupation of the active context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextUsed {
    pub tokens: u64,
    /// Conservative bound used for budget decisions.
    pub upper_bound: u64,
    /// Part of `tokens` that was measured or counted.
    pub measured_tokens: u64,
    /// Part of `tokens` that was estimated.
    pub estimated_tokens: u64,
    pub provenance: Provenance,
    /// How the estimated part was computed.
    pub method: String,
}

/// The selected model's limits, as configured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextWindow {
    pub model: String,
    /// Total window shared by input and output; `None` when the
    /// configuration does not state one.
    pub total: Option<u64>,
    pub max_input_tokens: u64,
    /// `None` when the provider does not report it.
    pub max_output_tokens: Option<u64>,
}

/// Consumption of the session. Categories are disjoint: `input_tokens`
/// excludes cache reads and writes, and `reasoning_tokens` is a part of
/// `output_tokens` (never added to it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTotals {
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Requests whose usage was recorded, compaction calls included.
    pub requests: u64,
    /// Requests for which the server reported no usage (not counted above).
    pub requests_without_usage: u64,
    /// Of `requests`, how many were compaction summaries.
    pub compaction_requests: u64,
}

impl SessionTotals {
    /// Every token billed: prompt (uncached + cache read + cache write) plus
    /// output.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens + self.output_tokens
    }

    fn add(&mut self, u: &Usage) {
        self.input_tokens += u.input_tokens;
        self.cache_read_tokens += u.cache_read_input_tokens;
        self.cache_write_tokens += u.cache_creation_input_tokens;
        self.output_tokens += u.output_tokens;
        self.reasoning_tokens += u.reasoning_tokens;
    }
}

/// Everything a status line needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextStatus {
    pub context_used: ContextUsed,
    pub context_window: ContextWindow,
    /// Prompt budget once the output is reserved.
    pub input_limit: u64,
    pub reserved_output: u64,
    pub margin: u64,
    pub total_tokens: u64,
    pub totals: SessionTotals,
    /// Missing configuration, invalidations and similar remarks.
    pub notes: Vec<String>,
}

impl ContextStatus {
    /// Occupation as a fraction of the prompt budget.
    pub fn fraction_used(&self) -> f64 {
        if self.input_limit == 0 {
            return 0.0;
        }
        self.context_used.tokens as f64 / self.input_limit as f64
    }
}

/// What to do before sending.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetDecision {
    Fits,
    /// Probably fits, but a precise check is warranted.
    Preflight(String),
    /// Manifestly over the limit: do not send.
    Exceeded,
}

// ─── Estimation ──────────────────────────────────────────────────────────────

/// Local estimates of messages, aware of what each protocol sends back.
#[derive(Debug, Clone)]
pub struct Estimator {
    /// Reasoning kept in the history is re-sent (and occupies the context).
    pub reasoning_resent: bool,
    /// Measured/estimated ratio learnt for this model (1.0 until measured).
    pub calibration: f64,
    pub image_tokens: u64,
    pub document_tokens: u64,
}

/// Framing tokens per message (role markers, separators).
const PER_MESSAGE: u64 = 4;
/// Framing tokens per request.
const PER_REQUEST: u64 = 8;

impl Estimator {
    pub fn new(policy: &ContextPolicy, reasoning_resent: bool, calibration: f64) -> Self {
        Self {
            reasoning_resent,
            calibration,
            image_tokens: policy.image_tokens,
            document_tokens: policy.document_tokens,
        }
    }

    pub fn text(&self, s: &str) -> TokenEstimate {
        tokens::estimate_text(s).scaled(self.calibration)
    }

    pub fn block(&self, b: &ContentBlock) -> TokenEstimate {
        match b {
            ContentBlock::Text { text } => self.text(text),
            ContentBlock::ToolUse { name, input, .. } => {
                self.text(name) + self.text(&input.to_string()) + tokens::fixed_cost(PER_MESSAGE)
            }
            ContentBlock::ToolResult { content, .. } => {
                let body = match content {
                    ToolResultContent::Text(t) => self.text(t),
                    ToolResultContent::Blocks(bs) => bs.iter().map(|b| self.block(b)).sum(),
                };
                body + tokens::fixed_cost(PER_MESSAGE)
            }
            // Reasoning occupies the context only where the protocol sends it
            // back; elsewhere it was output, not context.
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                if self.reasoning_resent {
                    self.text(thinking) + self.text(signature).scaled(0.25)
                } else {
                    TokenEstimate::ZERO
                }
            }
            ContentBlock::RedactedThinking { data } => {
                if self.reasoning_resent {
                    self.text(data).scaled(0.25)
                } else {
                    TokenEstimate::ZERO
                }
            }
            // Opaque protocol items (encrypted reasoning) are sent back as is;
            // their cost is not derivable from their encoded size.
            ContentBlock::ProtocolItem { item, .. } => {
                let e = self.text(&item.to_string()).scaled(0.5);
                TokenEstimate {
                    tokens: e.tokens,
                    upper: e.upper * 3,
                }
            }
            ContentBlock::Image { .. } => tokens::fixed_cost(self.image_tokens),
            ContentBlock::Document { .. }
            | ContentBlock::Audio { .. }
            | ContentBlock::Video { .. } => tokens::fixed_cost(self.document_tokens),
            ContentBlock::Opaque => TokenEstimate::ZERO,
        }
    }

    pub fn message(&self, m: &Message) -> TokenEstimate {
        let body = match &m.content {
            MessageContent::Text(t) => self.text(t),
            MessageContent::Blocks(bs) => bs.iter().map(|b| self.block(b)).sum(),
        };
        body + tokens::fixed_cost(PER_MESSAGE)
    }

    pub fn messages(&self, ms: &[Message]) -> TokenEstimate {
        ms.iter().map(|m| self.message(m)).sum()
    }

    pub fn tools(&self, tools: &[ToolDefinition]) -> TokenEstimate {
        tools
            .iter()
            .map(|t| {
                self.text(&t.name)
                    + self.text(&t.description)
                    + self.text(&t.input_schema.to_string())
            })
            .sum()
    }

    /// The complete request: instructions, tools and history.
    pub fn request(
        &self,
        system: Option<&str>,
        tools: &[ToolDefinition],
        messages: &[Message],
    ) -> TokenEstimate {
        system.map(|s| self.text(s)).unwrap_or_default()
            + self.tools(tools)
            + self.messages(messages)
            + tokens::fixed_cost(PER_REQUEST)
    }
}

// ─── Manager ─────────────────────────────────────────────────────────────────

/// A measurement of one executed (or counted) request.
#[derive(Debug, Clone)]
struct Measurement {
    model: String,
    version: u64,
    frame: u64,
    message_count: usize,
    prompt_tokens: u64,
    provenance: Provenance,
    /// Index and retained size of the assistant response appended after the
    /// measured request, when the response's own usage measured it.
    retained: Option<(usize, u64)>,
}

/// The parts of a request the manager looks at.
pub struct RequestView<'a> {
    pub system: Option<&'a str>,
    pub tools: &'a [ToolDefinition],
    pub messages: &'a [Message],
    /// `max_tokens` the request asks for.
    pub max_output: u32,
}

impl<'a> RequestView<'a> {
    pub fn of(r: &'a CompletionRequest) -> Self {
        Self {
            system: r.system.as_deref(),
            tools: &r.tools,
            messages: &r.messages,
            max_output: r.max_tokens,
        }
    }

    /// Identity of the instructions and tool definitions: when they change,
    /// a measurement of the previous ones no longer applies.
    fn frame(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.system.hash(&mut h);
        for t in self.tools {
            t.name.hash(&mut h);
            t.description.hash(&mut h);
            t.input_schema.to_string().hash(&mut h);
        }
        h.finish()
    }
}

/// The model as the manager sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelView {
    pub selection: String,
    pub limits: ModelLimits,
    pub reasoning_resent: bool,
    pub token_counting: bool,
    /// The provider gave no model information: only `context_window()`.
    pub limits_reported: bool,
}

impl ModelView {
    pub fn of(provider: &dyn Provider, label: &str) -> Self {
        match provider.model_info() {
            Some(ModelInfo {
                selection,
                limits,
                reasoning_resent,
                token_counting,
                ..
            }) => Self {
                selection,
                limits,
                reasoning_resent,
                token_counting,
                limits_reported: true,
            },
            None => Self {
                selection: format!("{}:{label}", provider.name()),
                limits: ModelLimits {
                    max_input_tokens: provider.context_window(label),
                    max_output_tokens: 0,
                    context_window_tokens: None,
                },
                reasoning_resent: true,
                token_counting: false,
                limits_reported: false,
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContextManager {
    policy: ContextPolicy,
    version: u64,
    measurement: Option<Measurement>,
    calibration: HashMap<String, f64>,
    totals: SessionTotals,
    last_invalidation: Option<String>,
}

impl ContextManager {
    pub fn new(policy: ContextPolicy) -> Self {
        Self {
            policy,
            ..Default::default()
        }
    }

    pub fn policy(&self) -> &ContextPolicy {
        &self.policy
    }

    pub fn totals(&self) -> &SessionTotals {
        &self.totals
    }

    /// Version of the active context; bumped by every rewrite.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn estimator(&self, model: &ModelView) -> Estimator {
        let cal = self
            .calibration
            .get(&model.selection)
            .copied()
            .unwrap_or(1.0);
        Estimator::new(&self.policy, model.reasoning_resent, cal)
    }

    /// The history was rewritten (compaction, removal, restore): the last
    /// measurement no longer describes it.
    pub fn invalidate(&mut self, reason: impl Into<String>) {
        self.version += 1;
        self.measurement = None;
        self.last_invalidation = Some(reason.into());
    }

    /// Record the `usage` of a request built from `request` and executed
    /// with `model`. `assistant` is the response as appended to the history,
    /// at `assistant_index`. Returns whether the usage carried a measurement.
    pub fn record_response(
        &mut self,
        model: &ModelView,
        request: &RequestView,
        usage: &Usage,
        assistant: Option<(usize, &Message)>,
    ) -> bool {
        let prompt =
            usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
        if prompt == 0 && usage.output_tokens == 0 {
            self.totals.requests_without_usage += 1;
            return false;
        }
        self.totals.requests += 1;
        self.totals.add(usage);
        if prompt == 0 {
            return false;
        }

        // Calibrate the local estimator for this model on the same request.
        let raw = Estimator::new(&self.policy, model.reasoning_resent, 1.0).request(
            request.system,
            request.tools,
            request.messages,
        );
        if raw.tokens > 200 {
            let ratio = (prompt as f64 / raw.tokens as f64).clamp(0.5, 2.0);
            self.calibration.insert(model.selection.clone(), ratio);
        }

        // What the response will occupy once sent back: its output, minus
        // the reasoning the protocol does not re-send.
        let retained = assistant.and_then(|(idx, msg)| {
            if usage.output_tokens == 0 {
                return None;
            }
            let dropped = if model.reasoning_resent {
                0
            } else if usage.reasoning_tokens > 0 {
                usage.reasoning_tokens
            } else {
                // Reasoning not reported separately: estimate the thinking text.
                let est = Estimator::new(&self.policy, true, 1.0);
                msg.content_blocks()
                    .iter()
                    .filter(|b| {
                        matches!(
                            b,
                            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
                        )
                    })
                    .map(|b| est.block(b).tokens)
                    .sum()
            };
            Some((
                idx,
                usage.output_tokens.saturating_sub(dropped) + PER_MESSAGE,
            ))
        });

        self.measurement = Some(Measurement {
            model: model.selection.clone(),
            version: self.version,
            frame: request.frame(),
            message_count: request.messages.len(),
            prompt_tokens: prompt,
            provenance: Provenance::Measured,
            retained,
        });
        true
    }

    /// Record a count from a counting endpoint for exactly `request`.
    pub fn record_count(&mut self, model: &ModelView, request: &RequestView, tokens: u64) {
        self.measurement = Some(Measurement {
            model: model.selection.clone(),
            version: self.version,
            frame: request.frame(),
            message_count: request.messages.len(),
            prompt_tokens: tokens,
            provenance: Provenance::Counted,
            retained: None,
        });
    }

    /// Record the usage of a compaction summary call: it counts in the session
    /// totals exactly once and does not describe the active context.
    pub fn record_compaction_usage(&mut self, usage: &Usage) {
        let prompt =
            usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
        if prompt == 0 && usage.output_tokens == 0 {
            self.totals.requests_without_usage += 1;
            return;
        }
        self.totals.requests += 1;
        self.totals.compaction_requests += 1;
        self.totals.add(usage);
    }

    /// The occupation of the context a request built from `view` would have.
    pub fn context_used(&self, model: &ModelView, view: &RequestView) -> ContextUsed {
        let est = self.estimator(model);
        let method = if self.calibration.contains_key(&model.selection) {
            format!(
                "{} × {:.2} (calibrated on this model's usage)",
                tokens::ESTIMATION_METHOD,
                est.calibration
            )
        } else {
            tokens::ESTIMATION_METHOD.to_string()
        };
        if let Some(m) = &self.measurement {
            let applies = m.model == model.selection
                && m.version == self.version
                && m.frame == view.frame()
                && view.messages.len() >= m.message_count;
            if applies {
                let mut delta = TokenEstimate::ZERO;
                for (i, msg) in view.messages.iter().enumerate().skip(m.message_count) {
                    match m.retained {
                        Some((idx, retained)) if idx == i => {
                            delta += TokenEstimate {
                                tokens: retained,
                                upper: retained,
                            };
                        }
                        _ => delta += est.message(msg),
                    }
                }
                let provenance = if delta.tokens == 0 {
                    m.provenance
                } else {
                    Provenance::Mixed
                };
                return ContextUsed {
                    tokens: m.prompt_tokens + delta.tokens,
                    upper_bound: m.prompt_tokens + delta.upper,
                    measured_tokens: m.prompt_tokens,
                    estimated_tokens: delta.tokens,
                    provenance,
                    method,
                };
            }
        }
        let e = est.request(view.system, view.tools, view.messages);
        ContextUsed {
            tokens: e.tokens,
            upper_bound: e.upper,
            measured_tokens: 0,
            estimated_tokens: e.tokens,
            provenance: Provenance::Estimated,
            method,
        }
    }

    /// Full status for `view` with `model`.
    pub fn status(&self, model: &ModelView, view: &RequestView) -> ContextStatus {
        let used = self.context_used(model, view);
        let limits = &model.limits;
        let reserved_output = if limits.max_output_tokens > 0 {
            (view.max_output as u64).min(limits.max_output_tokens)
        } else {
            view.max_output as u64
        };
        let input_limit = limits.input_budget(reserved_output);
        let mut notes = Vec::new();
        if !model.limits_reported {
            notes.push(
                "the provider reports no model limits: only its prompt budget is known".to_string(),
            );
        } else if limits.context_window_tokens.is_none() {
            notes.push(
                "no total context window configured (limits.context_window_tokens): input and \
                 output limits are applied separately"
                    .to_string(),
            );
        }
        if used.provenance == Provenance::Estimated {
            if let Some(why) = &self.last_invalidation {
                notes.push(format!("estimated since: {why}"));
            }
        }
        ContextStatus {
            context_window: ContextWindow {
                model: model.selection.clone(),
                total: limits.context_window_tokens,
                max_input_tokens: limits.max_input_tokens,
                max_output_tokens: model.limits_reported.then_some(limits.max_output_tokens),
            },
            margin: self.policy.margin(input_limit),
            input_limit,
            reserved_output,
            total_tokens: self.totals.total_tokens(),
            totals: self.totals.clone(),
            context_used: used,
            notes,
        }
    }

    /// The light check made before every request.
    pub fn decide(&self, status: &ContextStatus) -> BudgetDecision {
        let used = &status.context_used;
        let limit = status.input_limit.saturating_sub(status.margin);
        if used.tokens > limit {
            return BudgetDecision::Exceeded;
        }
        if used.upper_bound > limit {
            return BudgetDecision::Preflight(
                "the estimate's upper bound exceeds the budget".into(),
            );
        }
        if used.estimated_tokens >= self.policy.large_injection_tokens && used.measured_tokens > 0 {
            return BudgetDecision::Preflight(format!(
                "{} tokens were added since the last measurement",
                used.estimated_tokens
            ));
        }
        if used.upper_bound as f64 >= self.policy.preflight_ratio * status.input_limit as f64 {
            return BudgetDecision::Preflight("the context is close to its limit".into());
        }
        if used.provenance == Provenance::Estimated
            && self.last_invalidation.is_some()
            && used.upper_bound as f64 >= 0.5 * status.input_limit as f64
        {
            return BudgetDecision::Preflight(
                "the history was rewritten since the last measurement".into(),
            );
        }
        BudgetDecision::Fits
    }

    /// After the turn: should the history be compacted now?
    pub fn should_compact(&self, status: &ContextStatus) -> bool {
        status.input_limit > 0
            && status.context_used.tokens as f64
                >= self.policy.compact_threshold * status.input_limit as f64
    }
}

// ─── `bricks.toml` ───────────────────────────────────────────────────────────

/// Read the `[context]` table of a `bricks.toml` text.
pub fn policy_from_bricks_toml(text: &str) -> std::result::Result<ContextPolicy, String> {
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        context: Option<toml::Value>,
    }
    let doc: Doc = toml::from_str(text).map_err(|e| format!("invalid TOML: {e}"))?;
    let policy: ContextPolicy = match doc.context {
        None => ContextPolicy::default(),
        Some(v) => serde_path_to_error::deserialize(v).map_err(|e| {
            let path = e.path().to_string();
            format!("[context] field `{path}`: {}", e.into_inner())
        })?,
    };
    policy.validate()?;
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(input: u64, output: u64, window: Option<u64>, resent: bool) -> ModelView {
        ModelView {
            selection: "p/m".into(),
            limits: ModelLimits {
                max_input_tokens: input,
                max_output_tokens: output,
                context_window_tokens: window,
            },
            reasoning_resent: resent,
            token_counting: false,
            limits_reported: true,
        }
    }

    fn view<'a>(messages: &'a [Message], tools: &'a [ToolDefinition]) -> RequestView<'a> {
        RequestView {
            system: Some("You are helpful."),
            tools,
            messages,
            max_output: 1000,
        }
    }

    fn usage(input: u64, read: u64, write: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            cache_read_input_tokens: read,
            cache_creation_input_tokens: write,
            output_tokens: output,
            ..Default::default()
        }
    }

    #[test]
    fn without_usage_the_context_is_estimated_never_zero() {
        let mgr = ContextManager::default();
        let msgs = vec![Message::user("hello there, please read the file")];
        let used = mgr.context_used(&model(1000, 100, None, true), &view(&msgs, &[]));
        assert_eq!(used.provenance, Provenance::Estimated);
        assert!(used.tokens > 0 && used.upper_bound >= used.tokens);
        assert_eq!(used.measured_tokens, 0);
    }

    #[test]
    fn a_response_without_usage_keeps_the_estimate_and_is_counted_apart() {
        let mut mgr = ContextManager::default();
        let m = model(1000, 100, None, true);
        let msgs = vec![Message::user("hi")];
        assert!(!mgr.record_response(&m, &view(&msgs, &[]), &Usage::default(), None));
        assert_eq!(mgr.totals().requests_without_usage, 1);
        assert_eq!(mgr.totals().total_tokens(), 0);
        assert_eq!(
            mgr.context_used(&m, &view(&msgs, &[])).provenance,
            Provenance::Estimated
        );
    }

    #[test]
    fn cache_reads_and_writes_count_in_the_occupation_not_the_window() {
        let mut mgr = ContextManager::default();
        let m = model(200_000, 8_000, Some(200_000), true);
        let msgs = vec![Message::user("long cached prompt")];
        mgr.record_response(&m, &view(&msgs, &[]), &usage(100, 50_000, 2_000, 0), None);
        let st = mgr.status(&m, &view(&msgs, &[]));
        assert_eq!(st.context_used.tokens, 52_100);
        assert_eq!(st.context_used.provenance, Provenance::Measured);
        // The window is the configured one: the cache does not enlarge it.
        assert_eq!(st.context_window.total, Some(200_000));
        assert_eq!(st.input_limit, 199_000);
    }

    #[test]
    fn the_kept_response_is_counted_from_its_output_without_unsent_reasoning() {
        let tools: Vec<ToolDefinition> = Vec::new();
        for (resent, expected_delta) in [(true, 504), (false, 104)] {
            let mut mgr = ContextManager::default();
            let m = model(100_000, 8_000, None, resent);
            let mut msgs = vec![Message::user("question")];
            let req_len = msgs.len();
            let reply = Message::assistant_blocks(vec![
                ContentBlock::Thinking {
                    thinking: "t".repeat(1600),
                    signature: String::new(),
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ]);
            let mut u = usage(1_000, 0, 0, 500);
            u.reasoning_tokens = 400;
            mgr.record_response(
                &m,
                &view(&msgs[..req_len], &tools),
                &u,
                Some((req_len, &reply)),
            );
            msgs.push(reply);
            let used = mgr.context_used(&m, &view(&msgs, &tools));
            assert_eq!(used.provenance, Provenance::Mixed);
            assert_eq!(used.measured_tokens, 1_000);
            assert_eq!(
                used.estimated_tokens, expected_delta,
                "reasoning resent: {resent}"
            );
        }
    }

    #[test]
    fn appended_tool_results_are_estimated_on_top_of_the_measurement() {
        let mut mgr = ContextManager::default();
        let m = model(100_000, 8_000, None, true);
        let mut msgs = vec![Message::user("go")];
        mgr.record_response(&m, &view(&msgs, &[]), &usage(5_000, 0, 0, 0), None);
        msgs.push(Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "a".into(),
            content: ToolResultContent::Text("x ".repeat(40_000)),
            is_error: Some(false),
        }]));
        let used = mgr.context_used(&m, &view(&msgs, &[]));
        assert_eq!(used.measured_tokens, 5_000);
        assert!(used.estimated_tokens > 4_000);
        // A large injection asks for a precise pre-flight.
        let st = mgr.status(&m, &view(&msgs, &[]));
        assert!(
            matches!(mgr.decide(&st), BudgetDecision::Preflight(_)),
            "{:?}",
            mgr.decide(&st)
        );
    }

    #[test]
    fn invalidation_and_model_change_drop_the_measurement() {
        let mut mgr = ContextManager::default();
        let a = model(100_000, 8_000, None, true);
        let msgs = vec![Message::user("go")];
        mgr.record_response(&a, &view(&msgs, &[]), &usage(5_000, 0, 0, 10), None);
        assert_eq!(
            mgr.context_used(&a, &view(&msgs, &[])).provenance,
            Provenance::Measured
        );

        let mut b = a.clone();
        b.selection = "other/model".into();
        assert_eq!(
            mgr.context_used(&b, &view(&msgs, &[])).provenance,
            Provenance::Estimated
        );

        mgr.invalidate("compaction");
        let st = mgr.status(&a, &view(&msgs, &[]));
        assert_eq!(st.context_used.provenance, Provenance::Estimated);
        assert!(st.notes.iter().any(|n| n.contains("compaction")));
        // Totals are untouched by an invalidation.
        assert_eq!(mgr.totals().total_tokens(), 5_010);
    }

    #[test]
    fn changed_tools_or_instructions_invalidate_the_measurement() {
        let mut mgr = ContextManager::default();
        let m = model(100_000, 8_000, None, true);
        let msgs = vec![Message::user("go")];
        mgr.record_response(&m, &view(&msgs, &[]), &usage(5_000, 0, 0, 10), None);
        let tools = vec![ToolDefinition {
            name: "Read".into(),
            description: "read".into(),
            input_schema: json!({}),
        }];
        assert_eq!(
            mgr.context_used(&m, &view(&msgs, &tools)).provenance,
            Provenance::Estimated
        );
    }

    #[test]
    fn totals_include_compaction_once_and_never_decrease() {
        let mut mgr = ContextManager::default();
        let m = model(100_000, 8_000, None, true);
        let msgs = vec![Message::user("go")];
        mgr.record_response(&m, &view(&msgs, &[]), &usage(1_000, 200, 100, 50), None);
        mgr.record_compaction_usage(&usage(3_000, 0, 0, 400));
        let before = mgr.totals().total_tokens();
        assert_eq!(before, 1_350 + 3_400);
        assert_eq!(mgr.totals().compaction_requests, 1);
        mgr.invalidate("compaction");
        assert_eq!(mgr.totals().total_tokens(), before);
    }

    #[test]
    fn output_reserve_and_margin_shape_the_budget() {
        let mgr = ContextManager::default();
        // Shared 32k window: reserving 8k of output leaves 24k for the prompt.
        let m = model(30_000, 8_000, Some(32_000), true);
        let msgs = vec![Message::user("hi")];
        let mut v = view(&msgs, &[]);
        v.max_output = 8_000;
        let st = mgr.status(&m, &v);
        assert_eq!(st.input_limit, 24_000);
        assert_eq!(st.reserved_output, 8_000);
        assert_eq!(st.margin, 1_024);
        assert_eq!(mgr.decide(&st), BudgetDecision::Fits);
        // Asking for more output than the model allows reserves only its max.
        v.max_output = 50_000;
        assert_eq!(mgr.status(&m, &v).reserved_output, 8_000);
    }

    #[test]
    fn many_small_messages_end_up_exceeding_the_budget() {
        let mgr = ContextManager::default();
        let m = model(4_000, 500, None, true);
        let msgs: Vec<Message> = (0..400)
            .map(|i| Message::user(format!("note {i}: ok")))
            .collect();
        let st = mgr.status(&m, &view(&msgs, &[]));
        assert_eq!(
            mgr.decide(&st),
            BudgetDecision::Exceeded,
            "{:?}",
            st.context_used
        );
    }

    #[test]
    fn a_missing_window_is_reported_not_invented() {
        let mgr = ContextManager::default();
        let msgs = vec![Message::user("hi")];
        let st = mgr.status(&model(10_000, 1_000, None, true), &view(&msgs, &[]));
        assert_eq!(st.context_window.total, None);
        assert!(st
            .notes
            .iter()
            .any(|n| n.contains("no total context window")));
    }

    #[test]
    fn calibration_scales_later_estimates() {
        let mut mgr = ContextManager::default();
        let m = model(100_000, 8_000, None, true);
        let msgs = vec![Message::user("word ".repeat(2_000))];
        let raw = mgr.context_used(&m, &view(&msgs, &[])).tokens;
        mgr.record_response(&m, &view(&msgs, &[]), &usage(raw * 3 / 2, 0, 0, 1), None);
        mgr.invalidate("test");
        let calibrated = mgr.context_used(&m, &view(&msgs, &[]));
        assert!(
            calibrated.tokens > raw * 13 / 10,
            "{} vs {raw}",
            calibrated.tokens
        );
        assert!(calibrated.method.contains("calibrated"));
    }

    #[test]
    fn policy_reads_from_bricks_toml() {
        let p = policy_from_bricks_toml(
            "[context]\ncompact_threshold = 0.7\nsafety_margin_tokens = 2048\n",
        )
        .unwrap();
        assert_eq!((p.compact_threshold, p.safety_margin_tokens), (0.7, 2048));
        assert!(
            policy_from_bricks_toml("[context]\ncompact_treshold = 0.7\n")
                .unwrap_err()
                .contains("compact_treshold")
        );
        assert!(policy_from_bricks_toml("[context]\npreflight_ratio = 1.5\n").is_err());
    }
}
