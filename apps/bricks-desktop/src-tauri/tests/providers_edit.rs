//! Provider and model editing, end to end on a temporary
//! `providers.toml`: the write, the revision check, the file as the next
//! start reads it, and the secrets that must never leave the engine.
//!
//! The command layer (`commands::providers::apply_edit`) is `pub(crate)`, so
//! these tests run its steps in the same order: `commit` (checked write),
//! then a fresh registry from the file, then `DesktopCatalog::replace`. The
//! in-crate tests of `commands/providers/tests.rs` cover the command glue.
//! Nothing here touches `~/.bricks`: every file is in a temporary folder.

use bricks_agent::control::ModelCatalog;
use bricks_desktop_lib::contract::{
    IpcErrorCode, IpcResult, ModelPatch, ProviderPatch, ProvidersFile,
};
use bricks_desktop_lib::engine::catalog::DesktopCatalog;
use bricks_desktop_lib::services::providers::{commit, revision_of, view};
use bricks_desktop_lib::services::providers_toml::Edit;
use bricks_provider::config::ProviderConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Two providers and a removable model, with comments that must survive every
/// edit. The secret values are placeholders that the tests search for.
const FILE: &str = r#"# Fournisseurs de test : en-tête à conserver
schema_version = 1

# Le fournisseur local
[[providers]]
id = "local"
name = "Local"   # libellé affiché
endpoint = "http://127.0.0.1:9/v1"
protocol = "chat_completions"
auth = "none"

[[providers.models]]
id = "small"
name = "Small"
api_model = "small-1"
streaming = true
[providers.models.limits]
max_input_tokens = 8000
max_output_tokens = 1000

# Un modèle qui sera supprimé
[[providers.models]]
id = "big"
name = "Big"
api_model = "big-1"
[providers.models.limits]
max_input_tokens = 32000
max_output_tokens = 4000

# Le fournisseur cloud : sa clé vient de l'environnement
[[providers]]
id = "cloud"
name = "Cloud"
endpoint = "https://api.example.invalid/v1"
protocol = "anthropic_messages"
auth = "api_key_header"
auth_header = "x-api-key"
api_key_env = "BRICKS_TEST_CLOUD_KEY_UNSET"

[[providers.models]]
id = "m1"
name = "M1"
api_model = "m1"
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

# Un fournisseur à clé écrite dans le fichier (valeurs factices)
[[providers]]
id = "inline"
name = "Inline"
endpoint = "https://inline.example.invalid/v1"
protocol = "chat_completions"
auth = "bearer"
api_key = "SECRET-KEY-VALUE-7731"

[providers.headers]
X-Tenant = "HEADER-SECRET-VALUE-7731"

[[providers.models]]
id = "i1"
name = "I1"
api_model = "i1"
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
"#;

/// A providers file in a temporary folder, with its catalog loaded from it.
struct Setup {
    _dir: tempfile::TempDir,
    path: PathBuf,
    catalog: Arc<DesktopCatalog>,
}

fn setup_with(text: &str) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    std::fs::write(&path, text).unwrap();
    let catalog = DesktopCatalog::new(bricks_config::load_registry(Some(&path)).map(Arc::new));
    Setup {
        _dir: dir,
        path,
        catalog,
    }
}

fn setup() -> Setup {
    setup_with(FILE)
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

/// The command's steps: checked write, then the catalog reloaded from the file.
fn edit(s: &Setup, revision: &str, change: &Edit) -> IpcResult<ProvidersFile> {
    commit(&s.path, revision, change)?;
    s.catalog
        .replace(bricks_config::load_registry(Some(&s.path)).map(Arc::new));
    Ok(current(&s.path))
}

/// A fresh catalog, as the next start of the application builds it.
fn restarted(path: &Path) -> Arc<DesktopCatalog> {
    DesktopCatalog::new(bricks_config::load_registry(Some(path)).map(Arc::new))
}

fn selections(catalog: &DesktopCatalog) -> Vec<String> {
    let mut all: Vec<String> = catalog.models().into_iter().map(|m| m.selection).collect();
    all.sort();
    all
}

fn provider_name(catalog: &DesktopCatalog, selection: &str) -> String {
    catalog
        .models()
        .into_iter()
        .find(|m| m.selection == selection)
        .unwrap_or_else(|| panic!("{selection} is listed"))
        .provider
}

#[test]
fn an_edit_changes_its_fields_and_keeps_every_other_byte() {
    let s = setup();
    let before = std::fs::read_to_string(&s.path).unwrap();
    let rev_before = revision(&s.path);

    edit(
        &s,
        &rev_before,
        &Edit::ProviderUpdate {
            provider_id: "local".into(),
            patch: ProviderPatch {
                name: Some("Poste".into()),
                ..Default::default()
            },
        },
    )
    .expect("provider edit");
    edit(
        &s,
        &revision(&s.path),
        &Edit::ModelUpdate {
            provider_id: "local".into(),
            model_id: "small".into(),
            patch: ModelPatch {
                max_input_tokens: Some(9_000),
                ..Default::default()
            },
        },
    )
    .expect("model edit");

    let after = std::fs::read_to_string(&s.path).unwrap();
    let expected = before
        .replace(
            "name = \"Local\"   # libellé affiché",
            "name = \"Poste\"   # libellé affiché",
        )
        .replace("max_input_tokens = 8000", "max_input_tokens = 9000");
    assert_eq!(after, expected, "only the two changed values differ");
    assert_ne!(revision(&s.path), rev_before, "the revision moved");
    assert!(after.starts_with("# Fournisseurs de test : en-tête à conserver"));
    assert!(after.contains("# Un modèle qui sera supprimé"));
    assert!(
        after.contains("api_key = \"SECRET-KEY-VALUE-7731\""),
        "untouched secrets stay in the file"
    );
}

#[test]
fn after_a_restart_the_saved_edits_and_deletions_are_what_the_file_says() {
    let s = setup();
    let rev = revision(&s.path);
    edit(
        &s,
        &rev,
        &Edit::ProviderUpdate {
            provider_id: "local".into(),
            patch: ProviderPatch {
                name: Some("Poste".into()),
                ..Default::default()
            },
        },
    )
    .unwrap();
    let rev = revision(&s.path);
    edit(
        &s,
        &rev,
        &Edit::ModelDelete {
            provider_id: "local".into(),
            model_id: "big".into(),
        },
    )
    .unwrap();
    let rev = revision(&s.path);
    edit(
        &s,
        &rev,
        &Edit::ProviderDelete {
            provider_id: "cloud".into(),
        },
    )
    .unwrap();

    // The next start: a new registry, read from the same file.
    let next = restarted(&s.path);
    assert_eq!(
        selections(&next),
        vec!["inline/i1".to_string(), "local/small".to_string()]
    );
    assert_eq!(
        provider_name(&next, "local/small"),
        "Poste",
        "the rename survived the restart"
    );
    assert!(
        next.models()
            .iter()
            .all(|m| !m.selection.starts_with("cloud/")),
        "a deleted provider takes its models with it"
    );
    let file = current(&s.path);
    assert!(file.providers.iter().all(|p| p.id != "cloud"));
    let local = file.providers.iter().find(|p| p.id == "local").unwrap();
    assert_eq!(
        local.models.len(),
        1,
        "the deleted model is gone from the view too"
    );
    assert!(
        std::fs::read_to_string(&s.path)
            .unwrap()
            .contains("# Fournisseurs de test")
    );
}

#[test]
fn reading_the_list_changes_nothing_so_a_cancelled_edit_writes_nothing() {
    let s = setup();
    let bytes = std::fs::read(&s.path).unwrap();
    let rev = revision(&s.path);
    for _ in 0..3 {
        let _ = current(&s.path);
    }
    assert_eq!(std::fs::read(&s.path).unwrap(), bytes, "reads never write");
    assert_eq!(revision(&s.path), rev);
    assert_eq!(
        rev,
        revision_of(&bytes),
        "the revision is the hash of the bytes"
    );
}

#[test]
fn a_refused_edit_writes_nothing_and_its_message_names_no_secret() {
    let s = setup();
    let bytes = std::fs::read(&s.path).unwrap();
    let rev = revision(&s.path);
    let refused = [
        // A model limit must be positive.
        Edit::ModelUpdate {
            provider_id: "local".into(),
            model_id: "small".into(),
            patch: ModelPatch {
                max_input_tokens: Some(0),
                ..Default::default()
            },
        },
        // A key from the environment cannot be set on a provider with an inline key.
        Edit::ProviderUpdate {
            provider_id: "inline".into(),
            patch: ProviderPatch {
                api_key_env: Some("BRICKS_TEST_OTHER".into()),
                ..Default::default()
            },
        },
        // An unknown protocol.
        Edit::ProviderUpdate {
            provider_id: "local".into(),
            patch: ProviderPatch {
                protocol: Some("not_a_protocol".into()),
                ..Default::default()
            },
        },
        // An unknown provider or model.
        Edit::ModelDelete {
            provider_id: "local".into(),
            model_id: "absent".into(),
        },
    ];
    for change in &refused {
        let err = edit(&s, &rev, change).expect_err("refused");
        assert!(
            !err.message.contains("SECRET-KEY-VALUE")
                && !err.message.contains("HEADER-SECRET-VALUE"),
            "{change:?}: no secret in the message: {}",
            err.message
        );
        assert_eq!(
            std::fs::read(&s.path).unwrap(),
            bytes,
            "{change:?} wrote nothing"
        );
    }
    assert_eq!(selections(&s.catalog).len(), 4, "the catalog is unchanged");
}

#[test]
fn a_stale_revision_is_a_conflict_and_the_second_writer_must_reload() {
    let s = setup();
    let first_reader = revision(&s.path);
    let second_reader = revision(&s.path);

    // The second writer gets there first.
    edit(
        &s,
        &second_reader,
        &Edit::ProviderUpdate {
            provider_id: "local".into(),
            patch: ProviderPatch {
                name: Some("Premier".into()),
                ..Default::default()
            },
        },
    )
    .unwrap();
    let after_first = std::fs::read(&s.path).unwrap();

    let err = edit(
        &s,
        &first_reader,
        &Edit::ProviderDelete {
            provider_id: "cloud".into(),
        },
    )
    .expect_err("the revision is stale");
    assert_eq!(err.code, IpcErrorCode::Conflict);
    assert_eq!(
        std::fs::read(&s.path).unwrap(),
        after_first,
        "the stale write changed nothing"
    );
    assert!(
        selections(&s.catalog)
            .iter()
            .any(|m| m.starts_with("cloud/"))
    );

    // Reloading gives the current revision, and the write then goes through.
    let fresh = revision(&s.path);
    edit(
        &s,
        &fresh,
        &Edit::ProviderDelete {
            provider_id: "cloud".into(),
        },
    )
    .expect("after a reload the write is accepted");
    assert!(
        !selections(&s.catalog)
            .iter()
            .any(|m| m.starts_with("cloud/"))
    );
}

#[test]
fn a_broken_file_is_reported_and_is_never_written_over() {
    let s = setup_with("[[providers\napi_key = \"SECRET-IN-BROKEN-LINE-7731\"\n");
    let bytes = std::fs::read(&s.path).unwrap();
    let file = current(&s.path);
    assert!(file.providers.is_empty());
    let message = file.error.expect("the broken file is reported");
    assert!(
        !message.contains("SECRET-IN-BROKEN-LINE"),
        "no value in the message: {message}"
    );

    let err = edit(
        &s,
        &file.revision,
        &Edit::ProviderDelete {
            provider_id: "local".into(),
        },
    )
    .expect_err("an edit on a broken file is refused");
    assert_eq!(err.code, IpcErrorCode::Config);
    assert_eq!(
        std::fs::read(&s.path).unwrap(),
        bytes,
        "the broken file is left as it was"
    );
}

#[test]
fn a_missing_file_is_an_empty_list_and_an_edit_does_not_create_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    let file = current(&path);
    assert!(file.providers.is_empty());
    assert!(file.error.is_none(), "an absent file is not an error");

    let err = commit(
        &path,
        &file.revision,
        &Edit::ProviderDelete {
            provider_id: "local".into(),
        },
    )
    .expect_err("nothing to edit");
    assert_eq!(err.code, IpcErrorCode::NotFound);
    assert!(!path.exists(), "no file is created by an edit");
}

#[test]
fn the_views_and_the_errors_never_carry_a_secret_value() {
    let s = setup();
    let json = serde_json::to_string(&current(&s.path)).unwrap();
    assert!(
        !json.contains("SECRET-KEY-VALUE-7731"),
        "inline key value in the view"
    );
    assert!(
        !json.contains("HEADER-SECRET-VALUE-7731"),
        "header value in the view"
    );

    let file = current(&s.path);
    let inline = file.providers.iter().find(|p| p.id == "inline").unwrap();
    assert!(inline.has_inline_key, "the presence of the key is reported");
    assert_eq!(
        inline.header_names,
        vec!["X-Tenant".to_string()],
        "header names only"
    );

    // A refused edit of the inline provider: its message and the view after it.
    let err = edit(
        &s,
        &file.revision,
        &Edit::ProviderUpdate {
            provider_id: "inline".into(),
            patch: ProviderPatch {
                protocol: Some("not_a_protocol".into()),
                ..Default::default()
            },
        },
    )
    .expect_err("an unknown protocol is refused");
    assert_eq!(err.code, IpcErrorCode::Invalid);
    assert!(!err.message.contains("SECRET"), "{}", err.message);
    let json = serde_json::to_string(&current(&s.path)).unwrap();
    assert!(!json.contains("SECRET-KEY-VALUE-7731"));
    assert!(!json.contains("HEADER-SECRET-VALUE-7731"));
}

#[test]
fn an_edit_of_an_unknown_provider_is_not_found_and_writes_nothing() {
    let s = setup();
    let bytes = std::fs::read(&s.path).unwrap();
    let err = edit(
        &s,
        &revision(&s.path),
        &Edit::ProviderUpdate {
            provider_id: "nope".into(),
            patch: ProviderPatch {
                name: Some("X".into()),
                ..Default::default()
            },
        },
    )
    .expect_err("unknown provider");
    assert_eq!(err.code, IpcErrorCode::NotFound);
    assert_eq!(std::fs::read(&s.path).unwrap(), bytes);
}
