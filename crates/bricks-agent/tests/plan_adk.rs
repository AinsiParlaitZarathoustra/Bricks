//! The plan of an agent built directly with the SDK builder (no controller):
//! TodoWrite's plan is the agent's own, its updates are events of the agent,
//! a raw prompt gets the plan block once a plan exists, and a provider that
//! does not say its request identity still gets a working request.

use bricks_agent::Agent;
use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
use bricks_agent::events::AgentEvent;
use bricks_agent::system_prompt::SystemPromptOptions;
use bricks_provider::CompletionRequest;
use bricks_tools::todo_write::{PlanState, TodoWriteTool};
use serde_json::json;
use std::sync::{Arc, Mutex};

const RAW: &str = "RAW";

fn plan_call(id: &str) -> Reply {
    Reply::tool(
        id,
        "TodoWrite",
        json!({
            "todos": [
                {"content": "Lire le code", "status": "completed", "activeForm": "Lecture du code"},
                {"content": "Écrire les tests", "status": "in_progress", "activeForm": "Écriture des tests"}
            ],
            "explanation": "Plan initial"
        }),
    )
}

fn system(r: &CompletionRequest) -> &str {
    r.system.as_deref().unwrap_or("")
}

/// Published plan updates, as the event handler saw them.
type Seen = Arc<Mutex<Vec<(PlanState, Option<String>)>>>;

/// The agent and the folder it works in (kept until the test ends).
fn agent_with(
    script: &Arc<Script>,
    seen: &Seen,
    configure: impl FnOnce(bricks_agent::AgentBuilder) -> bricks_agent::AgentBuilder,
) -> (Agent, tempfile::TempDir) {
    let seen = Arc::clone(seen);
    let dir = tempfile::tempdir().unwrap();
    let builder = Agent::builder()
        .provider(ScriptedProvider::new("test/a", script.clone()))
        .model("test/a")
        .tools(vec![Box::new(TodoWriteTool)])
        .working_dir(dir.path())
        .on_event(move |e: &AgentEvent| {
            if let AgentEvent::PlanUpdated { plan, root_run_id } = e {
                seen.lock()
                    .unwrap()
                    .push((plan.clone(), root_run_id.clone()));
            }
        });
    (configure(builder).build().expect("the agent builds"), dir)
}

#[tokio::test]
async fn the_agents_own_plan_is_updated_and_published_as_its_event() {
    let script = Script::new(vec![plan_call("t1"), Reply::text("Plan noté.")]);
    let seen: Seen = Arc::default();
    let (agent, _dir) = agent_with(&script, &seen, |b| b);

    assert_eq!(
        agent
            .plan()
            .expect("TodoWrite gives the agent a plan")
            .current()
            .revision,
        0
    );
    agent.run("Planifie.").await.unwrap();

    let plan = agent.plan().unwrap().current();
    assert_eq!(plan.revision, 1);
    assert_eq!(plan.todos.len(), 2);
    assert_eq!(plan.explanation.as_deref(), Some("Plan initial"));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one event for one update");
    assert_eq!(seen[0].0, plan, "the event carries the adopted plan");
}

#[tokio::test]
async fn a_raw_prompt_gets_the_plan_block_once_a_plan_exists() {
    let script = Script::new(vec![
        plan_call("t1"),
        Reply::text("ok"),
        Reply::text("ok bis"),
    ]);
    let seen: Seen = Arc::default();
    let (agent, _dir) = agent_with(&script, &seen, |b| b.system_prompt(RAW));

    agent.run("Planifie.").await.unwrap();
    agent.run("Suite.").await.unwrap();

    let reqs = script.requests();
    // Before any update: the raw prompt, alone.
    assert_eq!(system(&reqs[0].1), RAW);
    // After the update, in the same run and in the next one: the raw prompt,
    // then the plan's complete state.
    for (i, (_, req)) in reqs.iter().enumerate().skip(1) {
        let s = system(req);
        assert!(
            s.starts_with("RAW\n\n<plan revision=\"1\""),
            "request {i}: {s}"
        );
        assert!(s.contains("Écrire les tests"), "request {i}: {s}");
    }
}

#[tokio::test]
async fn a_provider_without_a_request_identity_still_runs_and_names_no_api_model() {
    let script = Script::new(vec![Reply::text("Bonjour.")]);
    let seen: Seen = Arc::default();
    let options = SystemPromptOptions {
        is_non_interactive: true,
        working_directory: Some("/w".into()),
        ..Default::default()
    };
    let (agent, _dir) = agent_with(&script, &seen, |b| b.system_prompt_options(options));

    // ScriptedProvider does not implement `request_identity`: the default `None`.
    let out = agent.run("Salut.").await.expect("the request goes through");
    assert_eq!(out.turns, 1);

    let reqs = script.requests();
    let s = system(&reqs[0].1);
    assert!(s.contains("<model>"), "{s}");
    assert!(s.contains("Sélection Bricks : test/a"), "{s}");
    assert!(!s.contains("Nom envoyé à l'API"), "{s}");
}
