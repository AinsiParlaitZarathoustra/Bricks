//!.3, validation: reasoning profiles and the model form, end to end
//! on a temporary `providers.toml`. The patches are read from the JSON the
//! desktop form sends (`src/lib/features/settings/models.ts` `buildModelPatch`),
//! so the wire shape is checked as well as the write.
//!
//! Covered here: a free-named profile with nested parameters is saved, read
//! back after a restart and listed by the composer's catalogue; duplicate ids,
//! a parameter that is not a JSON object and a removed default are refused
//! with the file unchanged; a model edit from the form keeps the context
//! window the form does not show.
//!
//! Nothing touches `~/.bricks`: every file is in a temporary folder.

use bricks_agent::control::ModelCatalog;
use bricks_desktop_lib::contract::{IpcErrorCode, IpcResult, ModelPatch, ProvidersFile};
use bricks_desktop_lib::engine::catalog::DesktopCatalog;
use bricks_desktop_lib::services::providers::{commit, view};
use bricks_desktop_lib::services::providers_toml::Edit;
use bricks_provider::config::ProviderConfig;
use serde_json::{Value as Json, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One provider, one model with a context window inside its limits, and one
/// profile that is the default. Comments must survive every write.
const FILE: &str = r#"# Fournisseur de test : en-tête à conserver
schema_version = 1

[[providers]]
id = "local"
name = "Local"
endpoint = "http://127.0.0.1:9/v1"
protocol = "chat_completions"
auth = "none"

# Le modèle de test
[[providers.models]]
id = "small"
name = "Small"
api_model = "small-1"
[providers.models.limits]
max_input_tokens = 8000
max_output_tokens = 1000
context_window_tokens = 16000
[providers.models.reasoning]
default = "fast"
[[providers.models.reasoning.profiles]]
id = "fast"
label = "Rapide"
"#;

struct Setup {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    std::fs::write(&path, FILE).unwrap();
    Setup { _dir: dir, path }
}

fn no_credentials(_: &ProviderConfig) -> bool {
    false
}

fn current(path: &Path) -> ProvidersFile {
    view(path, &no_credentials)
}

fn revision(path: &Path) -> String {
    current(path).revision
}

/// The patch as the form sends it: the JSON of `ModelPatch`, read by serde.
fn patch_from_form(json: Json) -> ModelPatch {
    serde_json::from_value(json).expect("the form's JSON is a ModelPatch")
}

/// A model patch with every field unchanged except the given ones, shaped as
/// `buildModelPatch` sends it (`context_window_tokens` and `clear_context_window`
/// are never set by the form).
fn form_patch(changes: Json) -> ModelPatch {
    let mut base = json!({
        "name": null,
        "api_model": null,
        "max_input_tokens": null,
        "max_output_tokens": null,
        "context_window_tokens": null,
        "clear_context_window": false,
        "input_modalities": null,
        "reasoning_default": null,
        "clear_reasoning_default": false,
        "profiles": null
    });
    for (key, value) in changes.as_object().unwrap() {
        base[key] = value.clone();
    }
    patch_from_form(base)
}

fn model_edit(patch: ModelPatch) -> Edit {
    Edit::ModelUpdate {
        provider_id: "local".into(),
        model_id: "small".into(),
        patch,
    }
}

/// The checked write of the command, then the file as the next start reads it.
fn save(path: &Path, rev: &str, patch: ModelPatch) -> IpcResult<()> {
    commit(path, rev, &model_edit(patch))
}

fn model_view(path: &Path) -> bricks_desktop_lib::contract::ModelView {
    current(path).providers[0].models[0].clone()
}

fn fresh_catalog(path: &Path) -> Arc<DesktopCatalog> {
    DesktopCatalog::new(bricks_config::load_registry(Some(path)).map(Arc::new))
}

#[test]
fn a_model_edit_from_the_form_keeps_the_context_window_and_the_profiles() {
    let s = setup();
    let rev = revision(&s.path);
    save(
        &s.path,
        &rev,
        form_patch(json!({ "max_input_tokens": 9000 })),
    )
    .expect("the form's edit is written");

    let model = model_view(&s.path);
    assert_eq!(model.max_input_tokens, 9000, "the edited field changed");
    assert_eq!(
        model.context_window_tokens,
        Some(16_000),
        "the window the form does not show is kept"
    );
    assert_eq!(model.reasoning_default.as_deref(), Some("fast"));
    assert_eq!(model.profiles.len(), 1, "a null list leaves the profiles alone");
    assert_eq!(model.profiles[0].id, "fast");
    assert_eq!(model.profiles[0].label.as_deref(), Some("Rapide"));
}

#[test]
fn a_free_named_profile_with_nested_values_is_read_back_after_a_restart() {
    let s = setup();
    let rev = revision(&s.path);
    save(
        &s.path,
        &rev,
        form_patch(json!({
            "profiles": [
                { "id": "fast", "label": null, "parameters": {}, "remove": [] },
                {
                    "id": "approfondi",
                    "label": "Approfondi",
                    "parameters": { "reasoning": { "effort": "high", "budget": 2048 }, "stop": ["fin"] },
                    "remove": ["/temperature"]
                }
            ]
        })),
    )
    .expect("the free name and the nested values are saved");

    // The view of the file the form reloads.
    let model = model_view(&s.path);
    let saved = model
        .profiles
        .iter()
        .find(|p| p.id == "approfondi")
        .expect("the saved profile is listed");
    assert_eq!(saved.label.as_deref(), Some("Approfondi"));
    assert_eq!(
        saved.parameters,
        json!({ "reasoning": { "effort": "high", "budget": 2048 }, "stop": ["fin"] })
    );
    assert_eq!(saved.remove, vec!["/temperature".to_string()]);
    assert_eq!(model.context_window_tokens, Some(16_000), "the window is kept too");

    // The next start: the composer's catalogue lists the saved profile.
    let catalog = fresh_catalog(&s.path);
    let choice = catalog
        .models()
        .into_iter()
        .find(|m| m.selection == "local/small")
        .expect("the model is listed after a restart");
    let listed = choice
        .profiles
        .iter()
        .find(|p| p.id == "approfondi")
        .expect("the composer offers the saved profile");
    assert_eq!(listed.label, "Approfondi");
    assert_eq!(choice.default_profile.as_deref(), Some("fast"));
    assert_eq!(choice.context_window_tokens, Some(16_000));
}

#[test]
fn a_duplicate_profile_id_is_refused_and_the_file_is_unchanged() {
    let s = setup();
    let before = std::fs::read_to_string(&s.path).unwrap();
    let rev = revision(&s.path);
    let err = save(
        &s.path,
        &rev,
        form_patch(json!({
            "profiles": [
                { "id": "fast", "label": null, "parameters": {}, "remove": [] },
                { "id": "fast", "label": "Autre", "parameters": {}, "remove": [] }
            ]
        })),
    )
    .expect_err("two profiles with one id are refused");
    assert_eq!(err.code, IpcErrorCode::Invalid);
    assert!(err.message.contains("en double"), "{}", err.message);
    assert_eq!(std::fs::read_to_string(&s.path).unwrap(), before, "nothing written");
    assert_eq!(revision(&s.path), rev, "the revision is unchanged");
}

#[test]
fn a_parameter_block_that_is_not_a_json_object_is_refused_and_nothing_is_written() {
    let s = setup();
    let before = std::fs::read_to_string(&s.path).unwrap();
    let rev = revision(&s.path);
    for parameters in [json!(["a", "b"]), json!(3), json!("texte"), json!(null)] {
        let err = save(
            &s.path,
            &rev,
            form_patch(json!({
                "profiles": [{ "id": "fast", "label": null, "parameters": parameters, "remove": [] }]
            })),
        )
        .expect_err("a parameter block must be a JSON object");
        assert_eq!(err.code, IpcErrorCode::Invalid, "{parameters}: {}", err.message);
        assert_eq!(std::fs::read_to_string(&s.path).unwrap(), before, "nothing written for {parameters}");
    }
}

#[test]
fn a_protected_request_field_in_a_profile_is_refused_by_the_engine_rules() {
    let s = setup();
    let before = std::fs::read_to_string(&s.path).unwrap();
    let rev = revision(&s.path);
    // `messages` is built by the engine for every request: a profile cannot replace it.
    let err = save(
        &s.path,
        &rev,
        form_patch(json!({
            "profiles": [{ "id": "fast", "label": null, "parameters": { "messages": [] }, "remove": [] }]
        })),
    )
    .expect_err("a protected field is refused");
    assert_eq!(err.code, IpcErrorCode::Invalid);
    assert!(err.message.contains("messages"), "{}", err.message);
    assert_eq!(std::fs::read_to_string(&s.path).unwrap(), before, "nothing written");
}

#[test]
fn a_removed_default_profile_without_a_replacement_is_refused_and_nothing_is_written() {
    let s = setup();
    let before = std::fs::read_to_string(&s.path).unwrap();
    let rev = revision(&s.path);
    // `fast` is the default: a list without it must say what the default becomes.
    let err = save(
        &s.path,
        &rev,
        form_patch(json!({
            "profiles": [{ "id": "approfondi", "label": null, "parameters": {"temperature": 0.2}, "remove": [] }]
        })),
    )
    .expect_err("the default cannot silently disappear");
    assert_eq!(err.code, IpcErrorCode::Invalid);
    assert!(err.message.contains("« fast »"), "{}", err.message);
    assert_eq!(std::fs::read_to_string(&s.path).unwrap(), before, "nothing written");
}

#[test]
fn a_removed_default_is_replaced_in_the_same_patch_or_cleared() {
    let s = setup();
    let rev = revision(&s.path);
    save(
        &s.path,
        &rev,
        form_patch(json!({
            "profiles": [{ "id": "approfondi", "label": null, "parameters": {"temperature": 0.2}, "remove": [] }],
            "reasoning_default": "approfondi"
        })),
    )
    .expect("the default is named in the same patch");
    let model = model_view(&s.path);
    assert_eq!(model.reasoning_default.as_deref(), Some("approfondi"));
    assert_eq!(model.profiles.len(), 1);

    let rev = revision(&s.path);
    save(
        &s.path,
        &rev,
        form_patch(json!({
            "profiles": [],
            "clear_reasoning_default": true
        })),
    )
    .expect("the default is cleared with the list");
    let model = model_view(&s.path);
    assert_eq!(model.reasoning_default, None);
    assert!(model.profiles.is_empty());
    assert_eq!(model.context_window_tokens, Some(16_000));
}

#[test]
fn a_default_outside_the_list_is_refused_and_nothing_is_written() {
    let s = setup();
    let before = std::fs::read_to_string(&s.path).unwrap();
    let rev = revision(&s.path);
    let err = save(
        &s.path,
        &rev,
        form_patch(json!({
            "profiles": [{ "id": "fast", "label": null, "parameters": {}, "remove": [] }],
            "reasoning_default": "fantome"
        })),
    )
    .expect_err("a default must be one of the profiles");
    assert_eq!(err.code, IpcErrorCode::Invalid);
    assert!(err.message.contains("« fantome »"), "{}", err.message);
    assert_eq!(std::fs::read_to_string(&s.path).unwrap(), before, "nothing written");
}

#[test]
fn a_pointer_without_its_slash_is_refused_and_nothing_is_written() {
    let s = setup();
    let before = std::fs::read_to_string(&s.path).unwrap();
    let rev = revision(&s.path);
    let err = save(
        &s.path,
        &rev,
        form_patch(json!({
            "profiles": [{ "id": "fast", "label": null, "parameters": {}, "remove": ["temperature"] }]
        })),
    )
    .expect_err("a JSON Pointer starts with /");
    assert_eq!(err.code, IpcErrorCode::Invalid);
    assert_eq!(std::fs::read_to_string(&s.path).unwrap(), before, "nothing written");
}

#[test]
fn a_stale_revision_refuses_the_profile_edit_and_the_file_keeps_the_other_writer() {
    let s = setup();
    let stale = revision(&s.path);
    // Another writer saves first.
    save(&s.path, &stale, form_patch(json!({ "max_output_tokens": 2000 }))).unwrap();
    let after_other = std::fs::read_to_string(&s.path).unwrap();

    let err = save(
        &s.path,
        &stale,
        form_patch(json!({
            "profiles": [{ "id": "fast", "label": "Vite", "parameters": {}, "remove": [] }]
        })),
    )
    .expect_err("a stale revision is a conflict");
    assert_eq!(err.code, IpcErrorCode::Conflict);
    assert_eq!(std::fs::read_to_string(&s.path).unwrap(), after_other, "the other write is kept");
}
