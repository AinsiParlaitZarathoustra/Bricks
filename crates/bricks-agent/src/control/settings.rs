//! The `[agent]` section of `bricks.toml`.
//!
//! ```toml
//! [agent]
//! model = "my-provider/my-model"   # a `provider_id/model_id` of providers.toml
//! reasoning = "deep"               # one of that model's reasoning profiles
//! ```
//!
//! The model is never chosen here when the section is absent: the caller
//! must name one, and an unknown name is an error that lists the
//! configured models. Permissions are not configured in `bricks.toml`
//! (see `crate::permission`): a `[permissions]` section is reported and
//! not applied.

use serde::{Deserialize, Serialize};

/// Default for [`AgentSettings::retry_after_max_wait_ms`].
pub const RETRY_AFTER_MAX_WAIT_MS: u64 = 60_000;

/// `[agent]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSettings {
    /// Default model selection (`provider_id/model_id`).
    pub model: Option<String>,
    /// Default reasoning profile of that model.
    pub reasoning: Option<String>,
    pub max_tokens: Option<u32>,
    /// The longest `Retry-After` a provider may ask for before a retry. A
    /// longer one is not waited for: no retry is sent, and the error says
    /// why. `0`: no server-asked wait at all.
    pub retry_after_max_wait_ms: u64,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            model: None,
            reasoning: None,
            max_tokens: None,
            retry_after_max_wait_ms: RETRY_AFTER_MAX_WAIT_MS,
        }
    }
}

/// The `[agent]` table as read from the file. Its unknown keys are removed
/// by the configuration loader before it is read (see `bricks_config`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileAgent {
    model: Option<String>,
    reasoning: Option<String>,
    max_tokens: Option<u32>,
    retry_after_max_wait_ms: Option<u64>,
}

/// The diagnostic of a `bricks.toml` that still has a `[permissions]`
/// section: it is reported, never applied (not even its denials).
pub const OBSOLETE_PERMISSIONS: &str = "[permissions] is no longer read: its rules (denials included) \
     are not applied. Permissions are chosen per session: in the app, Settings › General for new \
     conversations and /config for one conversation; `--permissions` on the command line. \
     Remove this section.";

/// Parse `[agent]` from a `bricks.toml` text. An absent section gives the
/// defaults; an invalid one is an error naming it.
pub fn from_bricks_toml(text: &str) -> Result<AgentSettings, String> {
    let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut agent = AgentSettings::default();
    if let Some(section) = value.get("agent") {
        let f: FileAgent = section
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| format!("[agent]: {}", e.to_string().trim()))?;
        if let Some(m) = &f.model
            && !m.contains('/')
        {
            return Err(format!(
                "[agent]: model `{m}` must be `provider_id/model_id` as in providers.toml"
            ));
        }
        agent = AgentSettings {
            model: f.model,
            reasoning: f.reasoning,
            max_tokens: f.max_tokens,
            retry_after_max_wait_ms: f.retry_after_max_wait_ms.unwrap_or(RETRY_AFTER_MAX_WAIT_MS),
        };
    }
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_example_loads_cleanly() {
        let text = include_str!("../../../../docs/bricks.example.toml");
        let agent = from_bricks_toml(text).unwrap();
        assert_eq!(agent.max_tokens, None);
        let all = crate::BricksConfig::from_texts(Some(text), &[]);
        assert!(all.diagnostics.is_empty(), "{:?}", all.diagnostics);
    }

    #[test]
    fn invalid_sections_are_errors() {
        for (text, needle) in [
            ("[agent]\nmodel = \"no-slash\"\n", "provider_id/model_id"),
            ("[agent]\nbogus = 1\n", "unknown field"),
        ] {
            let e = from_bricks_toml(text).unwrap_err();
            assert!(e.contains(needle), "{text}: {e}");
        }
    }

    #[test]
    fn a_permissions_section_is_reported_and_not_applied() {
        let text = "[permissions]\ndefault = \"deny\"\n[permissions.tools]\nBash = \"deny\"\n";
        assert!(from_bricks_toml(text).is_ok(), "[agent] still loads");
        let all = crate::BricksConfig::from_texts(Some(text), &[]);
        assert!(
            all.diagnostics.iter().any(|d| d.contains("[permissions] is no longer read")),
            "{:?}",
            all.diagnostics
        );
    }

    #[test]
    fn the_retry_budget_is_read_checked_and_defaulted() {
        let a = from_bricks_toml("").unwrap();
        assert_eq!(a.retry_after_max_wait_ms, RETRY_AFTER_MAX_WAIT_MS);
        let a = from_bricks_toml("[agent]\nretry_after_max_wait_ms = 1500\n").unwrap();
        assert_eq!(a.retry_after_max_wait_ms, 1500);
        let a = from_bricks_toml("[agent]\nretry_after_max_wait_ms = 0\n").unwrap();
        assert_eq!(a.retry_after_max_wait_ms, 0, "0 is a valid budget");
        let e = from_bricks_toml("[agent]\nretry_after_max_wait_ms = -1\n").unwrap_err();
        assert!(e.contains("[agent]"), "{e}");
        let e = from_bricks_toml("[agent]\nretry_after = 5\n").unwrap_err();
        assert!(e.contains("[agent]") && e.contains("retry_after"), "{e}");
    }
}
