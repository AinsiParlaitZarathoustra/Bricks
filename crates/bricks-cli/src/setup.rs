//! Configuration loading: the existing loaders, with the working directory
//! explicit. Errors are complete messages for the user (exit code 2).

use crate::args::Global;
use cersei_agent::control::{EngineConfig, OpenOptions, SessionChoice};
use cersei_agent::system_prompt::{build_system_prompt, SystemPromptOptions};
use cersei_agent::BricksConfig;
use cersei_embeddings::{EmbeddingProvider, GeminiEmbeddings, HashingEmbeddings, OpenAiEmbeddings};
use cersei_memory::structured::extract::LlmExtractor;
use cersei_memory::structured::{MemoryConfig, StructuredMemory};
use cersei_provider::ProviderRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `~/.bricks`, or `$BRICKS_HOME`.
pub fn bricks_home() -> PathBuf {
    if let Some(h) = std::env::var_os("BRICKS_HOME") {
        return PathBuf::from(h);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".bricks")
}

pub fn sessions_dir() -> PathBuf {
    bricks_home().join("sessions")
}

/// The working directory, checked, made absolute and current (tools that
/// take relative paths resolve them against it).
pub fn working_dir(global: &Global) -> Result<PathBuf, String> {
    let wd = match &global.cd {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| format!("current directory: {e}"))?,
    };
    let wd = wd
        .canonicalize()
        .map_err(|e| format!("working directory {}: {e}", wd.display()))?;
    if !wd.is_dir() {
        return Err(format!("{} is not a directory", wd.display()));
    }
    std::env::set_current_dir(&wd).map_err(|e| format!("cannot enter {}: {e}", wd.display()))?;
    Ok(wd)
}

/// The providers file: `--providers`, else `$BRICKS_HOME/providers.toml`
/// when `BRICKS_HOME` is set, else the registry's default location.
fn providers_path(global: &Global) -> Option<PathBuf> {
    global.providers.clone().or_else(|| {
        std::env::var_os("BRICKS_HOME").map(|h| PathBuf::from(h).join("providers.toml"))
    })
}

pub fn registry(global: &Global) -> Result<ProviderRegistry, String> {
    ProviderRegistry::load(providers_path(global).as_deref()).map_err(|e| {
        format!(
            "{e}\nModels are configured in providers.toml (see docs/providers.md); \
             nothing is built in."
        )
    })
}

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

/// The long-term memory, when `[memory] enabled = true`.
fn long_term_memory(
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
    let memory = builder
        .open()
        .map_err(|e| format!("long-term memory {}: {e}", path.display()))?;
    Ok(Some((Arc::new(memory), cfg.space.clone())))
}

/// Everything the engine needs, from the existing loaders.
pub fn engine(global: &Global, interactive: bool) -> Result<EngineConfig, String> {
    let wd = working_dir(global)?;
    let registry = registry(global)?;
    let bricks = BricksConfig::load(&wd);
    let memory = long_term_memory(&wd, &registry)?;
    let tools = cersei_tools::coding();
    let mut tools_available: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
    // The controller adds the native sub-agent tools to the session agent.
    if bricks.agents.enabled {
        tools_available.push("Agent".into());
        tools_available.push("AgentProfiles".into());
    }
    let system = build_system_prompt(&SystemPromptOptions {
        is_non_interactive: !interactive,
        working_directory: Some(wd.display().to_string()),
        tools_available,
        has_memory: memory.is_some(),
        has_auto_compact: true,
        ..Default::default()
    });
    let mut cfg = EngineConfig::new(&wd, Arc::new(registry), bricks, sessions_dir());
    cfg.interactive = interactive;
    cfg.system_prompt = Some(system);
    if let Some((m, space)) = memory {
        cfg.long_term_memory = Some(m);
        cfg.memory_space = Some(space);
    }
    Ok(cfg)
}

pub fn open_options(global: &Global, session: Option<String>) -> OpenOptions {
    OpenOptions {
        session: match session {
            Some(id) => SessionChoice::Resume(id),
            None => SessionChoice::New,
        },
        model: global.model.clone(),
        reasoning: global.reasoning.clone(),
    }
}
