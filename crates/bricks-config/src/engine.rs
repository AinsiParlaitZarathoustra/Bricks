//! The engine configuration every frontend assembles the same way.

use crate::projects::Projects;
use cersei_agent::BricksConfig;
use cersei_agent::control::{EngineConfig, ModelCatalog, SkillsSetup};
use cersei_provider::ProviderRegistry;
use std::path::Path;
use std::sync::Arc;

/// Everything the engine needs for `workspace`. Nothing of a project is read
/// here: the controller asks [`Projects`] for the project of the session it
/// opens, so a broken `bricks.toml` in one folder does not prevent resuming
/// a session of another.
///
/// Skills: the session's project sources and the user's home folder
/// (`~/.agents/skills`, `~/.claude/skills`), plus the built-in ones.
pub fn engine_config(
    workspace: &Path,
    registry: Arc<ProviderRegistry>,
    interactive: bool,
) -> EngineConfig {
    let mut cfg = EngineConfig::new(
        workspace,
        Arc::clone(&registry) as Arc<dyn ModelCatalog>,
        BricksConfig::default(),
        crate::paths::sessions_dir(),
    );
    cfg.interactive = interactive;
    cfg.skills = Some(SkillsSetup::with_home(dirs::home_dir()));
    cfg.project_loader = Some(Arc::new(Projects::new(registry, interactive)));
    cfg
}
