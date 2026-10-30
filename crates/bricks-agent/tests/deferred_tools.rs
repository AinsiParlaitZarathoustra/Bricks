//! Deferred tools: a host's tools announced only once loaded, through
//! `LoadTools` or by a direct call. A loaded tool is announced from the next
//! request on; a deferred tool the host drops is hidden, and so is
//! `LoadTools` when nothing is left to load; a deferred name that an own tool
//! already uses is not lent. Scripted model, records each request.

use async_trait::async_trait;
use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
use bricks_agent::deferred::LOAD_TOOLS;
use bricks_agent::{Agent, ToolLender};
use bricks_provider::CompletionRequest;
use bricks_tools::permissions::AllowAll;
use bricks_tools::{PermissionLevel, Tool, ToolContext, ToolResult};
use bricks_types::{ContentBlock, MessageContent, ToolResultContent};
use serde_json::{Value, json};
use std::sync::Arc;

/// A read-only tool that always answers the same text.
struct Fixed {
    name: &'static str,
    answer: &'static str,
}

#[async_trait]
impl Tool for Fixed {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "answers with a fixed text"
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }
    async fn execute(&self, _: Value, _: &ToolContext) -> ToolResult {
        ToolResult::success(self.answer)
    }
}

fn fixed(name: &'static str, answer: &'static str) -> Arc<dyn Tool> {
    Arc::new(Fixed { name, answer })
}

/// The host's lent and deferred tools, changed between runs.
#[derive(Default)]
struct Lendable {
    lent: parking_lot::Mutex<Vec<Arc<dyn Tool>>>,
    deferred: parking_lot::Mutex<Vec<Arc<dyn Tool>>>,
}

impl Lendable {
    fn set_lent(&self, tools: Vec<Arc<dyn Tool>>) {
        *self.lent.lock() = tools;
    }

    fn set_deferred(&self, tools: Vec<Arc<dyn Tool>>) {
        *self.deferred.lock() = tools;
    }
}

impl ToolLender for Lendable {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.lent.lock().clone()
    }

    fn deferred_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.deferred.lock().clone()
    }
}

/// Whether the model was offered the tool `name` in one request.
fn offers(request: &CompletionRequest, name: &str) -> bool {
    request.tools.iter().any(|t| t.name == name)
}

/// The result of the call `id` in the request's history: (is_error, text).
fn result_of(request: &CompletionRequest, id: &str) -> (Option<bool>, String) {
    request
        .messages
        .iter()
        .find_map(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks.iter().find_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if tool_use_id == id => Some((
                    *is_error,
                    match content {
                        ToolResultContent::Text(t) => t.clone(),
                        ToolResultContent::Blocks(_) => panic!("a text result expected"),
                    },
                )),
                _ => None,
            }),
            MessageContent::Text(_) => None,
        })
        .unwrap_or_else(|| panic!("no result for {id}"))
}

fn agent(dir: &std::path::Path, script: &Arc<Script>, lender: &Arc<Lendable>) -> Agent {
    Agent::builder()
        .provider(ScriptedProvider::new("a", script.clone()))
        .model("a")
        .working_dir(dir)
        .permission_policy(AllowAll)
        .tool_lender(lender.clone())
        .build()
        .expect("build agent")
}

/// The names of the tools the agent lists now.
fn listed(a: &Agent) -> Vec<String> {
    a.tool_list().iter().map(|t| t.name().to_string()).collect()
}

fn zoom_and_type() -> Vec<Arc<dyn Tool>> {
    vec![
        fixed("mcp__cu__zoom", "zoomed"),
        fixed("mcp__cu__type_text", "typed"),
    ]
}

#[tokio::test]
async fn deferred_tools_are_not_announced_until_loaded() {
    let script = Script::new(vec![Reply::text("nothing needed")]);
    let lender = Arc::new(Lendable::default());
    lender.set_deferred(zoom_and_type());
    let dir = tempfile::tempdir().unwrap();
    let a = agent(dir.path(), &script, &lender);

    a.run("hello").await.unwrap();
    let reqs = script.requests();
    assert!(
        offers(&reqs[0].1, LOAD_TOOLS),
        "LoadTools is announced: {:?}",
        reqs[0].1.tools
    );
    assert!(
        !offers(&reqs[0].1, "mcp__cu__zoom") && !offers(&reqs[0].1, "mcp__cu__type_text"),
        "no deferred tool is announced: {:?}",
        reqs[0].1.tools
    );
    let names = listed(&a);
    assert!(names.contains(&LOAD_TOOLS.to_string()), "{names:?}");
    assert!(!names.contains(&"mcp__cu__zoom".to_string()), "{names:?}");
}

#[tokio::test]
async fn a_tool_loaded_by_name_is_announced_from_the_next_request_of_the_run() {
    let script = Script::new(vec![
        Reply::tool("ld1", LOAD_TOOLS, json!({"query": "select:zoom"})),
        Reply::tool("zm1", "mcp__cu__zoom", json!({})),
        Reply::text("done"),
    ]);
    let lender = Arc::new(Lendable::default());
    lender.set_deferred(zoom_and_type());
    let dir = tempfile::tempdir().unwrap();
    let a = agent(dir.path(), &script, &lender);

    a.run("zoom in").await.unwrap();
    let reqs = script.requests();
    assert_eq!(reqs.len(), 3, "one request per model turn");
    assert!(!offers(&reqs[0].1, "mcp__cu__zoom"));
    assert!(
        offers(&reqs[1].1, "mcp__cu__zoom"),
        "the loaded tool is announced in the next request: {:?}",
        reqs[1].1.tools
    );
    assert!(
        !offers(&reqs[1].1, "mcp__cu__type_text"),
        "only the tool asked for is loaded"
    );

    let (is_error, text) = result_of(&reqs[1].1, "ld1");
    assert_ne!(is_error, Some(true), "{text}");
    assert!(text.contains("mcp__cu__zoom"), "{text}");
    assert!(
        text.contains("Loaded, callable from the next request"),
        "{text}"
    );

    let (is_error, text) = result_of(&reqs[2].1, "zm1");
    assert_ne!(is_error, Some(true), "{text}");
    assert!(text.ends_with("zoomed"), "{text}");
}

#[tokio::test]
async fn a_direct_call_to_an_unloaded_deferred_tool_runs_and_loads_it() {
    let script = Script::new(vec![
        Reply::tool("zm1", "mcp__cu__zoom", json!({})),
        Reply::text("zoomed once"),
    ]);
    let lender = Arc::new(Lendable::default());
    lender.set_deferred(zoom_and_type());
    let dir = tempfile::tempdir().unwrap();
    let a = agent(dir.path(), &script, &lender);

    a.run("zoom").await.unwrap();
    let reqs = script.requests();
    assert!(!offers(&reqs[0].1, "mcp__cu__zoom"));
    let (is_error, text) = result_of(&reqs[1].1, "zm1");
    assert_ne!(is_error, Some(true), "the call runs: {text}");
    assert!(text.ends_with("zoomed"), "{text}");
    assert!(
        listed(&a).contains(&"mcp__cu__zoom".to_string()),
        "the tool is loaded: {:?}",
        listed(&a)
    );

    // The next run announces it, as a loaded tool.
    script.push(Reply::text("again"));
    a.run("again").await.unwrap();
    let reqs = script.requests();
    assert!(offers(&reqs[2].1, "mcp__cu__zoom"), "{:?}", reqs[2].1.tools);
}

#[tokio::test]
async fn a_search_without_a_match_is_an_error_listing_the_available_tools() {
    let script = Script::new(vec![
        Reply::tool("s1", LOAD_TOOLS, json!({"query": "calendar"})),
        Reply::tool("s2", LOAD_TOOLS, json!({"query": "select:nope"})),
        Reply::text("nothing loaded"),
    ]);
    let lender = Arc::new(Lendable::default());
    lender.set_deferred(zoom_and_type());
    let dir = tempfile::tempdir().unwrap();
    let a = agent(dir.path(), &script, &lender);

    a.run("a calendar").await.unwrap();
    let reqs = script.requests();
    let (is_error, text) = result_of(&reqs[1].1, "s1");
    assert_eq!(is_error, Some(true), "{text}");
    assert!(text.contains("calendar"), "{text}");
    assert!(
        text.contains("mcp__cu__zoom") && text.contains("mcp__cu__type_text"),
        "the error lists the available tools: {text}"
    );

    let (is_error, text) = result_of(&reqs[2].1, "s2");
    assert_eq!(is_error, Some(true), "{text}");
    assert!(text.contains("mcp__cu__zoom"), "{text}");

    assert!(!offers(&reqs[2].1, "mcp__cu__zoom"), "nothing was loaded");
    assert!(
        !offers(&reqs[2].1, "mcp__cu__type_text"),
        "nothing was loaded"
    );
}

#[tokio::test]
async fn a_lender_without_deferred_tools_offers_no_loadtools() {
    let script = Script::new(vec![Reply::text("hi")]);
    let lender = Arc::new(Lendable::default());
    lender.set_lent(vec![fixed("mcp__cu__click", "clicked")]);
    let dir = tempfile::tempdir().unwrap();
    let a = agent(dir.path(), &script, &lender);

    a.run("hi").await.unwrap();
    let reqs = script.requests();
    assert!(
        offers(&reqs[0].1, "mcp__cu__click"),
        "{:?}",
        reqs[0].1.tools
    );
    assert!(!offers(&reqs[0].1, LOAD_TOOLS), "{:?}", reqs[0].1.tools);
    assert!(
        !listed(&a).contains(&LOAD_TOOLS.to_string()),
        "{:?}",
        listed(&a)
    );
}

#[tokio::test]
async fn dropped_deferred_tools_are_hidden_and_loadtools_goes_with_them() {
    let script = Script::new(vec![
        Reply::tool("ld1", LOAD_TOOLS, json!({"query": "select:zoom"})),
        Reply::tool("zm1", "mcp__cu__zoom", json!({})),
        Reply::text("first done"),
    ]);
    let lender = Arc::new(Lendable::default());
    lender.set_deferred(zoom_and_type());
    let dir = tempfile::tempdir().unwrap();
    let a = agent(dir.path(), &script, &lender);

    a.run("first").await.unwrap();
    assert!(listed(&a).contains(&"mcp__cu__zoom".to_string()));

    // The host drops every deferred tool: the next run announces neither
    // LoadTools nor the loaded tool, and a call to the tool is refused.
    lender.set_deferred(Vec::new());
    script.push(Reply::tool("zm2", "mcp__cu__zoom", json!({})));
    script.push(Reply::text("second done"));
    a.run("second").await.unwrap();
    let reqs = script.requests();
    assert_eq!(reqs.len(), 5);
    assert!(
        !offers(&reqs[3].1, LOAD_TOOLS),
        "LoadTools is not offered: {:?}",
        reqs[3].1.tools
    );
    assert!(
        !offers(&reqs[3].1, "mcp__cu__zoom"),
        "the dropped tool is not offered: {:?}",
        reqs[3].1.tools
    );
    let (is_error, text) = result_of(&reqs[4].1, "zm2");
    assert_eq!(is_error, Some(true), "a dropped tool does not run: {text}");
    assert!(!text.ends_with("zoomed"), "{text}");

    let names = listed(&a);
    assert!(!names.contains(&LOAD_TOOLS.to_string()), "{names:?}");
    assert!(!names.contains(&"mcp__cu__zoom".to_string()), "{names:?}");
    assert!(a.tool_by_name("mcp__cu__zoom").is_none());
}

#[tokio::test]
async fn a_deferred_tool_named_like_an_own_tool_is_not_lent() {
    struct Own;
    #[async_trait]
    impl Tool for Own {
        fn name(&self) -> &str {
            "Probe"
        }
        fn description(&self) -> &str {
            "the agent's own"
        }
        fn permission_level(&self) -> PermissionLevel {
            PermissionLevel::ReadOnly
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object", "properties": {}})
        }
        async fn execute(&self, _: Value, _: &ToolContext) -> ToolResult {
            ToolResult::success("own")
        }
    }

    let script = Script::new(vec![
        // The name is taken: a load by that name finds nothing.
        Reply::tool("p0", LOAD_TOOLS, json!({"query": "select:Probe"})),
        Reply::tool("p1", "Probe", json!({})),
        Reply::text("done"),
    ]);
    let lender = Arc::new(Lendable::default());
    lender.set_deferred(vec![fixed("Probe", "deferred"), fixed("Other", "other")]);
    let dir = tempfile::tempdir().unwrap();
    let a = Agent::builder()
        .provider(ScriptedProvider::new("a", script.clone()))
        .model("a")
        .working_dir(dir.path())
        .permission_policy(AllowAll)
        .tool(Own)
        .tool_lender(lender.clone())
        .build()
        .unwrap();

    a.run("probe").await.unwrap();
    let reqs = script.requests();
    let probes = reqs[0].1.tools.iter().filter(|t| t.name == "Probe").count();
    assert_eq!(
        probes, 1,
        "the name is announced once: {:?}",
        reqs[0].1.tools
    );
    assert!(
        !offers(&reqs[0].1, "Other"),
        "Other is deferred, not loaded"
    );

    let (is_error, text) = result_of(&reqs[1].1, "p0");
    assert_eq!(is_error, Some(true), "{text}");
    assert!(!text.contains("deferred"), "{text}");

    let (_, text) = result_of(&reqs[2].1, "p1");
    assert!(text.ends_with("own"), "the agent's own tool runs: {text}");
    assert!(!text.contains("deferred"), "{text}");

    assert_eq!(
        a.tool_by_name("Probe").unwrap().description(),
        "the agent's own"
    );
    let loader = a
        .tool_by_name(LOAD_TOOLS)
        .unwrap()
        .description()
        .to_string();
    assert!(!loader.contains("Probe"), "{loader}");
    assert!(loader.contains("Other"), "{loader}");
}
