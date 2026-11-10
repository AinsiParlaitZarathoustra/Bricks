//! The move of former files to the current form, at startup: keys written in
//! `providers.toml` (inline, or named through an environment variable) go to
//! the secure storage, and the former default levels go. Nothing here prints
//! or returns a key.

use super::providers::{self, revision_of};
use super::providers_toml::Edit;
use bricks_provider::config::ProvidersConfig;
use bricks_provider::keys::KeyStore;
use std::path::Path;

/// What the move did, by provider id.
#[derive(Debug, Default, PartialEq)]
pub struct Migration {
    /// Keys now in the secure storage.
    pub moved: Vec<String>,
    /// Former key fields left in place: the variable they name is not set
    /// here, so there is no key to move (it is still read from there).
    pub left: Vec<String>,
    /// Former default levels were removed.
    pub defaults_removed: bool,
}

/// Moves what a former file holds. A file that is absent, unreadable or
/// invalid is left as it is (the registry reports it).
pub fn migrate(path: &Path, keys: &dyn KeyStore) -> Migration {
    migrate_with(path, keys, &|name| std::env::var(name).ok())
}

pub(crate) fn migrate_with(
    path: &Path,
    keys: &dyn KeyStore,
    env: &dyn Fn(&str) -> Option<String>,
) -> Migration {
    let mut out = Migration::default();
    let Some(config) = read(path) else {
        return out;
    };
    for p in config.providers.iter().filter(|p| p.has_legacy_key_fields()) {
        let stored = keys.get(&p.id).ok().flatten().is_some_and(|k| !k.trim().is_empty());
        let kept = if stored {
            true
        } else {
            match p.legacy_key(env) {
                // A key the storage refuses stays in the file.
                Some(key) => match keys.set(&p.id, &key) {
                    Ok(()) => true,
                    Err(_) => continue,
                },
                None => false,
            }
        };
        if !kept {
            out.left.push(p.id.clone());
            continue;
        }
        let edit = Edit::StripLegacyKeys {
            provider_id: p.id.clone(),
        };
        if commit(path, &edit) {
            out.moved.push(p.id.clone());
        }
    }
    let has_defaults = read(path).is_some_and(|c| {
        c.providers
            .iter()
            .flat_map(|p| p.models.iter())
            .any(|m| m.reasoning.legacy_default.is_some())
    });
    if has_defaults {
        out.defaults_removed = commit(path, &Edit::StripReasoningDefaults);
    }
    out
}

fn read(path: &Path) -> Option<ProvidersConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    ProvidersConfig::from_toml_str(&text, &path.display().to_string()).ok()
}

/// Applies `edit` to the file as it is now.
fn commit(path: &Path, edit: &Edit) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    providers::commit(path, &revision_of(&bytes), edit).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bricks_provider::keys::MemoryKeys;

    const FILE: &str = r#"schema_version = 1

[[providers]]
id = "inline"
name = "Inline"
endpoint = "https://a.example.com/v1"
protocol = "chat_completions"
auth = "bearer"
api_key = "sk-inline-123"

[[providers.models]]
id = "m"
name = "M"
api_model = "m"

[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[providers.models.reasoning]
default = "high"
profiles = [{ id = "low" }, { id = "high" }]

[[providers]]
id = "env"
name = "Env"
endpoint = "https://b.example.com/v1"
protocol = "chat_completions"
auth = "bearer"
api_key_env = "SOME_SET_VAR"

[[providers]]
id = "unset"
name = "Unset"
endpoint = "https://c.example.com/v1"
protocol = "chat_completions"
auth = "bearer"
api_key_env = "SOME_UNSET_VAR"

[[providers]]
id = "local"
name = "Local"
endpoint = "http://127.0.0.1:1/v1"
protocol = "chat_completions"
auth = "none"
"#;

    #[test]
    fn keys_move_to_the_storage_and_leave_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        std::fs::write(&path, FILE).unwrap();
        let keys = MemoryKeys::default();
        let env = |name: &str| (name == "SOME_SET_VAR").then(|| "sk-env-456".to_string());
        let report = migrate_with(&path, &keys, &env);
        assert_eq!(report.moved, vec!["inline".to_string(), "env".to_string()]);
        assert_eq!(report.left, vec!["unset".to_string()]);
        assert!(report.defaults_removed);
        assert_eq!(keys.get("inline").unwrap().as_deref(), Some("sk-inline-123"));
        assert_eq!(keys.get("env").unwrap().as_deref(), Some("sk-env-456"));
        assert_eq!(keys.get("unset").unwrap(), None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("api_key ="), "{text}");
        assert!(!text.contains("SOME_SET_VAR"), "{text}");
        // An unset variable keeps its reference: nothing to move, nothing lost.
        assert!(text.contains("api_key_env = \"SOME_UNSET_VAR\""), "{text}");
        assert!(!text.contains("sk-inline-123"));
        assert!(!text.contains("default = "), "{text}");
        assert!(text.contains("id = \"low\""));
        // A second pass moves nothing more.
        let again = migrate_with(&path, &keys, &env);
        assert!(again.moved.is_empty() && !again.defaults_removed);
    }

    #[test]
    fn a_key_already_stored_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        std::fs::write(&path, FILE).unwrap();
        let keys = MemoryKeys::default();
        keys.set("inline", "sk-newer").unwrap();
        migrate_with(&path, &keys, &|_| None);
        assert_eq!(keys.get("inline").unwrap().as_deref(), Some("sk-newer"));
    }

    #[test]
    fn an_absent_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let report = migrate_with(&dir.path().join("none.toml"), &MemoryKeys::default(), &|_| None);
        assert_eq!(report, Migration::default());
    }
}
