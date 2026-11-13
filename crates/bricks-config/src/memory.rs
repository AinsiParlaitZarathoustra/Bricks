//! The long-term memories: a project's from its `bricks.toml` (CLI and SDK),
//! and the desktop's two scopes ([`MemoryService`]). Every store is opened
//! once per process and canonical path ([`open_memories`]).

use crate::paths::bricks_home;
use bricks_embeddings::{EmbeddingProvider, GeminiEmbeddings, HashingEmbeddings, OpenAiEmbeddings};
use bricks_memory::structured::extract::LlmExtractor;
use bricks_memory::structured::{
    ExtractionConfig, Extractor, MemoryConfig, OpenMemories, StructuredMemory,
};
use bricks_provider::{AuthMode, ProviderConfig, ProviderRegistry};
use std::path::Path;
use std::sync::Arc;

fn provider_matches_kind(provider: &ProviderConfig, kind: &str) -> bool {
    let id = provider.id.to_ascii_lowercase();
    let endpoint = provider.endpoint.to_ascii_lowercase();
    match kind {
        "openai" => id == "openai" || endpoint.contains("openai.com"),
        "gemini" => {
            id == "gemini"
                || id == "google"
                || endpoint.contains("generativelanguage.googleapis.com")
        }
        _ => false,
    }
}

fn find_provider_credentials(
    registry: &ProviderRegistry,
    kind: &str,
) -> Option<(String, Option<String>)> {
    // Prefer exact id match before endpoint match.
    let providers = &registry.config().providers;
    let exact = providers.iter().find(|p| p.id.eq_ignore_ascii_case(kind));
    let matched = exact.or_else(|| providers.iter().find(|p| provider_matches_kind(p, kind)))?;
    let key = matched.key()?.to_string();
    let base_url = (kind == "openai" && !matched.endpoint.trim().is_empty())
        .then(|| matched.endpoint.clone());
    Some((key, base_url))
}

pub(crate) fn embedder_with_registry(
    spec: &str,
    registry: Option<&ProviderRegistry>,
) -> Result<Arc<dyn EmbeddingProvider>, String> {
    let (kind, arg) = match spec.split_once(':') {
        Some((k, a)) => (k, Some(a)),
        None => (spec, None),
    };
    Ok(match kind {
        "hashing" => {
            let dims = arg
                .map(|a| {
                    a.parse::<usize>()
                        .map_err(|_| format!("memory.embeddings: `{a}` is not a dimension"))
                })
                .transpose()?
                .unwrap_or(384);
            Arc::new(HashingEmbeddings::new(dims))
        }
        "openai" => {
            let mut e = match OpenAiEmbeddings::from_env() {
                Ok(e) => e,
                Err(env_err) => {
                    if let Some((key, base_url)) =
                        registry.and_then(|r| find_provider_credentials(r, "openai"))
                    {
                        let e = OpenAiEmbeddings::new(key);
                        if let Some(url) = base_url {
                            e.with_base_url(url)
                        } else {
                            e
                        }
                    } else {
                        return Err(format!("memory.embeddings openai: {env_err}"));
                    }
                }
            };
            if let Some(m) = arg {
                e = e.with_model(m);
            }
            Arc::new(e)
        }
        "gemini" => {
            let mut e = match GeminiEmbeddings::from_env() {
                Ok(e) => e,
                Err(env_err) => {
                    if let Some((key, _)) =
                        registry.and_then(|r| find_provider_credentials(r, "gemini"))
                    {
                        GeminiEmbeddings::new(key)
                    } else {
                        return Err(format!("memory.embeddings gemini: {env_err}"));
                    }
                }
            };
            if let Some(m) = arg {
                e = e.with_model(m);
            }
            Arc::new(e)
        }
        other => return Err(format!("memory.embeddings: unknown `{other}`")),
    })
}

/// Automatically select the best available embedding provider from env / user's
/// configured providers (`openai` -> `gemini`), falling back explicitly to
/// `HashingEmbeddings(384)` when offline or without credentials.
pub(crate) fn resolve_auto_embedder(
    registry: Option<&ProviderRegistry>,
    dimensions: usize,
) -> (Arc<dyn EmbeddingProvider>, bool) {
    for spec in ["openai", "gemini"] {
        if let Ok(emb) = embedder_with_registry(spec, registry) {
            return (emb, false);
        }
    }
    (Arc::new(HashingEmbeddings::new(dimensions)), true)
}

fn lightweight_score(model_id: &str, api_model: &str) -> Option<usize> {
    let combined = format!(
        "{} {}",
        model_id.to_ascii_lowercase(),
        api_model.to_ascii_lowercase()
    );
    const HINTS: [&str; 7] = [
        "nano",
        "mini",
        "flash-lite",
        "flash",
        "haiku",
        "lite",
        "small",
    ];
    HINTS.iter().position(|h| combined.contains(h))
}

fn provider_has_credentials(p: &ProviderConfig) -> bool {
    p.auth == AuthMode::None || p.key().is_some()
}

/// Resolve an extractor either from an explicit `provider_id/model_id` or, when
/// absent, choose a lightweight configured model from `registry` (zero network
/// calls).
pub(crate) fn resolve_extractor(
    explicit_model: Option<&str>,
    registry: Option<&ProviderRegistry>,
    extraction: &ExtractionConfig,
) -> Result<Option<Arc<dyn Extractor>>, String> {
    let Some(reg) = registry else {
        if let Some(sel) = explicit_model {
            return Err(format!(
                "memory.extractor_model `{sel}` requires a provider registry"
            ));
        }
        return Ok(None);
    };

    if let Some(sel) = explicit_model {
        let resolved = reg
            .resolve(sel)
            .map_err(|e| format!("memory.extractor_model: {e}"))?;
        let provider = resolved
            .build_provider()
            .map_err(|e| format!("memory.extractor_model: {e}"))?;
        return Ok(Some(Arc::new(LlmExtractor::new(
            Arc::new(provider),
            resolved.api_model.clone(),
            extraction.clone(),
        ))));
    }

    let mut candidates: Vec<(usize, String)> = Vec::new();
    for p in &reg.config().providers {
        if !provider_has_credentials(p) {
            continue;
        }
        for m in &p.models {
            let sel = format!("{}/{}", p.id, m.id);
            let rank = lightweight_score(&m.id, &m.api_model).unwrap_or(100);
            candidates.push((rank, sel));
        }
    }
    candidates.sort_by_key(|(rank, _)| *rank);
    for (_, sel) in candidates {
        if let Ok(resolved) = reg.resolve(&sel)
            && let Ok(provider) = resolved.build_provider()
        {
            return Ok(Some(Arc::new(LlmExtractor::new(
                Arc::new(provider),
                resolved.api_model.clone(),
                extraction.clone(),
            ))));
        }
    }
    Ok(None)
}

/// The stores this process has open: one handle per canonical path (a store
/// file is locked by the process that opens it).
pub fn open_memories() -> &'static OpenMemories {
    static OPEN: std::sync::OnceLock<OpenMemories> = std::sync::OnceLock::new();
    OPEN.get_or_init(OpenMemories::default)
}

/// The long-term memory, when `[memory] enabled = true`.
pub(crate) fn long_term_memory(
    wd: &Path,
    registry: &ProviderRegistry,
) -> Result<Option<(Arc<StructuredMemory>, String)>, String> {
    let text = match std::fs::read_to_string(wd.join("bricks.toml")) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    let cfg = MemoryConfig::from_bricks_toml(&text).map_err(|e| format!("bricks.toml: {e}"))?;
    if !cfg.enabled {
        return Ok(None);
    }
    let path = cfg
        .path
        .clone()
        .unwrap_or_else(|| bricks_home().join("memory").join("memory.grafeo"));
    let mut extractor = None;
    if let Some(sel) = &cfg.extractor_model {
        extractor = resolve_extractor(Some(sel), Some(registry), &cfg.extraction)?;
    }
    let memory = open_memories()
        .open(
            &path,
            &cfg.space,
            embedder_with_registry(&cfg.embeddings, Some(registry))?,
            &cfg,
            extractor,
        )
        .map_err(|e| format!("long-term memory {}: {e}", path.display()))?
        .memory;
    Ok(Some((memory, cfg.space.clone())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_registry(with_openai_key: bool, with_gemini_key: bool) -> ProviderRegistry {
        let mut toml = String::from("schema_version = 1\n\n");
        if with_openai_key {
            toml.push_str(
                r#"[[providers]]
id = "openai"
name = "OpenAI"
endpoint = "https://api.openai.com/v1"
protocol = "responses"
auth = "bearer"
api_key = "sk-openai-test"

[[providers.models]]
id = "flagship"
name = "GPT Flagship"
api_model = "gpt-5"
limits = { max_input_tokens = 128000, max_output_tokens = 8192 }

[[providers.models]]
id = "mini"
name = "GPT Mini"
api_model = "gpt-5-mini"
limits = { max_input_tokens = 128000, max_output_tokens = 8192 }

"#,
            );
        }
        if with_gemini_key {
            toml.push_str(
                r#"[[providers]]
id = "gemini"
name = "Gemini"
endpoint = "https://generativelanguage.googleapis.com/v1beta/openai"
protocol = "chat_completions"
auth = "bearer"
api_key = "gem-test-key"

[[providers.models]]
id = "flash"
name = "Gemini Flash"
api_model = "gemini-2.5-flash"
limits = { max_input_tokens = 128000, max_output_tokens = 8192 }

"#,
            );
        }
        if !with_openai_key && !with_gemini_key {
            toml.push_str(
                r#"[[providers]]
id = "unauth"
name = "No Key"
endpoint = "https://api.example.com/v1"
protocol = "chat_completions"
auth = "bearer"

[[providers.models]]
id = "mini"
name = "Mini"
api_model = "mini-1"
limits = { max_input_tokens = 128000, max_output_tokens = 8192 }
"#,
            );
        }
        ProviderRegistry::from_toml_str(&toml, "test.toml").unwrap()
    }

    #[test]
    fn resolves_embedder_from_registry_with_explicit_hashing_fallback() {
        let openai_reg = sample_registry(true, false);
        let (emb, is_fallback) = resolve_auto_embedder(Some(&openai_reg), 384);
        assert!(!is_fallback);
        assert_eq!(emb.model_id(), "openai/text-embedding-3-small:1536");

        let gemini_reg = sample_registry(false, true);
        let (emb, is_fallback) = resolve_auto_embedder(Some(&gemini_reg), 384);
        assert!(!is_fallback);
        assert_eq!(emb.model_id(), "gemini/gemini-embedding-001:3072");

        let no_key_reg = sample_registry(false, false);
        let (emb, is_fallback) = resolve_auto_embedder(Some(&no_key_reg), 384);
        assert!(is_fallback);
        assert_eq!(emb.model_id(), "hashing-v1:384");
    }

    #[test]
    fn resolves_lightweight_extractor_without_network() {
        let reg = sample_registry(true, false);
        let ext = resolve_extractor(None, Some(&reg), &ExtractionConfig::default())
            .unwrap()
            .expect("extractor should resolve from authenticated provider");
        assert!(ext.id().contains("gpt-5-mini"), "got {}", ext.id());

        let explicit =
            resolve_extractor(Some("openai/flagship"), Some(&reg), &ExtractionConfig::default())
                .unwrap()
                .expect("explicit extractor should resolve");
        assert!(explicit.id().contains("gpt-5"), "got {}", explicit.id());

        let no_key_reg = sample_registry(false, false);
        let none = resolve_extractor(None, Some(&no_key_reg), &ExtractionConfig::default()).unwrap();
        assert!(none.is_none());
    }
}
