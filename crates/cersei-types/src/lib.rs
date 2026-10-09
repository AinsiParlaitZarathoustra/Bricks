//! cersei-types: Provider-agnostic message types, errors, and content blocks
//! for the Cersei coding agent SDK.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

mod media;
pub use media::{detect_mime, MediaKind};
pub mod duration;
pub mod http;
pub mod tokens;

// ─── Roles ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

// ─── Modalities ──────────────────────────────────────────────────────────────

/// A kind of media a model can read or produce. Tool calls are not a
/// modality: they are structured control flow, carried by
/// [`ContentBlock::ToolUse`] / [`ContentBlock::ToolResult`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Image,
    Audio,
    Video,
    Document,
}

impl Modality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Modality::Text => "text",
            Modality::Image => "image",
            Modality::Audio => "audio",
            Modality::Video => "video",
            Modality::Document => "document",
        }
    }
}

impl std::fmt::Display for Modality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─── Content blocks ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
        // `skip_serializing_if`: a signature Cersei never captured must be
        // *omitted* when history is echoed back, not sent as `"signature": ""`
        // — adaptive-thinking models reject the empty string with a 400
        // (TOOL-CALLING-RELIABILITY.md §10.5 #7). A real signature captured
        // from `signature_delta` round-trips intact.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    Document {
        source: DocumentSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        citations: Option<CitationsConfig>,
    },
    /// Audio input. Kept as audio (never converted to text implicitly).
    Audio {
        source: MediaSource,
    },
    /// Video input. Kept as video (never converted to text implicitly).
    Video {
        source: MediaSource,
    },
    /// An item a wire protocol requires to be echoed back verbatim when the
    /// history is re-sent (e.g. a Responses-API `reasoning` item carrying
    /// `encrypted_content`). Only the protocol named in `protocol` reads it;
    /// every other adapter skips it.
    ProtocolItem {
        protocol: String,
        item: Value,
    },
    /// Escape hatch for provider-specific block types not covered above.
    #[serde(other)]
    Opaque,
}

impl ContentBlock {
    /// The input modality this block represents, or `None` for blocks that are
    /// not media (tool calls and results, reasoning, opaque protocol items).
    pub fn modality(&self) -> Option<Modality> {
        match self {
            ContentBlock::Text { .. } => Some(Modality::Text),
            ContentBlock::Image { .. } => Some(Modality::Image),
            ContentBlock::Audio { .. } => Some(Modality::Audio),
            ContentBlock::Video { .. } => Some(Modality::Video),
            ContentBlock::Document { .. } => Some(Modality::Document),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Identifier of a file already uploaded to the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSource {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Identifier of a file already uploaded to the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

/// Payload of an audio or video block: inline base64 data, a URL, or the id of
/// a file already uploaded to the provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaSource {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationsConfig {
    pub enabled: bool,
}

// ─── Messages ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// Provider-specific metadata (cache tokens, etc.)
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub provider_data: Value,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(content.into()),
            id: None,
            metadata: None,
        }
    }

    pub fn user_blocks(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Blocks(blocks),
            id: None,
            metadata: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text(content.into()),
            id: None,
            metadata: None,
        }
    }

    pub fn assistant_blocks(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
            id: None,
            metadata: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: MessageContent::Text(content.into()),
            id: None,
            metadata: None,
        }
    }

    /// Extract the first text content from this message.
    pub fn get_text(&self) -> Option<&str> {
        match &self.content {
            MessageContent::Text(t) => Some(t.as_str()),
            MessageContent::Blocks(blocks) => blocks.iter().find_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            }),
        }
    }

    /// Collect all text content blocks into one concatenated string.
    pub fn get_all_text(&self) -> String {
        match &self.content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    pub fn get_tool_use_blocks(&self) -> Vec<&ContentBlock> {
        match &self.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
                .collect(),
            _ => vec![],
        }
    }

    pub fn has_tool_use(&self) -> bool {
        !self.get_tool_use_blocks().is_empty()
    }

    pub fn content_blocks(&self) -> Vec<ContentBlock> {
        match &self.content {
            MessageContent::Text(t) => vec![ContentBlock::Text { text: t.clone() }],
            MessageContent::Blocks(b) => b.clone(),
        }
    }
}

// ─── System-prompt dynamic boundary ──────────────────────────────────────────

/// Marker an agent may embed in `CompletionRequest.system` to separate the
/// stable (cacheable) prefix from the per-turn dynamic tail (git status,
/// date, memory index). Providers split or strip it before the request goes
/// on the wire: Anthropic places its cache breakpoint on the stable half
/// only, so tail changes stop invalidating the cached prefix; providers with
/// automatic caching just remove the marker. A system string without the
/// marker is sent as a single block, unchanged.
///
/// Defined here (not in `cersei-agent`, which historically owned it and now
/// re-exports it) because the dependency direction is agent -> provider: the
/// providers that must consume the marker cannot import the agent crate.
pub const SYSTEM_PROMPT_DYNAMIC_BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";

// ─── Usage / Cost ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Usage {
    /// Uncached prompt tokens billed at the full input rate. The total prompt
    /// size is `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    /// Prompt tokens written to the provider's prompt cache this request
    /// (Anthropic: billed at ~1.25x the input rate for the default 5m TTL).
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// Prompt tokens served from the provider's prompt cache this request
    /// (Anthropic: billed at ~0.1x the input rate).
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Reasoning ("thinking") tokens. Informational: every supported protocol
    /// already counts them inside `output_tokens`, so this is a subset of
    /// `output_tokens` and must never be added to it.
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// How `cost_usd` was estimated (tariff, partial or complete). `None` when
    /// no price was known — an absent price is "unknown", never "free".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_estimate: Option<CostEstimate>,
    /// Provider-specific usage data not covered by the fields above.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub provider_usage: Value,
}

/// A cost estimate in USD, with the information needed to read it honestly.
///
/// This is an estimate from declared per-token prices, not an invoice: it
/// covers only the token categories whose price was known, and media billed
/// per second, per image or per other unit is not included.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CostEstimate {
    pub amount_usd: f64,
    /// Name of the tariff the estimate was computed with.
    pub tariff: String,
    /// True when some consumed category had no known price (or a billing unit
    /// not expressible as a per-token price), so `amount_usd` is a lower bound.
    pub partial: bool,
    /// Categories that were consumed but could not be priced.
    #[serde(default)]
    pub unpriced: Vec<String>,
}

impl Usage {
    fn has_tokens(&self) -> bool {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
            > 0
    }

    /// Accumulate a cost estimate across requests. An estimate that is
    /// missing for a request that consumed tokens makes the sum partial.
    fn merge_cost_across(&mut self, other: &Usage, self_had_tokens: bool) {
        self.cost_estimate = match (self.cost_estimate.take(), other.cost_estimate.as_ref()) {
            (Some(mut a), Some(b)) => {
                a.amount_usd += b.amount_usd;
                a.partial |= b.partial;
                if a.tariff != b.tariff {
                    a.tariff = format!("{}+{}", a.tariff, b.tariff);
                }
                for u in &b.unpriced {
                    if !a.unpriced.contains(u) {
                        a.unpriced.push(u.clone());
                    }
                }
                Some(a)
            }
            (Some(mut a), None) => {
                if other.has_tokens() {
                    a.partial = true;
                }
                Some(a)
            }
            (None, Some(b)) => {
                let mut b = b.clone();
                if self_had_tokens {
                    b.partial = true;
                }
                Some(b)
            }
            (None, None) => None,
        };
    }

    pub fn total(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.input_tokens + self.output_tokens
        }
    }

    /// Additive merge for accumulating usage ACROSS requests (session totals,
    /// cost tracking). Do not use this to combine usage events within one
    /// streamed message — see [`Usage::merge_cumulative`].
    pub fn merge(&mut self, other: &Usage) {
        let self_had_tokens = self.has_tokens();
        self.merge_cost_across(other, self_had_tokens);
        self.reasoning_tokens += other.reasoning_tokens;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.total_tokens = self.input_tokens + self.output_tokens;
        if let (Some(a), Some(b)) = (self.cost_usd, other.cost_usd) {
            self.cost_usd = Some(a + b);
        } else if other.cost_usd.is_some() {
            self.cost_usd = other.cost_usd;
        }
    }

    /// Merge for usage events WITHIN one streamed message, where counters are
    /// cumulative snapshots rather than increments: Anthropic's `message_start`
    /// carries the input/cache side plus a small initial `output_tokens`, and
    /// the final `message_delta` carries the cumulative output total. Adding
    /// them would double-count, so each field takes the larger snapshot.
    pub fn merge_cumulative(&mut self, other: &Usage) {
        self.reasoning_tokens = self.reasoning_tokens.max(other.reasoning_tokens);
        if other.cost_estimate.is_some() {
            self.cost_estimate = other.cost_estimate.clone();
        }
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .max(other.cache_creation_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .max(other.cache_read_input_tokens);
        self.total_tokens = self.input_tokens + self.output_tokens;
        if other.cost_usd.is_some() {
            self.cost_usd = other.cost_usd;
        }
    }
}

// ─── Stop reasons ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
    ContentFilter,
}

// ─── Tool definition (sent to providers) ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

// ─── Stream events ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum StreamEvent {
    MessageStart {
        id: String,
        model: String,
        /// Usage carried on the message-open event. Anthropic's `message_start`
        /// is the ONLY event that reports `cache_creation_input_tokens` /
        /// `cache_read_input_tokens`, so dropping this loses cache accounting.
        /// None for providers that report usage only at end of stream.
        usage: Option<Usage>,
    },
    ContentBlockStart {
        index: usize,
        block_type: String,
        /// For tool_use blocks: the tool use ID. Default: None.
        #[allow(unused)]
        id: Option<String>,
        /// For tool_use blocks: the tool name. Default: None.
        #[allow(unused)]
        name: Option<String>,
    },
    TextDelta {
        index: usize,
        text: String,
    },
    InputJsonDelta {
        index: usize,
        partial_json: String,
    },
    ThinkingDelta {
        index: usize,
        thinking: String,
    },
    /// Cryptographic signature for a thinking block (Anthropic
    /// `signature_delta`). Must be captured and echoed back verbatim in
    /// multi-turn history — dropping it (the pre-fix behaviour) meant every
    /// echoed thinking block carried an empty signature, which adaptive
    /// models reject.
    SignatureDelta {
        index: usize,
        signature: String,
    },
    ContentBlockStop {
        index: usize,
    },
    /// A complete [`ContentBlock::ProtocolItem`] the protocol needs echoed back
    /// verbatim (e.g. a Responses `reasoning` item). Placed at `index`.
    ProtocolItem {
        index: usize,
        protocol: String,
        item: Value,
    },
    MessageDelta {
        stop_reason: Option<StopReason>,
        usage: Option<Usage>,
    },
    MessageStop,
    Error {
        message: String,
    },
    Ping,
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(thiserror::Error, Debug)]
pub enum CerseiError {
    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Provider error {status}: {message}")]
    ProviderStatus {
        status: u16,
        message: String,
        /// The server's `Retry-After`, when it sent a valid one (a 503
        /// often does). It never makes an error retryable by itself.
        retry_after: Option<Duration>,
    },

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Tool error: {0}")]
    Tool(String),

    #[error("Permission denied: {0}")]
    Permission(String),

    #[error("Rate limit exceeded: {message}")]
    RateLimit {
        retry_after: Option<Duration>,
        message: String,
    },

    #[error("Context overflow: {used}/{limit} tokens")]
    ContextOverflow { used: u64, limit: u64 },

    #[error("Cancelled")]
    Cancelled,

    #[error("Configuration error: {0}")]
    Config(String),

    /// The request asks for something the selected model or its protocol
    /// adapter cannot carry. Raised before anything is sent.
    #[error("Unsupported request: {0}")]
    Unsupported(String),

    /// The input of a run or of a delegation is unusable (an empty prompt,
    /// an impossible limit). Raised before any provider is built or any
    /// request is sent.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl CerseiError {
    /// The error for a non-2xx provider response.
    ///
    /// Every provider funnels its HTTP failures through here so that "which
    /// statuses are worth retrying" is decided once, next to
    /// [`CerseiError::is_retryable`], rather than four times in four clients.
    pub fn from_http_status(
        status: u16,
        retry_after: Option<Duration>,
        message: impl Into<String>,
    ) -> Self {
        match status {
            429 => CerseiError::RateLimit {
                retry_after,
                message: message.into(),
            },
            _ => CerseiError::ProviderStatus {
                status,
                message: message.into(),
                retry_after,
            },
        }
    }

    /// The delay the server asked for (`Retry-After`), kept from the HTTP
    /// response. Whether to retry at all is [`Self::is_retryable`]'s call.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            CerseiError::RateLimit { retry_after, .. }
            | CerseiError::ProviderStatus { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// The HTTP status of the provider's answer, when there was one (a
    /// transport error that never got a response has none).
    pub fn http_status(&self) -> Option<u16> {
        match self {
            CerseiError::RateLimit { .. } => Some(429),
            CerseiError::ProviderStatus { status, .. } => Some(*status),
            CerseiError::Http(e) => e.status().map(|s| s.as_u16()),
            _ => None,
        }
    }

    /// A transport failure that timed out (no HTTP status).
    pub fn is_timeout(&self) -> bool {
        matches!(self, CerseiError::Http(e) if e.is_timeout())
    }

    /// The server refused the request because the prompt does not fit the
    /// model's context. Recognised from the documented error codes and
    /// messages of the supported protocols (HTTP 400/413/422, or an error
    /// event in a stream). Retrying the same request cannot succeed; the
    /// context has to shrink first.
    pub fn is_context_overflow(&self) -> bool {
        let message = match self {
            CerseiError::ContextOverflow { .. } => return true,
            CerseiError::ProviderStatus {
                status, message, ..
            } => {
                if !matches!(status, 400 | 413 | 422) {
                    return false;
                }
                message
            }
            CerseiError::Provider(message) => message,
            _ => return false,
        };
        let m = message.to_ascii_lowercase();
        [
            "context_length_exceeded",
            "context length",
            "context window",
            "maximum context",
            "prompt is too long",
            "input is too long",
            "too many tokens",
            "too many input tokens",
            "reduce the length of the messages",
            "exceeds the maximum number of tokens",
            "request_too_large",
        ]
        .iter()
        .any(|p| m.contains(p))
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            // A 429 is transient — unless the body says the account is out of
            // money, in which case every retry buys the same answer and the
            // backoff ladder just delays the inevitable by five sleeps.
            CerseiError::RateLimit { message, .. } => !Self::quota_exhausted(message),
            // 529 is Anthropic's "overloaded". 500/502/503/504 are standard
            // upstream/gateway blips — Gemini in particular returns 503
            // UNAVAILABLE under load, which used to be session-fatal.
            CerseiError::ProviderStatus { status, .. } => {
                matches!(status, 429 | 500 | 502 | 503 | 504 | 529)
            }
            // Transport-level failures that never produced a status: the
            // connection was refused, reset, or timed out. Scoped to the two
            // reqwest classes that are unambiguously transient; malformed-URL
            // and builder errors stay fatal.
            CerseiError::Http(e) => e.is_connect() || e.is_timeout(),
            _ => false,
        }
    }

    /// A 429 that means "no credit", not "slow down".
    ///
    /// OpenAI: `insufficient_quota` / "You exceeded your current quota".
    /// Anthropic: "credit balance is too low". Matched on the body because
    /// providers reuse HTTP 429 for both meanings; genuine rate limits
    /// (Gemini's RESOURCE_EXHAUSTED included) stay retryable.
    fn quota_exhausted(message: &str) -> bool {
        let m = message.to_ascii_lowercase();
        m.contains("insufficient_quota")
            || m.contains("exceeded your current quota")
            || m.contains("credit balance")
    }

    pub fn is_context_limit(&self) -> bool {
        matches!(self, CerseiError::ContextOverflow { .. })
    }
}

pub type Result<T> = std::result::Result<T, CerseiError>;

// ─── Session info ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub content: String,
    pub relevance: f32,
    pub source: String,
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    /// P3 #2: the cache accounting fields are first-class and must survive
    /// cross-request accumulation (session totals, CostTracker).
    #[test]
    fn merge_sums_cache_fields_across_requests() {
        let mut total = Usage {
            input_tokens: 100,
            output_tokens: 10,
            cache_creation_input_tokens: 3815,
            cache_read_input_tokens: 0,
            ..Default::default()
        };
        total.merge(&Usage {
            input_tokens: 50,
            output_tokens: 20,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 3815,
            ..Default::default()
        });
        assert_eq!(total.input_tokens, 150);
        assert_eq!(total.output_tokens, 30);
        assert_eq!(total.cache_creation_input_tokens, 3815);
        assert_eq!(total.cache_read_input_tokens, 3815);
    }

    /// Within one streamed message the usage events are cumulative snapshots
    /// (Anthropic: message_start carries input/cache + a small initial output;
    /// the final message_delta repeats output as a total). Each field takes
    /// the larger snapshot — adding would double-count.
    #[test]
    fn merge_cumulative_takes_snapshots_not_sums() {
        let mut msg = Usage {
            input_tokens: 3571,
            output_tokens: 2,
            cache_read_input_tokens: 6656,
            ..Default::default()
        };
        msg.merge_cumulative(&Usage {
            output_tokens: 727,
            ..Default::default()
        });
        assert_eq!(msg.input_tokens, 3571, "input snapshot must be kept");
        assert_eq!(msg.output_tokens, 727, "727, not 2 + 727");
        assert_eq!(msg.cache_read_input_tokens, 6656);

        // Applying the same snapshot twice must not grow anything.
        let before = (msg.input_tokens, msg.output_tokens);
        msg.merge_cumulative(&Usage {
            output_tokens: 727,
            ..Default::default()
        });
        assert_eq!((msg.input_tokens, msg.output_tokens), before);
    }
}

#[cfg(test)]
mod retry_tests {
    #[test]
    fn context_overflow_refusals_are_recognised() {
        use super::CerseiError;
        let overflow = [
            CerseiError::ProviderStatus {
                status: 400,
                message: r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#.into(),
                retry_after: None,
            },
            CerseiError::ProviderStatus {
                status: 400,
                message: r#"{"error":{"code":"context_length_exceeded","message":"This model's maximum context length is 128000 tokens."}}"#.into(),
                retry_after: None,
            },
            CerseiError::ProviderStatus {
                status: 413,
                message: "request_too_large".into(),
                retry_after: None,
            },
            CerseiError::Provider("stream error: context_length_exceeded".into()),
            CerseiError::ContextOverflow { used: 10, limit: 5 },
        ];
        for e in overflow {
            assert!(e.is_context_overflow(), "{e}");
            assert!(!e.is_retryable() || matches!(e, CerseiError::Provider(_)));
        }
        let other = [
            CerseiError::ProviderStatus {
                status: 400,
                message: "invalid tool schema".into(),
                retry_after: None,
            },
            CerseiError::ProviderStatus {
                status: 500,
                message: "context length".into(),
                retry_after: None,
            },
            CerseiError::Auth("bad key".into()),
        ];
        for e in other {
            assert!(!e.is_context_overflow(), "{e}");
        }
    }

    use super::*;

    fn status(code: u16) -> CerseiError {
        CerseiError::from_http_status(code, None, "upstream said no")
    }

    #[test]
    fn transient_5xx_statuses_are_retryable() {
        // 503 is the Gemini gap from TOOL-CALLING-RELIABILITY.md §10.5 #1:
        // it used to be session-fatal.
        for code in [429, 500, 502, 503, 504, 529] {
            assert!(status(code).is_retryable(), "{code} must be retryable");
        }
    }

    #[test]
    fn client_errors_are_fatal() {
        for code in [400, 401, 403, 404, 413, 422] {
            assert!(!status(code).is_retryable(), "{code} must not be retried");
        }
    }

    #[test]
    fn the_servers_delay_is_kept_and_decides_nothing() {
        let d = Some(Duration::from_secs(12));
        let e429 = CerseiError::from_http_status(429, d, "slow down");
        let e503 = CerseiError::from_http_status(503, d, "unavailable");
        for e in [&e429, &e503] {
            assert_eq!(e.retry_after(), d, "{e}");
            assert!(e.is_retryable(), "{e}");
        }
        assert_eq!(
            (e429.http_status(), e503.http_status()),
            (Some(429), Some(503))
        );
        // The display is unchanged by the delay.
        assert_eq!(e503.to_string(), "Provider error 503: unavailable");
        // A definitive error stays definitive whatever the header says.
        for code in [400, 401, 403, 404, 422] {
            let e = CerseiError::from_http_status(code, d, "no");
            assert_eq!(e.retry_after(), d);
            assert!(!e.is_retryable(), "{code}");
        }
        let quota = CerseiError::from_http_status(429, d, "insufficient_quota");
        assert!(!quota.is_retryable());
        assert_eq!(CerseiError::Auth("x".into()).retry_after(), None);
        assert_eq!(CerseiError::Auth("x".into()).http_status(), None);
    }

    #[test]
    fn a_genuine_rate_limit_is_retryable() {
        let err = CerseiError::from_http_status(
            429,
            Some(Duration::from_secs(2)),
            r#"{"error":{"type":"rate_limit_error","message":"Too many requests"}}"#,
        );
        assert!(err.is_retryable());
    }

    /// §10.5 #2: a 429 whose body says the account is out of credit is not
    /// transient. Retrying it five times used to stall the session through the
    /// whole backoff ladder to receive the same refusal.
    #[test]
    fn quota_exhaustion_is_not_retried() {
        for body in [
            // OpenAI's shape, verbatim fields.
            r#"{"error":{"type":"insufficient_quota","message":"You exceeded your current quota, please check your plan and billing details."}}"#,
            // Anthropic's shape.
            r#"{"error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#,
        ] {
            let err = CerseiError::from_http_status(429, None, body);
            assert!(
                !err.is_retryable(),
                "quota exhaustion must fail fast, retried anyway: {body}"
            );
        }
    }

    #[test]
    fn quota_matching_is_case_insensitive() {
        let err = CerseiError::from_http_status(429, None, "INSUFFICIENT_QUOTA");
        assert!(!err.is_retryable());
    }

    #[test]
    fn non_http_errors_stay_fatal() {
        assert!(!CerseiError::Provider("anything".into()).is_retryable());
        assert!(!CerseiError::Auth("bad key".into()).is_retryable());
        assert!(!CerseiError::Cancelled.is_retryable());
    }
}

#[cfg(test)]
mod modality_tests {
    use super::*;

    #[test]
    fn blocks_report_their_modality() {
        assert_eq!(
            ContentBlock::Text { text: "x".into() }.modality(),
            Some(Modality::Text)
        );
        assert_eq!(
            ContentBlock::image_url("u").modality(),
            Some(Modality::Image)
        );
        assert_eq!(
            ContentBlock::audio_url("u").modality(),
            Some(Modality::Audio)
        );
        assert_eq!(
            ContentBlock::video_url("u").modality(),
            Some(Modality::Video)
        );
        assert_eq!(
            ContentBlock::document_url("u").modality(),
            Some(Modality::Document)
        );
        // Tool calls and reasoning are not media.
        let call = ContentBlock::ToolUse {
            id: "i".into(),
            name: "n".into(),
            input: Value::Null,
        };
        assert_eq!(call.modality(), None);
        assert_eq!(
            ContentBlock::Thinking {
                thinking: "t".into(),
                signature: String::new()
            }
            .modality(),
            None
        );
    }

    #[test]
    fn new_blocks_roundtrip_through_json() {
        for block in [
            ContentBlock::audio_base64("audio/wav", "UklG"),
            ContentBlock::video_url("https://x/v.mp4"),
            ContentBlock::image_file_id("file-1"),
            ContentBlock::document_file_id("file-2"),
            ContentBlock::ProtocolItem {
                protocol: "responses".into(),
                item: serde_json::json!({"type": "reasoning", "encrypted_content": "E"}),
            },
        ] {
            let json = serde_json::to_value(&block).unwrap();
            let back: ContentBlock = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(serde_json::to_value(&back).unwrap(), json);
        }
        let v = serde_json::to_value(ContentBlock::video_url("u")).unwrap();
        assert_eq!(v["type"], "video");
        assert_eq!(v["source"]["type"], "url");
    }

    #[test]
    fn older_serialized_blocks_and_usage_still_load() {
        // A session saved before `file_id` and the reasoning/cost fields existed.
        let img: ContentBlock = serde_json::from_str(
            r#"{"type":"image","source":{"type":"base64","media_type":"image/png","data":"QQ=="}}"#,
        )
        .unwrap();
        assert!(matches!(img, ContentBlock::Image { source } if source.file_id.is_none()));
        let usage: Usage = serde_json::from_str(r#"{"input_tokens":5,"output_tokens":2}"#).unwrap();
        assert_eq!(
            (usage.reasoning_tokens, usage.cost_estimate.is_none()),
            (0, true)
        );
    }

    #[test]
    fn reasoning_tokens_are_a_subset_of_output_not_an_addend() {
        let mut total = Usage {
            output_tokens: 100,
            reasoning_tokens: 60,
            ..Default::default()
        };
        total.merge(&Usage {
            output_tokens: 50,
            reasoning_tokens: 10,
            ..Default::default()
        });
        assert_eq!(total.output_tokens, 150);
        assert_eq!(total.reasoning_tokens, 70);
        assert_eq!(
            total.total(),
            150,
            "reasoning is already inside the output count"
        );
        let mut snap = Usage {
            reasoning_tokens: 3,
            ..Default::default()
        };
        snap.merge_cumulative(&Usage {
            reasoning_tokens: 9,
            ..Default::default()
        });
        snap.merge_cumulative(&Usage {
            reasoning_tokens: 9,
            ..Default::default()
        });
        assert_eq!(snap.reasoning_tokens, 9);
    }
}
