//! Provider configuration: the single source of truth for which providers and
//! models exist, how to reach them and what they can do.
//!
//! The file is TOML (`~/.bricks/providers.toml`) or JSON — both deserialize
//! into the same Serde types, with the same defaults and the same validation.
//! Nothing here talks to the network and nothing reads the environment: secret
//! references (`api_key_env`) are only *resolved* when a provider is built.
//!
//! Errors always name the provider, the model and the field at fault, and never
//! echo a value that could be a secret.

use crate::decimal::Decimal;
use cersei_types::Modality;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// The only schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Default location of the configuration file, relative to the home directory.
pub const DEFAULT_CONFIG_RELATIVE_PATH: &str = ".bricks/providers.toml";

/// `~/.bricks/providers.toml`, or `None` when no home directory is known.
pub fn default_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(DEFAULT_CONFIG_RELATIVE_PATH))
}

// ─── Secrets ─────────────────────────────────────────────────────────────────

/// A string that must never be printed. `Debug` is redacted and there is no
/// `Display`; the value is only reachable through [`Secret::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Secret)
    }
}

// ─── Protocol and authentication ─────────────────────────────────────────────

/// The wire protocol of a model: request, response and streaming formats. It is
/// unrelated to any structured JSON the model is asked to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    ChatCompletions,
    Responses,
    AnthropicMessages,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::ChatCompletions => "chat_completions",
            Protocol::Responses => "responses",
            Protocol::AnthropicMessages => "anthropic_messages",
        }
    }

    /// Path of the protocol's endpoint relative to the provider's base URL.
    pub fn default_path(&self) -> &'static str {
        match self {
            Protocol::ChatCompletions => "chat/completions",
            Protocol::Responses => "responses",
            Protocol::AnthropicMessages => "messages",
        }
    }

    /// Authentication mode the protocol conventionally uses. A model that
    /// overrides the protocol without stating its own `auth` follows this
    /// (unless the provider is `none`).
    pub fn conventional_auth(&self) -> AuthMode {
        match self {
            Protocol::ChatCompletions | Protocol::Responses => AuthMode::Bearer,
            Protocol::AnthropicMessages => AuthMode::ApiKeyHeader,
        }
    }

    /// Headers the protocol requires on every request. Explicit `headers`
    /// entries (provider, then model) override these by name.
    pub fn required_headers(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Protocol::ChatCompletions | Protocol::Responses => &[],
            Protocol::AnthropicMessages => &[("anthropic-version", "2023-06-01")],
        }
    }

    /// Top-level request fields carrying content the engine builds. Free-form
    /// `parameters` and reasoning profiles can neither set nor remove them.
    pub fn protected_fields(&self) -> &'static [&'static str] {
        match self {
            Protocol::ChatCompletions => &[
                "model",
                "messages",
                "tools",
                "tool_choice",
                "stream",
                "stream_options",
            ],
            Protocol::Responses => &[
                "model",
                "input",
                "instructions",
                "tools",
                "tool_choice",
                "stream",
            ],
            Protocol::AnthropicMessages => &[
                "model",
                "messages",
                "system",
                "tools",
                "tool_choice",
                "stream",
            ],
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How the API key is attached to a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `<auth_header>: <key>` (default header `x-api-key`).
    ApiKeyHeader,
    /// No credentials (local servers).
    None,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMode::Bearer => "bearer",
            AuthMode::ApiKeyHeader => "api_key_header",
            AuthMode::None => "none",
        }
    }
}

/// Default header name for [`AuthMode::ApiKeyHeader`].
pub const DEFAULT_API_KEY_HEADER: &str = "x-api-key";

// ─── Document types ──────────────────────────────────────────────────────────

/// Protocol quirks some servers need. Every field is optional; a model's value
/// overrides the provider's field by field.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    /// `chat_completions`: name of the output-limit field (`max_tokens`, or
    /// `max_completion_tokens` for servers that renamed it). Default `max_tokens`.
    pub max_tokens_field: Option<String>,
    /// `chat_completions`: field carrying reasoning text on assistant messages
    /// (for example `reasoning_content`). When set, reasoning is also echoed
    /// back under this name on assistant turns that made tool calls. Reasoning
    /// found under `reasoning_content` or `reasoning` is always *read*.
    pub reasoning_field: Option<String>,
    /// `chat_completions`: send `stream_options.include_usage`. Default true.
    pub stream_usage: Option<bool>,
    /// `anthropic_messages`: place `cache_control` breakpoints on the stable
    /// prefix (tools, system prompt). Default true.
    pub prompt_cache_markers: Option<bool>,
}

/// Compat after provider/model overlay, with defaults applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCompat {
    pub max_tokens_field: String,
    pub reasoning_field: Option<String>,
    pub stream_usage: bool,
    pub prompt_cache_markers: bool,
}

impl Compat {
    pub fn resolve(provider: &Compat, model: &Compat) -> ResolvedCompat {
        ResolvedCompat {
            max_tokens_field: model
                .max_tokens_field
                .clone()
                .or_else(|| provider.max_tokens_field.clone())
                .unwrap_or_else(|| "max_tokens".to_string()),
            reasoning_field: model
                .reasoning_field
                .clone()
                .or_else(|| provider.reasoning_field.clone()),
            stream_usage: model.stream_usage.or(provider.stream_usage).unwrap_or(true),
            prompt_cache_markers: model
                .prompt_cache_markers
                .or(provider.prompt_cache_markers)
                .unwrap_or(true),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    /// Base URL, possibly with a version segment (`/v1`) or a gateway prefix.
    pub endpoint: String,
    /// Default protocol of the provider's models.
    pub protocol: Protocol,
    pub auth: AuthMode,
    /// Header carrying the key when `auth = "api_key_header"`.
    #[serde(default)]
    pub auth_header: Option<String>,
    /// Inline key. Prefer `api_key_env`; never commit a file containing this.
    #[serde(default)]
    pub api_key: Option<Secret>,
    /// Name of the environment variable holding the key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Replaces the protocol's relative path for models using the provider's
    /// default protocol.
    #[serde(default)]
    pub path: Option<String>,
    /// Extra static request headers (values are never printed).
    #[serde(default)]
    pub headers: BTreeMap<String, Secret>,
    #[serde(default)]
    pub compat: Compat,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// Local identifier, unique within the provider.
    pub id: String,
    pub name: String,
    /// Exact model identifier sent to the server.
    pub api_model: String,
    #[serde(default)]
    pub protocol: Option<Protocol>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthMode>,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, Secret>,
    #[serde(default)]
    pub compat: Compat,
    #[serde(default = "default_text_only")]
    pub input_modalities: Vec<Modality>,
    #[serde(default = "default_text_only")]
    pub output_modalities: Vec<Modality>,
    /// MIME types accepted as `document` input (required with `document`).
    #[serde(default)]
    pub document_mime_types: Vec<String>,
    /// The server streams responses. Default false (non-streaming request).
    #[serde(default)]
    pub streaming: bool,
    /// The server supports tool calls. Default false.
    #[serde(default)]
    pub tool_calls: bool,
    pub limits: Limits,
    #[serde(default)]
    pub pricing: Option<PricingConfig>,
    #[serde(default)]
    pub reasoning: ReasoningConfig,
    /// Request parameters sent with every request for this model, merged into
    /// the adapter's request (JSON-compatible, nesting allowed).
    #[serde(default)]
    pub parameters: Map<String, Value>,
    /// JSON Pointers removed from the adapter's request *before* `parameters`
    /// are merged — for options a server rejects but the engine sets (for
    /// example `/temperature` on a model that refuses sampling parameters).
    #[serde(default)]
    pub remove_parameters: Vec<String>,
    /// Server-side token counting, used for precise pre-flight checks. Off
    /// unless this table is present. Only protocols with a documented
    /// counting route accept it (`anthropic_messages`, `responses`); nothing
    /// is ever guessed for other servers.
    #[serde(default)]
    pub token_counting: Option<TokenCounting>,
}

/// `[providers.models.token_counting]`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TokenCounting {
    /// Route of the counting endpoint, relative to the endpoint (or an
    /// absolute URL). Default: the protocol's documented route —
    /// `messages/count_tokens` or `responses/input_tokens`.
    #[serde(default)]
    pub path: Option<String>,
}

impl Protocol {
    /// The documented token-counting route of the protocol, if it has one.
    pub fn token_counting_path(self) -> Option<&'static str> {
        match self {
            Protocol::AnthropicMessages => Some("messages/count_tokens"),
            Protocol::Responses => Some("responses/input_tokens"),
            Protocol::ChatCompletions => None,
        }
    }
}

fn default_text_only() -> Vec<Modality> {
    vec![Modality::Text]
}

/// Declared limits. They do not change the server's own limits.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    /// Total window shared by input and output, when the server shares it.
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
}

impl Limits {
    /// Tokens available for the prompt once the output budget (which includes
    /// reasoning on all three protocols) is reserved. With a shared window this
    /// is `min(max_input, window - reserved_output)`.
    pub fn input_budget(&self, reserved_output: u64) -> u64 {
        match self.context_window_tokens {
            Some(window) => self
                .max_input_tokens
                .min(window.saturating_sub(reserved_output)),
            None => self.max_input_tokens,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningConfig {
    /// Profile applied when none is selected. Must name one of `profiles`.
    #[serde(default)]
    pub default: Option<String>,
    /// Ordered, fully user-defined profiles. Empty = no choice is exposed.
    #[serde(default)]
    pub profiles: Vec<ReasoningProfile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningProfile {
    /// Free identifier; never sent to the server by itself.
    pub id: String,
    /// Display label (defaults to the id).
    #[serde(default)]
    pub label: Option<String>,
    /// Request parameters this profile sends.
    #[serde(default)]
    pub parameters: Map<String, Value>,
    /// JSON Pointers removed from the request before `parameters` are applied.
    #[serde(default)]
    pub remove: Vec<String>,
}

impl ReasoningProfile {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}

/// Prices as written in the file. See [`crate::pricing::Pricing`] for the
/// validated runtime form.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingConfig {
    pub currency: String,
    pub per_tokens: u64,
    #[serde(default)]
    pub input: Option<Decimal>,
    #[serde(default)]
    pub output: Option<Decimal>,
    #[serde(default)]
    pub cache_read: Option<Decimal>,
    #[serde(default)]
    pub cache_write: Option<Decimal>,
    /// Cache-write prices by retention variant (for example `"5m"`, `"1h"`).
    #[serde(default)]
    pub cache_write_variants: BTreeMap<String, Decimal>,
    /// Named alternative tariffs (off-peak, volume, …).
    #[serde(default)]
    pub variants: BTreeMap<String, TariffConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TariffConfig {
    #[serde(default)]
    pub input: Option<Decimal>,
    #[serde(default)]
    pub output: Option<Decimal>,
    #[serde(default)]
    pub cache_read: Option<Decimal>,
    #[serde(default)]
    pub cache_write: Option<Decimal>,
    #[serde(default)]
    pub cache_write_variants: BTreeMap<String, Decimal>,
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// A configuration problem, located precisely. Never contains a secret value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// File path, or a label such as `<inline toml>`.
    pub origin: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub field: String,
    pub message: String,
}

impl ConfigError {
    fn new(origin: &str, field: impl Into<String>, message: impl Into<String>) -> Self {
        ConfigError {
            origin: origin.to_string(),
            provider: None,
            model: None,
            field: field.into(),
            message: message.into(),
        }
    }

    fn at_provider(mut self, provider: &str) -> Self {
        self.provider = Some(provider.to_string());
        self
    }

    fn at_model(mut self, model: &str) -> Self {
        self.model = Some(model.to_string());
        self
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.origin)?;
        if let Some(p) = &self.provider {
            write!(f, "provider \"{p}\", ")?;
        }
        if let Some(m) = &self.model {
            write!(f, "model \"{m}\", ")?;
        }
        if self.field.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "field `{}`: {}", self.field, self.message)
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<ConfigError> for cersei_types::CerseiError {
    fn from(e: ConfigError) -> Self {
        cersei_types::CerseiError::Config(e.to_string())
    }
}

// ─── Loading ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Toml,
    Json,
}

impl ProvidersConfig {
    /// Load and validate a configuration file.
    ///
    /// `explicit` replaces the default path entirely — the two are never
    /// merged. The extension selects the format: `.toml` or `.json`; anything
    /// else is rejected.
    pub fn load(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let path = match explicit {
            Some(p) => p.to_path_buf(),
            None => default_config_path().ok_or_else(|| {
                ConfigError::new(
                    "providers config",
                    "",
                    "no home directory found, so the default `~/.bricks/providers.toml` \
                     cannot be located; pass an explicit path",
                )
            })?,
        };
        let origin = path.display().to_string();
        let format = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("toml") => Format::Toml,
            Some("json") => Format::Json,
            other => {
                return Err(ConfigError::new(
                    &origin,
                    "",
                    format!(
                        "unsupported file extension {}; use `.toml` or `.json`",
                        other
                            .map(|e| format!("`.{e}`"))
                            .unwrap_or_else(|| "(none)".into())
                    ),
                ))
            }
        };
        let text = std::fs::read_to_string(&path).map_err(|e| {
            let hint = if e.kind() == std::io::ErrorKind::NotFound {
                "file not found; create it (an annotated example ships in \
                 `docs/providers.example.toml`)"
                    .to_string()
            } else {
                format!("cannot read file: {}", e.kind())
            };
            ConfigError::new(&origin, "", hint)
        })?;
        Self::parse(&text, format, &origin)
    }

    /// Parse and validate TOML text. `origin` labels errors.
    pub fn from_toml_str(text: &str, origin: &str) -> Result<Self, ConfigError> {
        Self::parse(text, Format::Toml, origin)
    }

    /// Parse and validate JSON text. `origin` labels errors.
    pub fn from_json_str(text: &str, origin: &str) -> Result<Self, ConfigError> {
        Self::parse(text, Format::Json, origin)
    }

    fn parse(text: &str, format: Format, origin: &str) -> Result<Self, ConfigError> {
        // Both formats go through the same generic value, so they cannot drift.
        let raw: Value = match format {
            Format::Toml => toml::from_str(text).map_err(|e| {
                // The toml error's Display quotes the offending source line,
                // which may be an `api_key`. Report position and message only.
                let (line, col) = e.span().map(|s| line_col(text, s.start)).unwrap_or((0, 0));
                let at = if line > 0 {
                    format!(" (line {line}, column {col})")
                } else {
                    String::new()
                };
                ConfigError::new(origin, "", format!("invalid TOML{at}: {}", e.message()))
            })?,
            Format::Json => serde_json::from_str(text).map_err(|e| {
                ConfigError::new(
                    origin,
                    "",
                    format!(
                        "invalid JSON (line {}, column {}): {}",
                        e.line(),
                        e.column(),
                        json_syntax_kind(&e)
                    ),
                )
            })?,
        };
        let config: ProvidersConfig = serde_path_to_error::deserialize(&raw)
            .map_err(|e| locate_serde_error(origin, &raw, e))?;
        config.validate(origin)?;
        Ok(config)
    }

    /// Structural validation: uniqueness, ranges, consistency, reserved fields.
    pub fn validate(&self, origin: &str) -> Result<(), ConfigError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ConfigError::new(
                origin,
                "schema_version",
                format!(
                    "unsupported schema_version {}; this build reads version {SCHEMA_VERSION}",
                    self.schema_version
                ),
            ));
        }
        let mut seen = BTreeSet::new();
        for p in &self.providers {
            if !seen.insert(p.id.as_str()) {
                return Err(
                    ConfigError::new(origin, "id", "duplicate provider id").at_provider(&p.id)
                );
            }
            p.validate(origin)?;
        }
        Ok(())
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }
}

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let col = before
        .rsplit('\n')
        .next()
        .map(|l| l.chars().count())
        .unwrap_or(0)
        + 1;
    (line, col)
}

/// serde_json syntax errors never quote input, but keep only the category.
fn json_syntax_kind(e: &serde_json::Error) -> String {
    let s = e.to_string();
    // "expected value at line 1 column 5" -> "expected value"
    s.split(" at line ")
        .next()
        .unwrap_or("syntax error")
        .to_string()
}

/// Turn a path-annotated serde error into a located, secret-free `ConfigError`.
fn locate_serde_error(
    origin: &str,
    raw: &Value,
    err: serde_path_to_error::Error<serde_json::Error>,
) -> ConfigError {
    use serde_path_to_error::Segment;

    let mut provider: Option<String> = None;
    let mut model: Option<String> = None;
    let mut cursor: Option<&Value> = Some(raw);
    let mut field = String::new();
    let mut section = 0; // 0 = top, 1 = inside a provider, 2 = inside a model
    let mut pending_key: Option<String> = None;

    for seg in err.path().iter() {
        match seg {
            Segment::Map { key } => {
                cursor = cursor.and_then(|c| c.get(key.as_str()));
                pending_key = Some(key.clone());
                if !(section == 0 && key == "providers") && !(section == 1 && key == "models") {
                    if !field.is_empty() {
                        field.push('.');
                    }
                    field.push_str(key);
                }
            }
            Segment::Seq { index } => {
                cursor = cursor.and_then(|c| c.get(*index));
                match (section, pending_key.as_deref()) {
                    (0, Some("providers")) => {
                        section = 1;
                        provider = cursor
                            .and_then(|c| c.get("id"))
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        if provider.is_none() {
                            provider = Some(format!("#{index}"));
                        }
                    }
                    (1, Some("models")) => {
                        section = 2;
                        model = cursor
                            .and_then(|c| c.get("id"))
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        if model.is_none() {
                            model = Some(format!("#{index}"));
                        }
                    }
                    _ => field.push_str(&format!("[{index}]")),
                }
                pending_key = None;
            }
            Segment::Enum { variant } => {
                if !field.is_empty() {
                    field.push('.');
                }
                field.push_str(variant);
            }
            Segment::Unknown => {}
        }
    }

    let message = sanitize_serde_message(&err.inner().to_string());
    // serde reports these on the parent struct; name the child field.
    let (field, message) = if let Some(name) = backticked_after(&message, "missing field ") {
        (
            join_field(&field, &name),
            "required field is missing".to_string(),
        )
    } else if let Some(name) = backticked_after(&message, "unknown field ") {
        // For an unknown key serde's path already ends with that key.
        let located = if field == name || field.ends_with(&format!(".{name}")) {
            field
        } else {
            join_field(&field, &name)
        };
        (
            located,
            "unknown field (check the spelling against the schema)".to_string(),
        )
    } else {
        (field, message)
    };

    ConfigError {
        origin: origin.to_string(),
        provider,
        model,
        field,
        message,
    }
}

fn join_field(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

fn backticked_after(message: &str, prefix: &str) -> Option<String> {
    let rest = message.strip_prefix(prefix)?;
    let rest = rest.strip_prefix('`')?;
    Some(rest.split('`').next()?.to_string())
}

/// serde's `invalid type`/`invalid value` messages quote the offending value
/// (`invalid type: string "sk-…", expected u64`). A mistyped secret must not be
/// echoed, so cut everything between the kind and `, expected`.
fn sanitize_serde_message(message: &str) -> String {
    for prefix in ["invalid type: ", "invalid value: "] {
        if let Some(rest) = message.strip_prefix(prefix) {
            let kind = rest.split([' ', ',']).next().unwrap_or("value");
            let expected = rest.find(", expected").map(|i| &rest[i..]).unwrap_or("");
            return format!("wrong type (got {kind}){expected}");
        }
    }
    message.to_string()
}

// ─── Validation ──────────────────────────────────────────────────────────────

fn valid_selector_part(s: &str, allow_slash: bool) -> bool {
    !s.is_empty()
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && (allow_slash || !s.contains('/'))
}

fn valid_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn valid_mime(s: &str) -> bool {
    match s.split_once('/') {
        Some((a, b)) => {
            !a.is_empty()
                && !b.is_empty()
                && !s.chars().any(|c| c.is_whitespace() || c.is_control())
                && !b.contains('/')
        }
        None => false,
    }
}

/// Validate a JSON Pointer used by a reasoning profile's `remove`.
fn valid_pointer(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Err("an empty pointer would remove the whole request".into());
    }
    if !p.starts_with('/') {
        return Err("a JSON Pointer must start with `/`".into());
    }
    let mut chars = p.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.peek(), Some('0') | Some('1')) {
            return Err("`~` must be escaped as `~0` or `~1`".into());
        }
    }
    Ok(())
}

/// First segment of a JSON Pointer, unescaped.
pub(crate) fn pointer_root(p: &str) -> &str {
    p.trim_start_matches('/').split('/').next().unwrap_or("")
}

impl ProviderConfig {
    fn validate(&self, origin: &str) -> Result<(), ConfigError> {
        let e =
            |field: &str, msg: String| ConfigError::new(origin, field, msg).at_provider(&self.id);

        if !valid_selector_part(&self.id, false) {
            return Err(e(
                "id",
                "must be non-empty, without whitespace or `/` (selection is `provider_id/model_id`)"
                    .into(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(e("name", "must not be empty".into()));
        }
        validate_endpoint(&self.endpoint).map_err(|m| e("endpoint", m))?;
        validate_auth(
            self.auth,
            self.auth_header.as_deref(),
            self.api_key.as_ref(),
            self.api_key_env.as_deref(),
        )
        .map_err(|(f, m)| e(f, m))?;
        validate_headers(&self.headers).map_err(|(f, m)| e(&f, m))?;
        if let Some(path) = &self.path {
            crate::endpoint::validate_path_override(path).map_err(|m| e("path", m))?;
        }
        validate_compat(&self.compat).map_err(|(f, m)| e(&format!("compat.{f}"), m))?;

        let mut seen = BTreeSet::new();
        for m in &self.models {
            if !seen.insert(m.id.as_str()) {
                return Err(ConfigError::new(
                    origin,
                    "id",
                    "duplicate model id within this provider",
                )
                .at_provider(&self.id)
                .at_model(&m.id));
            }
            m.validate(origin, self)?;
        }
        Ok(())
    }
}

fn validate_endpoint(endpoint: &str) -> Result<(), String> {
    let url = url::Url::parse(endpoint).map_err(|_| "not a valid URL".to_string())?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("scheme must be http or https".into());
    }
    if url.host_str().is_none() {
        return Err("URL has no host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("must not embed credentials; use api_key_env".into());
    }
    Ok(())
}

fn validate_auth(
    auth: AuthMode,
    auth_header: Option<&str>,
    api_key: Option<&Secret>,
    api_key_env: Option<&str>,
) -> Result<(), (&'static str, String)> {
    if let Some(h) = auth_header {
        if auth != AuthMode::ApiKeyHeader {
            return Err((
                "auth_header",
                "only valid with auth = \"api_key_header\"".into(),
            ));
        }
        reqwest::header::HeaderName::from_bytes(h.as_bytes())
            .map_err(|_| ("auth_header", "not a valid HTTP header name".to_string()))?;
    }
    match auth {
        AuthMode::None => {
            if api_key.is_some() || api_key_env.is_some() {
                return Err((
                    "auth",
                    "auth = \"none\" cannot be combined with api_key or api_key_env".into(),
                ));
            }
        }
        AuthMode::Bearer | AuthMode::ApiKeyHeader => match (api_key, api_key_env) {
            (Some(_), Some(_)) => {
                return Err((
                    "api_key",
                    "api_key and api_key_env are mutually exclusive".into(),
                ))
            }
            (None, None) => {
                return Err((
                    "api_key_env",
                    format!(
                    "auth = \"{}\" needs a key source: set api_key_env (recommended) or api_key",
                    auth.as_str()
                ),
                ))
            }
            (Some(k), None) => {
                if k.is_blank() {
                    return Err(("api_key", "must not be empty".into()));
                }
            }
            (None, Some(name)) => {
                if !valid_env_name(name) {
                    return Err((
                        "api_key_env",
                        "must be an environment variable name (letters, digits, `_`)".into(),
                    ));
                }
            }
        },
    }
    Ok(())
}

fn validate_headers(headers: &BTreeMap<String, Secret>) -> Result<(), (String, String)> {
    for (name, value) in headers {
        let field = format!("headers.{name}");
        if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
            return Err((field, "not a valid HTTP header name".into()));
        }
        if reqwest::header::HeaderValue::from_str(value.expose()).is_err() {
            return Err((field, "not a valid HTTP header value".into()));
        }
    }
    Ok(())
}

fn validate_compat(c: &Compat) -> Result<(), (&'static str, String)> {
    if let Some(f) = &c.max_tokens_field {
        if f.trim().is_empty() {
            return Err(("max_tokens_field", "must not be empty".into()));
        }
    }
    if let Some(f) = &c.reasoning_field {
        if f.trim().is_empty() {
            return Err(("reasoning_field", "must not be empty".into()));
        }
    }
    Ok(())
}

impl ModelConfig {
    /// The protocol this model actually speaks.
    pub fn effective_protocol(&self, provider: &ProviderConfig) -> Protocol {
        self.protocol.unwrap_or(provider.protocol)
    }

    fn validate(&self, origin: &str, provider: &ProviderConfig) -> Result<(), ConfigError> {
        let e = |field: &str, msg: String| {
            ConfigError::new(origin, field, msg)
                .at_provider(&provider.id)
                .at_model(&self.id)
        };

        if !valid_selector_part(&self.id, true) {
            return Err(e("id", "must be non-empty and without whitespace".into()));
        }
        if self.name.trim().is_empty() {
            return Err(e("name", "must not be empty".into()));
        }
        if self.api_model.trim().is_empty() {
            return Err(e("api_model", "must not be empty".into()));
        }
        if let Some(ep) = &self.endpoint {
            validate_endpoint(ep).map_err(|m| e("endpoint", m))?;
        }
        if let Some(path) = &self.path {
            crate::endpoint::validate_path_override(path).map_err(|m| e("path", m))?;
        }
        validate_headers(&self.headers).map_err(|(f, m)| e(&f, m))?;
        validate_compat(&self.compat).map_err(|(f, m)| e(&format!("compat.{f}"), m))?;

        // Model-level auth overrides: they select how the provider's key is
        // attached; they cannot introduce a key of their own.
        let effective_auth = self.effective_auth(provider);
        if let Some(h) = &self.auth_header {
            if effective_auth != AuthMode::ApiKeyHeader {
                return Err(e(
                    "auth_header",
                    "only valid with auth = \"api_key_header\"".into(),
                ));
            }
            reqwest::header::HeaderName::from_bytes(h.as_bytes())
                .map_err(|_| e("auth_header", "not a valid HTTP header name".into()))?;
        }
        if provider.auth == AuthMode::None && effective_auth != AuthMode::None {
            return Err(e(
                "auth",
                "the provider has no key (auth = \"none\"); a model cannot require one".into(),
            ));
        }

        // Modalities.
        for (field, list) in [
            ("input_modalities", &self.input_modalities),
            ("output_modalities", &self.output_modalities),
        ] {
            if list.is_empty() {
                return Err(e(field, "must list at least one modality".into()));
            }
            let mut seen = BTreeSet::new();
            for m in list.iter() {
                if !seen.insert(*m) {
                    return Err(e(field, format!("modality `{m}` listed twice")));
                }
            }
        }
        let has_document = self.input_modalities.contains(&Modality::Document);
        if has_document && self.document_mime_types.is_empty() {
            return Err(e(
                "document_mime_types",
                "required when `document` is an input modality (list the accepted MIME types)"
                    .into(),
            ));
        }
        if !has_document && !self.document_mime_types.is_empty() {
            return Err(e(
                "document_mime_types",
                "set but `document` is not an input modality".into(),
            ));
        }
        for mime in &self.document_mime_types {
            if !valid_mime(mime) {
                return Err(e(
                    "document_mime_types",
                    "not a MIME type (`type/subtype`)".into(),
                ));
            }
        }

        // Limits.
        let l = &self.limits;
        if l.max_input_tokens == 0 {
            return Err(e(
                "limits.max_input_tokens",
                "must be greater than 0".into(),
            ));
        }
        if l.max_output_tokens == 0 {
            return Err(e(
                "limits.max_output_tokens",
                "must be greater than 0".into(),
            ));
        }
        if let Some(w) = l.context_window_tokens {
            if w < l.max_output_tokens {
                return Err(e(
                    "limits.context_window_tokens",
                    "must be at least max_output_tokens".into(),
                ));
            }
            if w < l.max_input_tokens {
                return Err(e(
                    "limits.context_window_tokens",
                    "must be at least max_input_tokens".into(),
                ));
            }
        }

        // Token counting.
        if let Some(tc) = &self.token_counting {
            let protocol = self.effective_protocol(provider);
            if protocol.token_counting_path().is_none() && tc.path.is_none() {
                return Err(e(
                    "token_counting",
                    format!(
                        "the `{protocol}` protocol has no standard token-counting endpoint; \
                         remove this table (a local estimate is used instead)"
                    ),
                ));
            }
            if protocol.token_counting_path().is_none() {
                return Err(e(
                    "token_counting.path",
                    format!(
                        "counting is only supported for protocols with a documented counting \
                         request (anthropic_messages, responses), not `{protocol}`"
                    ),
                ));
            }
            if let Some(path) = &tc.path {
                crate::endpoint::validate_path_override(path)
                    .map_err(|m| e("token_counting.path", m))?;
            }
        }

        // Pricing.
        if let Some(p) = &self.pricing {
            crate::pricing::Pricing::from_config(p)
                .map_err(|(f, m)| e(&format!("pricing.{f}"), m))?;
        }

        // Parameters and reasoning profiles.
        let protocol = self.effective_protocol(provider);
        let protected = protocol.protected_fields();
        if let Some(f) = &self.compat.max_tokens_field {
            if protected.contains(&f.as_str()) {
                return Err(e(
                    "compat.max_tokens_field",
                    format!("`{f}` is a reserved field"),
                ));
            }
        }
        for key in self.parameters.keys() {
            if protected.contains(&key.as_str()) {
                return Err(e(
                    &format!("parameters.{key}"),
                    format!(
                        "`{key}` is a structural field of the `{protocol}` protocol and cannot be \
                         set by configuration"
                    ),
                ));
            }
        }
        for ptr in &self.remove_parameters {
            valid_pointer(ptr).map_err(|m| e("remove_parameters", m))?;
            if protected.contains(&pointer_root(ptr)) {
                return Err(e(
                    "remove_parameters",
                    format!("`{ptr}` targets a structural field of the `{protocol}` protocol"),
                ));
            }
        }
        let mut profile_ids = BTreeSet::new();
        for (i, prof) in self.reasoning.profiles.iter().enumerate() {
            let at = |f: &str| format!("reasoning.profiles[{i}].{f}");
            if prof.id.trim().is_empty() {
                return Err(e(&at("id"), "must not be empty".into()));
            }
            if !profile_ids.insert(prof.id.as_str()) {
                return Err(e(&at("id"), format!("duplicate profile id `{}`", prof.id)));
            }
            for key in prof.parameters.keys() {
                if protected.contains(&key.as_str()) {
                    return Err(e(
                        &at(&format!("parameters.{key}")),
                        format!("`{key}` is a structural field of the `{protocol}` protocol"),
                    ));
                }
            }
            for ptr in &prof.remove {
                valid_pointer(ptr).map_err(|m| e(&at("remove"), m))?;
                if protected.contains(&pointer_root(ptr)) {
                    return Err(e(
                        &at("remove"),
                        format!("`{ptr}` targets a structural field of the `{protocol}` protocol"),
                    ));
                }
            }
        }
        if let Some(def) = &self.reasoning.default {
            if !profile_ids.contains(def.as_str()) {
                return Err(e(
                    "reasoning.default",
                    format!("`{def}` is not one of this model's reasoning profiles"),
                ));
            }
        }

        // The endpoint must assemble.
        crate::endpoint::assemble(
            self.endpoint.as_deref().unwrap_or(&provider.endpoint),
            protocol,
            self.effective_path_override(provider, protocol),
        )
        .map_err(|m| e("endpoint", m))?;
        Ok(())
    }

    /// The auth mode in force for this model (see [`Protocol::conventional_auth`]).
    pub fn effective_auth(&self, provider: &ProviderConfig) -> AuthMode {
        if let Some(a) = self.auth {
            return a;
        }
        if provider.auth == AuthMode::None {
            return AuthMode::None;
        }
        match self.protocol {
            Some(p) if p != provider.protocol => p.conventional_auth(),
            _ => provider.auth,
        }
    }

    /// The relative-path override in force: the model's own, or the provider's
    /// when the model speaks the provider's default protocol.
    pub fn effective_path_override<'a>(
        &'a self,
        provider: &'a ProviderConfig,
        protocol: Protocol,
    ) -> Option<&'a str> {
        self.path.as_deref().or_else(|| {
            if protocol == provider.protocol {
                provider.path.as_deref()
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub(crate) const BASIC: &str = r#"
schema_version = 1

[[providers]]
id = "custom"
name = "Mon fournisseur"
endpoint = "https://api.example.com/v1"
protocol = "chat_completions"
auth = "bearer"
api_key_env = "BRICKS_CUSTOM_API_KEY"

[[providers.models]]
id = "flash"
name = "Mon modèle Flash"
api_model = "vendor-flash"
input_modalities = ["text", "image"]
output_modalities = ["text"]
streaming = true
tool_calls = true

[providers.models.limits]
max_input_tokens = 96000
max_output_tokens = 32000
context_window_tokens = 128000

[providers.models.pricing]
currency = "USD"
per_tokens = 1000000
input = "0.30"
output = "1.20"
cache_read = "0.03"
cache_write = "0.40"

[providers.models.reasoning]
default = "approfondi"

[[providers.models.reasoning.profiles]]
id = "rapide"
label = "Rapide"
parameters = { reasoning_effort = "low" }

[[providers.models.reasoning.profiles]]
id = "approfondi"
label = "Approfondi"
parameters = { reasoning_effort = "high" }

[[providers.models]]
id = "expert"
name = "Mon modèle Expert"
api_model = "vendor-expert"
protocol = "responses"
input_modalities = ["text"]
output_modalities = ["text"]
streaming = true
tool_calls = true

[providers.models.limits]
max_input_tokens = 64000
max_output_tokens = 16000

[providers.models.reasoning]
profiles = []
"#;
}

#[cfg(test)]
mod tests {
    use super::tests_support::BASIC;
    use super::*;

    fn err(text: &str) -> ConfigError {
        ProvidersConfig::from_toml_str(text, "test.toml").unwrap_err()
    }

    #[test]
    fn contract_example_loads() {
        let cfg = ProvidersConfig::from_toml_str(BASIC, "test.toml").unwrap();
        assert_eq!(cfg.providers.len(), 1);
        let p = &cfg.providers[0];
        assert_eq!(p.models.len(), 2);
        assert_eq!(p.models[0].reasoning.profiles.len(), 2);
        assert!(p.models[1].reasoning.profiles.is_empty());
        assert_eq!(p.models[1].effective_protocol(p), Protocol::Responses);
        // Defaults apply to omitted booleans/lists.
        assert_eq!(p.models[1].input_modalities, vec![Modality::Text]);
    }

    #[test]
    fn toml_and_json_are_equivalent() {
        let toml_cfg = ProvidersConfig::from_toml_str(BASIC, "a.toml").unwrap();
        let as_value: Value = toml::from_str(BASIC).unwrap();
        let json_text = serde_json::to_string(&as_value).unwrap();
        let json_cfg = ProvidersConfig::from_json_str(&json_text, "a.json").unwrap();
        assert_eq!(format!("{toml_cfg:?}"), format!("{json_cfg:?}"));
    }

    #[test]
    fn default_path_is_under_the_home_directory() {
        if let Some(p) = default_config_path() {
            assert!(p.ends_with(".bricks/providers.toml"), "{}", p.display());
        }
    }

    #[test]
    fn unknown_extension_is_rejected_and_explicit_path_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = dir.path().join("providers.yaml");
        std::fs::write(&yaml, "x").unwrap();
        let e = ProvidersConfig::load(Some(&yaml)).unwrap_err();
        assert!(e.message.contains("unsupported file extension"), "{e}");

        let json = dir.path().join("p.json");
        let value: Value = toml::from_str(BASIC).unwrap();
        std::fs::write(&json, serde_json::to_string(&value).unwrap()).unwrap();
        assert!(ProvidersConfig::load(Some(&json)).is_ok());

        let missing = dir.path().join("missing.toml");
        let e = ProvidersConfig::load(Some(&missing)).unwrap_err();
        assert!(e.message.contains("file not found"), "{e}");
    }

    #[test]
    fn errors_name_provider_model_and_field() {
        let text = BASIC.replace("max_input_tokens = 96000", "max_input_tokens = 0");
        let e = err(&text);
        assert_eq!(e.provider.as_deref(), Some("custom"));
        assert_eq!(e.model.as_deref(), Some("flash"));
        assert_eq!(e.field, "limits.max_input_tokens");
        let shown = e.to_string();
        assert!(shown.contains("provider \"custom\"") && shown.contains("model \"flash\""));
    }

    #[test]
    fn missing_and_unknown_fields_are_located() {
        let e = err(&BASIC.replace("api_model = \"vendor-flash\"\n", ""));
        assert_eq!(
            (e.model.as_deref(), e.field.as_str()),
            (Some("flash"), "api_model")
        );
        assert_eq!(e.message, "required field is missing");

        let e = err(&BASIC.replace("streaming = true\ntool_calls = true\n\n[providers.models.limits]\nmax_input_tokens = 96000", "streamin = true\ntool_calls = true\n\n[providers.models.limits]\nmax_input_tokens = 96000"));
        assert_eq!(e.field, "streamin");
        assert!(e.message.contains("unknown field"));
    }

    #[test]
    fn duplicates_are_rejected() {
        let dup_model = BASIC.replace("id = \"expert\"", "id = \"flash\"");
        let e = err(&dup_model);
        assert_eq!(e.field, "id");
        assert!(e.message.contains("duplicate model id"));

        let two_providers = format!(
            "{BASIC}\n{}",
            BASIC
                .split_once("[[providers]]")
                .unwrap()
                .1
                .split_once("[[providers.models]]")
                .map(|(h, _)| format!("[[providers]]{h}"))
                .unwrap()
        );
        let e = err(&two_providers);
        assert!(e.message.contains("duplicate provider id"), "{e}");
    }

    #[test]
    fn schema_version_is_checked() {
        let e = err(&BASIC.replace("schema_version = 1", "schema_version = 2"));
        assert_eq!(e.field, "schema_version");
        let e = err(&BASIC.replace("schema_version = 1\n", ""));
        assert_eq!(e.field, "schema_version");
    }

    #[test]
    fn auth_rules() {
        // Both key sources.
        let both = BASIC.replace(
            "api_key_env = \"BRICKS_CUSTOM_API_KEY\"",
            "api_key_env = \"X\"\napi_key = \"k\"",
        );
        assert!(err(&both).message.contains("mutually exclusive"));
        // None with a key.
        let none = BASIC.replace("auth = \"bearer\"", "auth = \"none\"");
        assert!(err(&none).message.contains("cannot be combined"));
        // Bearer without a key.
        let nokey = BASIC.replace("api_key_env = \"BRICKS_CUSTOM_API_KEY\"\n", "");
        assert!(err(&nokey).message.contains("needs a key source"));
        // Bad env name.
        let bad = BASIC.replace("BRICKS_CUSTOM_API_KEY", "not a name");
        assert_eq!(err(&bad).field, "api_key_env");
        // auth_header only with api_key_header.
        let hdr = BASIC.replace(
            "auth = \"bearer\"",
            "auth = \"bearer\"\nauth_header = \"x-key\"",
        );
        assert_eq!(err(&hdr).field, "auth_header");
        // Credentials in the URL.
        let url = BASIC.replace(
            "https://api.example.com/v1",
            "https://user:pw@api.example.com/v1",
        );
        let e = err(&url);
        assert_eq!(e.field, "endpoint");
        assert!(!e.to_string().contains("pw"));
    }

    #[test]
    fn secrets_never_appear_in_debug_or_errors() {
        let key = "sk-super-secret-1234567890";
        let text = BASIC
            .replace(
                "api_key_env = \"BRICKS_CUSTOM_API_KEY\"",
                &format!("api_key = \"{key}\""),
            )
            .replace("max_input_tokens = 96000", "max_input_tokens = 0");
        let e = err(&text);
        assert!(!e.to_string().contains(key));

        let ok = BASIC.replace(
            "api_key_env = \"BRICKS_CUSTOM_API_KEY\"",
            &format!("api_key = \"{key}\""),
        );
        let cfg = ProvidersConfig::from_toml_str(&ok, "t").unwrap();
        assert!(!format!("{cfg:?}").contains(key));
        assert!(format!("{cfg:?}").contains("redacted"));

        // A mistyped secret is not echoed back by the type error.
        let wrong = BASIC.replace(
            "api_key_env = \"BRICKS_CUSTOM_API_KEY\"",
            "api_key = 123456789",
        );
        let e = err(&wrong);
        assert!(!e.to_string().contains("123456789"), "{e}");

        // A TOML syntax error on a line holding a key must not quote the line.
        let broken = format!("{BASIC}\napi_key = \"{key}\" garbage\n");
        let e = err(&broken);
        assert!(!e.to_string().contains(key), "{e}");
        assert!(e.message.starts_with("invalid TOML"), "{e}");

        // Header values are secrets too.
        let h = BASIC.replace(
            "auth = \"bearer\"\n",
            "auth = \"bearer\"\nheaders = { \"x-token\" = \"hdr-secret-value\" }\n",
        );
        let cfg = ProvidersConfig::from_toml_str(&h, "t").unwrap();
        assert!(!format!("{cfg:?}").contains("hdr-secret-value"));
    }

    #[test]
    fn modality_and_document_rules() {
        let doc = BASIC.replace(
            "input_modalities = [\"text\", \"image\"]",
            "input_modalities = [\"text\", \"document\"]",
        );
        assert_eq!(err(&doc).field, "document_mime_types");
        let ok = doc.replace("streaming = true\ntool_calls = true\n\n[providers.models.limits]\nmax_input_tokens = 96000",
            "document_mime_types = [\"application/pdf\"]\nstreaming = true\ntool_calls = true\n\n[providers.models.limits]\nmax_input_tokens = 96000");
        ProvidersConfig::from_toml_str(&ok, "t").unwrap();
        let dup = BASIC.replace("[\"text\", \"image\"]", "[\"text\", \"text\"]");
        assert!(err(&dup).message.contains("listed twice"));
        let unknown = BASIC.replace("[\"text\", \"image\"]", "[\"text\", \"smell\"]");
        assert!(err(&unknown).field.starts_with("input_modalities"));
    }

    #[test]
    fn limits_rules() {
        let small_window = BASIC.replace(
            "context_window_tokens = 128000",
            "context_window_tokens = 1000",
        );
        assert_eq!(err(&small_window).field, "limits.context_window_tokens");
        let l = Limits {
            max_input_tokens: 96000,
            max_output_tokens: 32000,
            context_window_tokens: Some(128000),
        };
        assert_eq!(l.input_budget(32000), 96000);
        assert_eq!(
            l.input_budget(64000),
            64000,
            "a larger output reserve shrinks the prompt budget"
        );
        let unshared = Limits {
            max_input_tokens: 5,
            max_output_tokens: 1,
            context_window_tokens: None,
        };
        assert_eq!(unshared.input_budget(1), 5);
    }

    #[test]
    fn reasoning_rules() {
        let e = err(&BASIC.replace("default = \"approfondi\"", "default = \"ultra\""));
        assert_eq!(e.field, "reasoning.default");
        let dup = BASIC.replace("id = \"rapide\"", "id = \"approfondi\"");
        assert!(err(&dup).message.contains("duplicate profile id"));
        // A default with no profiles at all.
        let none = BASIC.replace(
            "[providers.models.reasoning]\nprofiles = []",
            "[providers.models.reasoning]\ndefault = \"x\"\nprofiles = []",
        );
        assert_eq!(err(&none).field, "reasoning.default");
    }

    #[test]
    fn structural_fields_are_protected_at_load() {
        let model_param = BASIC.replace(
            "[providers.models.reasoning]\ndefault = \"approfondi\"",
            "[providers.models.parameters]\nmessages = []\n\n[providers.models.reasoning]\ndefault = \"approfondi\"",
        );
        assert_eq!(err(&model_param).field, "parameters.messages");
        let prof = BASIC.replace(
            "parameters = { reasoning_effort = \"low\" }",
            "parameters = { tools = [] }",
        );
        assert!(err(&prof).field.contains("parameters.tools"));
        let rm = BASIC.replace(
            "parameters = { reasoning_effort = \"low\" }",
            "remove = [\"/messages/0\"]",
        );
        assert!(err(&rm).field.ends_with(".remove"));
        let rp = BASIC.replacen(
            "tool_calls = true\n",
            "tool_calls = true\nremove_parameters = [\"/messages\"]\n",
            1,
        );
        assert_eq!(err(&rp).field, "remove_parameters");
        let bad_ptr = BASIC.replace(
            "parameters = { reasoning_effort = \"low\" }",
            "remove = [\"temperature\"]",
        );
        assert!(err(&bad_ptr).message.contains("must start with"));
        // `input` is structural for responses, not for chat_completions.
        let resp_input = BASIC.replace(
            "[providers.models.reasoning]\nprofiles = []",
            "[providers.models.parameters]\ninput = []\n\n[providers.models.reasoning]\nprofiles = []",
        );
        assert_eq!(err(&resp_input).field, "parameters.input");
    }

    #[test]
    fn pricing_rules() {
        let e = err(&BASIC.replace("currency = \"USD\"", "currency = \"EUR\""));
        assert_eq!(e.field, "pricing.currency");
        let e = err(&BASIC.replace("per_tokens = 1000000", "per_tokens = 1000"));
        assert_eq!(e.field, "pricing.per_tokens");
        let e = err(&BASIC.replace("input = \"0.30\"", "input = \"-1\""));
        assert!(e.field.starts_with("pricing"), "{e}");
    }

    #[test]
    fn selector_parts_are_validated() {
        assert_eq!(
            err(&BASIC.replace("id = \"custom\"", "id = \"a/b\"")).field,
            "id"
        );
        assert_eq!(
            err(&BASIC.replace("id = \"custom\"", "id = \"\"")).field,
            "id"
        );
        // Model ids may contain `/` (selection splits at the first one only).
        ProvidersConfig::from_toml_str(
            &BASIC.replace("id = \"flash\"", "id = \"org/flash:7b\""),
            "t",
        )
        .unwrap();
    }

    #[test]
    fn effective_auth_follows_protocol_override() {
        let cfg = ProvidersConfig::from_toml_str(BASIC, "t").unwrap();
        let p = &cfg.providers[0];
        assert_eq!(p.models[0].effective_auth(p), AuthMode::Bearer);
        // `expert` overrides to responses: still bearer by convention.
        assert_eq!(p.models[1].effective_auth(p), AuthMode::Bearer);
        let anth = BASIC.replace(
            "protocol = \"responses\"",
            "protocol = \"anthropic_messages\"",
        );
        let cfg = ProvidersConfig::from_toml_str(&anth, "t").unwrap();
        let p = &cfg.providers[0];
        assert_eq!(p.models[1].effective_auth(p), AuthMode::ApiKeyHeader);
        let explicit = anth.replace(
            "protocol = \"anthropic_messages\"",
            "protocol = \"anthropic_messages\"\nauth = \"bearer\"",
        );
        let cfg = ProvidersConfig::from_toml_str(&explicit, "t").unwrap();
        let p = &cfg.providers[0];
        assert_eq!(p.models[1].effective_auth(p), AuthMode::Bearer);
    }

    #[test]
    fn compat_overlay() {
        let provider = Compat {
            max_tokens_field: Some("max_completion_tokens".into()),
            ..Default::default()
        };
        let model = Compat {
            stream_usage: Some(false),
            ..Default::default()
        };
        let r = Compat::resolve(&provider, &model);
        assert_eq!(r.max_tokens_field, "max_completion_tokens");
        assert!(!r.stream_usage && r.prompt_cache_markers);
        assert_eq!(
            Compat::resolve(&Compat::default(), &Compat::default()).max_tokens_field,
            "max_tokens"
        );
    }
}
