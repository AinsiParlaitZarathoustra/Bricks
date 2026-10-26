//! The activations of one agent.
//!
//! A tool call that activates a skill only *proposes* it (a pending entry
//! under a one-time id); the engine commits it once the instructions are
//! actually in the agent's context. A refused, failed or cancelled call
//! leaves nothing active. Each agent has its own ledger: activations are
//! never shared between agents.

use crate::service::Activation;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How a skill was activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The model, through the `Skill` tool.
    Model,
    /// A person (`/skill`, `--skill`), before their request.
    User,
    /// A sub-agent profile's `skills`.
    Profile,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Model => "model",
            Origin::User => "user",
            Origin::Profile => "profile",
        }
    }
}

/// One active skill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveSkill {
    /// The id written in the activation text (`<skill_activation id=…>`).
    pub id: String,
    pub origin: Origin,
    #[serde(flatten)]
    pub activation: Activation,
    /// Suspended at restoration (the source changed, disappeared or is no
    /// longer allowed): kept for the record, never injected again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended: Option<String>,
}

#[derive(Debug, Default)]
pub struct Ledger {
    active: Vec<ActiveSkill>,
    pending: HashMap<String, (Activation, Origin)>,
}

/// What is stored with a session: identity, source, revision, origin — the
/// instructions themselves are read again from the source and checked
/// against the revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredActivation {
    pub id: String,
    pub name: String,
    pub source: String,
    pub revision: String,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredLedger {
    pub schema: u32,
    pub activations: Vec<StoredActivation>,
}

impl Ledger {
    pub fn new_id() -> String {
        format!("skill_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
    }

    /// The active (not suspended) entry with this source and revision.
    pub fn active_revision(&self, source: &str, revision: &str) -> Option<&ActiveSkill> {
        self.active
            .iter()
            .find(|a| a.suspended.is_none() && a.activation.same_revision(source, revision))
    }

    pub fn propose(&mut self, activation: Activation, origin: Origin) -> String {
        let id = Self::new_id();
        self.pending.insert(id.clone(), (activation, origin));
        id
    }

    /// Take a proposal (once). `None` when unknown: nothing to commit.
    pub fn take_pending(&mut self, id: &str) -> Option<(Activation, Origin)> {
        self.pending.remove(id)
    }

    /// Forget proposals that were never committed (end of a run,
    /// cancellation).
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub fn commit(&mut self, id: String, activation: Activation, origin: Origin) {
        // A new revision of the same skill replaces the older one.
        self.active.retain(|a| {
            !(a.activation.source == activation.source && a.activation.name == activation.name)
        });
        self.active.push(ActiveSkill {
            id,
            origin,
            activation,
            suspended: None,
        });
    }

    pub fn active(&self) -> impl Iterator<Item = &ActiveSkill> {
        self.active.iter().filter(|a| a.suspended.is_none())
    }

    pub fn all(&self) -> &[ActiveSkill] {
        &self.active
    }

    /// Drop every activation (context cleared, project changed).
    pub fn clear(&mut self) {
        self.active.clear();
        self.pending.clear();
    }

    /// Keep only the activations `keep` accepts (an earlier message was
    /// revised: the activations given after it left the context).
    pub fn retain(&mut self, keep: impl Fn(&ActiveSkill) -> bool) {
        self.active.retain(keep);
        self.pending.clear();
    }

    pub fn suspend(&mut self, id: &str, why: String) {
        if let Some(a) = self.active.iter_mut().find(|a| a.id == id) {
            a.suspended = Some(why);
        }
    }

    pub fn resume_entry(&mut self, id: &str, activation: Activation) {
        if let Some(a) = self.active.iter_mut().find(|a| a.id == id) {
            a.activation = activation;
            a.suspended = None;
        }
    }

    pub fn stored(&self) -> StoredLedger {
        StoredLedger {
            schema: 1,
            activations: self
                .active
                .iter()
                .map(|a| StoredActivation {
                    id: a.id.clone(),
                    name: a.activation.name.clone(),
                    source: a.activation.source.clone(),
                    revision: a.activation.revision.clone(),
                    origin: a.origin,
                    suspended: a.suspended.clone(),
                })
                .collect(),
        }
    }

    /// Restore entries checked by the caller: `Ok` is the activation read
    /// again (same revision), `Err` the reason it is suspended.
    pub fn restore(&mut self, entries: Vec<(StoredActivation, Result<Activation, String>)>) {
        self.active.clear();
        self.pending.clear();
        for (s, checked) in entries {
            match checked {
                Ok(activation) => self.active.push(ActiveSkill {
                    id: s.id,
                    origin: s.origin,
                    activation,
                    suspended: None,
                }),
                Err(why) => self.active.push(ActiveSkill {
                    id: s.id,
                    origin: s.origin,
                    activation: crate::service::Activation {
                        name: s.name,
                        description: String::new(),
                        source: s.source,
                        scope: crate::registry::Scope::Explicit,
                        revision: s.revision,
                        dir: None,
                        file: None,
                        body: String::new(),
                        resources: Vec::new(),
                        resources_truncated: false,
                        allowed_tools: None,
                        compatibility: None,
                        notes: Vec::new(),
                    },
                    suspended: Some(why),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Scope;

    fn act(name: &str, rev: &str) -> Activation {
        Activation {
            name: name.into(),
            description: "d".into(),
            source: "user:.agents".into(),
            scope: Scope::Personal,
            revision: rev.into(),
            dir: None,
            file: None,
            body: "b".into(),
            resources: vec![],
            resources_truncated: false,
            allowed_tools: None,
            compatibility: None,
            notes: vec![],
        }
    }

    #[test]
    fn only_committed_proposals_are_active() {
        let mut l = Ledger::default();
        let id = l.propose(act("a", "r1"), Origin::Model);
        assert!(l.active_revision("user:.agents", "r1").is_none());
        let (a, o) = l.take_pending(&id).unwrap();
        assert!(l.take_pending(&id).is_none(), "once");
        l.commit(id, a, o);
        assert!(l.active_revision("user:.agents", "r1").is_some());
        let lost = l.propose(act("b", "r2"), Origin::Model);
        l.clear_pending();
        assert!(l.take_pending(&lost).is_none());
        assert_eq!(l.active().count(), 1);
    }

    #[test]
    fn a_new_revision_replaces_the_old_one() {
        let mut l = Ledger::default();
        l.commit("1".into(), act("a", "r1"), Origin::Model);
        l.commit("2".into(), act("a", "r2"), Origin::User);
        assert_eq!(l.active().count(), 1);
        assert!(l.active_revision("user:.agents", "r2").is_some());
    }

    #[test]
    fn stored_form_has_no_instructions() {
        let mut l = Ledger::default();
        l.commit("1".into(), act("a", "r1"), Origin::Model);
        let text = serde_json::to_string(&l.stored()).unwrap();
        assert!(text.contains("\"revision\":\"r1\""));
        assert!(!text.contains("\"body\""));
        let s = l.stored().activations[0].clone();
        let mut l2 = Ledger::default();
        l2.restore(vec![(s, Err("changed".into()))]);
        assert_eq!(l2.active().count(), 0);
        assert_eq!(l2.all()[0].suspended.as_deref(), Some("changed"));
    }
}
