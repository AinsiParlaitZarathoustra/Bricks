//! Tools a host lends an agent (`ToolLender`): read at the start of each run
//! and kept for that run. A lent tool is offered to the model and runs in the
//! run that starts after it is lent; once the host drops it, the next run
//! does not offer it, and a call to it is refused. A lent tool named like one
//! of the agent's own tools is not lent. Scripted model, records each request.

use async_trait::async_trait;
use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
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

/// The host's lent tools, changed between runs.
#[derive(Default)]
struct Lendable {
    tools: parking_lot::Mutex<Vec<Arc<dyn Tool>>>,
}

impl Lendable {
    fn set(&self, tools: Vec<Arc<dyn Tool>>) {
        *self.tools.lock() = tools;
    }
}

impl ToolLender for Lendable {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.lock().clone()
    }
}

/// The names of the tools the model was offered in one request.
fn offered(request: &CompletionRequest) -> Vec<String> {
    request.tools.iter().map(|t| t.name.clone()).collect()
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

#[tokio::test]
async fn a_lent_tool_is_offered_and_runs_until_the_host_drops_it() {
    let script = Script::new(vec![
        // Run 1: the lent tool is offered and called.
        Reply::tool("l1", "LentTool", json!({})),
        Reply::text("first done"),
        // Run 2: the model calls it anyway.
        Reply::tool("l2", "LentTool", json!({})),
        Reply::text("second done"),
        // Run 3: lent again.
        Reply::tool("l3", "LentTool", json!({})),
        Reply::text("third done"),
    ]);
    let lender = Arc::new(Lendable::default());
    lender.set(vec![fixed("LentTool", "lent says hi")]);
    let dir = tempfile::tempdir().unwrap();
    let a = agent(dir.path(), &script, &lender);

    a.run("first").await.unwrap();
    let reqs = script.requests();
    assert!(
        offered(&reqs[0].1).contains(&"LentTool".to_string()),
        "run 1 offers it: {:?}",
        offered(&reqs[0].1)
    );
    let (is_error, text) = result_of(&reqs[1].1, "l1");
    assert_ne!(is_error, Some(true));
    assert!(text.ends_with("lent says hi"), "{text}");
    assert!(a.tool_by_name("LentTool").is_some(), "lent for the run");

    // The host drops it: the next run does not offer it, and the call is refused.
    lender.set(Vec::new());
    a.run("second").await.unwrap();
    let reqs = script.requests();
    assert!(
        !offered(&reqs[2].1).contains(&"LentTool".to_string()),
        "run 2 does not offer it: {:?}",
        offered(&reqs[2].1)
    );
    let (is_error, text) = result_of(&reqs[3].1, "l2");
    assert_eq!(is_error, Some(true), "a dropped tool does not run: {text}");
    assert!(!text.contains("lent says hi"), "{text}");
    assert!(a.tool_by_name("LentTool").is_none());

    // The host lends it again: the run after offers it again.
    lender.set(vec![fixed("LentTool", "back again")]);
    a.run("third").await.unwrap();
    let reqs = script.requests();
    assert!(offered(&reqs[4].1).contains(&"LentTool".to_string()));
    let (_, text) = result_of(&reqs[5].1, "l3");
    assert!(text.ends_with("back again"), "{text}");
}

#[tokio::test]
async fn a_lent_tool_named_like_an_own_tool_is_not_lent() {
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
        Reply::tool("p1", "Probe", json!({})),
        Reply::text("done"),
    ]);
    let lender = Arc::new(Lendable::default());
    lender.set(vec![fixed("Probe", "lent"), fixed("Other", "other")]);
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
    let names = offered(&reqs[0].1);
    assert_eq!(
        names.iter().filter(|n| *n == "Probe").count(),
        1,
        "the name is offered once: {names:?}"
    );
    assert!(names.contains(&"Other".to_string()), "{names:?}");
    let (_, text) = result_of(&reqs[1].1, "p1");
    assert!(text.ends_with("own"), "the agent's own tool runs: {text}");
    assert!(!text.contains("lent"), "{text}");
    let listed: Vec<String> = a.tool_list().iter().map(|t| t.name().to_string()).collect();
    assert_eq!(
        listed.iter().filter(|n| *n == "Probe").count(),
        1,
        "{listed:?}"
    );
    assert_eq!(
        a.tool_by_name("Probe").unwrap().description(),
        "the agent's own"
    );
}
