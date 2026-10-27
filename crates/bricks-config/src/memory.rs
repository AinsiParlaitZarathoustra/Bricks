//! The long-term memories: a project's from its `bricks.toml` (CLI and SDK),
//! and the desktop's two scopes ([`MemoryService`]). Every store is opened
//! once per process and canonical path ([`open_memories`]).

use crate::paths::bricks_home;
use bricks_embeddings::{EmbeddingProvider, GeminiEmbeddings, HashingEmbeddings, OpenAiEmbeddings};
use bricks_memory::structured::extract::LlmExtractor;
use bricks_memory::structured::{MemoryConfig, OpenMemories, StructuredMemory};
use bricks_provider::ProviderRegistry;
use std::path::Path;
use std::sync::Arc;

fn embedder(spec: &str) -> Result<Arc<dyn EmbeddingProvider>, String> {
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
            let e = OpenAiEmbeddings::from_env()
                .map_err(|e| format!("memory.embeddings openai: {e}"))?;
            Arc::new(match arg {
                Some(m) => e.with_model(m),
                None => e,
            })
        }
        "gemini" => {
            let e = GeminiEmbeddings::from_env()
                .map_err(|e| format!("memory.embeddings gemini: {e}"))?;
            Arc::new(match arg {
                Some(m) => e.with_model(m),
                None => e,
            })
        }
        other => return Err(format!("memory.embeddings: unknown `{other}`")),
    })
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
        let resolved = registry
            .resolve(sel)
            .map_err(|e| format!("memory.extractor_model: {e}"))?;
        let provider = resolved
            .build_provider()
            .map_err(|e| format!("memory.extractor_model: {e}"))?;
        extractor = Some(Arc::new(LlmExtractor::new(
            Arc::new(provider),
            resolved.api_model.clone(),
            cfg.extraction.clone(),
        )) as Arc<dyn bricks_memory::structured::Extractor>);
    }
    let memory = open_memories()
        .open(
            &path,
            &cfg.space,
            embedder(&cfg.embeddings)?,
            &cfg,
            extractor,
        )
        .map_err(|e| format!("long-term memory {}: {e}", path.display()))?
        .memory;
    Ok(Some((memory, cfg.space.clone())))
}
