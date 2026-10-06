//! The provider registry: resolves `provider_id/model_id` against the loaded
//! configuration. The configuration file is the only catalogue — nothing is
//! detected from model names and nothing is built in.

use crate::config::{
    AuthMode, Compat, ConfigError, Limits, ModelConfig, Protocol, ProviderConfig, ProvidersConfig,
    ReasoningConfig, ResolvedCompat, Secret, DEFAULT_API_KEY_HEADER,
};
use crate::modality::Capabilities;
use crate::pricing::Pricing;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use url::Url;

/// A loaded, validated configuration.
#[derive(Debug, Clone)]
pub struct ProviderRegistry {
    origin: String,
    config: Arc<ProvidersConfig>,
}

/// One selectable model, for listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
    pub model_name: String,
}

impl ModelRef {
    /// The `provider_id/model_id` string that selects it.
    pub fn selection(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

impl ProviderRegistry {
    /// Load from `explicit`, or from `~/.bricks/providers.toml` when `None`.
    /// The two are never merged. A missing or invalid file is an error: there
    /// is no fallback catalogue.
    pub fn load(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let config = ProvidersConfig::load(explicit)?;
        let origin = explicit
            .map(|p| p.display().to_string())
            .or_else(|| crate::config::default_config_path().map(|p| p.display().to_string()))
            .unwrap_or_else(|| "providers config".into());
        Ok(Self {
            origin,
            config: Arc::new(config),
        })
    }

    /// Wrap an already-parsed configuration (it is validated again).
    pub fn from_config(config: ProvidersConfig) -> Result<Self, ConfigError> {
        config.validate("providers config")?;
        Ok(Self {
            origin: "providers config".into(),
            config: Arc::new(config),
        })
    }

    /// Parse TOML text (tests, embedded configurations).
    pub fn from_toml_str(text: &str, origin: &str) -> Result<Self, ConfigError> {
        Ok(Self {
            origin: origin.to_string(),
            config: Arc::new(ProvidersConfig::from_toml_str(text, origin)?),
        })
    }

    pub fn config(&self) -> &ProvidersConfig {
        &self.config
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Every model of every provider, in file order. No cap is applied.
    pub fn models(&self) -> Vec<ModelRef> {
        self.config
            .providers
            .iter()
            .flat_map(|p| {
                p.models.iter().map(move |m| ModelRef {
                    provider_id: p.id.clone(),
                    provider_name: p.name.clone(),
                    model_id: m.id.clone(),
                    model_name: m.name.clone(),
                })
            })
            .collect()
    }

    fn selection_error(&self, message: String) -> ConfigError {
        ConfigError {
            origin: self.origin.clone(),
            provider: None,
            model: None,
            field: "selection".into(),
            message,
        }
    }

    /// Resolve `provider_id/model_id`. The provider is never inferred from the
    /// model name; the string is split at the first `/` only, so model ids may
    /// themselves contain `/`.
    pub fn resolve(&self, selection: &str) -> Result<ResolvedModel, ConfigError> {
        let Some((provider_id, model_id)) = selection.split_once('/') else {
            return Err(self.selection_error(format!(
                "`{selection}` is not `provider_id/model_id`; the provider is never guessed from \
                 the model name"
            )));
        };
        let provider = self.config.provider(provider_id).ok_or_else(|| {
            let known = self
                .config
                .providers
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>();
            self.selection_error(format!(
                "unknown provider `{provider_id}`; configured providers: {}",
                if known.is_empty() {
                    "(none)".to_string()
                } else {
                    known.join(", ")
                }
            ))
        })?;
        let model = provider
            .models
            .iter()
            .find(|m| m.id == model_id)
            .ok_or_else(|| {
                let known = provider
                    .models
                    .iter()
                    .map(|m| m.id.as_str())
                    .collect::<Vec<_>>();
                self.selection_error(format!(
                    "provider `{provider_id}` has no model `{model_id}`; its models: {}",
                    if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    }
                ))
            })?;
        ResolvedModel::new(&self.origin, provider, model)
    }
}

/// Where the API key comes from. Never printed.
#[derive(Clone)]
pub(crate) enum KeySource {
    Inline(Secret),
    Env(String),
    None,
}

/// A model with every inheritance and override applied.
#[derive(Clone)]
pub struct ResolvedModel {
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
    pub model_name: String,
    /// Exact model identifier sent to the server.
    pub api_model: String,
    /// The protocol this model speaks (provider default, or its override).
    pub protocol: Protocol,
    /// Final request URL.
    pub url: Url,
    /// Token-counting URL, only when `token_counting` is configured.
    pub count_url: Option<Url>,
    pub auth: AuthMode,
    pub auth_header: String,
    pub(crate) key_source: KeySource,
    /// Required protocol headers, overridden by provider then model headers.
    pub(crate) headers: BTreeMap<String, Secret>,
    pub compat: ResolvedCompat,
    pub limits: Limits,
    pub declared: Capabilities,
    pub parameters: Map<String, Value>,
    pub remove_parameters: Vec<String>,
    pub reasoning: ReasoningConfig,
    pub pricing: Option<Pricing>,
}

impl std::fmt::Debug for ResolvedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedModel")
            .field("selection", &self.selection())
            .field("api_model", &self.api_model)
            .field("protocol", &self.protocol)
            .field("url", &self.url.as_str())
            .field("auth", &self.auth)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl ResolvedModel {
    pub(crate) fn new(
        origin: &str,
        provider: &ProviderConfig,
        model: &ModelConfig,
    ) -> Result<Self, ConfigError> {
        let protocol = model.effective_protocol(provider);
        let url = crate::endpoint::assemble(
            model.endpoint.as_deref().unwrap_or(&provider.endpoint),
            protocol,
            model.effective_path_override(provider, protocol),
        )
        .map_err(|message| ConfigError {
            origin: origin.to_string(),
            provider: Some(provider.id.clone()),
            model: Some(model.id.clone()),
            field: "endpoint".into(),
            message,
        })?;

        let count_url = match &model.token_counting {
            Some(tc) => Some(
                crate::endpoint::assemble(
                    model.endpoint.as_deref().unwrap_or(&provider.endpoint),
                    protocol,
                    Some(
                        tc.path
                            .as_deref()
                            .or(protocol.token_counting_path())
                            .unwrap_or(""),
                    ),
                )
                .map_err(|message| ConfigError {
                    origin: origin.to_string(),
                    provider: Some(provider.id.clone()),
                    model: Some(model.id.clone()),
                    field: "token_counting.path".into(),
                    message,
                })?,
            ),
            None => None,
        };

        let auth = model.effective_auth(provider);
        let auth_header = model
            .auth_header
            .clone()
            .or_else(|| {
                // The provider's header applies unless the protocol changed.
                (protocol == provider.protocol && model.auth.is_none())
                    .then(|| provider.auth_header.clone())
                    .flatten()
            })
            .unwrap_or_else(|| DEFAULT_API_KEY_HEADER.to_string());

        let key_source = match (&provider.api_key, &provider.api_key_env) {
            (Some(k), _) => KeySource::Inline(k.clone()),
            (None, Some(env)) => KeySource::Env(env.clone()),
            (None, None) => KeySource::None,
        };

        // Required headers < provider headers < model headers (by name).
        let mut headers: BTreeMap<String, Secret> = protocol
            .required_headers()
            .iter()
            .map(|(k, v)| (k.to_string(), Secret::new(*v)))
            .collect();
        for (k, v) in provider.headers.iter().chain(model.headers.iter()) {
            headers.retain(|existing, _| !existing.eq_ignore_ascii_case(k));
            headers.insert(k.clone(), v.clone());
        }

        Ok(ResolvedModel {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            model_id: model.id.clone(),
            model_name: model.name.clone(),
            api_model: model.api_model.clone(),
            protocol,
            url,
            count_url,
            auth,
            auth_header,
            key_source,
            headers,
            compat: Compat::resolve(&provider.compat, &model.compat),
            limits: model.limits,
            declared: Capabilities::declared(model),
            parameters: model.parameters.clone(),
            remove_parameters: model.remove_parameters.clone(),
            reasoning: model.reasoning.clone(),
            pricing: model
                .pricing
                .as_ref()
                .map(Pricing::from_config)
                .transpose()
                .map_err(|(field, message)| ConfigError {
                    origin: origin.to_string(),
                    provider: Some(provider.id.clone()),
                    model: Some(model.id.clone()),
                    field: format!("pricing.{field}"),
                    message,
                })?,
        })
    }

    /// `provider_id/model_id`.
    pub fn selection(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }

    /// Declared ∩ what this protocol's adapter can transport.
    pub fn effective_capabilities(&self) -> Capabilities {
        self.declared.effective(self.protocol)
    }

    /// Name of the environment variable holding the key, if one is referenced.
    pub fn api_key_env(&self) -> Option<&str> {
        match &self.key_source {
            KeySource::Env(n) => Some(n),
            _ => None,
        }
    }

    /// Start building a provider for this model.
    pub fn provider(&self) -> crate::provider::ProviderBuilder {
        crate::provider::ProviderBuilder::new(self.clone())
    }

    /// Build a provider with defaults (secrets from the process environment).
    pub fn build_provider(&self) -> cersei_types::Result<crate::provider::ConfiguredProvider> {
        self.provider().build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests_support::BASIC;

    fn reg() -> ProviderRegistry {
        ProviderRegistry::from_toml_str(BASIC, "test.toml").unwrap()
    }

    #[test]
    fn resolves_by_explicit_selection_only() {
        let r = reg();
        let m = r.resolve("custom/flash").unwrap();
        assert_eq!(m.selection(), "custom/flash");
        assert_eq!(m.api_model, "vendor-flash");
        assert_eq!(m.protocol, Protocol::ChatCompletions);
        assert_eq!(
            m.url.as_str(),
            "https://api.example.com/v1/chat/completions"
        );
        let expert = r.resolve("custom/expert").unwrap();
        assert_eq!(expert.protocol, Protocol::Responses);
        assert_eq!(expert.url.as_str(), "https://api.example.com/v1/responses");

        for bad in ["flash", "vendor-flash", "gpt-4o"] {
            let e = r.resolve(bad).unwrap_err();
            assert!(e.message.contains("never guessed"), "{bad}: {e}");
        }
        let e = r.resolve("nope/flash").unwrap_err();
        assert!(e.message.contains("unknown provider `nope`") && e.message.contains("custom"));
        let e = r.resolve("custom/nope").unwrap_err();
        assert!(e.message.contains("no model `nope`") && e.message.contains("flash, expert"));
    }

    #[test]
    fn model_ids_may_contain_slashes() {
        let text = BASIC.replace("id = \"flash\"", "id = \"org/flash:7b\"");
        let r = ProviderRegistry::from_toml_str(&text, "t").unwrap();
        assert_eq!(
            r.resolve("custom/org/flash:7b").unwrap().model_id,
            "org/flash:7b"
        );
    }

    #[test]
    fn no_cap_on_providers_or_models() {
        let mut text = String::from("schema_version = 1\n");
        for p in 0..40 {
            text.push_str(&format!(
                "[[providers]]\nid = \"p{p}\"\nname = \"P\"\nendpoint = \"http://localhost:{}\"\nprotocol = \"chat_completions\"\nauth = \"none\"\n",
                8000 + p
            ));
            for m in 0..60 {
                text.push_str(&format!(
                    "[[providers.models]]\nid = \"m{m}\"\nname = \"M\"\napi_model = \"a{m}\"\n[providers.models.limits]\nmax_input_tokens = 10\nmax_output_tokens = 10\n"
                ));
            }
        }
        let r = ProviderRegistry::from_toml_str(&text, "big.toml").unwrap();
        assert_eq!(r.models().len(), 2400);
        assert_eq!(r.resolve("p39/m59").unwrap().api_model, "a59");
    }

    #[test]
    fn protocol_override_gets_its_own_path_auth_and_headers() {
        let text = BASIC
            .replace("[[providers.models]]\nid = \"expert\"", "[[providers.models]]\nid = \"claude\"\nprotocol = \"anthropic_messages\"\nname = \"C\"\napi_model = \"vc\"\ninput_modalities = [\"text\"]\noutput_modalities = [\"text\"]\n[providers.models.limits]\nmax_input_tokens = 10\nmax_output_tokens = 10\n[[providers.models]]\nid = \"expert\"")
            .replace("path = \"x\"", "");
        let r = ProviderRegistry::from_toml_str(&text, "t").unwrap();
        let c = r.resolve("custom/claude").unwrap();
        assert_eq!(
            c.url.as_str(),
            "https://api.example.com/v1/messages",
            "never chat/completions"
        );
        assert_eq!(c.auth, AuthMode::ApiKeyHeader);
        assert_eq!(c.auth_header, "x-api-key");
        assert!(c.headers.contains_key("anthropic-version"));
        let f = r.resolve("custom/flash").unwrap();
        assert_eq!(f.auth, AuthMode::Bearer);
        assert!(f.headers.is_empty());
    }

    #[test]
    fn path_and_endpoint_overrides_apply_to_the_right_models() {
        let text = BASIC
            .replace(
                "auth = \"bearer\"\n",
                "auth = \"bearer\"\npath = \"custom/chat\"\n",
            )
            .replace(
                "api_model = \"vendor-expert\"\n",
                "api_model = \"vendor-expert\"\nendpoint = \"https://other.example.com/gw\"\n",
            );
        let r = ProviderRegistry::from_toml_str(&text, "t").unwrap();
        // Provider-level path applies to the provider's default protocol only.
        assert_eq!(
            r.resolve("custom/flash").unwrap().url.as_str(),
            "https://api.example.com/v1/custom/chat"
        );
        // `expert` overrides the protocol: it does not inherit `custom/chat`.
        assert_eq!(
            r.resolve("custom/expert").unwrap().url.as_str(),
            "https://other.example.com/gw/responses"
        );
    }

    #[test]
    fn headers_merge_by_name_case_insensitively() {
        let text = BASIC
            .replace("[[providers.models]]\nid = \"expert\"", "[[providers.models]]\nid = \"expert\"\nprotocol = \"anthropic_messages\"")
            .replace("protocol = \"responses\"\n", "")
            .replace("auth = \"bearer\"\n", "auth = \"bearer\"\nheaders = { \"Anthropic-Version\" = \"2099-01-01\", \"x-team\" = \"a\" }\n");
        let r = ProviderRegistry::from_toml_str(&text, "t").unwrap();
        let m = r.resolve("custom/expert").unwrap();
        assert_eq!(m.headers.len(), 2);
        assert_eq!(
            m.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("anthropic-version"))
                .unwrap()
                .1
                .expose(),
            "2099-01-01"
        );
    }

    #[test]
    fn debug_never_shows_keys_or_header_values() {
        let text = BASIC
            .replace(
                "api_key_env = \"BRICKS_CUSTOM_API_KEY\"",
                "api_key = \"sk-visible-never\"",
            )
            .replace(
                "auth = \"bearer\"\n",
                "auth = \"bearer\"\nheaders = { \"x-t\" = \"hv-secret\" }\n",
            );
        let r = ProviderRegistry::from_toml_str(&text, "t").unwrap();
        let m = r.resolve("custom/flash").unwrap();
        let shown = format!("{m:?} {r:?}");
        assert!(
            !shown.contains("sk-visible-never") && !shown.contains("hv-secret"),
            "{shown}"
        );
    }

    /// The shipped example must stay loadable, secret-free, and mean what the
    /// documentation says it means.
    #[test]
    fn the_shipped_example_loads_resolves_and_holds_no_secret() {
        let text = include_str!("../../../docs/providers.example.toml");
        assert!(
            !text
                .lines()
                .any(|l| l.trim_start().starts_with("api_key =")),
            "examples use api_key_env only"
        );
        let reg = ProviderRegistry::from_toml_str(text, "providers.example.toml").unwrap();

        // Every model resolves and builds once its key reference is satisfied.
        for m in reg.models() {
            let resolved = reg.resolve(&m.selection()).unwrap();
            resolved
                .provider()
                .env(|_| Some("test-key".into()))
                .build()
                .unwrap_or_else(|e| panic!("{}: {e}", m.selection()));
        }

        // Endpoint rules from the documentation.
        let url = |sel: &str| reg.resolve(sel).unwrap().url.to_string();
        assert_eq!(
            url("custom/flash"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(url("custom/expert"), "https://api.example.com/v1/responses");
        assert_eq!(
            url("messages-example/thinker"),
            "https://messages.example.com/v1/messages"
        );
        assert_eq!(
            url("responses-example/agent"),
            "https://responses.example.com/v1/responses"
        );
        assert_eq!(
            url("local/coder"),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            url("multi/flash-chat"),
            "https://api.multi.example.com/chat/completions"
        );
        assert_eq!(
            url("multi/flash-responses"),
            "https://api.multi.example.com/responses"
        );
        assert_eq!(
            url("multi/flash-messages"),
            "https://api.multi.example.com/messages"
        );
        assert_eq!(
            url("gateway/deployed"),
            "https://gw.example.com/team-a/openai/deployments/my-deployment/chat/completions?api-version=2025-01-01"
        );

        // The protocol override carries its own conventions.
        let msgs = reg.resolve("multi/flash-messages").unwrap();
        assert_eq!(msgs.auth, AuthMode::ApiKeyHeader);
        assert!(msgs.headers.contains_key("anthropic-version"));
        assert_eq!(
            reg.resolve("multi/flash-chat").unwrap().auth,
            AuthMode::Bearer
        );

        // A named tariff exists where documented, and the contract example's
        // empty profile list is honoured.
        assert!(reg
            .resolve("multi/flash-chat")
            .unwrap()
            .pricing
            .unwrap()
            .tariff("offpeak")
            .is_some());
        assert!(reg
            .resolve("custom/expert")
            .unwrap()
            .reasoning
            .profiles
            .is_empty());
        assert_eq!(
            reg.resolve("custom/flash")
                .unwrap()
                .reasoning
                .profiles
                .len(),
            2
        );
        assert_eq!(reg.resolve("local/coder").unwrap().auth, AuthMode::None);
    }

    #[test]
    fn capabilities_and_pricing_are_resolved() {
        let m = reg().resolve("custom/flash").unwrap();
        assert!(m.pricing.is_some());
        assert!(m.declared.tool_calls && m.declared.streaming);
        assert_eq!(m.limits.input_budget(m.limits.max_output_tokens), 96000);
        assert_eq!(m.api_key_env(), Some("BRICKS_CUSTOM_API_KEY"));
    }

    fn counting_config(protocol: &str, table: &str) -> String {
        format!(
            r#"
schema_version = 1
[[providers]]
id = "p"
name = "P"
endpoint = "https://gw.example.com/team/v1?x=1"
protocol = "{protocol}"
auth = "none"
[[providers.models]]
id = "m"
name = "M"
api_model = "m"
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
{table}
"#
        )
    }

    #[test]
    fn token_counting_is_opt_in_and_uses_the_documented_route() {
        let reg =
            |p: &str, t: &str| ProviderRegistry::from_toml_str(&counting_config(p, t), "c.toml");
        // Absent: no counting URL, whatever the protocol.
        let m = reg("anthropic_messages", "")
            .unwrap()
            .resolve("p/m")
            .unwrap();
        assert!(m.count_url.is_none());
        let on = "[providers.models.token_counting]";
        let m = reg("anthropic_messages", on)
            .unwrap()
            .resolve("p/m")
            .unwrap();
        assert_eq!(
            m.count_url.unwrap().as_str(),
            "https://gw.example.com/team/v1/messages/count_tokens?x=1"
        );
        let m = reg("responses", on).unwrap().resolve("p/m").unwrap();
        assert_eq!(
            m.count_url.unwrap().as_str(),
            "https://gw.example.com/team/v1/responses/input_tokens?x=1"
        );
        let m = reg("responses", &format!("{on}\npath = \"custom/count\""))
            .unwrap()
            .resolve("p/m")
            .unwrap();
        assert_eq!(
            m.count_url.unwrap().as_str(),
            "https://gw.example.com/team/v1/custom/count?x=1"
        );
    }

    #[test]
    fn token_counting_is_refused_where_no_route_is_documented() {
        let on = "[providers.models.token_counting]";
        for table in [on.to_string(), format!("{on}\npath = \"tokenize\"")] {
            let e = ProviderRegistry::from_toml_str(
                &counting_config("chat_completions", &table),
                "c.toml",
            )
            .unwrap_err();
            assert!(e.field.starts_with("token_counting"), "{e}");
        }
        let e = ProviderRegistry::from_toml_str(
            &counting_config("responses", &format!("{on}\nurl = \"x\"")),
            "c.toml",
        )
        .unwrap_err();
        assert!(e.to_string().contains("url"), "{e}");
    }
}
