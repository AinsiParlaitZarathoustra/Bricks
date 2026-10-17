//! `[agent]` and `[permissions]` sections of `bricks.toml`.
//!
//! ```toml
//! [agent]
//! model = "my-provider/my-model"   # a `provider_id/model_id` of providers.toml
//! reasoning = "deep"               # one of that model's reasoning profiles
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
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

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

/// A runtime override of the project's approval policy, set by a frontend
/// (the desktop's preference). It is read at each decision, so a change
/// applies to the next call of every open session and of their running
/// sub-agents, without restarting anything. [`ApprovalMode::Standard`] (the
/// default, and the CLI's) leaves the project's rules and session grants
/// exactly as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    #[default]
    Standard,
    /// Every call the rules do not refuse is asked. Session grants ignored.
    AlwaysAsk,
    /// Every call the rules do not refuse runs without asking.
    NoConfirmation,
}

impl ApprovalMode {
    fn code(self) -> u8 {
        match self {
            ApprovalMode::Standard => 0,
            ApprovalMode::AlwaysAsk => 1,
            ApprovalMode::NoConfirmation => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => ApprovalMode::AlwaysAsk,
            2 => ApprovalMode::NoConfirmation,
            _ => ApprovalMode::Standard,
        }
    }
}

/// The shared, atomic cell holding the [`ApprovalMode`] of one engine
/// configuration. Cloned as an `Arc` into every session it builds.
#[derive(Debug)]
pub struct ApprovalOverride {
    mode: AtomicU8,
}

impl ApprovalOverride {
    pub fn new(mode: ApprovalMode) -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicU8::new(mode.code()),
        })
    }

    pub fn get(&self) -> ApprovalMode {
        ApprovalMode::from_code(self.mode.load(Ordering::SeqCst))
    }

    pub fn set(&self, mode: ApprovalMode) {
        self.mode.store(mode.code(), Ordering::SeqCst);
    }
}

/// What a call gets once the runtime override is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    pub action: Action,
    /// Whether an earlier "allow for this session" still applies.
    pub honour_session_grants: bool,
}

impl ApprovalRules {
    /// The one decision point of the approval gate, for the session agent
    /// and its sub-agents alike. Priority:
    /// 1. `Forbidden` tools, and explicit `deny` rules (by tool or level):
    ///    refused, whatever the mode;
    /// 2. `AlwaysAsk`: asked, session grants ignored;
    /// 3. `NoConfirmation`: allowed;
    /// 4. `Standard`: the rules and session grants, unchanged.
    pub fn effective(&self, mode: ApprovalMode, tool: &str, level: PermissionLevel) -> Effective {
        let rule = self.action_for(tool, level);
        if rule == Action::Deny {
            return Effective {
                action: Action::Deny,
                honour_session_grants: true,
            };
        }
        match mode {
            ApprovalMode::Standard => Effective {
                action: rule,
                honour_session_grants: true,
            },
            ApprovalMode::AlwaysAsk => Effective {
                action: Action::Ask,
                honour_session_grants: false,
            },
            ApprovalMode::NoConfirmation => Effective {
                action: Action::Allow,
                honour_session_grants: true,
            },
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
    pub max_tokens: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAgent {
    model: Option<String>,
    reasoning: Option<String>,
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
        assert_eq!(agent.max_tokens, None);
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
            ("[agent]\nbogus = 1\n", "unknown field"),
            ("[permissions]\nwrite = \"maybe\"\n", "[permissions]"),
        ] {
            let e = from_bricks_toml(text).unwrap_err();
            assert!(e.contains(needle), "{text}: {e}");
        }
    }

    #[test]
    fn the_override_follows_the_priority_table() {
        use ApprovalMode::*;
        // Project rules: Read allowed, Write asked, Bash denied by tool rule,
        // and a level rule that denies `dangerous` tools.
        let (_, rules) = from_bricks_toml(
            "[permissions]\ndangerous = \"deny\"\n\n[permissions.tools]\nBash = \"deny\"\n",
        )
        .unwrap();
        let eff = |mode, tool: &str, level| rules.effective(mode, tool, level);
        let deny = Effective { action: Action::Deny, honour_session_grants: true };
        let ask_no_grants = Effective { action: Action::Ask, honour_session_grants: false };
        let allow = Effective { action: Action::Allow, honour_session_grants: true };

        // Standard: exactly the rules, session grants honoured.
        assert_eq!(eff(Standard, "Read", PermissionLevel::ReadOnly), allow);
        assert_eq!(
            eff(Standard, "Write", PermissionLevel::Write),
            Effective { action: Action::Ask, honour_session_grants: true }
        );
        assert_eq!(eff(Standard, "Bash", PermissionLevel::Execute), deny);
        assert_eq!(eff(Standard, "Dangerous", PermissionLevel::Dangerous), deny);

        // Forbidden and explicit denials win over every mode.
        for mode in [Standard, AlwaysAsk, NoConfirmation] {
            assert_eq!(eff(mode, "X", PermissionLevel::Forbidden), deny, "{mode:?}");
            assert_eq!(eff(mode, "Bash", PermissionLevel::Execute), deny, "{mode:?}");
            assert_eq!(eff(mode, "Dangerous", PermissionLevel::Dangerous), deny, "{mode:?}");
        }

        // AlwaysAsk: every other call is asked, even when the rules allow it,
        // and session grants are ignored.
        assert_eq!(eff(AlwaysAsk, "Read", PermissionLevel::ReadOnly), ask_no_grants);
        assert_eq!(eff(AlwaysAsk, "Write", PermissionLevel::Write), ask_no_grants);
        assert_eq!(eff(AlwaysAsk, "Edit", PermissionLevel::None), ask_no_grants);

        // NoConfirmation: every other call runs without asking.
        assert_eq!(eff(NoConfirmation, "Read", PermissionLevel::ReadOnly), allow);
        assert_eq!(eff(NoConfirmation, "Write", PermissionLevel::Write), allow);
    }

    #[test]
    fn the_shared_override_is_read_at_each_decision() {
        let cell = ApprovalOverride::new(ApprovalMode::Standard);
        let other_view = Arc::clone(&cell);
        assert_eq!(other_view.get(), ApprovalMode::Standard, "default is standard");
        cell.set(ApprovalMode::AlwaysAsk);
        assert_eq!(other_view.get(), ApprovalMode::AlwaysAsk, "every holder sees the change");
        cell.set(ApprovalMode::NoConfirmation);
        assert_eq!(other_view.get(), ApprovalMode::NoConfirmation);
        cell.set(ApprovalMode::Standard);
        assert_eq!(other_view.get(), ApprovalMode::Standard);
    }
}
