//! Coordinator mode: multi-agent orchestration.
//!
//! When active, the agent acts as a coordinator that spawns parallel worker
//! agents using the Agent tool. Workers have restricted tool access (no Agent,
//! SendMessage) to prevent uncontrolled recursion.

use bricks_tools::Tool;

/// Agent execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    /// Full access to all tools including Agent spawning.
    Coordinator,
    /// Restricted: cannot spawn sub-agents or use coordination tools.
    Worker,
    /// Standard mode: all tools available (no special orchestration).
    Normal,
}

/// Tools restricted to coordinator mode only (workers can't use these).
pub const COORDINATOR_ONLY_TOOLS: &[&str] = &["Agent", "SendMessage", "TeamCreate", "TeamDelete"];

/// Check if coordinator mode is active via environment variable.
pub fn is_coordinator_mode() -> bool {
    match std::env::var("CERSEI_COORDINATOR_MODE") {
        Ok(v) => !v.is_empty() && v != "0" && v != "false",
        Err(_) => false,
    }
}

/// Filter tools based on agent mode.
/// Workers lose coordinator-only tools. Coordinators and Normal keep everything.
pub fn filter_tools_for_mode(tools: Vec<Box<dyn Tool>>, mode: AgentMode) -> Vec<Box<dyn Tool>> {
    match mode {
        AgentMode::Worker => tools
            .into_iter()
            .filter(|t| !COORDINATOR_ONLY_TOOLS.contains(&t.name()))
            .collect(),
        AgentMode::Coordinator | AgentMode::Normal => tools,
    }
}

/// System prompt section for coordinator mode.
pub fn coordinator_system_prompt() -> &'static str {
    "## Coordinator Mode\n\n\
    You are operating as an orchestrator. Your role is to:\n\
    1. Break complex tasks into independent sub-tasks\n\
    2. Spawn parallel worker agents using the Agent tool\n\
    3. Each worker prompt must be fully self-contained\n\
    4. Synthesize findings from all workers before responding\n\n\
    Workers cannot spawn their own sub-agents. They have access to \
    filesystem, shell, and web tools only."
}

/// Format a context section listing available tools for the coordinator.
pub fn coordinator_context(tools: &[Box<dyn Tool>]) -> String {
    let tool_list: Vec<String> = tools
        .iter()
        .filter(|t| !["Agent"].contains(&t.name()))
        .map(|t| format!("- {}: {}", t.name(), t.description()))
        .collect();

    format!("Available tools for workers:\n{}", tool_list.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_worker_tools() {
        let tools = bricks_tools::all();
        let original_count = tools.len();
        let filtered = filter_tools_for_mode(tools, AgentMode::Worker);
        // Workers should have fewer tools (coordinator-only removed)
        assert!(filtered.len() <= original_count);
        assert!(
            filtered
                .iter()
                .all(|t| !COORDINATOR_ONLY_TOOLS.contains(&t.name()))
        );
    }

    #[test]
    fn test_filter_coordinator_keeps_all() {
        let tools = bricks_tools::all();
        let count = tools.len();
        let filtered = filter_tools_for_mode(tools, AgentMode::Coordinator);
        assert_eq!(filtered.len(), count);
    }

    #[test]
    fn test_coordinator_prompt() {
        let prompt = coordinator_system_prompt();
        assert!(prompt.contains("orchestrator"));
        assert!(prompt.contains("sub-tasks"));
    }

    #[test]
    fn test_coordinator_context() {
        let tools = bricks_tools::all();
        let ctx = coordinator_context(&tools);
        assert!(ctx.contains("Available tools"));
        assert!(ctx.contains("Read"));
        assert!(ctx.contains("Bash"));
    }

    /// A stand-in tool: the filters look at names only.
    struct Named(&'static str);

    #[async_trait::async_trait]
    impl Tool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "stand-in"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _input: serde_json::Value,
            _ctx: &bricks_tools::ToolContext,
        ) -> bricks_tools::ToolResult {
            bricks_tools::ToolResult::success("")
        }
    }

    fn named(names: &[&'static str]) -> Vec<Box<dyn Tool>> {
        names
            .iter()
            .map(|n| Box::new(Named(n)) as Box<dyn Tool>)
            .collect()
    }

    fn names_of(tools: &[Box<dyn Tool>]) -> Vec<String> {
        tools.iter().map(|t| t.name().to_string()).collect()
    }

    const REMOVED: [&str; 4] = [
        "EnterWorktree",
        "ExitWorktree",
        "SyntheticOutput",
        "ToolSearch",
    ];

    #[test]
    fn the_coordinator_only_tools_are_agent_and_the_team_tools() {
        assert_eq!(
            COORDINATOR_ONLY_TOOLS,
            &["Agent", "SendMessage", "TeamCreate", "TeamDelete"]
        );
    }

    #[test]
    fn the_standard_set_has_no_removed_tool_and_no_coordinator_only_tool() {
        let names = names_of(&bricks_tools::all());
        assert_eq!(names.len(), 16, "{names:?}");
        for gone in REMOVED {
            assert!(
                !names.iter().any(|n| n == gone),
                "{gone} is removed: {names:?}"
            );
        }
        for name in COORDINATOR_ONLY_TOOLS {
            assert!(
                !names.iter().any(|n| n == *name),
                "{name} is not standard: {names:?}"
            );
        }
    }

    #[test]
    fn a_worker_loses_exactly_the_coordinator_only_tools_it_is_given() {
        let tools = || {
            named(&[
                "Read",
                "Agent",
                "SendMessage",
                "TeamCreate",
                "TeamDelete",
                "Bash",
            ])
        };
        let worker = names_of(&filter_tools_for_mode(tools(), AgentMode::Worker));
        assert_eq!(worker, ["Read", "Bash"]);
        let coordinator = names_of(&filter_tools_for_mode(tools(), AgentMode::Coordinator));
        assert_eq!(coordinator.len(), 6, "{coordinator:?}");
        let normal = names_of(&filter_tools_for_mode(tools(), AgentMode::Normal));
        assert_eq!(normal.len(), 6, "{normal:?}");
    }

    #[test]
    fn a_worker_of_the_standard_set_keeps_all_sixteen_tools() {
        let worker = filter_tools_for_mode(bricks_tools::all(), AgentMode::Worker);
        assert_eq!(worker.len(), 16);
    }

    #[test]
    fn the_worker_context_leaves_out_agent_and_names_no_removed_tool() {
        let ctx = coordinator_context(&named(&["Read", "Agent", "Bash"]));
        assert!(!ctx.contains("- Agent:"), "{ctx}");
        assert!(ctx.contains("- Read: stand-in"), "{ctx}");
        assert!(ctx.contains("- Bash: stand-in"), "{ctx}");

        let ctx = coordinator_context(&bricks_tools::all());
        for gone in REMOVED {
            assert!(!ctx.contains(&format!("- {gone}:")), "{gone} listed: {ctx}");
        }
    }
}
