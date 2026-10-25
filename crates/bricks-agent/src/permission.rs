//! Permissions: one engine decides every tool call.
//!
//! A session has one [`SessionPolicy`] — a profile (ASK, EXECUTE or CUSTOM)
//! and, for CUSTOM, its rules — kept in a [`PolicyCell`] with a revision that
//! grows at each change. [`PermissionEngine`] turns each call into a
//! [`PermissionVerdict`] (allow, ask or deny, with the reason and the policy
//! revision it was made under). A call that must be asked goes to the
//! session's approver — a person through a frontend, a terminal, or an
//! embedder's [`PermissionPolicy`]; nobody is ever assumed to agree, and a
//! denial is final.
//!
//! The policy is the session's own: sub-agents use their parent's cell, two
//! sessions never share one. Nothing comes from `bricks.toml`; the default
//! for new sessions is chosen once and kept by [`PermissionStore`].

use async_trait::async_trait;
use bricks_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use bricks_tools::{PermissionLevel, ToolCategory};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a policy says about one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionEffect {
    Allow,
    Ask,
    Deny,
}

/// A decision, with why and under which policy revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionVerdict {
    pub effect: PermissionEffect,
    pub reason: String,
    pub policy_revision: u64,
}

/// What a call does, for the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    /// Reads and searches the workspace.
    Read,
    /// Writes files.
    Write,
    /// Runs commands, starts sub-agents, or anything dangerous.
    Execute,
    /// Reaches the network.
    Network,
}

impl ActionClass {
    /// The class of a call from its effective level (for its input) and its
    /// tool's category. `None`: forbidden, whatever the policy.
    pub fn of(level: PermissionLevel, category: ToolCategory) -> Option<Self> {
        Some(match level {
            PermissionLevel::Forbidden => return None,
            PermissionLevel::Write => ActionClass::Write,
            PermissionLevel::Execute | PermissionLevel::Dangerous => ActionClass::Execute,
            PermissionLevel::None | PermissionLevel::ReadOnly => match category {
                ToolCategory::Web => ActionClass::Network,
                _ => ActionClass::Read,
            },
        })
    }

    fn label(self) -> &'static str {
        match self {
            ActionClass::Read => "a read",
            ActionClass::Write => "a write",
            ActionClass::Execute => "an execution",
            ActionClass::Network => "a network access",
        }
    }
}

/// The three presentations of the one engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionProfile {
    /// Reads and searches run; writes, commands, network and sub-agents are
    /// asked.
    Ask,
    /// Everything runs without asking.
    Execute,
    /// The rules of [`CustomRules`]: the only profile that can deny.
    Custom,
}

/// The rules of the CUSTOM profile: an effect per class, and per tool
/// (a tool's own rule wins over its class).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CustomRules {
    pub read: PermissionEffect,
    pub write: PermissionEffect,
    pub execute: PermissionEffect,
    pub network: PermissionEffect,
    pub tools: BTreeMap<String, PermissionEffect>,
}

impl Default for CustomRules {
    /// The ASK profile's effects, as a starting point.
    fn default() -> Self {
        Self {
            read: PermissionEffect::Allow,
            write: PermissionEffect::Ask,
            execute: PermissionEffect::Ask,
            network: PermissionEffect::Ask,
            tools: BTreeMap::new(),
        }
    }
}

impl CustomRules {
    fn for_class(&self, class: ActionClass) -> PermissionEffect {
        match class {
            ActionClass::Read => self.read,
            ActionClass::Write => self.write,
            ActionClass::Execute => self.execute,
            ActionClass::Network => self.network,
        }
    }
}

/// A session's policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPolicy {
    pub profile: PermissionProfile,
    /// Used by the CUSTOM profile only (kept when another one is chosen, so
    /// coming back to CUSTOM finds them again).
    #[serde(default)]
    pub custom: CustomRules,
    /// Whether a person chose it. An unchosen policy behaves as ASK.
    #[serde(default)]
    pub chosen: bool,
}

impl SessionPolicy {
    /// Nothing chosen yet: reads run, everything else is asked.
    pub fn unchosen() -> Self {
        Self {
            profile: PermissionProfile::Ask,
            custom: CustomRules::default(),
            chosen: false,
        }
    }

    /// A chosen profile with the default custom rules.
    pub fn chosen(profile: PermissionProfile) -> Self {
        Self {
            profile,
            custom: CustomRules::default(),
            chosen: true,
        }
    }

    /// The effect for a call, and why.
    pub fn effect(
        &self,
        tool: &str,
        level: PermissionLevel,
        category: ToolCategory,
    ) -> (PermissionEffect, String) {
        let Some(class) = ActionClass::of(level, category) else {
            return (
                PermissionEffect::Deny,
                format!("`{tool}` is forbidden in this context"),
            );
        };
        let profile = match (self.chosen, self.profile) {
            (false, _) => "no permission policy is chosen yet (reads run, the rest is asked)",
            (true, PermissionProfile::Ask) => "the ASK profile",
            (true, PermissionProfile::Execute) => "the EXECUTE profile",
            (true, PermissionProfile::Custom) => "the CUSTOM profile",
        };
        let effect = match self.profile {
            PermissionProfile::Execute if self.chosen => PermissionEffect::Allow,
            PermissionProfile::Custom if self.chosen => {
                if let Some(effect) = self.custom.tools.get(tool) {
                    return (
                        *effect,
                        format!("`{tool}` has its own rule in the CUSTOM profile"),
                    );
                }
                self.custom.for_class(class)
            }
            _ => match class {
                ActionClass::Read => PermissionEffect::Allow,
                _ => PermissionEffect::Ask,
            },
        };
        (effect, format!("{} under {profile}", class.label()))
    }
}

/// A session's policy and its revision, shared by its agents.
#[derive(Clone)]
pub struct PolicyCell(Arc<parking_lot::RwLock<(SessionPolicy, u64)>>);

impl PolicyCell {
    pub fn new(policy: SessionPolicy, revision: u64) -> Self {
        Self(Arc::new(parking_lot::RwLock::new((policy, revision))))
    }

    pub fn get(&self) -> (SessionPolicy, u64) {
        self.0.read().clone()
    }

    pub fn revision(&self) -> u64 {
        self.0.read().1
    }

    /// Replace the policy; returns the new revision.
    pub fn set(&self, policy: SessionPolicy) -> u64 {
        self.set_saved(policy, |_, _| Ok(())).unwrap_or_default()
    }

    /// Replace the policy once `save` has recorded it under its new
    /// revision; returns that revision. When `save` fails nothing changes.
    /// Calls in progress keep the verdict they already have.
    pub fn set_saved(
        &self,
        policy: SessionPolicy,
        save: impl FnOnce(&SessionPolicy, u64) -> Result<(), String>,
    ) -> Result<u64, String> {
        let mut g = self.0.write();
        let revision = g.1 + 1;
        save(&policy, revision)?;
        *g = (policy, revision);
        Ok(revision)
    }

    /// The verdict on a call under the policy in force now.
    pub fn verdict(
        &self,
        tool: &str,
        level: PermissionLevel,
        category: ToolCategory,
    ) -> PermissionVerdict {
        let (policy, revision) = self.get();
        let (effect, reason) = policy.effect(tool, level, category);
        PermissionVerdict {
            effect,
            reason,
            policy_revision: revision,
        }
    }
}

/// The engine: the session's policy decides; a call it says to ask goes to
/// the approver. Sub-agents get the same engine (stamped with their id).
pub struct PermissionEngine {
    cell: PolicyCell,
    approver: Arc<dyn PermissionPolicy>,
}

impl PermissionEngine {
    pub fn new(cell: PolicyCell, approver: Arc<dyn PermissionPolicy>) -> Self {
        Self { cell, approver }
    }

    pub fn cell(&self) -> &PolicyCell {
        &self.cell
    }
}

#[async_trait]
impl PermissionPolicy for PermissionEngine {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        let verdict = self.cell.verdict(
            &request.tool_name,
            request.permission_level,
            request.category,
        );
        match verdict.effect {
            PermissionEffect::Allow => PermissionDecision::Allow,
            PermissionEffect::Deny => PermissionDecision::Deny(format!(
                "{} (policy revision {})",
                verdict.reason, verdict.policy_revision
            )),
            PermissionEffect::Ask => {
                let mut request = request.clone();
                request.policy_revision = Some(verdict.policy_revision);
                match self.approver.check(&request).await {
                    PermissionDecision::Deny(reason) => PermissionDecision::Deny(reason),
                    // An answer covers this call only: nothing is granted
                    // for later calls.
                    _ => PermissionDecision::AllowOnce,
                }
            }
        }
    }

    fn requires_stream(&self) -> bool {
        self.approver.requires_stream()
    }
}

/// The default policy for new sessions, chosen once by a person:
/// `<bricks_home>/permissions.json`.
#[derive(Debug, Clone)]
pub struct PermissionStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultFile {
    version: u32,
    #[serde(default)]
    default: Option<SessionPolicy>,
}

/// The policy stored with a session (in its files folder).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredPolicy {
    pub version: u32,
    pub policy: SessionPolicy,
    pub revision: u64,
}

/// The file name of a session's policy, in its files folder.
pub const SESSION_POLICY_FILE: &str = "permissions.json";

impl PermissionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The store of a Bricks home folder.
    pub fn in_home(bricks_home: &Path) -> Self {
        Self::new(bricks_home.join("permissions.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The default policy: `Ok(None)` when none was chosen. An unreadable
    /// file is an error (never taken for "nothing chosen").
    pub fn default_policy(&self) -> Result<Option<SessionPolicy>, String> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice::<DefaultFile>(&bytes)
                .map(|f| f.default.map(|p| SessionPolicy { chosen: true, ..p }))
                .map_err(|e| format!("{} cannot be read: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{} cannot be read: {e}", self.path.display())),
        }
    }

    /// Choose the default policy for new sessions.
    pub fn set_default(&self, policy: &SessionPolicy) -> Result<(), String> {
        let file = DefaultFile {
            version: 1,
            default: Some(SessionPolicy {
                chosen: true,
                ..policy.clone()
            }),
        };
        write_atomic(&self.path, &file)
    }

    /// The policy a new session starts with.
    pub fn initial_policy(&self) -> SessionPolicy {
        match self.default_policy() {
            Ok(Some(p)) => p,
            Ok(None) => SessionPolicy::unchosen(),
            Err(e) => {
                tracing::warn!("{e}; the session starts with no policy chosen");
                SessionPolicy::unchosen()
            }
        }
    }
}

/// Read a session's stored policy (`Ok(None)`: none stored).
pub fn load_session_policy(files_dir: &Path) -> Result<Option<StoredPolicy>, String> {
    let path = files_dir.join(SESSION_POLICY_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{} cannot be read: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} cannot be read: {e}", path.display())),
    }
}

/// Store a session's policy with its revision.
pub fn save_session_policy(
    files_dir: &Path,
    policy: &SessionPolicy,
    revision: u64,
) -> Result<(), String> {
    write_atomic(
        &files_dir.join(SESSION_POLICY_FILE),
        &StoredPolicy {
            version: 1,
            policy: policy.clone(),
            revision,
        },
    )
}

fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| format!("{} cannot be written: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use PermissionEffect::*;

    fn effect(
        p: &SessionPolicy,
        tool: &str,
        level: PermissionLevel,
        cat: ToolCategory,
    ) -> PermissionEffect {
        p.effect(tool, level, cat).0
    }

    #[test]
    fn ask_reads_and_asks_the_rest() {
        let p = SessionPolicy::chosen(PermissionProfile::Ask);
        assert_eq!(
            effect(
                &p,
                "Read",
                PermissionLevel::ReadOnly,
                ToolCategory::FileSystem
            ),
            Allow
        );
        assert_eq!(
            effect(&p, "Edit", PermissionLevel::Write, ToolCategory::FileSystem),
            Ask
        );
        assert_eq!(
            effect(&p, "Bash", PermissionLevel::Execute, ToolCategory::Shell),
            Ask
        );
        assert_eq!(
            effect(&p, "WebFetch", PermissionLevel::ReadOnly, ToolCategory::Web),
            Ask
        );
        assert_eq!(
            effect(
                &p,
                "Agent",
                PermissionLevel::Execute,
                ToolCategory::Orchestration
            ),
            Ask
        );
    }

    #[test]
    fn execute_allows_everything_but_the_forbidden() {
        let p = SessionPolicy::chosen(PermissionProfile::Execute);
        for (l, c) in [
            (PermissionLevel::Write, ToolCategory::FileSystem),
            (PermissionLevel::Dangerous, ToolCategory::Shell),
            (PermissionLevel::ReadOnly, ToolCategory::Web),
        ] {
            assert_eq!(effect(&p, "x", l, c), Allow);
        }
        assert_eq!(
            effect(&p, "x", PermissionLevel::Forbidden, ToolCategory::Shell),
            Deny
        );
    }

    #[test]
    fn custom_tool_rules_win_over_classes() {
        let mut p = SessionPolicy::chosen(PermissionProfile::Custom);
        p.custom.write = Deny;
        p.custom.tools.insert("Write".into(), Allow);
        assert_eq!(
            effect(&p, "Edit", PermissionLevel::Write, ToolCategory::FileSystem),
            Deny
        );
        assert_eq!(
            effect(
                &p,
                "Write",
                PermissionLevel::Write,
                ToolCategory::FileSystem
            ),
            Allow
        );
    }

    #[test]
    fn an_unchosen_policy_behaves_as_ask_even_if_it_says_execute() {
        let mut p = SessionPolicy::unchosen();
        p.profile = PermissionProfile::Execute;
        assert_eq!(
            effect(&p, "Bash", PermissionLevel::Execute, ToolCategory::Shell),
            Ask
        );
        assert!(
            p.effect("Bash", PermissionLevel::Execute, ToolCategory::Shell)
                .1
                .contains("no permission policy is chosen")
        );
    }

    #[test]
    fn revisions_grow_and_verdicts_carry_them() {
        let cell = PolicyCell::new(SessionPolicy::unchosen(), 0);
        assert_eq!(
            cell.verdict("Bash", PermissionLevel::Execute, ToolCategory::Shell)
                .policy_revision,
            0
        );
        assert_eq!(
            cell.set(SessionPolicy::chosen(PermissionProfile::Execute)),
            1
        );
        let v = cell.verdict("Bash", PermissionLevel::Execute, ToolCategory::Shell);
        assert_eq!((v.effect, v.policy_revision), (Allow, 1));
    }

    #[test]
    fn the_default_is_chosen_once_and_an_unreadable_file_is_not_nothing() {
        let t = tempfile::tempdir().unwrap();
        let store = PermissionStore::in_home(t.path());
        assert_eq!(store.default_policy().unwrap(), None);
        assert_eq!(store.initial_policy(), SessionPolicy::unchosen());
        store
            .set_default(&SessionPolicy::chosen(PermissionProfile::Execute))
            .unwrap();
        let p = store.default_policy().unwrap().unwrap();
        assert!(p.chosen && p.profile == PermissionProfile::Execute);
        std::fs::write(store.path(), "{ broken").unwrap();
        assert!(store.default_policy().is_err());
        assert_eq!(store.initial_policy(), SessionPolicy::unchosen());
    }

    #[test]
    fn a_session_policy_round_trips() {
        let t = tempfile::tempdir().unwrap();
        assert!(load_session_policy(t.path()).unwrap().is_none());
        let mut p = SessionPolicy::chosen(PermissionProfile::Custom);
        p.custom.tools.insert("Bash".into(), Deny);
        save_session_policy(t.path(), &p, 7).unwrap();
        let s = load_session_policy(t.path()).unwrap().unwrap();
        assert_eq!((s.policy, s.revision), (p, 7));
    }
}
