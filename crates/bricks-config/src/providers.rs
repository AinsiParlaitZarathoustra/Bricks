//! The provider registry (`providers.toml`), loaded from plain paths.

use bricks_provider::ProviderRegistry;
use std::path::Path;

/// Loads the registry from `explicit`, or from the default location when
/// `None`. Errors are complete messages for the user.
pub fn load_registry(explicit: Option<&Path>) -> Result<ProviderRegistry, String> {
    ProviderRegistry::load(explicit).map_err(|e| {
        format!(
            "{e}\nModels are configured in providers.toml (see docs/providers.md); \
             nothing is built in."
        )
    })
}
