//! Tool-surface guarantees: the built-in sets are exactly the kept tools,
//! and every registered tool carries a substantive description.
//!
//! The description is the only per-tool guidance channel that reaches the
//! model for most tools (the system prompt names just a handful), so the
//! floor test keeps a new tool from shipping with a bare label. Whether any
//! given description wording helps a model call the tool correctly is not
//! measurable offline and is not claimed here.

use async_trait::async_trait;
use bricks_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use serde_json::Value;

#[test]
fn every_tool_has_a_substantive_description() {
    for tool in bricks_tools::all() {
        let desc = tool.description();
        assert!(
            desc.trim().len() >= 30,
            "{} ships a bare description ({} chars): {desc:?}",
            tool.name(),
            desc.len()
        );
    }
}

/// The native tools removed so far: fifteen in 14.8.1, the three
/// background-task tools when `Job` took over, and four unused system tools.
const REMOVED: [&str; 22] = [
    "EnterPlanMode",
    "ExitPlanMode",
    "CronCreate",
    "CronList",
    "CronDelete",
    "TaskCreate",
    "TaskGet",
    "TaskUpdate",
    "TaskList",
    "TaskStop",
    "TaskOutput",
    "RemoteTrigger",
    "Config",
    "AskUserQuestion",
    "Sleep",
    "BashTaskStatus",
    "BashTaskOutput",
    "BashTaskStop",
    "EnterWorktree",
    "ExitWorktree",
    "SyntheticOutput",
    "ToolSearch",
];

fn names(tools: &[Box<dyn bricks_tools::Tool>]) -> Vec<String> {
    tools.iter().map(|t| t.name().to_string()).collect()
}

#[test]
fn the_built_in_surface_is_exactly_the_kept_tools() {
    let all = names(&bricks_tools::all());
    assert_eq!(
        all,
        [
            "Read",
            "ViewImage",
            "Write",
            "Edit",
            "ApplyPatch",
            "Glob",
            "Grep",
            "CodeSearch",
            "CodeScout",
            "CodeStats",
            "Bash",
            "Job",
            "PowerShell",
            "WebFetch",
            "WebSearch",
            "TodoWrite",
        ]
    );
    // `all()` is the coding tools plus the plan.
    let mut coding_and_plan = names(&bricks_tools::coding());
    assert_eq!(coding_and_plan.len(), 15);
    coding_and_plan.extend(names(&bricks_tools::planning()));
    assert_eq!(all, coding_and_plan);
    // Job follows the jobs Bash records: it comes with a registry, not alone.
    assert!(names(&bricks_tools::coding()).contains(&"Job".to_string()));
    assert!(names(&bricks_tools::shell()).contains(&"Bash".to_string()));
    assert!(!names(&bricks_tools::shell()).contains(&"Job".to_string()));
    assert_eq!(names(&bricks_tools::planning()), ["TodoWrite"]);
    for set in [
        bricks_tools::all(),
        bricks_tools::coding(),
        bricks_tools::planning(),
        bricks_tools::filesystem(),
        bricks_tools::shell(),
        bricks_tools::web(),
    ] {
        let set = names(&set);
        for gone in REMOVED {
            assert!(!set.contains(&gone.to_string()), "{gone} in {set:?}");
        }
    }
}

/// An integrator's own tool, named like a removed built-in.
struct Homonym;

#[async_trait]
impl Tool for Homonym {
    fn name(&self) -> &str {
        "ToolSearch"
    }
    fn description(&self) -> &str {
        "An integrator's own search tool, unrelated to the removed built-in."
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    async fn execute(&self, _input: Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::success("own")
    }
}

#[test]
fn an_integrators_tool_named_like_a_removed_one_is_kept() {
    // The SDK filters nothing by name: a removed built-in's name is free.
    let mut tools = bricks_tools::all();
    tools.push(Box::new(Homonym));
    assert_eq!(
        names(&tools).iter().filter(|n| *n == "ToolSearch").count(),
        1
    );
}
