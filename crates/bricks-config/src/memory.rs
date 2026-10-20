//! The long-term memory of a project, opened once per store and
//! configuration.

use crate::paths::bricks_home;
use bricks_embeddings::{EmbeddingProvider, GeminiEmbeddings, HashingEmbeddings, OpenAiEmbeddings};
use bricks_memory::structured::extract::LlmExtractor;
use bricks_memory::structured::{MemoryConfig, StructuredMemory};
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

/// Long-term memories opened by this process, by file and configuration:
/// one store is never opened twice (sessions of several projects may share
/// the default one).
pub(crate) type OpenMemories =
    parking_lot::Mutex<std::collections::HashMap<String, Arc<StructuredMemory>>>;

/// The long-term memory, when `[memory] enabled = true`.
pub(crate) fn long_term_memory(
    wd: &Path,
    registry: &ProviderRegistry,
    open: &OpenMemories,
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
    let key = format!("{}\n{cfg:?}", path.display());
    if let Some(m) = open.lock().get(&key) {
        return Ok(Some((Arc::clone(m), cfg.space.clone())));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut builder = StructuredMemory::builder(embedder(&cfg.embeddings)?)
        .path(&path)
        .config(cfg.clone());
    if let Some(sel) = &cfg.extractor_model {
        let resolved = registry
            .resolve(sel)
            .map_err(|e| format!("memory.extractor_model: {e}"))?;
        let provider = resolved
            .build_provider()
            .map_err(|e| format!("memory.extractor_model: {e}"))?;
        builder = builder.extractor(Arc::new(LlmExtractor::new(
            Arc::new(provider),
            resolved.api_model.clone(),
            cfg.extraction.clone(),
        )));
    }
    let memory = Arc::new(
        builder
            .open()
            .map_err(|e| format!("long-term memory {}: {e}", path.display()))?,
    );
    open.lock().insert(key, Arc::clone(&memory));
    Ok(Some((memory, cfg.space.clone())))
}
