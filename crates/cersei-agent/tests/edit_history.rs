//! A session recorded before `MultiEdit` was removed keeps working: its
//! stored call stays as it was recorded, the session reopens and runs a new
//! turn, and the removed name is an unknown tool, not an alias of `Edit`.

use cersei_agent::BricksConfig;
use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::*;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

fn config(dir: &Path, script: std::sync::Arc<Script>, sessions: &Path) -> EngineConfig {
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    let mut cfg = EngineConfig::new(
        dir,
        ScriptedCatalog::new(&["a"], script),
        bricks,
        sessions.to_path_buf(),
    );
    cfg.interactive = false;
    cfg
}

async fn until_finished(events: &mut EventStream) -> Vec<Envelope> {
    let mut out = Vec::new();
    loop {
        let e = tokio::time::timeout(Duration::from_secs(30), events.next())
            .await
            .expect("no event within 30s")
            .expect("the event stream closed");
        let done = matches!(e.event, Event::RunFinished { .. });
        out.push(e);
        if done {
            return out;
        }
    }
}

#[tokio::test]
async fn a_session_with_a_recorded_multi_edit_call_reopens_and_runs_again() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join(".sessions");
    let file = dir.path().join("old.txt");
    std::fs::write(&file, "alpha\n").unwrap();

    // The first session: the model calls the removed name, then answers.
    let first_script = Script::new(vec![
        Reply::tool(
            "old-1",
            "MultiEdit",
            json!({
                "file_path": file.to_str().unwrap(),
                "edits": [{ "old_string": "alpha", "new_string": "beta" }]
            }),
        ),
        Reply::text("noted"),
    ]);
    let (ctl, mut events) = Controller::open(
        config(dir.path(), first_script, &sessions),
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    ctl.send(Command::Submit {
        prompt: Prompt::text("old call"),
    })
    .unwrap();
    let evs = until_finished(&mut events).await;
    assert!(
        evs.iter()
            .all(|e| !matches!(&e.event, Event::EditApplied { .. })),
        "the removed name must not edit anything"
    );
    ctl.wait_idle().await;
    let id = ctl.session_id();
    // The file is untouched: MultiEdit is not an alias of Edit.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha\n");
    // The recorded call keeps its name and its input.
    let recorded = serde_json::to_string(&ctl.history().await).unwrap();
    assert!(recorded.contains("MultiEdit"), "{recorded}");
    assert!(recorded.contains("old_string"), "{recorded}");
    ctl.close().await;

    // Reopen the session: the history loads, and a new turn runs.
    let again = Script::new(vec![Reply::text("after the reopen")]);
    let (ctl2, mut events2) = Controller::open(
        config(dir.path(), again, &sessions),
        OpenOptions {
            session: SessionChoice::Resume(id.clone()),
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    let Event::SessionOpened {
        resumed,
        message_count,
        ..
    } = tokio::time::timeout(Duration::from_secs(10), events2.next())
        .await
        .unwrap()
        .unwrap()
        .event
    else {
        panic!("the first event of a resume is SessionOpened");
    };
    assert!(resumed);
    assert!(message_count >= 3, "{message_count}");

    ctl2.send(Command::Submit {
        prompt: Prompt::text("and now?"),
    })
    .unwrap();
    let evs2 = until_finished(&mut events2).await;
    assert!(
        evs2.iter()
            .any(|e| matches!(&e.event, Event::RunFinished { .. })),
        "the resumed session finished its turn"
    );
    ctl2.wait_idle().await;
    let after = serde_json::to_string(&ctl2.history().await).unwrap();
    assert!(after.contains("MultiEdit"), "the old record is still there");
    assert!(
        after.contains("after the reopen"),
        "the new turn is recorded"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha\n");
}

/// The tools the model is offered: one `Edit`, no `MultiEdit`, and the
/// description of `Edit` names no removed tool.
#[tokio::test]
async fn the_tools_offered_to_the_model_have_one_edit_and_no_multi_edit() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join(".sessions");
    let (ctl, _events) = Controller::open(
        config(dir.path(), Script::new(vec![]), &sessions),
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    let tools = ctl.tools();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names.iter().filter(|n| **n == "Edit").count(),
        1,
        "{names:?}"
    );
    assert!(!names.contains(&"MultiEdit"), "{names:?}");
    let edit = tools.iter().find(|t| t.name == "Edit").unwrap();
    assert!(
        !edit.description.contains("MultiEdit"),
        "{}",
        edit.description
    );
    ctl.close().await;
}
