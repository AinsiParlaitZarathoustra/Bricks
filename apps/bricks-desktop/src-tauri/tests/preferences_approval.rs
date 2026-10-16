//! The tool-approval preference (Sprint 14.2) in `preferences.json`: it is
//! saved, it is read back after a restart, an old file without it means
//! `standard`, and a value the file does not know never loosens the policy.
//! Every file is in a temporary folder; nothing touches `~/.bricks`.

use bricks_desktop_lib::contract::{Preferences, Theme, ToolApproval};
use bricks_desktop_lib::engine::approval_mode;
use bricks_desktop_lib::services::prefs::PrefsStore;
use cersei_agent::control::ApprovalMode;

fn file_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("desktop").join("preferences.json")
}

#[test]
fn each_policy_is_saved_as_its_wire_value_and_read_back_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = file_in(&dir);
    for (policy, wire, mode) in [
        (ToolApproval::Standard, "standard", ApprovalMode::Standard),
        (
            ToolApproval::AlwaysAsk,
            "always_ask",
            ApprovalMode::AlwaysAsk,
        ),
        (
            ToolApproval::NoConfirmation,
            "no_confirmation",
            ApprovalMode::NoConfirmation,
        ),
    ] {
        let prefs = Preferences {
            tool_approval: policy,
            ..Preferences::default()
        };
        assert_eq!(
            serde_json::to_value(&prefs).unwrap()["tool_approval"],
            wire,
            "the frontend reads this value"
        );
        PrefsStore::at(file.clone()).set(prefs).expect("saved");

        // The next start: a new store on the same file.
        let back = PrefsStore::at(file.clone()).get();
        assert_eq!(back.tool_approval, policy, "{wire} survives a restart");
        assert_eq!(approval_mode(back.tool_approval), mode);
    }
}

#[test]
fn a_saved_policy_keeps_the_other_preferences_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = file_in(&dir);
    let store = PrefsStore::at(file.clone());
    let prefs = Preferences {
        theme: Theme::Dark,
        left_width: 300,
        tool_approval: ToolApproval::AlwaysAsk,
        ..Preferences::default()
    };
    store.set(prefs).unwrap();
    let mut again = PrefsStore::at(file.clone()).get();
    assert!(matches!(again.theme, Theme::Dark));
    assert_eq!(again.left_width, 300);
    again.tool_approval = ToolApproval::NoConfirmation;
    PrefsStore::at(file.clone()).set(again).unwrap();

    let last = PrefsStore::at(file).get();
    assert_eq!(last.tool_approval, ToolApproval::NoConfirmation);
    assert!(matches!(last.theme, Theme::Dark), "the theme is kept");
    assert_eq!(last.left_width, 300, "the width is kept");
}

#[test]
fn an_old_file_without_the_setting_means_standard_and_keeps_its_values() {
    let dir = tempfile::tempdir().unwrap();
    let file = file_in(&dir);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        r#"{
  "theme": "dark",
  "left_open": false,
  "right_open": true,
  "left_width": 300,
  "right_width": 320,
  "terminal_open": true,
  "terminal_height": 200,
  "last_project_id": "proj-old",
  "last_session_id": null
}"#,
    )
    .unwrap();
    let prefs = PrefsStore::at(file).get();
    assert_eq!(
        prefs.tool_approval,
        ToolApproval::Standard,
        "absent means standard"
    );
    assert_eq!(approval_mode(prefs.tool_approval), ApprovalMode::Standard);
    assert!(
        matches!(prefs.theme, Theme::Dark),
        "the other values are read"
    );
    assert_eq!(prefs.left_width, 300);
    assert!(!prefs.left_open);
    assert_eq!(prefs.last_project_id.as_deref(), Some("proj-old"));
}

#[test]
fn a_value_the_file_does_not_know_never_becomes_a_looser_policy() {
    let dir = tempfile::tempdir().unwrap();
    let file = file_in(&dir);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    for unknown in [
        "\"yolo\"",
        "\"no-confirmation\"",
        "\"NoConfirmation\"",
        "null",
    ] {
        std::fs::write(&file, format!("{{\"tool_approval\": {unknown}}}")).unwrap();
        let prefs = PrefsStore::at(file.clone()).get();
        assert_ne!(
            prefs.tool_approval,
            ToolApproval::NoConfirmation,
            "{unknown} must not allow every call"
        );
        assert_eq!(
            approval_mode(prefs.tool_approval),
            ApprovalMode::Standard,
            "{unknown} falls back to the defaults"
        );
    }
}

#[test]
fn a_save_that_cannot_be_written_changes_nothing_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    // The parent of the file is a regular file: the folder cannot be created.
    let blocker = dir.path().join("desktop");
    std::fs::write(&blocker, "pas un dossier").unwrap();
    let store = PrefsStore::at(blocker.join("preferences.json"));
    let prefs = Preferences {
        tool_approval: ToolApproval::NoConfirmation,
        ..Preferences::default()
    };
    assert!(store.set(prefs).is_err(), "the save is reported");
    assert_eq!(
        store.get().tool_approval,
        ToolApproval::Standard,
        "the policy read back is the previous one"
    );
    assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "pas un dossier");
}
