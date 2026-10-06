//! `[agent]` and `[permissions]` sections of `bricks.toml`.
//!
//! ```toml
//! [agent]
//! model = "my-provider/my-model"   # a `provider_id/model_id` of providers.toml
//! reasoning = "deep"               # one of that model's reasoning profiles
//! max_turns = 50
//!
//! [permissions]
//! default = "ask"                  # when no rule below matches
//! read_only = "allow"              # by permission level of the tool
//! write = "ask"
//! execute = "ask"
//! dangerous = "ask"
//!
//! [permissions.tools]              # by tool name, before the levels
//! WebFetch = "allow"
//! Bash = "ask"
//! ```
//!
//! The model is never chosen here when the section is absent: the caller
//! must name one, and an unknown name is an error that lists the
//! configured models.

use cersei_tools::PermissionLevel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What happens to a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Run it without asking.
    Allow,
    /// Ask a person first; without one (non-interactive), the run stops.
    Ask,
    /// Refuse it.
    Deny,
}

/// The approval policy: by tool name first, then by permission level, then
/// `default`. `Forbidden` tools are always refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovalRules {
    pub default: Action,
    pub none: Action,
    pub read_only: Action,
    pub write: Action,
    pub execute: Action,
    pub dangerous: Action,
    pub tools: BTreeMap<String, Action>,
}

impl Default for ApprovalRules {
    fn default() -> Self {
        Self {
            default: Action::Ask,
            none: Action::Allow,
            read_only: Action::Allow,
            write: Action::Ask,
            execute: Action::Ask,
            dangerous: Action::Ask,
            tools: BTreeMap::new(),
        }
    }
}

impl ApprovalRules {
    /// Everything allowed (tests, trusted automation that chose it
    /// explicitly). Forbidden tools stay refused.
    pub fn allow_all() -> Self {
        Self {
            default: Action::Allow,
            none: Action::Allow,
            read_only: Action::Allow,
            write: Action::Allow,
            execute: Action::Allow,
            dangerous: Action::Allow,
            tools: BTreeMap::new(),
        }
    }

    pub fn action_for(&self, tool: &str, level: PermissionLevel) -> Action {
        if level == PermissionLevel::Forbidden {
            return Action::Deny;
        }
        if let Some(a) = self.tools.get(tool) {
            return *a;
        }
        match level {
            PermissionLevel::None => self.none,
            PermissionLevel::ReadOnly => self.read_only,
            PermissionLevel::Write => self.write,
            PermissionLevel::Execute => self.execute,
            PermissionLevel::Dangerous => self.dangerous,
            PermissionLevel::Forbidden => Action::Deny,
        }
    }
}

/// `[agent]`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct AgentSettings {
    /// Default model selection (`provider_id/model_id`).
    pub model: Option<String>,
    /// Default reasoning profile of that model.
    pub reasoning: Option<String>,
    pub max_turns: Option<u32>,
    pub max_tokens: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAgent {
    model: Option<String>,
    reasoning: Option<String>,
    max_turns: Option<u32>,
    max_tokens: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePermissions {
    default: Option<Action>,
    none: Option<Action>,
    read_only: Option<Action>,
    write: Option<Action>,
    execute: Option<Action>,
    dangerous: Option<Action>,
    #[serde(default)]
    tools: BTreeMap<String, Action>,
}

/// Parse both sections from a `bricks.toml` text. Absent sections give the
/// defaults; an invalid section is an error naming it.
pub fn from_bricks_toml(text: &str) -> Result<(AgentSettings, ApprovalRules), String> {
    let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut agent = AgentSettings::default();
    if let Some(section) = value.get("agent") {
        let f: FileAgent = section
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| format!("[agent]: {}", e.to_string().trim()))?;
        if let Some(m) = &f.model {
            if !m.contains('/') {
                return Err(format!(
                    "[agent]: model `{m}` must be `provider_id/model_id` as in providers.toml"
                ));
            }
        }
        if f.max_turns == Some(0) {
            return Err("[agent]: max_turns must be at least 1".into());
        }
        agent = AgentSettings {
            model: f.model,
            reasoning: f.reasoning,
            max_turns: f.max_turns,
            max_tokens: f.max_tokens,
        };
    }
    let mut rules = ApprovalRules::default();
    if let Some(section) = value.get("permissions") {
        let f: FilePermissions = section
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| format!("[permissions]: {}", e.to_string().trim()))?;
        let set = |slot: &mut Action, v: Option<Action>| {
            if let Some(v) = v {
                *slot = v;
            }
        };
        set(&mut rules.default, f.default);
        set(&mut rules.none, f.none);
        set(&mut rules.read_only, f.read_only);
        set(&mut rules.write, f.write);
        set(&mut rules.execute, f.execute);
        set(&mut rules.dangerous, f.dangerous);
        rules.tools = f.tools;
    }
    Ok((agent, rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_ask_before_writing_or_executing() {
        let (agent, rules) = from_bricks_toml("").unwrap();
        assert_eq!(agent, AgentSettings::default());
        assert_eq!(
            rules.action_for("Read", PermissionLevel::ReadOnly),
            Action::Allow
        );
        assert_eq!(
            rules.action_for("Write", PermissionLevel::Write),
            Action::Ask
        );
        assert_eq!(
            rules.action_for("Bash", PermissionLevel::Execute),
            Action::Ask
        );
        assert_eq!(
            rules.action_for("X", PermissionLevel::Forbidden),
            Action::Deny
        );
    }

    #[test]
    fn tool_rules_come_before_levels() {
        let (agent, rules) = from_bricks_toml(
            "[agent]\nmodel = \"p/m\"\nreasoning = \"deep\"\n\n[permissions]\nwrite = \"allow\"\n\n[permissions.tools]\nBash = \"deny\"\nWebFetch = \"allow\"\n",
        )
        .unwrap();
        assert_eq!(agent.model.as_deref(), Some("p/m"));
        assert_eq!(
            rules.action_for("Bash", PermissionLevel::Execute),
            Action::Deny
        );
        assert_eq!(
            rules.action_for("Edit", PermissionLevel::Write),
            Action::Allow
        );
        assert_eq!(
            rules.action_for("WebFetch", PermissionLevel::ReadOnly),
            Action::Allow
        );
        assert_eq!(
            ApprovalRules::allow_all().action_for("X", PermissionLevel::Forbidden),
            Action::Deny,
            "forbidden stays forbidden"
        );
    }

    #[test]
    fn the_documented_example_loads_cleanly() {
        let text = include_str!("../../../../docs/bricks.example.toml");
        let (agent, rules) = from_bricks_toml(text).unwrap();
        assert_eq!(agent.max_turns, Some(50));
        assert_eq!(
            rules,
            ApprovalRules::default(),
            "the example documents the defaults"
        );
        let all = crate::BricksConfig::from_texts(Some(text), &[]);
        assert!(all.diagnostics.is_empty(), "{:?}", all.diagnostics);
    }

    #[test]
    fn invalid_sections_are_errors() {
        for (text, needle) in [
            ("[agent]\nmodel = \"no-slash\"\n", "provider_id/model_id"),
            ("[agent]\nmax_turns = 0\n", "max_turns"),
            ("[agent]\nbogus = 1\n", "unknown field"),
            ("[permissions]\nwrite = \"maybe\"\n", "[permissions]"),
        ] {
            let e = from_bricks_toml(text).unwrap_err();
            assert!(e.contains(needle), "{text}: {e}");
        }
    }
}
