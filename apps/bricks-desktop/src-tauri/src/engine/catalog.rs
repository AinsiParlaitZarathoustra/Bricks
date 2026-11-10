//! The model catalog of the desktop: the provider registry as last loaded
//! from `providers.toml`, swappable when the file changes. Every session
//! reads it through the engine's `ModelCatalog`, so a change reaches the
//! sessions at their next run (their running provider is never replaced
//! mid-run).

use bricks_agent::control::{ModelCatalog, ModelChoice};
use bricks_provider::{Provider, ProviderRegistry};
use parking_lot::RwLock;
use std::sync::Arc;

struct Current {
    /// The registry, or why the file cannot be used.
    registry: Result<Arc<ProviderRegistry>, String>,
    /// Counts the replacements: the engine rebuilds a session's provider
    /// when this differs from the one it was built with.
    revision: u64,
}

pub struct DesktopCatalog {
    current: RwLock<Current>,
}

impl DesktopCatalog {
    pub fn new(initial: Result<Arc<ProviderRegistry>, String>) -> Arc<Self> {
        Arc::new(Self {
            current: RwLock::new(Current {
                registry: initial,
                revision: 0,
            }),
        })
    }

    /// The registry in force now, or why there is none.
    pub fn registry(&self) -> Result<Arc<ProviderRegistry>, String> {
        self.current.read().registry.clone()
    }

    /// Replaces the catalog. Sessions pick the change up at their next run.
    pub fn replace(&self, next: Result<Arc<ProviderRegistry>, String>) {
        let mut current = self.current.write();
        current.registry = next;
        current.revision += 1;
    }
}

impl ModelCatalog for DesktopCatalog {
    fn models(&self) -> Vec<ModelChoice> {
        match self.registry() {
            Ok(registry) => <ProviderRegistry as ModelCatalog>::models(&registry),
            Err(_) => Vec::new(),
        }
    }

    /// A removed model is an error here: nothing substitutes it.
    fn build(&self, selection: &str, reasoning: Option<&str>) -> Result<Box<dyn Provider>, String> {
        let registry = self.registry()?;
        <ProviderRegistry as ModelCatalog>::build(&registry, selection, reasoning)
    }

    fn revision(&self) -> u64 {
        self.current.read().revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_MODEL: &str = r#"
schema_version = 1

[[providers]]
id = "local"
name = "Local"
endpoint = "http://127.0.0.1:9/v1"
protocol = "chat_completions"
auth = "none"

[[providers.models]]
id = "small"
name = "Small"
api_model = "small"
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
"#;

    #[test]
    fn a_replacement_changes_the_models_and_the_revision() {
        let first = ProviderRegistry::from_toml_str(ONE_MODEL, "test").unwrap();
        let catalog = DesktopCatalog::new(Ok(Arc::new(first)));
        assert_eq!(catalog.models().len(), 1);
        assert_eq!(catalog.revision(), 0);

        catalog.replace(Ok(Arc::new(
            ProviderRegistry::from_toml_str("schema_version = 1\n", "test").unwrap(),
        )));
        assert!(
            catalog.models().is_empty(),
            "the removed model is no longer listed"
        );
        assert_eq!(catalog.revision(), 1);
    }

    #[test]
    fn a_removed_model_is_a_build_error_and_a_broken_file_says_why() {
        let first = ProviderRegistry::from_toml_str(ONE_MODEL, "test").unwrap();
        let catalog = DesktopCatalog::new(Ok(Arc::new(first)));
        assert!(catalog.build("local/small", None).is_ok());

        catalog.replace(Ok(Arc::new(
            ProviderRegistry::from_toml_str("schema_version = 1\n", "test").unwrap(),
        )));
        let err = catalog
            .build("local/small", None)
            .err()
            .expect("removed model");
        assert!(err.contains("local"), "names what is missing: {err}");

        catalog.replace(Err("providers.toml: invalid TOML".into()));
        assert!(catalog.models().is_empty());
        assert_eq!(
            catalog.build("local/small", None).err().as_deref(),
            Some("providers.toml: invalid TOML")
        );
    }
}
