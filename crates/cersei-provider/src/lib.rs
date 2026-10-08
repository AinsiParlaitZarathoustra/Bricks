//! cersei-provider: configurable LLM providers.
//!
//! Which providers and models exist is decided entirely by a configuration file
//! (`~/.bricks/providers.toml`, or an explicit `.toml`/`.json` path), loaded
//! into a [`ProviderRegistry`]. A model is selected explicitly as
//! `provider_id/model_id` and turned into a [`ConfiguredProvider`], which drives
//! one of three wire protocols — `chat_completions`, `responses`,
//! `anthropic_messages` — from configuration alone. Adding a provider or model
//! compatible with one of those protocols needs a configuration change, not a
//! recompilation.
//!
//! ```no_run
//! use cersei_provider::ProviderRegistry;
//!
//! let registry = ProviderRegistry::load(None)?;              // ~/.bricks/providers.toml
//! let model = registry.resolve("custom/flash")?;             // explicit selection
//! let provider = model.build_provider()?;                    // reads api_key_env now
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod adapt;
pub mod config;
pub mod decimal;
pub mod endpoint;
pub mod modality;
pub mod pricing;
pub mod protocol;
pub mod provider;
pub mod reasoning;
pub mod registry;
mod stream;

use async_trait::async_trait;
use cersei_types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

// Re-exports
pub use adapt::{adapt_tools, SchemaDialect};
pub use config::{
    default_config_path, AuthMode, ConfigError, ModelConfig, Protocol, ProviderConfig,
    ProvidersConfig, Secret, SCHEMA_VERSION,
};
pub use decimal::Decimal;
pub use modality::{protocol_support, Capabilities, ProtocolSupport};
pub use pricing::{Pricing, DEFAULT_TARIFF};
pub use provider::{ConfiguredProvider, ProviderBuilder, REASONING_PROFILE_OPTION};
pub use registry::{ModelRef, ProviderRegistry, ResolvedModel};
pub use stream::StreamAccumulator;

/// Load the configuration (`explicit` path, else `~/.bricks/providers.toml`),
/// resolve `selection` (`provider_id/model_id`) and build its provider.
/// There is no fallback: a missing or invalid configuration is an error.
pub fn provider_from_config(
    explicit: Option<&std::path::Path>,
    selection: &str,
) -> Result<ConfiguredProvider> {
    ProviderRegistry::load(explicit)?
        .resolve(selection)?
        .build_provider()
}

/// The delay a `Retry-After` header asks for (RFC 9110 §10.2.3), if the
/// provider sent a usable one: delta-seconds, or an HTTP date measured from
/// now. `None` for an absent, empty or invalid value — the caller then keeps
/// its own backoff. The value only says how long to wait; whether to retry
/// is decided elsewhere.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    retry_after_at(value, std::time::SystemTime::now())
}

/// [`parse_retry_after`] for one header value, against the reference time
/// `now` (read once by the caller):
///
/// * delta-seconds: decimal digits only (no sign, fraction or exponent);
///   `0` is a valid zero delay; a number too large to represent is invalid;
/// * an HTTP date (IMF-fixdate, RFC 850 or asctime, via `httpdate`): the
///   time left until it, zero when it is now or past;
/// * anything else: `None` (an invalid value is never a zero delay).
pub fn retry_after_at(value: &str, now: std::time::SystemTime) -> Option<std::time::Duration> {
    let v = value.trim_matches(|c| c == ' ' || c == '\t');
    if v.is_empty() {
        return None;
    }
    if v.bytes().all(|b| b.is_ascii_digit()) {
        return v.parse::<u64>().ok().map(std::time::Duration::from_secs);
    }
    let date = httpdate::parse_http_date(v).ok()?;
    Some(
        date.duration_since(now)
            .unwrap_or(std::time::Duration::ZERO),
    )
}

#[cfg(test)]
mod retry_after_tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn seconds_and_http_dates_are_read_against_one_reference_time() {
        // Sun, 06 Nov 1994 08:49:37 GMT
        let now = UNIX_EPOCH + Duration::from_secs(784_111_777);
        let at = |v: &str| retry_after_at(v, now);
        assert_eq!(at("12"), Some(Duration::from_secs(12)));
        assert_eq!(at("  0\t"), Some(Duration::ZERO));
        assert_eq!(at("007"), Some(Duration::from_secs(7)));
        // Future dates, in the three HTTP forms: exactly the time left.
        assert_eq!(
            at("Sun, 06 Nov 1994 08:50:07 GMT"),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            at("Sunday, 06-Nov-94 08:51:37 GMT"),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            at("Sun Nov  6 09:49:37 1994"),
            Some(Duration::from_secs(3600))
        );
        // Now or past: a valid zero delay, not an invalid value.
        assert_eq!(at("Sun, 06 Nov 1994 08:49:37 GMT"), Some(Duration::ZERO));
        assert_eq!(at("Sat, 05 Nov 1994 08:49:37 GMT"), Some(Duration::ZERO));
        // Invalid: never a zero delay.
        for bad in [
            "",
            "   ",
            "-1",
            "+5",
            "1.5",
            "1e3",
            "12s",
            "soon",
            "0x10",
            "99999999999999999999999",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "06 Nov 1994",
        ] {
            assert_eq!(at(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_header_is_read_from_a_response() {
        let mut h = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&h), None);
        h.insert(reqwest::header::RETRY_AFTER, "3".parse().unwrap());
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(3)));
        let later = SystemTime::now() + Duration::from_secs(3600);
        h.insert(
            reqwest::header::RETRY_AFTER,
            httpdate::fmt_http_date(later).parse().unwrap(),
        );
        let d = parse_retry_after(&h).unwrap();
        assert!(
            d > Duration::from_secs(3590) && d <= Duration::from_secs(3600),
            "{d:?}"
        );
    }
}

// ─── Provider trait ──────────────────────────────────────────────────────────

/// Declared token limits of the bound model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelLimits {
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    /// Total window shared by input and output, when configured. `None`
    /// means the configuration does not state one: nothing is inferred.
    pub context_window_tokens: Option<u64>,
}

impl ModelLimits {
    /// Tokens available for the prompt once `reserved_output` is set aside.
    pub fn input_budget(&self, reserved_output: u64) -> u64 {
        match self.context_window_tokens {
            Some(w) => self.max_input_tokens.min(w.saturating_sub(reserved_output)),
            None => self.max_input_tokens,
        }
    }
}

/// What the context manager needs to know about the bound model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Identity of the model (`provider_id/model_id`); measurements made
    /// with one model never apply to another.
    pub selection: String,
    /// Wire protocol (`chat_completions`, `responses`, `anthropic_messages`).
    pub protocol: &'static str,
    pub limits: ModelLimits,
    /// Reasoning kept in the history is sent back to the server (and so
    /// occupies the context): true for `anthropic_messages` and `responses`,
    /// and for `chat_completions` when `compat.reasoning_field` is set.
    pub reasoning_resent: bool,
    /// A token-counting endpoint is configured for this model.
    pub token_counting: bool,
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// The configured provider id (e.g. `custom`).
    fn name(&self) -> &str;

    /// Tokens available for the prompt with this model: the declared
    /// `max_input_tokens`, reduced when the window is shared with the output.
    fn context_window(&self, model: &str) -> u64;

    /// Identity, limits and protocol of the bound model. `None` for providers
    /// that do not come from configuration (tests, custom implementations):
    /// the context manager then uses [`Provider::context_window`] only and
    /// reports the missing information.
    fn model_info(&self) -> Option<ModelInfo> {
        None
    }

    /// Send a streaming completion request.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream>;

    /// Send a blocking (non-streaming) completion request.
    async fn complete_blocking(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        self.complete(request).await?.collect().await
    }

    /// Count the input tokens of a complete request with the server's
    /// counting endpoint. `Ok(None)` when no endpoint is configured for this
    /// model — callers then fall back to a local estimate. Counting is a
    /// pre-flight measurement: it never sends the request itself.
    async fn count_request_tokens(&self, _request: &CompletionRequest) -> Result<Option<u64>> {
        Ok(None)
    }
}

// Blanket impl: Box<dyn Provider> is itself a Provider.
#[async_trait]
impl Provider for Box<dyn Provider> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn context_window(&self, model: &str) -> u64 {
        (**self).context_window(model)
    }
    fn model_info(&self) -> Option<ModelInfo> {
        (**self).model_info()
    }
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream> {
        (**self).complete(request).await
    }
    async fn complete_blocking(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        (**self).complete_blocking(request).await
    }
    async fn count_request_tokens(&self, request: &CompletionRequest) -> Result<Option<u64>> {
        (**self).count_request_tokens(request).await
    }
}

// ─── Completion request/response ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub stop_sequences: Vec<String>,
    /// Engine-level options an adapter understands: `tool_choice = "required"`
    /// (forced tool call) and `reasoning_profile` (profile id for this call).
    /// Everything else a model needs is configuration, not request options.
    pub options: ProviderOptions,
    /// Output modalities the caller needs. Empty means text only. A modality
    /// the model or its adapter cannot produce is refused before sending.
    pub output_modalities: Vec<Modality>,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            system: None,
            tools: Vec::new(),
            max_tokens: 16384,
            temperature: None,
            stop_sequences: Vec::new(),
            options: ProviderOptions::default(),
            output_modalities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProviderOptions {
    entries: HashMap<String, serde_json::Value>,
}

impl ProviderOptions {
    pub fn set(&mut self, key: impl Into<String>, value: impl Serialize) {
        if let Ok(v) = serde_json::to_value(value) {
            self.entries.insert(key.into(), v);
        }
    }

    pub fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        self.entries
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    pub fn has(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }
}

#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub message: Message,
    pub usage: Usage,
    pub stop_reason: StopReason,
}

// ─── Completion stream ───────────────────────────────────────────────────────

/// A streaming response from a provider. Wraps a channel of StreamEvents.
pub struct CompletionStream {
    rx: mpsc::Receiver<StreamEvent>,
}

impl CompletionStream {
    pub fn new(rx: mpsc::Receiver<StreamEvent>) -> Self {
        Self { rx }
    }

    /// Consume the stream and collect into a complete response.
    pub async fn collect(mut self) -> Result<CompletionResponse> {
        let mut acc = StreamAccumulator::new();
        while let Some(event) = self.rx.recv().await {
            if let StreamEvent::Error { message } = &event {
                return Err(CerseiError::Provider(message.clone()));
            }
            acc.process_event(event);
        }
        acc.into_response()
    }

    /// Access the underlying receiver for real-time event processing.
    pub fn into_receiver(self) -> mpsc::Receiver<StreamEvent> {
        self.rx
    }
}
