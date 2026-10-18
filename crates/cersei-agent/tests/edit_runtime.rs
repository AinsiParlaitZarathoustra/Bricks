//! Edits through the runner: the runtime does not require a read before a
//! change, a refusal is effective, and a change is decided on its current
//! state. Scripted replies drive the model; the files on disk are the
//! evidence.

use async_trait::async_trait;
use cersei_agent::Agent;
use cersei_agent::control::scripted::{Reply, Script, ScriptedProvider};
use cersei_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

/// A permission policy that counts its questions and records the diff each
/// one was asked about.
struct Asked {
    deny: bool,
    diffs: Mutex<Vec<String>>,
}

impl Asked {
    fn new(deny: bool) -> Arc<Self> {
        Arc::new(Self {
            deny,
            diffs: Mutex::new(Vec::new()),
        })
    }

    fn questions(&self) -> usize {
        self.diffs.lock().unwrap().len()
    }
}

#[async_trait]
impl PermissionPolicy for Asked {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        let diff = request
            .preview
            .as_ref()
            .and_then(|p| p.files.first())
            .map(|f| f.diff.clone())
            .unwrap_or_default();
        self.diffs.lock().unwrap().push(diff);
        if self.deny {
            PermissionDecision::Deny("not today".into())
        } else {
            PermissionDecision::Allow
        }
    }
}

fn write_file(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// Builds an agent that runs `replies` with the given policy and tools.
fn agent(dir: &Path, replies: Vec<Reply>, policy: Arc<Asked>) -> Agent {
    Agent::builder()
        .provider(ScriptedProvider::new("a", Script::new(replies)))
        .model("a")
        .working_dir(dir)
        .tools(cersei_tools::filesystem())
        .permission_policy_arc(policy)
        .build()
        .expect("build agent")
}

/// The tool call with id `id` of the run.
fn call<'a>(
    calls: &'a [cersei_agent::ToolCallRecord],
    id: &str,
) -> &'a cersei_agent::ToolCallRecord {
    calls
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no tool call {id} in {calls:?}"))
}

#[tokio::test]
async fn an_edit_of_an_unread_file_runs_when_it_is_permitted() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(dir.path(), "a.txt", "alpha\nbeta\n");
    let policy = Asked::new(false);
    let out = agent(
        dir.path(),
        vec![
            Reply::tool(
                "e1",
                "Edit",
                json!({"file_path": path, "old_string": "beta", "new_string": "BETA"}),
            ),
            Reply::text("done"),
        ],
        Arc::clone(&policy),
    )
    .run("edit a.txt")
    .await
    .unwrap();

    let e = call(&out.tool_calls, "e1");
    assert!(!e.is_error, "{}", e.result);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\nBETA\n");
    assert_eq!(policy.questions(), 1);
}

/// A refusal of permission is effective: the edit is not written.
#[tokio::test]
async fn a_denied_edit_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(dir.path(), "a.txt", "alpha\nbeta\n");
    let policy = Asked::new(true);
    let out = agent(
        dir.path(),
        vec![
            Reply::tool(
                "e1",
                "Edit",
                json!({"file_path": path, "old_string": "beta", "new_string": "BETA"}),
            ),
            Reply::text("done"),
        ],
        Arc::clone(&policy),
    )
    .run("edit a.txt")
    .await
    .unwrap();

    let e = call(&out.tool_calls, "e1");
    assert!(
        e.is_error && e.result.contains("Permission denied"),
        "{}",
        e.result
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\nbeta\n");
    assert_eq!(policy.questions(), 1);
}

/// A read and an edit of the same file in one batch: no refusal is made up
/// for the read that has not landed yet.
#[tokio::test]
async fn a_read_and_an_edit_of_one_file_in_one_batch_are_both_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(dir.path(), "a.txt", "alpha\nbeta\n");
    let out = agent(
        dir.path(),
        vec![
            Reply::tools(vec![
                ("r1", "Read", json!({"file_path": path})),
                (
                    "e1",
                    "Edit",
                    json!({"file_path": path, "old_string": "beta", "new_string": "BETA"}),
                ),
            ]),
            Reply::text("done"),
        ],
        Asked::new(false),
    )
    .run("read and edit a.txt")
    .await
    .unwrap();

    assert!(!call(&out.tool_calls, "r1").is_error);
    let e = call(&out.tool_calls, "e1");
    assert!(!e.is_error, "{}", e.result);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\nBETA\n");
}

/// Writing, notebook editing and patching an existing file need no read
/// either.
#[tokio::test]
async fn write_notebook_and_patch_of_unread_files_run() {
    let dir = tempfile::tempdir().unwrap();
    let text = write_file(dir.path(), "w.txt", "old\n");
    let nb = write_file(
        dir.path(),
        "n.ipynb",
        &json!({
            "nbformat": 4, "nbformat_minor": 5, "metadata": {},
            "cells": [{"cell_type": "code", "source": ["x = 1\n"], "outputs": [], "metadata": {}, "execution_count": null}]
        })
        .to_string(),
    );
    let patched = write_file(dir.path(), "p.txt", "one\ntwo\n");
    let patch = "--- a/p.txt\n+++ b/p.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n";
    let out = agent(
        dir.path(),
        vec![
            Reply::tool(
                "w1",
                "Write",
                json!({"file_path": text, "content": "new\n"}),
            ),
            Reply::tool(
                "n1",
                "NotebookEdit",
                json!({"file_path": nb, "cell_index": 0, "new_source": "x = 2"}),
            ),
            Reply::tool("p1", "ApplyPatch", json!({"patch": patch})),
            Reply::text("done"),
        ],
        Asked::new(false),
    )
    .run("update the files")
    .await
    .unwrap();

    for id in ["w1", "n1", "p1"] {
        let c = call(&out.tool_calls, id);
        assert!(!c.is_error, "{id}: {}", c.result);
    }
    assert_eq!(std::fs::read_to_string(&text).unwrap(), "new\n");
    assert!(std::fs::read_to_string(&nb).unwrap().contains("x = 2"));
    assert_eq!(std::fs::read_to_string(&patched).unwrap(), "one\nTWO\n");
}
