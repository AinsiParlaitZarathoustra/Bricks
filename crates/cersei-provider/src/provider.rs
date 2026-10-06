//! `ConfiguredProvider`: the one [`Provider`] implementation.
//!
//! It is bound to a single resolved model and drives the right protocol adapter
//! from configuration alone — there is no per-vendor code. Each request goes
//! through the same pipeline:
//!
//! 1. refuse, before sending, anything the model or adapter cannot carry;
//! 2. build the protocol request;
//! 3. merge the model's parameters and the selected reasoning profile;
//! 4. send it, mapping HTTP statuses to typed errors the runner can retry;
//! 5. decode the (streamed or complete) response into stream events, attaching
//!    a cost estimate to the usage when prices are known.
//!
//! Keys are held in a [`Secret`], marked sensitive on the wire, and scrubbed
//! from every error message this module produces.

use crate::config::{AuthMode, Secret};
use crate::modality::validate_messages;
use crate::protocol::{self, sse::SseDecoder, BuildCtx};
use crate::reasoning;
use crate::registry::{KeySource, ResolvedModel};
use crate::{CompletionRequest, CompletionStream, Provider};
use async_trait::async_trait;
use cersei_types::{CerseiError, Modality, Result, StreamEvent, Usage};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Option key a request can use to pick a reasoning profile for that call.
pub const REASONING_PROFILE_OPTION: &str = "reasoning_profile";

type EnvLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Builds a [`ConfiguredProvider`], resolving the secret reference.
pub struct ProviderBuilder {
    model: ResolvedModel,
    profile: Option<String>,
    tariff: Option<String>,
    env: Option<EnvLookup>,
    client: Option<reqwest::Client>,
}

impl ProviderBuilder {
    pub(crate) fn new(model: ResolvedModel) -> Self {
        Self {
            model,
            profile: None,
            tariff: None,
            env: None,
            client: None,
        }
    }

    /// Reasoning profile used when a request does not name one. Overrides the
    /// model's configured default.
    pub fn reasoning_profile(mut self, id: impl Into<String>) -> Self {
        self.profile = Some(id.into());
        self
    }

    /// Tariff used for cost estimates (default: the base tariff).
    pub fn tariff(mut self, name: impl Into<String>) -> Self {
        self.tariff = Some(name.into());
        self
    }

    /// Where `api_key_env` is looked up (default: the process environment).
    pub fn env(mut self, lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.env = Some(Box::new(lookup));
        self
    }

    /// Use a specific HTTP client (proxy, timeouts, …).
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.client = Some(client);
        self
    }

    pub fn build(self) -> Result<ConfiguredProvider> {
        let m = &self.model;
        let who = m.selection();

        // Fail early on a profile or tariff that does not exist.
        if let Some(id) = &self.profile {
            reasoning::select_profile(&m.reasoning, Some(id))
                .map_err(|e| CerseiError::Config(format!("{who}: {e}")))?;
        }
        if let Some(name) = &self.tariff {
            match &m.pricing {
                Some(p) if p.tariff(name).is_some() => {}
                Some(p) => {
                    return Err(CerseiError::Config(format!(
                        "{who}: unknown tariff `{name}`; available: {}",
                        p.tariff_names().collect::<Vec<_>>().join(", ")
                    )))
                }
                None => {
                    return Err(CerseiError::Config(format!(
                        "{who}: tariff `{name}` requested but the model declares no pricing"
                    )))
                }
            }
        }

        let key = if m.auth == AuthMode::None {
            None
        } else {
            match &m.key_source {
                KeySource::Inline(k) => Some(k.clone()),
                KeySource::Env(var) => {
                    let value = match &self.env {
                        Some(lookup) => lookup(var),
                        None => std::env::var(var).ok(),
                    };
                    match value.filter(|v| !v.trim().is_empty()) {
                        Some(v) => Some(Secret::new(v.trim())),
                        None => {
                            return Err(CerseiError::Auth(format!(
                                "provider \"{}\": environment variable `{var}` (api_key_env) is \
                                 not set or empty",
                                m.provider_id
                            )))
                        }
                    }
                }
                KeySource::None => {
                    return Err(CerseiError::Auth(format!(
                        "provider \"{}\": auth = \"{}\" but no key source is configured",
                        m.provider_id,
                        m.auth.as_str()
                    )))
                }
            }
        };

        Ok(ConfiguredProvider {
            model: Arc::new(self.model),
            key,
            profile: self.profile,
            tariff: self.tariff,
            client: self.client.unwrap_or_default(),
        })
    }
}

/// A provider bound to one resolved model.
#[derive(Clone)]
pub struct ConfiguredProvider {
    model: Arc<ResolvedModel>,
    key: Option<Secret>,
    profile: Option<String>,
    tariff: Option<String>,
    client: reqwest::Client,
}

impl std::fmt::Debug for ConfiguredProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfiguredProvider")
            .field("model", &*self.model)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .field("profile", &self.profile)
            .finish()
    }
}

impl ConfiguredProvider {
    pub fn model(&self) -> &ResolvedModel {
        &self.model
    }

    pub fn into_boxed(self) -> Box<dyn Provider> {
        Box::new(self)
    }

    /// Remove the key (and any header value) from a message that is about to
    /// be shown or logged.
    fn redact(&self, text: &str) -> String {
        redact_secrets(text, self.key.as_ref(), &self.model)
    }

    fn request_headers(&self, stream: bool) -> Result<HeaderMap> {
        let mut map = HeaderMap::new();
        map.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if stream {
            map.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        }
        for (name, value) in &self.model.headers {
            let n = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| CerseiError::Config(format!("invalid header name `{name}`")))?;
            let mut v = HeaderValue::from_str(value.expose())
                .map_err(|_| CerseiError::Config(format!("invalid value for header `{name}`")))?;
            v.set_sensitive(true);
            map.insert(n, v);
        }
        if let Some(key) = &self.key {
            match self.model.auth {
                AuthMode::Bearer => {
                    let mut v = HeaderValue::from_str(&format!("Bearer {}", key.expose()))
                        .map_err(|_| {
                            CerseiError::Auth("the API key is not a valid header value".into())
                        })?;
                    v.set_sensitive(true);
                    map.insert(AUTHORIZATION, v);
                }
                AuthMode::ApiKeyHeader => {
                    let n = HeaderName::from_bytes(self.model.auth_header.as_bytes())
                        .map_err(|_| CerseiError::Config("invalid auth_header".into()))?;
                    let mut v = HeaderValue::from_str(key.expose()).map_err(|_| {
                        CerseiError::Auth("the API key is not a valid header value".into())
                    })?;
                    v.set_sensitive(true);
                    map.insert(n, v);
                }
                AuthMode::None => {}
            }
        }
        Ok(map)
    }

    /// Validate a request and build the body the protocol sends, with the
    /// model's parameters and the selected reasoning profile applied.
    fn request_body(&self, request: &CompletionRequest, stream: bool) -> Result<serde_json::Value> {
        let m = &self.model;
        validate_messages(
            &request.messages,
            &request.output_modalities,
            !request.tools.is_empty(),
            &m.declared,
            m.protocol,
        )
        .map_err(|e| CerseiError::Unsupported(format!("{}: {e}", m.selection())))?;

        let ctx = BuildCtx {
            api_model: &m.api_model,
            compat: &m.compat,
            max_output_tokens: m.limits.max_output_tokens,
            stream,
        };
        let mut body = match m.protocol {
            crate::config::Protocol::ChatCompletions => {
                protocol::chat_completions::build_body(&ctx, request)?
            }
            crate::config::Protocol::Responses => protocol::responses::build_body(&ctx, request)?,
            crate::config::Protocol::AnthropicMessages => {
                protocol::anthropic_messages::build_body(&ctx, request)?
            }
        };

        let requested = request
            .options
            .get::<String>(REASONING_PROFILE_OPTION)
            .or_else(|| self.profile.clone());
        let profile = reasoning::select_profile(&m.reasoning, requested.as_deref())
            .map_err(|e| CerseiError::Config(format!("{}: {e}", m.selection())))?;
        reasoning::apply(
            &mut body,
            m.protocol,
            &m.parameters,
            &m.remove_parameters,
            profile,
        )
        .map_err(|e| CerseiError::Config(format!("{}: {e}", m.selection())))?;
        Ok(body)
    }

    /// Estimate the cost of a usage under this provider's tariff. `None` when
    /// the model declares no prices (unknown, not free).
    pub fn estimate_cost(&self, usage: &Usage) -> Option<cersei_types::CostEstimate> {
        self.model
            .pricing
            .as_ref()?
            .estimate(usage, self.tariff.as_deref(), &[])
            .ok()
            .flatten()
    }
}

pub(crate) fn redact_secrets(text: &str, key: Option<&Secret>, model: &ResolvedModel) -> String {
    let mut out = text.to_string();
    let candidates = key
        .into_iter()
        .map(Secret::expose)
        .chain(model.headers.values().map(Secret::expose));
    for secret in candidates {
        if secret.len() >= 4 {
            out = out.replace(secret, "***");
        }
    }
    out
}

/// Attaches cost estimates to the cumulative usage as events pass through.
struct CostRelay {
    cum: Usage,
    model: Arc<ResolvedModel>,
    tariff: Option<String>,
    unmetered: Vec<Modality>,
}

impl CostRelay {
    fn map(&mut self, event: StreamEvent) -> StreamEvent {
        match event {
            StreamEvent::MessageStart { id, model, usage } => {
                if let Some(u) = &usage {
                    self.absorb(u);
                }
                StreamEvent::MessageStart { id, model, usage }
            }
            StreamEvent::MessageDelta { stop_reason, usage } => {
                if let Some(u) = &usage {
                    self.absorb(u);
                }
                let usage = match &self.model.pricing {
                    Some(pricing) => {
                        let est = pricing
                            .estimate(&self.cum, self.tariff.as_deref(), &self.unmetered)
                            .ok()
                            .flatten();
                        self.cum.cost_usd = est.as_ref().map(|e| e.amount_usd);
                        self.cum.cost_estimate = est;
                        Some(self.cum.clone())
                    }
                    None => usage,
                };
                StreamEvent::MessageDelta { stop_reason, usage }
            }
            other => other,
        }
    }

    fn absorb(&mut self, u: &Usage) {
        self.cum.merge_cumulative(u);
        if !u.provider_usage.is_null() {
            self.cum.provider_usage = u.provider_usage.clone();
        }
    }
}

enum Decoder {
    Chat(Box<protocol::chat_completions::StreamState>),
    Responses(Box<protocol::responses::StreamState>),
    Anthropic,
}

impl Decoder {
    fn on_event(&mut self, ev: protocol::sse::SseEvent) -> (Vec<StreamEvent>, bool) {
        match self {
            Decoder::Chat(s) => {
                let events = s.on_data(&ev.data);
                (events, s.saw_done())
            }
            Decoder::Responses(s) => (s.on_data(&ev.data), false),
            Decoder::Anthropic => (
                protocol::anthropic_messages::parse_sse_event(ev.event.as_deref(), &ev.data)
                    .into_iter()
                    .collect(),
                false,
            ),
        }
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        match self {
            Decoder::Chat(s) => s.finish(),
            Decoder::Responses(s) => s.finish(),
            Decoder::Anthropic => Vec::new(),
        }
    }
}

#[async_trait]
impl Provider for ConfiguredProvider {
    fn name(&self) -> &str {
        &self.model.provider_id
    }

    /// The prompt budget: `max_input_tokens`, reduced when the window is shared
    /// with the output (whose reserve includes reasoning on all protocols).
    fn context_window(&self, _model: &str) -> u64 {
        let l = &self.model.limits;
        l.input_budget(l.max_output_tokens)
    }

    fn model_info(&self) -> Option<crate::ModelInfo> {
        let m = &self.model;
        Some(crate::ModelInfo {
            selection: m.selection(),
            protocol: m.protocol.as_str(),
            limits: crate::ModelLimits {
                max_input_tokens: m.limits.max_input_tokens,
                max_output_tokens: m.limits.max_output_tokens,
                context_window_tokens: m.limits.context_window_tokens,
            },
            reasoning_resent: match m.protocol {
                crate::config::Protocol::ChatCompletions => m.compat.reasoning_field.is_some(),
                _ => true,
            },
            token_counting: m.count_url.is_some(),
        })
    }

    /// Count with the configured endpoint: the same body as the request,
    /// restricted to the fields the counting route accepts.
    async fn count_request_tokens(&self, request: &CompletionRequest) -> Result<Option<u64>> {
        let m = &self.model;
        let Some(url) = m.count_url.clone() else {
            return Ok(None);
        };
        let body = self.request_body(request, false)?;
        let allowed: &[&str] = match m.protocol {
            crate::config::Protocol::AnthropicMessages => &[
                "model",
                "messages",
                "system",
                "tools",
                "tool_choice",
                "thinking",
                "mcp_servers",
            ],
            crate::config::Protocol::Responses => &[
                "model",
                "input",
                "instructions",
                "tools",
                "tool_choice",
                "reasoning",
                "text",
                "truncation",
                "parallel_tool_calls",
            ],
            crate::config::Protocol::ChatCompletions => return Ok(None),
        };
        let body: serde_json::Map<String, serde_json::Value> = body
            .as_object()
            .map(|o| {
                o.iter()
                    .filter(|(k, _)| allowed.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let http = self
            .client
            .post(url)
            .headers(self.request_headers(false)?)
            .json(&body)
            .build()
            .map_err(|e| CerseiError::Http(e.without_url()))?;
        let response = self
            .client
            .execute(http)
            .await
            .map_err(|e| CerseiError::Http(e.without_url()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = crate::parse_retry_after(response.headers());
            let text = response.text().await.unwrap_or_default();
            return Err(CerseiError::from_http_status(
                status,
                retry_after,
                self.redact(&text),
            ));
        }
        let json: serde_json::Value = response.json().await.map_err(|e| {
            CerseiError::Provider(format!(
                "token count response is not valid JSON: {}",
                self.redact(&e.without_url().to_string())
            ))
        })?;
        json.get("input_tokens")
            .and_then(serde_json::Value::as_u64)
            .map(Some)
            .ok_or_else(|| {
                CerseiError::Provider("token count response has no integer `input_tokens`".into())
            })
    }

    /// The request's `model` field is ignored: this provider *is* one model, and
    /// the identifier sent to the server is always the configured `api_model`.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream> {
        let m = &self.model;
        let stream = m.declared.streaming;
        let body = self.request_body(&request, stream)?;

        let http = self
            .client
            .post(m.url.clone())
            .headers(self.request_headers(stream)?)
            .json(&body)
            .build()
            .map_err(|e| CerseiError::Http(e.without_url()))?;

        // F-02: the status is checked before anything is spawned, so a non-2xx
        // is a typed `Err` from `complete()` — the only place the runner's
        // retry loop can see it.
        let response = self
            .client
            .execute(http)
            .await
            .map_err(|e| CerseiError::Http(e.without_url()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after = crate::parse_retry_after(response.headers());
            let text = response.text().await.unwrap_or_default();
            return Err(CerseiError::from_http_status(
                status,
                retry_after,
                self.redact(&text),
            ));
        }

        let mut unmetered: Vec<Modality> = request
            .messages
            .iter()
            .flat_map(protocol::blocks_of)
            .filter_map(|b| b.modality())
            .chain(request.output_modalities.iter().copied())
            .filter(|md| *md != Modality::Text)
            .collect();
        unmetered.sort();
        unmetered.dedup();
        let mut relay = CostRelay {
            cum: Usage::default(),
            model: Arc::clone(&self.model),
            tariff: self.tariff.clone(),
            unmetered,
        };

        let (tx, rx) = mpsc::channel(256);
        let this = self.clone();
        let protocol = m.protocol;

        if !stream {
            let json: serde_json::Value = response.json().await.map_err(|e| {
                CerseiError::Provider(format!(
                    "response is not valid JSON: {}",
                    this.redact(&e.without_url().to_string())
                ))
            })?;
            let events = match protocol {
                crate::config::Protocol::ChatCompletions => {
                    protocol::chat_completions::response_to_events(&json)?
                }
                crate::config::Protocol::Responses => {
                    protocol::responses::response_to_events(&json)?
                }
                crate::config::Protocol::AnthropicMessages => {
                    protocol::anthropic_messages::response_to_events(&json)?
                }
            };
            tokio::spawn(async move {
                for ev in events {
                    if tx.send(relay.map(ev)).await.is_err() {
                        return;
                    }
                }
            });
            return Ok(CompletionStream::new(rx));
        }

        tokio::spawn(async move {
            let mut decoder = SseDecoder::new();
            let mut state = match protocol {
                crate::config::Protocol::ChatCompletions => Decoder::Chat(Box::default()),
                crate::config::Protocol::Responses => Decoder::Responses(Box::default()),
                crate::config::Protocol::AnthropicMessages => Decoder::Anthropic,
            };
            let mut bytes = response.bytes_stream();
            'read: while let Some(chunk) = bytes.next().await {
                match chunk {
                    Ok(b) => {
                        for sse in decoder.push(&b) {
                            let (events, done) = state.on_event(sse);
                            for ev in events {
                                if tx.send(relay.map(ev)).await.is_err() {
                                    return;
                                }
                            }
                            if done {
                                break 'read;
                            }
                        }
                    }
                    Err(e) => {
                        let message = this.redact(&e.without_url().to_string());
                        let _ = tx.send(StreamEvent::Error { message }).await;
                        return;
                    }
                }
            }
            for sse in decoder.finish() {
                let (events, _) = state.on_event(sse);
                for ev in events {
                    if tx.send(relay.map(ev)).await.is_err() {
                        return;
                    }
                }
            }
            for ev in state.finish() {
                if tx.send(relay.map(ev)).await.is_err() {
                    return;
                }
            }
        });
        Ok(CompletionStream::new(rx))
    }
}
