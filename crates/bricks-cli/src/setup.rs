//! Configuration loading: the existing loaders, with the project folder
//! explicit. Errors are complete messages for the user (exit code 2).
//!
//! The workspace given on the command line (`--workspace`, alias `--cd`)
//! is where a new session starts. A project's settings (`bricks.toml`,
//! system prompt, long-term memory) are read by [`CliProjects`] for the
//! folder of the session actually opened — a resumed session's own folder,
//! not the one Bricks was started in.

use crate::args::Global;
use cersei_agent::control::{
    EngineConfig, OpenOptions, ProjectContext, ProjectLoader, SessionChoice,
};
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

/// The workspace to open: `arg` (or `launch` without one), with a leading
/// `~` or `~/` expanded from `home` (no shell is involved, so a quoted `~`
/// is expanded too; `~user` and variables are not), a relative path taken
/// from `launch`, then made canonical and checked to be a readable folder.
pub fn resolve_workspace(
    launch: &Path,
    arg: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, String> {
    let given = match arg {
        None => launch.to_path_buf(),
        Some(p) => {
            let mut parts = p.components();
            let expanded = match parts.next() {
                Some(std::path::Component::Normal(first)) if first == "~" => {
                    let home = home.ok_or_else(|| {
                        format!(
                            "cannot expand `~` in {}: the home folder is unknown; give the full \
                             path",
                            p.display()
                        )
                    })?;
                    home.join(parts.as_path())
                }
                Some(std::path::Component::Normal(first))
                    if first.as_encoded_bytes().starts_with(b"~") =>
                {
                    return Err(format!(
                        "`{}`: only `~` and `~/…` are expanded (not `~user`); give the full path",
                        p.display()
                    ));
                }
                _ => p.to_path_buf(),
            };
            if expanded.is_absolute() {
                expanded
            } else {
                launch.join(expanded)
            }
        }
    };
    let canonical = match std::fs::canonicalize(&given) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "the workspace {} does not exist (nothing is created)",
                given.display()
            ))
        }
        Err(e) => return Err(format!("workspace {}: {e}", given.display())),
    };
    if !canonical.is_dir() {
        return Err(format!(
            "the workspace {} is a file, not a folder",
            canonical.display()
        ));
    }
    std::fs::read_dir(&canonical)
        .map_err(|e| format!("the workspace {} cannot be read: {e}", canonical.display()))?;
    Ok(canonical)
}

/// The workspace of this command line.
pub fn workspace(global: &Global, launch: &Path) -> Result<PathBuf, String> {
    resolve_workspace(
        launch,
        global.workspace.as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// The providers file: `--providers`, else `$BRICKS_HOME/providers.toml`
/// when `BRICKS_HOME` is set, else the registry's default location.
fn providers_path(global: &Global, launch: &Path) -> Option<PathBuf> {
    global
        .providers
        .as_ref()
        .map(|p| launch.join(p))
        .or_else(|| {
            std::env::var_os("BRICKS_HOME").map(|h| PathBuf::from(h).join("providers.toml"))
        })
}

pub fn registry(global: &Global, launch: &Path) -> Result<ProviderRegistry, String> {
    ProviderRegistry::load(providers_path(global, launch).as_deref()).map_err(|e| {
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

/// Long-term memories opened by this process, by file and configuration:
/// one store is never opened twice (sessions of several projects may share
/// the default one).
type OpenMemories = parking_lot::Mutex<std::collections::HashMap<String, Arc<StructuredMemory>>>;

/// The long-term memory, when `[memory] enabled = true`.
fn long_term_memory(
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

/// Reads a project with the existing loaders: `BricksConfig::load`, the
/// long-term memory of its `bricks.toml`, the system prompt for its folder.
pub struct CliProjects {
    registry: Arc<ProviderRegistry>,
    interactive: bool,
    memories: OpenMemories,
}

impl ProjectLoader for CliProjects {
    fn load(&self, wd: &Path) -> Result<ProjectContext, String> {
        let bricks = BricksConfig::load(wd);
        let memory = long_term_memory(wd, &self.registry, &self.memories)?;
        let tools = cersei_tools::coding();
        let mut tools_available: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        // The controller adds the native sub-agent tools to the session agent.
        if bricks.agents.enabled {
            tools_available.push("Agent".into());
            tools_available.push("AgentProfiles".into());
        }
        let system = build_system_prompt(&SystemPromptOptions {
            is_non_interactive: !self.interactive,
            working_directory: Some(wd.display().to_string()),
            tools_available,
            has_memory: memory.is_some(),
            has_auto_compact: true,
            ..Default::default()
        });
        let (long_term_memory, memory_space) = match memory {
            Some((m, space)) => (
                Some(m as Arc<dyn cersei_memory::LongTermMemory>),
                Some(space),
            ),
            None => (None, None),
        };
        Ok(ProjectContext {
            working_dir: wd.to_path_buf(),
            bricks,
            system_prompt: Some(system),
            long_term_memory,
            memory_space,
            mcp_servers: Vec::new(),
            agent_profile_sources: None,
        })
    }
}

/// Everything the engine needs. Nothing of a project is read here: the
/// controller asks [`CliProjects`] for the project of the session it opens
/// (so a broken `bricks.toml` here does not prevent resuming a session of
/// another project).
pub fn engine(global: &Global, launch: &Path, interactive: bool) -> Result<EngineConfig, String> {
    let wd = workspace(global, launch)?;
    let registry = Arc::new(registry(global, launch)?);
    let mut cfg = EngineConfig::new(
        &wd,
        Arc::clone(&registry) as Arc<dyn cersei_agent::control::ModelCatalog>,
        BricksConfig::default(),
        sessions_dir(),
    );
    cfg.interactive = interactive;
    cfg.project_loader = Some(Arc::new(CliProjects {
        registry,
        interactive,
        memories: Default::default(),
    }));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_is_resolved_without_a_shell() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let home = root.join("home");
        let launch = root.join("launch");
        let project = home.join("Documents").join("Mon projet");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(launch.join("sub")).unwrap();
        let r = |arg: Option<&str>| resolve_workspace(&launch, arg.map(Path::new), Some(&home));
        assert_eq!(r(None).unwrap(), launch, "default: the launch folder");
        assert_eq!(r(Some("sub")).unwrap(), launch.join("sub"));
        assert_eq!(r(Some("./sub/..")).unwrap(), launch);
        assert_eq!(r(Some(project.to_str().unwrap())).unwrap(), project);
        // A quoted `~` reaches Bricks as is: expanded here, spaces kept.
        assert_eq!(r(Some("~/Documents/Mon projet")).unwrap(), project);
        assert_eq!(r(Some("~")).unwrap(), home);
        // Not expanded: `~user`, variables, a `~` that is not first.
        let e = r(Some("~bob/x")).unwrap_err();
        assert!(e.contains("~user"), "{e}");
        assert!(r(Some("$HOME/Documents"))
            .unwrap_err()
            .contains("does not exist"));
        assert!(r(Some("sub/~")).unwrap_err().contains("does not exist"));
        // No home folder: a clear error.
        let e = resolve_workspace(&launch, Some(Path::new("~/x")), None).unwrap_err();
        assert!(e.contains("home folder is unknown"), "{e}");
    }

    #[test]
    fn a_missing_folder_or_a_file_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let e = resolve_workspace(root.path(), Some(Path::new("absent")), None).unwrap_err();
        assert!(
            e.contains("does not exist") && e.contains("nothing is created"),
            "{e}"
        );
        assert!(!root.path().join("absent").exists());
        std::fs::write(root.path().join("f.txt"), "x").unwrap();
        let e = resolve_workspace(root.path(), Some(Path::new("f.txt")), None).unwrap_err();
        assert!(e.contains("is a file"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_alias_is_the_folder_it_points_to() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("atlas");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.path().join("alias")).unwrap();
        assert_eq!(
            resolve_workspace(root.path(), Some(Path::new("alias")), None).unwrap(),
            real.canonicalize().unwrap()
        );
    }
}
