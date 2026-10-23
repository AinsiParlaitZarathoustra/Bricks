//! Project loading: the settings of the folder a session works in.

use crate::memory::{OpenMemories, long_term_memory};
use bricks_agent::BricksConfig;
use bricks_agent::control::{ProjectContext, ProjectLoader};
use bricks_agent::system_prompt::{SystemPromptOptions, build_system_prompt};
use bricks_provider::ProviderRegistry;
use std::path::Path;
use std::sync::Arc;

/// Reads a project with the existing loaders: `BricksConfig::load`, the
/// long-term memory of its `bricks.toml`, the system prompt for its folder.
pub struct Projects {
    registry: Arc<ProviderRegistry>,
    /// Whether a person can answer: decides the non-interactive prompt
    /// wording of the system prompt.
    interactive: bool,
    memories: OpenMemories,
}

impl Projects {
    pub fn new(registry: Arc<ProviderRegistry>, interactive: bool) -> Self {
        Self {
            registry,
            interactive,
            memories: Default::default(),
        }
    }
}

/// The names of the tools the session agent announces when the controller
/// builds it from the engine's tools (`bricks_tools::coding`): those,
/// `TodoWrite`, and the native sub-agent tools when `[agents] enabled`.
/// The prompt itself no longer relies on this list: its conditional
/// guidance follows the definitions each request really carries.
pub fn announced_tools(bricks: &BricksConfig) -> Vec<String> {
    let mut names: Vec<String> = bricks_tools::coding()
        .iter()
        .map(|t| t.name().to_string())
        .collect();
    names.push("TodoWrite".to_string());
    if bricks.agents.enabled && !names.iter().any(|n| n == "Agent") {
        names.extend(
            ["Agent", "Agents", "AgentControl", "AgentProfiles", "SendMessage"].map(String::from),
        );
    }
    names
}

impl ProjectLoader for Projects {
    fn load(&self, wd: &Path) -> Result<ProjectContext, String> {
        let bricks = BricksConfig::load(wd);
        let memory = long_term_memory(wd, &self.registry, &self.memories)?;
        // Assembled for each request (tools announced, date, environment,
        // git, model, plan); the text here is the same prompt without a
        // request, for inspection.
        let options = SystemPromptOptions {
            is_non_interactive: !self.interactive,
            working_directory: Some(wd.display().to_string()),
            has_memory: memory.is_some(),
            has_auto_compact: true,
            ..Default::default()
        };
        let system = build_system_prompt(&SystemPromptOptions {
            tools_available: announced_tools(&bricks),
            ..options.clone()
        });
        let (long_term_memory, memory_space) = match memory {
            Some((m, space)) => (
                Some(m as Arc<dyn bricks_memory::LongTermMemory>),
                Some(space),
            ),
            None => (None, None),
        };
        Ok(ProjectContext {
            working_dir: wd.to_path_buf(),
            bricks,
            system_prompt: Some(system),
            prompt_options: Some(options),
            long_term_memory,
            memory_space,
            mcp_servers: Vec::new(),
            agent_profile_sources: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announced_tools_follow_the_session_agent() {
        let mut b = BricksConfig::default();
        let names = announced_tools(&b);
        for t in [
            "CodeScout",
            "Agent",
            "Agents",
            "AgentControl",
            "AgentProfiles",
            "SendMessage",
        ] {
            assert!(names.iter().any(|n| n == t), "{t}");
        }
        b.agents.enabled = false;
        let names = announced_tools(&b);
        assert!(!names.iter().any(|n| n.starts_with("Agent") || n == "SendMessage"));
    }
}
