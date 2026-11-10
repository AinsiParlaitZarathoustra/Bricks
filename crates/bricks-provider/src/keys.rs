//! API keys: kept in the system's secure storage (the macOS Keychain, the
//! Windows Credential Manager, the Linux kernel keyring), one entry per
//! provider id. `providers.toml` never holds a key.
//!
//! An environment variable named after the provider overrides the stored key
//! for one process (`BRICKS_API_KEY_<ID>`), for headless runs and CI.
//!
//! Nothing here logs or formats a key: errors carry the provider id only.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where API keys are kept. One key per provider id.
pub trait KeyStore: Send + Sync {
    /// The stored key, `None` when there is none.
    fn get(&self, provider: &str) -> Result<Option<String>, String>;
    /// Stores `key`, replacing any previous one.
    fn set(&self, provider: &str, key: &str) -> Result<(), String>;
    /// Removes the key. Removing an absent key is not an error.
    fn delete(&self, provider: &str) -> Result<(), String>;
}

/// The variable that overrides the stored key of `provider` for one process:
/// `BRICKS_API_KEY_` and the id in upper case, every other character an `_`.
pub fn override_var(provider: &str) -> String {
    let id: String = provider
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("BRICKS_API_KEY_{id}")
}

/// `~/.bricks`, or `$BRICKS_HOME`.
fn default_home() -> PathBuf {
    if let Some(h) = std::env::var_os("BRICKS_HOME") {
        return PathBuf::from(h);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".bricks")
}

/// The system's secure storage, scoped to one Bricks home: the default home
/// uses the service `Bricks`; another home (`BRICKS_HOME`) gets its own
/// service, so a test home never reads or replaces the real keys.
#[derive(Debug, Clone)]
pub struct SystemKeychain {
    service: String,
}

impl SystemKeychain {
    pub fn for_home(home: &Path) -> Self {
        let standard = dirs::home_dir().map(|h| h.join(".bricks"));
        let service = if standard.as_deref() == Some(home) {
            "Bricks".to_string()
        } else {
            format!("Bricks ({})", home.display())
        };
        Self { service }
    }

    /// The keychain of the home this process uses (`$BRICKS_HOME` or `~/.bricks`).
    pub fn current() -> Self {
        Self::for_home(&default_home())
    }

    pub fn service(&self) -> &str {
        &self.service
    }
}

#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
impl KeyStore for SystemKeychain {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(&self.service, provider).map_err(|e| failure(provider, &e))?;
        match entry.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(failure(provider, &e)),
        }
    }

    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(&self.service, provider).map_err(|e| failure(provider, &e))?;
        entry.set_password(key).map_err(|e| failure(provider, &e))
    }

    fn delete(&self, provider: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(&self.service, provider).map_err(|e| failure(provider, &e))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(failure(provider, &e)),
        }
    }
}

/// A keyring error, without anything it may quote.
#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
fn failure(provider: &str, e: &keyring::Error) -> String {
    let kind = match e {
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => {
            "the secure storage is not available"
        }
        keyring::Error::BadEncoding(_) => "the stored key is not valid text",
        keyring::Error::TooLong(..) | keyring::Error::Invalid(..) => "the key is not accepted",
        keyring::Error::Ambiguous(_) => "several keys are stored",
        _ => "the secure storage failed",
    };
    format!("provider \"{provider}\": {kind}")
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
impl KeyStore for SystemKeychain {
    fn get(&self, _provider: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn set(&self, provider: &str, _key: &str) -> Result<(), String> {
        Err(format!(
            "provider \"{provider}\": no secure storage on this system; set {}",
            override_var(provider)
        ))
    }
    fn delete(&self, _provider: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Keys kept in memory: for tests, and for embedders that manage keys themselves.
#[derive(Debug, Default)]
pub struct MemoryKeys {
    keys: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}

impl KeyStore for MemoryKeys {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        Ok(self.keys.lock().unwrap().get(provider).cloned())
    }
    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        self.keys
            .lock()
            .unwrap()
            .insert(provider.to_string(), key.to_string());
        Ok(())
    }
    fn delete(&self, provider: &str) -> Result<(), String> {
        self.keys.lock().unwrap().remove(provider);
        Ok(())
    }
}

/// The key of `provider`: the override variable first, then the store.
/// Blank values count as absent.
pub fn lookup(
    provider: &str,
    env: &dyn Fn(&str) -> Option<String>,
    store: &dyn KeyStore,
) -> Result<Option<String>, String> {
    if let Some(v) = env(&override_var(provider)).filter(|v| !v.trim().is_empty()) {
        return Ok(Some(v.trim().to_string()));
    }
    Ok(store
        .get(provider)?
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_string()))
}

/// The store used when none is given: the system keychain of this home.
pub fn default_store() -> Arc<dyn KeyStore> {
    Arc::new(SystemKeychain::current())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_variable_is_named_after_the_id() {
        assert_eq!(override_var("opencode-go"), "BRICKS_API_KEY_OPENCODE_GO");
        assert_eq!(override_var("ifm"), "BRICKS_API_KEY_IFM");
    }

    #[test]
    fn the_variable_wins_over_the_store_and_blanks_are_absent() {
        let store = MemoryKeys::default();
        store.set("p", "stored").unwrap();
        let none = |_: &str| None;
        assert_eq!(lookup("p", &none, &store).unwrap().as_deref(), Some("stored"));
        let set = |name: &str| (name == "BRICKS_API_KEY_P").then(|| " env ".to_string());
        assert_eq!(lookup("p", &set, &store).unwrap().as_deref(), Some("env"));
        let blank = |_: &str| Some("  ".to_string());
        assert_eq!(lookup("p", &blank, &store).unwrap().as_deref(), Some("stored"));
        store.delete("p").unwrap();
        assert_eq!(lookup("p", &none, &store).unwrap(), None);
    }

    #[test]
    fn a_test_home_has_its_own_service() {
        let other = SystemKeychain::for_home(Path::new("/tmp/some-home"));
        assert_eq!(other.service(), "Bricks (/tmp/some-home)");
        if let Some(h) = dirs::home_dir() {
            assert_eq!(SystemKeychain::for_home(&h.join(".bricks")).service(), "Bricks");
        }
    }
}
