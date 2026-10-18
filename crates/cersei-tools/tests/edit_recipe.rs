//! Recipe gaps of the Edit tool, through its public API: Unicode and line
//! endings kept, one published schema and no `MultiEdit` in the catalogue,
//! and a symbolic link edited through its own path.
//!
//! The concurrency cases (barriers inside the critical section) live next to
//! the tool, in `src/file_edit.rs`, where the test-only hooks are visible.

use cersei_tools::file_edit::FileEditTool;
use cersei_tools::file_write::FileWriteTool;
use cersei_tools::notebook_edit::NotebookEditTool;
use cersei_tools::permissions::AllowAll;
use cersei_tools::{CostTracker, Extensions, Tool, ToolContext};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: "edit-recipe".into(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

fn one(path: &Path, old: &str, new: &str) -> Value {
    json!({ "file_path": path.to_str().unwrap(), "old_string": old, "new_string": new })
}

/// Accented letters, an emoji and CJK text around the match: the replaced
/// region changes, every other byte stays as it was.
#[tokio::test]
async fn unicode_match_changes_only_its_region() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("u.txt");
    let before = "café 🙂\nnaïve — 漢字\nfin ✓\n";
    std::fs::write(&path, before).unwrap();
    let res = FileEditTool
        .execute(
            one(&path, "naïve — 漢字", "NAÏVE — 漢字!"),
            &ctx(dir.path()),
        )
        .await;
    assert!(!res.is_error, "{}", res.content);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        "café 🙂\nNAÏVE — 漢字!\nfin ✓\n".as_bytes()
    );
}

/// A multi-line old string written with LF matches a CRLF file, and the
/// written lines keep CRLF: no bare LF is introduced.
#[tokio::test]
async fn multi_line_edit_of_a_crlf_file_writes_no_bare_lf() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.txt");
    std::fs::write(&path, "a\r\nb\r\nc\r\nd\r\n").unwrap();
    let res = FileEditTool
        .execute(one(&path, "b\nc", "B\nC\nC2"), &ctx(dir.path()))
        .await;
    assert!(!res.is_error, "{}", res.content);
    let after = std::fs::read(&path).unwrap();
    assert_eq!(after, b"a\r\nB\r\nC\r\nC2\r\nd\r\n");
}

/// The published schema is the one shape: `file_path` required, one
/// replacement or `edits`, nothing else.
#[test]
fn the_published_schema_has_one_replacement_or_edits_and_no_multi_edit() {
    let schema = FileEditTool.input_schema();
    let mut keys: Vec<&str> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "edits",
            "file_path",
            "new_string",
            "old_string",
            "replace_all"
        ]
    );
    assert_eq!(schema["required"], json!(["file_path"]));
    assert_eq!(
        schema["properties"]["edits"]["items"]["required"],
        json!(["old_string"])
    );
    assert!(!FileEditTool.description().contains("MultiEdit"));
    assert!(!schema.to_string().contains("MultiEdit"));
}

/// The active catalogue has exactly one `Edit` and no `MultiEdit`, and the
/// tool search does not offer one.
#[tokio::test]
async fn the_active_catalogue_has_no_multi_edit() {
    let tools = cersei_tools::all();
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(
        names.iter().filter(|n| **n == "Edit").count(),
        1,
        "{names:?}"
    );
    assert!(!names.contains(&"MultiEdit"), "{names:?}");

    let dir = tempfile::tempdir().unwrap();
    let search = tools
        .iter()
        .find(|t| t.name() == "ToolSearch")
        .expect("ToolSearch registered");
    let found = search
        .execute(json!({ "query": "MultiEdit" }), &ctx(dir.path()))
        .await;
    // The search may echo the query in its refusal; it must not offer a tool.
    assert!(
        found.content.contains("No tools found"),
        "{}",
        found.content
    );
}

/// Editing through a symbolic link changes the file the link points to and
/// keeps the link.
#[cfg(unix)]
#[tokio::test]
async fn an_edit_through_a_symlink_changes_the_target_and_keeps_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "one\n").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let res = FileEditTool
        .execute(one(&link, "one", "uno"), &ctx(dir.path()))
        .await;
    assert!(!res.is_error, "{}", res.content);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "uno\n");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link was replaced by a regular file"
    );
}

/// `Write` through a link: the file it points to gets the content.
#[cfg(unix)]
#[tokio::test]
async fn a_write_through_a_symlink_changes_the_target_and_keeps_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "one\n").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let res = FileWriteTool
        .execute(
            json!({ "file_path": link.to_str().unwrap(), "content": "two\n" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!res.is_error, "{}", res.content);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "two\n");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link was replaced by a regular file"
    );
}

/// `NotebookEdit` through a link: the notebook it points to is edited.
#[cfg(unix)]
#[tokio::test]
async fn a_notebook_edit_through_a_symlink_changes_the_target_and_keeps_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("nb.ipynb");
    let notebook = json!({
        "nbformat": 4, "nbformat_minor": 5, "metadata": {},
        "cells": [{"cell_type": "code", "source": ["x = 1\n"], "outputs": [], "metadata": {}, "execution_count": null}]
    });
    std::fs::write(&target, notebook.to_string()).unwrap();
    let link = dir.path().join("link.ipynb");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let res = NotebookEditTool
        .execute(
            json!({ "file_path": link.to_str().unwrap(), "cell_index": 0, "new_source": "x = 2" }),
            &ctx(dir.path()),
        )
        .await;
    assert!(!res.is_error, "{}", res.content);
    let edited: Value = serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(edited["cells"][0]["source"][0], json!("x = 2"));
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link was replaced by a regular file"
    );
}
