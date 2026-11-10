//! The move of former files to the current form, at startup: a key named
//! through an environment variable (`api_key_env`) is written in the file as
//! its `api_key` when the variable is set here, and the variable's name goes
//! either way; the former default levels go too. Nothing here prints or
//! returns a key.

use super::providers::{self, revision_of};
use super::providers_toml::Edit;
use bricks_provider::config::ProvidersConfig;
use std::path::Path;

/// What the move did, by provider id.
#[derive(Debug, Default, PartialEq)]
pub struct Migration {
    /// Keys read from their variable and written in the file.
    pub moved: Vec<String>,
    /// Variable names removed with no key to write (the variable was unset).
    pub dropped: Vec<String>,
    /// Former default levels were removed.
    pub defaults_removed: bool,
}

/// Moves what a former file holds. A file that is absent, unreadable or
/// invalid is left as it is (the registry reports it).
pub fn migrate(path: &Path) -> Migration {
    migrate_with(path, &|name| std::env::var(name).ok())
}

pub(crate) fn migrate_with(path: &Path, env: &dyn Fn(&str) -> Option<String>) -> Migration {
    let mut out = Migration::default();
    let Some(config) = read(path) else {
        return out;
    };
    for p in &config.providers {
        let Some(var) = &p.legacy_api_key_env else {
            continue;
        };
        let key = env(var)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && p.key().is_none());
        let wrote = key.is_some();
        let edit = Edit::DropKeyVariable {
            provider_id: p.id.clone(),
            key,
        };
        if commit(path, &edit) {
            if wrote {
                out.moved.push(p.id.clone());
            } else {
                out.dropped.push(p.id.clone());
            }
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
    fn variables_become_keys_in_the_file_and_their_names_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        std::fs::write(&path, FILE).unwrap();
        let env = |name: &str| (name == "SOME_SET_VAR").then(|| " sk-env-456 ".to_string());
        let report = migrate_with(&path, &env);
        assert_eq!(report.moved, vec!["env".to_string()]);
        assert_eq!(report.dropped, vec!["unset".to_string()]);
        assert!(report.defaults_removed);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("api_key = \"sk-inline-123\""), "{text}");
        assert!(text.contains("api_key = \"sk-env-456\""), "{text}");
        assert!(!text.contains("api_key_env"), "{text}");
        assert!(!text.contains("default = "), "{text}");
        assert!(text.contains("id = \"low\""));
        // A second pass changes nothing.
        let again = migrate_with(&path, &env);
        assert_eq!(again, Migration::default());
    }

    #[test]
    fn a_key_already_written_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        let both = FILE.replace(
            "api_key_env = \"SOME_SET_VAR\"",
            "api_key = \"sk-newer\"\napi_key_env = \"SOME_SET_VAR\"",
        );
        std::fs::write(&path, both).unwrap();
        migrate_with(&path, &|n| (n == "SOME_SET_VAR").then(|| "sk-older".into()));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("api_key = \"sk-newer\"") && !text.contains("sk-older"), "{text}");
    }

    #[test]
    fn an_absent_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let report = migrate_with(&dir.path().join("none.toml"), &|_| None);
        assert_eq!(report, Migration::default());
    }
}
