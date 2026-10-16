//! Provider editing, end to end on a temporary `providers.toml`: the file,
//! the revision check, the catalog reload.

use super::*;
use crate::services::providers_toml::fixtures::PROVIDERS;
use cersei_agent::control::ModelCatalog;
use tempfile::tempdir;

/// A providers file in a temporary folder, its catalog and its lock.
fn setup() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Arc<DesktopCatalog>,
    Mutex<()>,
) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    std::fs::write(&path, PROVIDERS).unwrap();
    let registry = bricks_config::load_registry(Some(&path)).map(Arc::new);
    let catalog = DesktopCatalog::new(registry);
    (dir, path, catalog, Mutex::new(()))
}

fn revision(path: &Path) -> String {
    current_file(path).revision
}

#[test]
fn a_removed_model_is_unselectable_and_the_catalog_reflects_the_file() {
    let (_dir, path, catalog, lock) = setup();
    assert!(catalog.models().iter().any(|m| m.selection == "local/big"));
    assert!(catalog.build("local/big", None).is_ok());
    let before = catalog.revision();

    let file = apply_edit(
        &path,
        &lock,
        &catalog,
        &revision(&path),
        &Edit::ModelDelete {
            provider_id: "local".into(),
            model_id: "big".into(),
        },
    )
    .expect("delete");

    let local = file.providers.iter().find(|p| p.id == "local").unwrap();
    assert_eq!(local.models.len(), 1, "the view lists the remaining model");
    assert!(catalog.models().iter().all(|m| m.selection != "local/big"));
    let err = catalog
        .build("local/big", None)
        .err()
        .expect("a removed model does not build");
    assert!(err.contains("big"), "the error names the model: {err}");
    assert!(catalog.revision() > before, "sessions see the change");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        !on_disk.contains("big-1"),
        "the table is gone from the file"
    );
    assert!(
        on_disk.contains("Fournisseurs de test"),
        "the header comment is kept"
    );
}

#[test]
fn a_provider_update_reaches_the_catalog_and_keeps_the_rest_of_the_file() {
    let (_dir, path, catalog, lock) = setup();
    let before = std::fs::read_to_string(&path).unwrap();
    apply_edit(
        &path,
        &lock,
        &catalog,
        &revision(&path),
        &Edit::ProviderUpdate {
            provider_id: "local".into(),
            patch: ProviderPatch {
                name: Some("Poste".into()),
                ..Default::default()
            },
        },
    )
    .expect("update");
    let models = catalog.models();
    let small = models
        .iter()
        .find(|m| m.selection == "local/small")
        .unwrap();
    assert_eq!(small.provider, "Poste", "the catalog shows the new name");
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        after,
        before.replace(
            "name = \"Local\"   # libellé",
            "name = \"Poste\"   # libellé"
        )
    );
}

#[test]
fn a_stale_revision_is_a_conflict_and_changes_nothing() {
    let (_dir, path, catalog, lock) = setup();
    let before = std::fs::read(&path).unwrap();
    let rev_before = catalog.revision();
    let err = apply_edit(
        &path,
        &lock,
        &catalog,
        "not-the-revision",
        &Edit::ProviderDelete {
            provider_id: "cloud".into(),
        },
    )
    .expect_err("stale");
    assert_eq!(err.code, IpcErrorCode::Conflict);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file is unchanged"
    );
    assert_eq!(catalog.revision(), rev_before, "the catalog is unchanged");
}

#[test]
fn a_refused_patch_leaves_the_file_and_the_catalog_as_they_were() {
    let (_dir, path, catalog, lock) = setup();
    let before = std::fs::read(&path).unwrap();
    let rev_before = catalog.revision();
    let refusals = [
        Edit::ModelUpdate {
            provider_id: "local".into(),
            model_id: "small".into(),
            patch: ModelPatch {
                max_input_tokens: Some(0),
                ..Default::default()
            },
        },
        Edit::ProviderUpdate {
            provider_id: "local".into(),
            patch: ProviderPatch {
                protocol: Some("not_a_protocol".into()),
                ..Default::default()
            },
        },
        Edit::ModelUpdate {
            provider_id: "local".into(),
            model_id: "small".into(),
            patch: ModelPatch {
                reasoning_default: Some("missing-profile".into()),
                ..Default::default()
            },
        },
        Edit::ProviderUpdate {
            provider_id: "inline".into(),
            patch: ProviderPatch {
                api_key_env: Some("BRICKS_TEST_OTHER".into()),
                ..Default::default()
            },
        },
    ];
    for edit in &refusals {
        let err = apply_edit(&path, &lock, &catalog, &revision(&path), edit).expect_err("refused");
        assert_eq!(err.code, IpcErrorCode::Invalid, "{edit:?}: {}", err.message);
        assert!(!err.message.contains("SECRET"), "no secret in the message");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "{edit:?} wrote nothing"
        );
    }
    assert_eq!(catalog.revision(), rev_before);
}

#[test]
fn a_deleted_provider_takes_its_models_with_it() {
    let (_dir, path, catalog, lock) = setup();
    let file = apply_edit(
        &path,
        &lock,
        &catalog,
        &revision(&path),
        &Edit::ProviderDelete {
            provider_id: "cloud".into(),
        },
    )
    .expect("delete provider");
    assert!(file.providers.iter().all(|p| p.id != "cloud"));
    assert!(
        catalog
            .models()
            .iter()
            .all(|m| !m.selection.starts_with("cloud/"))
    );
    assert!(catalog.build("cloud/m1", None).is_err());
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Fournisseurs de test")
    );
}

#[test]
fn the_view_never_carries_a_secret_value() {
    let (_dir, path, _catalog, _lock) = setup();
    let file = current_file(&path);
    let json = serde_json::to_string(&file).unwrap();
    assert!(!json.contains("SECRET-KEY-VALUE"), "inline key value");
    assert!(!json.contains("HEADER-SECRET-VALUE"), "header value");
    let inline = file.providers.iter().find(|p| p.id == "inline").unwrap();
    assert!(inline.has_inline_key, "the presence of a key is reported");
    assert_eq!(
        inline.header_names,
        vec!["X-Tenant".to_string()],
        "header names only"
    );
    assert!(file.error.is_none());
}

#[test]
fn a_missing_file_is_an_empty_list_and_a_broken_one_says_why() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    let missing = current_file(&path);
    assert!(missing.providers.is_empty());
    assert!(missing.error.is_none());
    assert_eq!(missing.path, path.display().to_string());

    std::fs::write(&path, "[[providers\napi_key = \"SECRET-IN-BROKEN-LINE\"\n").unwrap();
    let broken = current_file(&path);
    assert!(broken.providers.is_empty());
    let message = broken.error.expect("the broken file is reported");
    assert!(
        !message.contains("SECRET-IN-BROKEN-LINE"),
        "no value in the message: {message}"
    );
}
