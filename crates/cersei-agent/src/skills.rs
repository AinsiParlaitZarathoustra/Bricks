//! Skills in the engine: the catalog in each agent's system prompt, the
//! activations of each agent (committed only on the verified path), their
//! fit in the context, their protection from removals and compaction, and
//! their storage with the session.
//!
//! An activation's text lives where it was given — the `Skill` tool result,
//! the user's pre-run message, a sub-agent's system prompt — marked by its
//! id (`<skill_activation id="…">`). The engine finds its active
//! activations by those ids (taken from the ledger, never from a tool's
//! name or metadata alone): they are kept out of the tool-result budget,
//! re-added after a compaction that summarised them away, and checked
//! again when a session is restored. Nothing is ever cut: an activation
//! that does not fit is refused.

use crate::events::AgentEvent;
use crate::Agent;
use bricks_skills::{render_activation, Activation, Origin, StoredActivation, StoredLedger};
use cersei_tools::skill_tool::SkillSession;
use cersei_types::*;
use std::collections::HashSet;
use std::path::PathBuf;

const STORE_FILE: &str = "skills.json";
const MARK: &str = "<skill_activation id=\"";

/// Ids of activations whose text is in these messages.
pub(crate) fn ids_in(messages: &[Message]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut scan = |t: &str| {
        let mut rest = t;
        while let Some(i) = rest.find(MARK) {
            rest = &rest[i + MARK.len()..];
            if let Some(end) = rest.find('"') {
                out.insert(rest[..end].to_string());
                rest = &rest[end..];
            }
        }
    };
    for m in messages {
        match &m.content {
            MessageContent::Text(t) => scan(t),
            MessageContent::Blocks(bs) => {
                for b in bs {
                    match b {
                        ContentBlock::Text { text } => scan(text),
                        ContentBlock::ToolResult {
                            content: ToolResultContent::Text(t),
                            ..
                        } => scan(t),
                        ContentBlock::ToolResult {
                            content: ToolResultContent::Blocks(inner),
                            ..
                        } => {
                            for ib in inner {
                                if let ContentBlock::Text { text } = ib {
                                    scan(text)
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

impl Agent {
    pub(crate) fn skill_session(&self) -> Option<&SkillSession> {
        self.skills.as_ref()
    }

    /// The skills' block of the system prompt (activable skills only).
    pub(crate) fn skills_catalog(&self) -> Option<String> {
        self.skills.as_ref()?.access.catalog().render()
    }

    /// Markers of the active activations (protected text).
    pub(crate) fn active_skill_markers(&self) -> Vec<String> {
        match &self.skills {
            None => Vec::new(),
            Some(s) => s
                .ledger
                .lock()
                .active()
                .filter(|a| a.origin != Origin::Profile)
                .map(|a| format!("{MARK}{}\"", a.id))
                .collect(),
        }
    }

    /// The skills active for this agent: (name, source, revision, origin).
    pub fn active_skills(&self) -> Vec<(String, String, String, &'static str)> {
        match &self.skills {
            None => Vec::new(),
            Some(s) => s
                .ledger
                .lock()
                .active()
                .map(|a| {
                    (
                        a.activation.name.clone(),
                        a.activation.source.clone(),
                        a.activation.revision.clone(),
                        a.origin.as_str(),
                    )
                })
                .collect(),
        }
    }

    /// Check that `names` can be activated now (exist, allowed, valid,
    /// compatible), without activating anything.
    pub fn check_skills(&self, names: &[String]) -> std::result::Result<(), String> {
        if names.is_empty() {
            return Ok(());
        }
        let Some(s) = &self.skills else {
            return Err("skills are not configured for this session".into());
        };
        for n in names {
            s.access.activate(n).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn skills_store(&self) -> Option<PathBuf> {
        let (memory, sid) = (self.memory.as_ref()?, self.session_id.as_ref()?);
        Some(memory.session_files_dir(sid)?.join(STORE_FILE))
    }

    /// Store the ledger with the session (identities and revisions only).
    pub(crate) fn save_skills(&self) {
        let (Some(s), Some(path)) = (&self.skills, self.skills_store()) else {
            return;
        };
        let stored = s.ledger.lock().stored();
        if stored.activations.is_empty() && !path.exists() {
            return;
        }
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&stored)?)?;
            std::fs::rename(&tmp, &path)
        };
        if let Err(e) = write() {
            tracing::warn!("could not store the session's skills: {e}");
        }
    }

    /// Restore the ledger stored with the session: each activation is
    /// checked against its source (same winner, same revision, allowed
    /// now). Those that fail are suspended — never replaced by a new text
    /// without saying so. Returns the events to report.
    pub(crate) fn restore_skills(&self) -> Vec<AgentEvent> {
        let (Some(s), Some(path)) = (&self.skills, self.skills_store()) else {
            return Vec::new();
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return Vec::new(); // an older session: nothing to restore
        };
        let stored: StoredLedger = match serde_json::from_slice(&bytes) {
            Ok(l) => l,
            Err(e) => {
                return vec![AgentEvent::Status(format!(
                    "The session's skills could not be read ({e}); none is active."
                ))]
            }
        };
        let mut events = Vec::new();
        let mut checked: Vec<(StoredActivation, std::result::Result<Activation, String>)> =
            Vec::new();
        for a in stored.activations {
            let result = match &a.suspended {
                Some(why) => Err(why.clone()),
                None => s.access.verify(&a.name, &a.source, &a.revision),
            };
            match &result {
                Ok(_) => events.push(AgentEvent::SkillActivated {
                    agent_id: None,
                    name: a.name.clone(),
                    source: a.source.clone(),
                    revision: a.revision.clone(),
                    origin: "restored".into(),
                }),
                Err(why) => events.push(AgentEvent::SkillNotActivated {
                    agent_id: None,
                    name: a.name.clone(),
                    reason: format!("suspended: {why}"),
                }),
            }
            checked.push((a, result));
        }
        s.ledger.lock().restore(checked);
        events
    }

    /// After the trust changed: suspended activations whose source is now
    /// allowed and unchanged become active again; active ones whose source
    /// is no longer allowed are suspended.
    pub(crate) fn recheck_skills(&self) -> Vec<AgentEvent> {
        let Some(s) = &self.skills else {
            return Vec::new();
        };
        let entries: Vec<_> = s.ledger.lock().all().to_vec();
        let mut events = Vec::new();
        for e in entries {
            if e.origin == Origin::Profile {
                continue;
            }
            let check = s.access.verify(
                &e.activation.name,
                &e.activation.source,
                &e.activation.revision,
            );
            match (e.suspended.is_some(), check) {
                (true, Ok(act)) => {
                    s.ledger.lock().resume_entry(&e.id, act);
                    events.push(AgentEvent::SkillActivated {
                        agent_id: None,
                        name: e.activation.name.clone(),
                        source: e.activation.source.clone(),
                        revision: e.activation.revision.clone(),
                        origin: "restored".into(),
                    });
                }
                (false, Err(why)) => {
                    s.ledger.lock().suspend(&e.id, why.clone());
                    events.push(AgentEvent::SkillNotActivated {
                        agent_id: None,
                        name: e.activation.name.clone(),
                        reason: format!("suspended: {why}"),
                    });
                }
                _ => {}
            }
        }
        self.save_skills();
        events
    }

    /// The text of an activation, as the model receives it.
    pub(crate) fn render_skill(&self, a: &Activation, id: &str) -> String {
        render_activation(a, id, Some(&self.working_dir))
    }

    /// Activations whose text is missing from `messages` (summarised away
    /// by a compaction), rendered again: appended by the caller to the
    /// compacted history, so the instructions stay available.
    pub(crate) fn skills_to_readd(&self, messages: &[Message]) -> Vec<String> {
        let Some(s) = &self.skills else {
            return Vec::new();
        };
        let present = ids_in(messages);
        s.ledger
            .lock()
            .active()
            .filter(|a| a.origin != Origin::Profile && !present.contains(&a.id))
            .map(|a| self.render_skill(&a.activation, &a.id))
            .collect()
    }

    /// Activations of the stored session that are active but whose text is
    /// not in the active history (e.g. it was cleared before an older
    /// version re-added them): the same as after a compaction.
    pub(crate) fn readd_missing_skills(&self) {
        let texts = self.skills_to_readd(&self.messages.lock());
        if texts.is_empty() {
            return;
        }
        let blocks: Vec<ContentBlock> = texts
            .into_iter()
            .map(|text| ContentBlock::Text { text })
            .collect();
        let msg = Message::user_blocks(blocks);
        self.raw_history.lock().push(msg.clone());
        self.messages.lock().push(msg);
    }

    pub(crate) fn clear_skills(&self) {
        if let Some(s) = &self.skills {
            s.ledger.lock().clear();
            self.save_skills();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_found_in_every_kind_of_block() {
        let msgs = vec![
            Message::user("x <skill_activation id=\"skill_a\" name=\"n\"> y"),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: ToolResultContent::Text(
                    "<skill_activation id=\"skill_b\">…<skill_activation id=\"skill_c\">".into(),
                ),
                is_error: Some(false),
            }]),
        ];
        let ids = ids_in(&msgs);
        assert!(ids.contains("skill_a") && ids.contains("skill_b") && ids.contains("skill_c"));
        assert_eq!(ids.len(), 3);
    }
}
