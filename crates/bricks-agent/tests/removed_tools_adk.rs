//! Removed native tools, not their names: an embedder's own tool called
//! `TaskCreate` is announced and runs as any other tool, and a call to a
//! removed native tool (CronCreate, NotebookEdit, EnterWorktree, ExitWorktree,
//! SyntheticOutput, ToolSearch) is an unknown tool: an error result that lists
//! the tools the agent has, with no panic and no success.

use async_trait::async_trait;
use bricks_agent::Agent;
use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
use bricks_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use bricks_types::{ContentBlock, ToolResultContent};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The results the agent recorded: (tool_use_id, text, is_error).
fn tool_results(agent: &Agent) -> Vec<(String, String, bool)> {
    agent
        .messages()
        .iter()
        .flat_map(|m| m.content_blocks())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content: ToolResultContent::Text(t),
                is_error,
            } => Some((tool_use_id.clone(), t.clone(), is_error.unwrap_or(false))),
            _ => None,
        })
        .collect()
}

/// The result of the call `id`, with its text and error flag.
fn result_of<'a>(results: &'a [(String, String, bool)], id: &str) -> &'a (String, String, bool) {
    results
        .iter()
        .find(|(call, _, _)| call == id)
        .unwrap_or_else(|| panic!("no result for {id}: {results:?}"))
}

/// A custom tool with the name of a removed native one.
struct CustomTaskCreate(Arc<AtomicBool>);

#[async_trait]
impl Tool for CustomTaskCreate {
    fn name(&self) -> &str {
        "TaskCreate"
    }
    fn description(&self) -> &str {
        "An embedder's own task tool, unrelated to any built-in tool."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"]})
    }
    async fn execute(&self, _input: Value, _ctx: &ToolContext) -> ToolResult {
        self.0.store(true, Ordering::SeqCst);
        ToolResult::success("custom task recorded")
    }
}

#[tokio::test]
async fn a_custom_tool_keeps_a_removed_name_and_a_removed_tool_is_unknown() {
    let ran = Arc::new(AtomicBool::new(false));
    let script = Script::new(vec![
        Reply::tool(
            "c1",
            "CronCreate",
            json!({"schedule": "* * * * *", "prompt": "x"}),
        ),
        Reply::tool("c2", "TaskCreate", json!({"title": "Ship it"})),
        Reply::tool(
            "c3",
            "NotebookEdit",
            json!({"notebook_path": "a.ipynb", "new_source": "x"}),
        ),
        Reply::text("Fini."),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::builder()
        .provider(ScriptedProvider::new("test/a", script.clone()))
        .model("test/a")
        .tools(vec![Box::new(CustomTaskCreate(Arc::clone(&ran)))])
        .working_dir(dir.path())
        .build()
        .expect("the agent builds");
    let out = agent.run("Planifie et crée.").await.expect("the run ends");
    assert_eq!(out.text().trim(), "Fini.");

    let reqs = script.requests();
    let names: Vec<&str> = reqs[0].1.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["TaskCreate"],
        "the custom tool is announced, nothing else"
    );
    assert!(ran.load(Ordering::SeqCst), "the custom tool ran");
    // Without CodeStats announced, its line is not sent either.
    let sys = reqs[0].1.system.as_deref().unwrap_or("");
    assert!(!sys.contains("CodeStats"), "{sys}");

    // The removed native tools came back as error results to the model.
    let fed = serde_json::to_string(&reqs[1].1.messages).unwrap();
    assert!(fed.contains("CronCreate"), "{fed}");
    let fed = serde_json::to_string(&reqs[3].1.messages).unwrap();
    assert!(fed.contains("NotebookEdit"), "{fed}");
    assert!(!dir.path().join("a.ipynb").exists(), "nothing was written");
    assert!(
        fed.contains("is_error\":true") || fed.contains("\"is_error\": true"),
        "{fed}"
    );
}

const UNKNOWN_HINT: &str = "Use one of the available tools above, with its exact name.";

/// A call to each tool removed after the first list (worktrees, synthetic
/// output, tool search) is the standard unknown-tool error: it names the call,
/// lists the tools the agent has, and offers no removed tool by name.
#[tokio::test]
async fn the_later_removed_tools_are_unknown_with_the_standard_error() {
    let removed = [
        "EnterWorktree",
        "ExitWorktree",
        "SyntheticOutput",
        "ToolSearch",
    ];
    let calls: Vec<(&str, &str, Value)> = vec![
        ("w1", "EnterWorktree", json!({})),
        ("w2", "ExitWorktree", json!({})),
        ("w3", "SyntheticOutput", json!({"value": 42})),
        ("w4", "ToolSearch", json!({"query": "worktree"})),
    ];
    let script = Script::new(vec![Reply::tools(calls), Reply::text("Fini.")]);
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::builder()
        .provider(ScriptedProvider::new("test/a", script.clone()))
        .model("test/a")
        .tools(bricks_tools::all())
        .working_dir(dir.path())
        .build()
        .expect("the agent builds");
    let out = agent.run("Travaille.").await.expect("the run ends");
    assert_eq!(out.text().trim(), "Fini.");

    let results = tool_results(&agent);
    assert_eq!(results.len(), 4, "{results:?}");
    let total = bricks_tools::all().len();
    assert_eq!(total, 16, "the standard set has sixteen tools");
    for (id, name) in ["w1", "w2", "w3", "w4"].iter().zip(removed) {
        let (_, text, is_error) = result_of(&results, id);
        assert!(*is_error, "{name}: {text}");
        assert!(
            text.contains(&format!("Tool '{name}' not found.")),
            "{name}: {text}"
        );
        assert!(
            text.contains(&format!("Available tools ({total}")),
            "{name}: {text}"
        );
        assert!(text.contains(UNKNOWN_HINT), "{name}: {text}");
        // The echoes of the called name (the header and the message) are the
        // only place it may appear: the list and the hint offer no removed tool.
        let offered = text.replace(name, "");
        for gone in removed {
            assert!(!offered.contains(gone), "{name} offers {gone}: {text}");
        }
        let listed = text
            .lines()
            .find(|l| l.contains("Available tools"))
            .expect("the list of tools");
        assert!(
            listed.contains("Read") && listed.contains("Bash"),
            "{listed}"
        );
    }
}

/// The unknown-tool message of an ordinary typo: a near name is suggested,
/// the tools are listed, the hint is the standard one, and ToolSearch is
/// never mentioned, since the agent does not have it.
#[tokio::test]
async fn an_unknown_tool_message_lists_the_tools_and_never_mentions_tool_search() {
    let script = Script::new(vec![
        Reply::tool("g1", "Grepp", json!({"pattern": "x"})),
        Reply::text("Fini."),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::builder()
        .provider(ScriptedProvider::new("test/a", script.clone()))
        .model("test/a")
        .tools(bricks_tools::all())
        .working_dir(dir.path())
        .build()
        .expect("the agent builds");
    agent.run("Cherche.").await.expect("the run ends");

    let results = tool_results(&agent);
    let (_, text, is_error) = result_of(&results, "g1");
    assert!(*is_error, "{text}");
    assert!(text.contains("Tool 'Grepp' not found."), "{text}");
    assert!(text.contains("Did you mean: Grep?"), "{text}");
    assert!(
        text.contains(&format!("Available tools ({})", bricks_tools::all().len())),
        "{text}"
    );
    assert!(text.contains(UNKNOWN_HINT), "{text}");
    assert!(!text.contains("ToolSearch"), "{text}");
}
