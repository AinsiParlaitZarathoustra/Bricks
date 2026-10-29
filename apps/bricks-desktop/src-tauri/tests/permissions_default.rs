//! The permission settings the desktop keeps, in a temporary home (nothing
//! touches `~/.bricks`):
//! - the default policy for new sessions is `<home>/permissions.json`: chosen
//!   once, read at every start, and never taken for "nothing chosen" when the
//!   file is unreadable;
//! - `preferences.json` no longer has a tool-approval setting: a file written
//!   with the former `tool_approval` key keeps its other values, never chooses
//!   a policy, and loses the key on the next save.

use bricks_agent::permission::{
    CustomRules, PermissionEffect, PermissionProfile, PermissionStore, SessionPolicy,
};
use bricks_desktop_lib::contract::{Preferences, Theme};
use bricks_desktop_lib::services::prefs::PrefsStore;

fn prefs_file(home: &std::path::Path) -> std::path::PathBuf {
    home.join("desktop").join("preferences.json")
}

#[test]
fn the_default_policy_is_chosen_once_and_read_at_every_start() {
    let home = tempfile::tempdir().unwrap();
    let store = PermissionStore::in_home(home.path());
    assert_eq!(store.default_policy().unwrap(), None, "nothing chosen yet");
    assert_eq!(store.initial_policy(), SessionPolicy::unchosen());

    store
        .set_default(&SessionPolicy::chosen(PermissionProfile::Execute))
        .unwrap();
    // The next start: a new store on the same home.
    let next_start = PermissionStore::in_home(home.path());
    let kept = next_start.initial_policy();
    assert_eq!(kept.profile, PermissionProfile::Execute);
    assert!(kept.chosen, "a saved default is a choice");

    // A new choice replaces it; the next start reads that one.
    let mut custom = SessionPolicy::chosen(PermissionProfile::Custom);
    custom
        .custom
        .tools
        .insert("Bash".into(), PermissionEffect::Deny);
    next_start.set_default(&custom).unwrap();
    assert_eq!(
        PermissionStore::in_home(home.path()).initial_policy(),
        custom
    );
}

#[test]
fn an_unreadable_default_file_is_an_error_and_new_sessions_ask() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("permissions.json"), "{ cassé").unwrap();
    let store = PermissionStore::in_home(home.path());
    let err = store
        .default_policy()
        .expect_err("reported, never read as nothing");
    assert!(err.contains("permissions.json"), "{err}");
    assert_eq!(
        store.initial_policy(),
        SessionPolicy::unchosen(),
        "the session starts with nothing chosen (asked)"
    );
}

#[test]
fn a_custom_default_round_trips_with_its_rules() {
    let home = tempfile::tempdir().unwrap();
    let store = PermissionStore::in_home(home.path());
    let mut policy = SessionPolicy::chosen(PermissionProfile::Custom);
    policy.custom = CustomRules {
        read: PermissionEffect::Allow,
        write: PermissionEffect::Deny,
        execute: PermissionEffect::Ask,
        network: PermissionEffect::Deny,
        tools: [("Write".to_string(), PermissionEffect::Allow)]
            .into_iter()
            .collect(),
    };
    store.set_default(&policy).unwrap();
    assert_eq!(store.default_policy().unwrap(), Some(policy.clone()));
    // The wire form the frontend reads and writes.
    let wire = serde_json::to_value(&policy).unwrap();
    assert_eq!(wire["profile"], "custom");
    assert_eq!(wire["chosen"], true);
    assert_eq!(wire["custom"]["write"], "deny");
    assert_eq!(wire["custom"]["tools"]["Write"], "allow");
}

#[test]
fn the_preferences_have_no_tool_approval_setting() {
    let value = serde_json::to_value(Preferences::default()).unwrap();
    assert!(
        value.get("tool_approval").is_none(),
        "the approval setting is gone from the preferences"
    );
}

#[test]
fn an_old_file_with_the_former_setting_keeps_its_values_and_never_chooses_a_policy() {
    let home = tempfile::tempdir().unwrap();
    let file = prefs_file(home.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    for former in [
        "\"no_confirmation\"",
        "\"always_ask\"",
        "\"standard\"",
        "\"yolo\"",
    ] {
        std::fs::write(
            &file,
            format!("{{\"theme\": \"dark\", \"left_width\": 300, \"tool_approval\": {former}}}"),
        )
        .unwrap();
        let prefs = PrefsStore::at(file.clone()).get();
        assert!(
            matches!(prefs.theme, Theme::Dark),
            "{former}: theme is read"
        );
        assert_eq!(prefs.left_width, 300, "{former}: width is read");
        // The former setting chooses no default for new sessions.
        assert_eq!(
            PermissionStore::in_home(home.path())
                .default_policy()
                .unwrap(),
            None,
            "{former}: nothing is chosen from it"
        );
    }
}

#[test]
fn a_save_drops_the_former_setting_and_keeps_the_other_values() {
    let home = tempfile::tempdir().unwrap();
    let file = prefs_file(home.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        r#"{"theme": "dark", "left_width": 300, "tool_approval": "no_confirmation"}"#,
    )
    .unwrap();
    let store = PrefsStore::at(file.clone());
    let prefs = store.get();
    store.set(prefs).unwrap();

    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(
        saved.get("tool_approval").is_none(),
        "the key is not written back"
    );
    assert_eq!(saved["theme"], "dark");
    assert_eq!(saved["left_width"], 300);
}
