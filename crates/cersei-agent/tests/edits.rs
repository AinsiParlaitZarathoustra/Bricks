//! `edit_applied`, the source of the interface's `+N −N`: emitted once per
//! successful structured change, with the counts of the complete diff;
//! never for a refusal, a denial or a failed write.

use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::*;
use cersei_agent::BricksConfig;
use serde_json::json;
use std::time::Duration;

async fn run(dir: &std::path::Path, replies: Vec<Reply>, rules: ApprovalRules) -> Vec<Envelope> {
    let script = Script::new(replies);
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.permissions = rules;
    let mut cfg = EngineConfig::new(
        dir,
        ScriptedCatalog::new(&["a"], script),
        bricks,
        dir.join(".sessions"),
    );
    cfg.interactive = false;
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    ctl.send(Command::Submit {
        prompt: Prompt::text("go"),
    })
    .unwrap();
    let mut out = Vec::new();
    loop {
        let e = tokio::time::timeout(Duration::from_secs(30), events.next())
            .await
            .unwrap()
            .unwrap();
        let done = matches!(e.event, Event::RunFinished { .. });
        out.push(e);
        if done {
            return out;
        }
    }
}

fn allow_writes() -> ApprovalRules {
    ApprovalRules {
        write: Action::Allow,
        ..Default::default()
    }
}

/// `(tool_call_id, [(path, added, removed)])`.
type Applied = (String, Vec<(String, usize, usize)>);

/// `(tool_call_id, [(path, added, removed)])` of every `edit_applied`.
fn applied(evs: &[Envelope]) -> Vec<Applied> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::EditApplied {
                tool_call_id,
                files,
                agent_id,
                changeset_id,
                ..
            } => {
                assert!(agent_id.is_none() && changeset_id.is_none());
                Some((
                    tool_call_id.clone(),
                    files
                        .iter()
                        .map(|f| (f.path.clone(), f.added, f.removed))
                        .collect(),
                ))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn every_native_edit_tool_reports_its_complete_diff_once() {
    let dir = tempfile::tempdir().unwrap();
    // A large file: the counts are those of the whole diff, whatever the
    // shown output keeps.
    let big: String = (0..5000).map(|i| format!("line {i}\n")).collect();
    std::fs::write(dir.path().join("p.txt"), "un\ndeux\ntrois\n").unwrap();
    let patch = "--- a/p.txt\n+++ b/p.txt\n@@ -1,3 +1,3 @@\n un\n-deux\n+DEUX\n trois\n";
    let evs = run(
        dir.path(),
        vec![
            Reply::tool("w1", "Write", json!({"file_path": "big.txt", "content": big})),
            Reply::tool(
                "e1",
                "Edit",
                json!({"file_path": "big.txt", "old_string": "line 10\n", "new_string": "ten\nTEN\n"}),
            ),
            Reply::tool(
                "m1",
                "MultiEdit",
                json!({"file_path": "big.txt", "edits": [
                    {"old_string": "line 20\n", "new_string": ""},
                    {"old_string": "line 30\n", "new_string": "thirty\n"}
                ]}),
            ),
            Reply::tool("r1", "Read", json!({"file_path": "p.txt"})),
            Reply::tool("p1", "ApplyPatch", json!({"patch": patch})),
            // No final newline: one line all the same.
            Reply::tool("w2", "Write", json!({"file_path": "last.txt", "content": "sans fin"})),
            Reply::text("fini"),
        ],
        allow_writes(),
    )
    .await;
    let a = applied(&evs);
    assert_eq!(
        a,
        vec![
            ("w1".into(), vec![("big.txt".into(), 5000, 0)]),
            ("e1".into(), vec![("big.txt".into(), 2, 1)]),
            ("m1".into(), vec![("big.txt".into(), 1, 2)]),
            ("p1".into(), vec![("p.txt".into(), 1, 1)]),
            ("w2".into(), vec![("last.txt".into(), 1, 0)]),
        ],
        "{a:?}"
    );
}

#[tokio::test]
async fn refusals_denials_and_failures_report_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a_file"), "x").unwrap();
    // Denied by the policy (nobody to ask): previewed, never written.
    let deny = ApprovalRules {
        write: Action::Deny,
        ..Default::default()
    };
    let evs = run(
        dir.path(),
        vec![
            Reply::tool(
                "w1",
                "Write",
                json!({"file_path": "n.txt", "content": "x\n"}),
            ),
            Reply::text("ok"),
        ],
        deny,
    )
    .await;
    assert!(applied(&evs).is_empty());
    assert!(!dir.path().join("n.txt").exists());
    // Allowed, but the tool refuses (nothing matches) or the write fails
    // (its parent is a file): no change reported.
    let evs = run(
        dir.path(),
        vec![
            Reply::tool("r1", "Read", json!({"file_path": "a_file"})),
            Reply::tool(
                "e1",
                "Edit",
                json!({"file_path": "a_file", "old_string": "absent", "new_string": "y"}),
            ),
            Reply::tool(
                "w2",
                "Write",
                json!({"file_path": "a_file/child.txt", "content": "x\n"}),
            ),
            // Writing the same content again: a change of +0 −0.
            Reply::tool(
                "w3",
                "Write",
                json!({"file_path": "a_file", "content": "x"}),
            ),
            Reply::text("ok"),
        ],
        allow_writes(),
    )
    .await;
    let a = applied(&evs);
    assert!(a.iter().all(|(id, _)| id == "w3"), "{a:?}");
    assert!(
        a.iter()
            .all(|(_, f)| f.iter().all(|x| (x.1, x.2) == (0, 0))),
        "{a:?}"
    );
}
