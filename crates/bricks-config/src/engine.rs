//! The engine configuration every frontend assembles the same way.

use crate::projects::Projects;
use bricks_agent::BricksConfig;
use bricks_agent::control::{EngineConfig, ModelCatalog, SkillsSetup};
use bricks_provider::ProviderRegistry;
use std::path::Path;
use std::sync::Arc;

/// Everything the engine needs for `workspace`. Nothing of a project is read
/// here: the controller asks [`Projects`] for the project of the session it
/// opens, so a broken `bricks.toml` in one folder does not prevent resuming
/// a session of another.
///
/// Skills: the ones installed in Bricks' store (`<bricks_home>/skills`) and
/// the built-in one. No project or home folder of another client is read.
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
    cfg.skills = Some(SkillsSetup::with_store(Some(crate::paths::skills_dir())));
    cfg.project_loader = Some(Arc::new(Projects::new(registry, interactive)));
    cfg
}
