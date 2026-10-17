//! Sprint 14.4, validation: the command labels of runs survive a restart of the
//! desktop. The store is the one the engine opens under its home folder
//! (`<home>/desktop/run-labels`, see `engine/mod.rs`). A restart is simulated by
//! dropping the store and opening it again on the same temporary home.
//!
//! Nothing here opens a conversation: `AppState::open_session` reads the global
//! session folder (see the report on `engine/sessions.rs`), so these tests only use
//! the label store and the wire types.

use bricks_desktop_lib::contract::{DesktopEvent, OpenedSession, RunLabel};
use bricks_desktop_lib::services::run_labels::{RunLabelStore, normalize_label};
use serde_json::json;
use std::path::Path;

fn label(run: &str, text: &str, prompt: &str, at: i64) -> RunLabel {
    RunLabel {
        run_id: run.into(),
        label: text.into(),
        prompt: prompt.into(),
        at,
        user_index: None,
    }
}

/// The store the desktop opens under its home folder.
fn store_in(home: &Path) -> RunLabelStore {
    RunLabelStore::at(home.join("desktop").join("run-labels"))
}

#[test]
fn labels_are_still_there_after_a_restart_in_their_order() {
    let home = tempfile::tempdir().unwrap();
    let first = label("run_a", "/skill debug", "corrige le test", 1);
    let second = label("run_b", "/skill review", "relis", 2);
    {
        let before = store_in(home.path());
        assert!(before.append("sess-1", &first).unwrap());
        assert!(before.append("sess-1", &second).unwrap());
    } // the process stops here
    let after = store_in(home.path());
    assert_eq!(after.list("sess-1").unwrap(), vec![first, second]);
}

#[test]
fn a_run_labelled_before_the_restart_is_not_labelled_again_after_it() {
    // A replay of the run after a restart must not add a second label for it.
    let home = tempfile::tempdir().unwrap();
    let first = label("run_a", "/skill debug", "corrige le test", 1);
    assert!(store_in(home.path()).append("sess-1", &first).unwrap());

    let after = store_in(home.path());
    assert!(!after.append("sess-1", &label("run_a", "/skill debug", "corrige le test", 9)).unwrap());
    assert_eq!(after.list("sess-1").unwrap(), vec![first]);
}

#[test]
fn labels_stay_apart_per_session_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    assert!(store_in(home.path())
        .append("sess-1", &label("run_a", "/skill debug", "x", 1))
        .unwrap());
    assert!(store_in(home.path())
        .append("sess-2", &label("run_b", "/skill review", "y", 2))
        .unwrap());

    let after = store_in(home.path());
    assert_eq!(after.list("sess-1").unwrap()[0].run_id, "run_a");
    assert_eq!(after.list("sess-2").unwrap()[0].run_id, "run_b");
    assert_eq!(after.list("sess-1").unwrap().len(), 1);
    assert_eq!(after.list("sess-2").unwrap().len(), 1);
}

#[test]
fn a_deleted_session_has_no_labels_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    assert!(store_in(home.path())
        .append("sess-1", &label("run_a", "/skill debug", "x", 1))
        .unwrap());
    store_in(home.path()).delete("sess-1").unwrap();

    let after = store_in(home.path());
    assert!(!after.exists("sess-1"));
    assert!(after.list("sess-1").unwrap().is_empty());
}

#[test]
fn a_label_cut_short_by_a_crash_does_not_hide_the_labels_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let first = label("run_a", "/skill debug", "x", 1);
    store_in(home.path()).append("sess-1", &first).unwrap();

    // A write cut short by a crash: a broken line without its newline.
    let file = home.path().join("desktop").join("run-labels").join("sess-1.jsonl");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("{\"run_id\":\"run_x\",\"lab");
    std::fs::write(&file, text).unwrap();

    let after = store_in(home.path());
    assert_eq!(after.list("sess-1").unwrap(), vec![first.clone()]);
    let second = label("run_b", "/skill review", "relis", 2);
    assert!(after.append("sess-1", &second).unwrap());

    let again = store_in(home.path());
    assert_eq!(again.list("sess-1").unwrap(), vec![first, second]);
}

#[test]
fn the_label_kept_is_the_trimmed_one() {
    let home = tempfile::tempdir().unwrap();
    let typed = normalize_label("  /skill debug  ").expect("a label");
    store_in(home.path())
        .append("sess-1", &label("run_a", &typed, "x", 1))
        .unwrap();

    let after = store_in(home.path());
    assert_eq!(after.list("sess-1").unwrap()[0].label, "/skill debug");
}

#[test]
fn a_session_opened_without_a_run_labels_field_reads_as_no_label() {
    // A session opened by an older desktop has no `run_labels` key: it reads as none.
    let wire = json!({
        "session": {
            "id": "sess-1",
            "project_id": "proj-1",
            "title": "Essai",
            "created_at": 1,
            "updated_at": 1,
            "message_count": 0,
            "model": "p/m",
            "reasoning": null,
            "working_dir": "/work/demo",
            "open": true,
            "running": false
        },
        "snapshot": {},
        "history": [],
        "recent_events": [],
        "last_seq": 0
    });
    let opened: OpenedSession = serde_json::from_value(wire).unwrap();
    assert!(opened.run_labels.is_empty());
}

#[test]
fn the_run_labeled_event_has_the_shape_the_frontend_reads() {
    let event = DesktopEvent::RunLabeled {
        session_id: "sess-1".into(),
        label: label("run_a", "/skill debug", "fix it", 5),
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({
            "kind": "run_labeled",
            "session_id": "sess-1",
            "label": { "run_id": "run_a", "label": "/skill debug", "prompt": "fix it", "at": 5, "user_index": null }
        })
    );
}

#[test]
fn a_label_stored_before_the_message_index_reads_back_without_one() {
    // Labels written by an older desktop have no `user_index` key: they still load.
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("desktop").join("run-labels");
    std::fs::create_dir_all(&dir).unwrap();
    let old = r#"{"run_id":"run_a","label":"/skill debug","prompt":"corrige","at":1}"#;
    std::fs::write(dir.join("sess-1.jsonl"), format!("{old}\n")).unwrap();

    let listed = store_in(home.path()).list("sess-1").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].user_index, None);
    assert_eq!(listed[0].label, "/skill debug");
}

#[test]
fn the_message_index_is_kept_across_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let mut indexed = label("run_a", "/skill debug", "même", 1);
    indexed.user_index = Some(3);
    store_in(home.path()).append("sess-1", &indexed).unwrap();

    let after = store_in(home.path());
    assert_eq!(after.list("sess-1").unwrap()[0].user_index, Some(3));
}
