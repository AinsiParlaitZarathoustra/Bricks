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
//!
//! Before each request, every activation is checked against its source
//! (identity, source, revision): one whose skill was removed, replaced or
//! changed is withdrawn. Its text is replaced in the active history by a
//! short note (tool calls and results stay paired; the raw history keeps
//! everything), a sub-agent's preloaded copy leaves its system prompt, and a
//! compaction summary written from it is set aside: the active history is
//! rebuilt from the pre-compaction snapshots, cleaned, and compacted again
//! when it does not fit.

use crate::Agent;
use crate::events::AgentEvent;
use bricks_skills::{Activation, Origin, StoredActivation, StoredLedger, render_activation};
use bricks_tools::skill_tool::SkillSession;
use bricks_types::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const STORE_FILE: &str = "skills.json";
const MARK: &str = "<skill_activation id=\"";
const END: &str = "</skill_activation>";

/// What replaces the instructions of a withdrawn activation.
fn withdrawn_note(name: &str) -> String {
    format!(
        "[Skill `{name}` is no longer available: its instructions were withdrawn and no \
         longer apply.]"
    )
}

/// `text` with the activations of `withdrawn` (id → name) replaced by a
/// note; `None` when none is there.
pub(crate) fn scrub_text(text: &str, withdrawn: &HashMap<String, String>) -> Option<String> {
    if !text.contains(MARK) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(i) = rest.find(MARK) {
        let after = &rest[i + MARK.len()..];
        let name = after.find('"').and_then(|end| withdrawn.get(&after[..end]));
        match name {
            Some(name) => {
                out.push_str(&rest[..i]);
                out.push_str(&withdrawn_note(name));
                // The body is escaped: the first closing tag is its own.
                rest = match rest[i..].find(END) {
                    Some(e) => &rest[i + e + END.len()..],
                    None => "",
                };
                changed = true;
            }
            None => {
                out.push_str(&rest[..i + MARK.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// Replace the withdrawn activations in every text of `messages`.
pub(crate) fn scrub_messages(
    messages: &mut [Message],
    withdrawn: &HashMap<String, String>,
) -> bool {
    let mut changed = false;
    let mut fix = |t: &mut String| {
        if let Some(new) = scrub_text(t, withdrawn) {
            *t = new;
            changed = true;
        }
    };
    for m in messages.iter_mut() {
        match &mut m.content {
            MessageContent::Text(t) => fix(t),
            MessageContent::Blocks(bs) => {
                for b in bs.iter_mut() {
                    match b {
                        ContentBlock::Text { text } => fix(text),
                        ContentBlock::ToolResult {
                            content: ToolResultContent::Text(t),
                            ..
                        } => fix(t),
                        ContentBlock::ToolResult {
                            content: ToolResultContent::Blocks(inner),
                            ..
                        } => {
                            for ib in inner.iter_mut() {
                                if let ContentBlock::Text { text } = ib {
                                    fix(text)
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    changed
}

fn is_summary(b: &ContentBlock) -> bool {
    matches!(b, ContentBlock::Text { text } if text.starts_with(crate::compact::SUMMARY_OPEN))
}

fn has_summary(m: &Message) -> bool {
    match &m.content {
        MessageContent::Text(t) => t.starts_with(crate::compact::SUMMARY_OPEN),
        MessageContent::Blocks(bs) => bs.iter().any(is_summary),
    }
}

/// Whether the summary made at compaction `k` (1-based) was written from
/// a withdrawn activation's text: its snapshot holds one, or the summary
/// it summarised was itself written from one.
fn summary_tainted(
    snapshots: &[crate::CompactionSnapshot],
    k: usize,
    withdrawn: &HashSet<String>,
) -> bool {
    let Some(snap) = k.checked_sub(1).and_then(|i| snapshots.get(i)) else {
        return false;
    };
    if !ids_in(&snap.messages).is_disjoint(withdrawn) {
        return true;
    }
    snap.messages.first().is_some_and(has_summary) && summary_tainted(snapshots, k - 1, withdrawn)
}

/// Same role and same blocks (a compaction rewrites a kept text message as
/// blocks when it merges the summary into it).
fn same(a: &Message, b: &Message) -> bool {
    a.role == b.role
        && serde_json::to_value(a.content_blocks()).ok()
            == serde_json::to_value(b.content_blocks()).ok()
}

/// The active history with its summary replaced by what it summarised,
/// when that summary was written from a withdrawn activation (`None`: it
/// was not, or there is none). The summary in the active history is the
/// last compaction's: a later clear or compaction would have replaced it.
pub(crate) fn expand_tainted_summary(
    active: &[Message],
    snapshots: &[crate::CompactionSnapshot],
    withdrawn: &HashSet<String>,
) -> Option<Vec<Message>> {
    let first = active.first()?;
    if !has_summary(first) || !summary_tainted(snapshots, snapshots.len(), withdrawn) {
        return None;
    }
    let before = &snapshots.last()?.messages;
    // The first message without the summary (and the instructions a
    // compaction re-added after it): what was the first kept message.
    let mut head = first.clone();
    let rest_blocks: Vec<ContentBlock> = first
        .content_blocks()
        .into_iter()
        .filter(|b| {
            !is_summary(b) && !matches!(b, ContentBlock::Text { text } if text.starts_with(MARK))
        })
        .collect();
    let mut after: Vec<Message> = Vec::new();
    if !rest_blocks.is_empty() {
        head.content = MessageContent::Blocks(rest_blocks);
        after.push(head);
    }
    after.extend_from_slice(&active[1..]);
    // The kept messages are the snapshot's last ones: join the two without
    // repeating them.
    let overlap = (0..=before.len().min(after.len()))
        .rev()
        .find(|&n| (0..n).all(|i| same(&before[before.len() - n + i], &after[i])))
        .unwrap_or(0);
    let mut out = before.clone();
    out.extend_from_slice(&after[overlap..]);
    // The snapshot may open with an older tainted summary: expand it too.
    match expand_tainted_summary(&out, &snapshots[..snapshots.len() - 1], withdrawn) {
        Some(deeper) => Some(deeper),
        None => Some(out),
    }
}

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
                ))];
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

    /// Before a request: withdraw the activations whose skill is no longer
    /// the one that was activated (removed, installed again, changed, no
    /// longer allowed), and take their instructions out of what the model
    /// will read. Returns the events to report.
    pub(crate) fn reconcile_skills(&self) -> Vec<AgentEvent> {
        let Some(s) = &self.skills else {
            return Vec::new();
        };
        let entries: Vec<_> = s.ledger.lock().all().to_vec();
        let mut events = Vec::new();
        let mut suspended = false;
        for e in entries.iter().filter(|e| e.suspended.is_none()) {
            let a = &e.activation;
            if let Err(why) = s.access.verify(&a.name, &a.source, &a.revision) {
                s.ledger.lock().suspend(&e.id, why.clone());
                suspended = true;
                events.push(AgentEvent::SkillNotActivated {
                    agent_id: None,
                    name: a.name.clone(),
                    reason: format!("withdrawn: {why}"),
                });
            }
        }
        if suspended {
            self.save_skills();
        }
        let withdrawn: HashMap<String, String> = s
            .ledger
            .lock()
            .all()
            .iter()
            .filter(|a| a.suspended.is_some())
            .map(|a| (a.id.clone(), a.activation.name.clone()))
            .collect();
        if withdrawn.is_empty() {
            return events;
        }
        let ids: HashSet<String> = withdrawn.keys().cloned().collect();
        let mut rewritten = false;
        {
            let mut msgs = self.messages.lock();
            // Summaries are checked once per set of withdrawn activations
            // and snapshots; the texts at every request (cheap).
            let snapshots = self.snapshots.lock();
            let mut sorted: Vec<&String> = ids.iter().collect();
            sorted.sort();
            let fingerprint = format!("{}|{sorted:?}", snapshots.len());
            let mut checked = self.skills_checked.lock();
            if checked.as_deref() != Some(fingerprint.as_str()) {
                if let Some(rebuilt) = expand_tainted_summary(&msgs, &snapshots, &ids) {
                    *msgs = rebuilt;
                    rewritten = true;
                }
                *checked = Some(fingerprint);
            }
            if scrub_messages(&mut msgs, &withdrawn) {
                rewritten = true;
            }
        }
        if rewritten {
            self.context
                .lock()
                .invalidate("withdrawn skill instructions left the active context");
            events.push(AgentEvent::Status(
                "Instructions of skills that are no longer available were withdrawn from the \
                 context."
                    .into(),
            ));
        }
        events
    }

    /// `text` without the instructions of withdrawn activations (a
    /// sub-agent's preloaded skills live in its system prompt).
    pub(crate) fn without_withdrawn_skills(&self, text: String) -> String {
        let Some(s) = &self.skills else {
            return text;
        };
        let withdrawn: HashMap<String, String> = s
            .ledger
            .lock()
            .all()
            .iter()
            .filter(|a| a.suspended.is_some())
            .map(|a| (a.id.clone(), a.activation.name.clone()))
            .collect();
        if withdrawn.is_empty() {
            return text;
        }
        scrub_text(&text, &withdrawn).unwrap_or(text)
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

    fn activation(id: &str, name: &str, body: &str) -> String {
        format!(
            "<skill_activation id=\"{id}\" name=\"{name}\" source=\"store:x\" revision=\"r\">\n\
             --- instructions ---\n{body}\n--- end of instructions ---\n</skill_activation>"
        )
    }

    fn withdrawn(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(i, n)| (i.to_string(), n.to_string()))
            .collect()
    }

    fn tool_result(id: &str, text: String) -> Message {
        Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: ToolResultContent::Text(text),
            is_error: Some(false),
        }])
    }

    fn all_text(messages: &[Message]) -> String {
        serde_json::to_string(messages).unwrap()
    }

    #[test]
    fn only_the_withdrawn_activation_is_replaced_and_its_call_stays_paired() {
        let kept = activation("skill_k", "kept", "KEEP-ME");
        let gone = activation("skill_g", "gone", "GONE-SECRET");
        let mut msgs = vec![
            Message::user("Please use the gone skill: GONE-SECRET is a word I typed."),
            tool_result(
                "call_1",
                format!("{gone}\n\nRequest to apply it to (from the call): staging"),
            ),
            Message::user_blocks(vec![ContentBlock::Text {
                text: format!("before {kept} middle {gone} after"),
            }]),
        ];
        let w = withdrawn(&[("skill_g", "gone")]);
        assert!(scrub_messages(&mut msgs, &w));
        let text = all_text(&msgs);
        // The user's own words are left alone; only the engine's block goes.
        assert!(text.contains("GONE-SECRET is a word I typed"));
        assert_eq!(text.matches("GONE-SECRET").count(), 1, "{text}");
        assert!(text.contains("KEEP-ME"));
        assert!(text.contains("Skill `gone` is no longer available"));
        assert!(text.contains("Request to apply it to (from the call): staging"));
        assert!(text.contains("before ") && text.contains(" middle ") && text.contains(" after"));
        match &msgs[1].content {
            MessageContent::Blocks(b) => assert!(matches!(
                &b[0],
                ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_1"
            )),
            _ => panic!("the tool result stays a tool result"),
        }
        // Nothing left to do the second time.
        assert!(!scrub_messages(&mut msgs, &w));
    }

    fn summary(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: format!(
                "{}\n## Summary\n{text}\n</context_summary>",
                crate::compact::SUMMARY_OPEN
            ),
        }
    }

    fn snapshot(number: usize, messages: Vec<Message>) -> crate::CompactionSnapshot {
        crate::CompactionSnapshot {
            number,
            messages,
            memory_key: None,
        }
    }

    #[test]
    fn a_summary_written_from_a_withdrawn_skill_is_replaced_by_what_it_summarised() {
        let gone = activation("skill_g", "gone", "GONE-SECRET");
        // Before the compaction: four messages; the last one was kept.
        let before = vec![
            Message::user("start"),
            tool_result("call_1", gone),
            Message::assistant("done with it"),
            Message::user("continue"),
        ];
        // After: the summary (paraphrasing the skill) merged into the kept
        // message, then a newer answer.
        let active = vec![
            Message::user_blocks(vec![
                summary("The gone skill says GONE-PARAPHRASE."),
                ContentBlock::Text {
                    text: "continue".into(),
                },
            ]),
            Message::assistant("newer"),
        ];
        let snaps = vec![snapshot(1, before.clone())];
        let ids: HashSet<String> = ["skill_g".to_string()].into();
        let mut rebuilt = expand_tainted_summary(&active, &snaps, &ids).expect("tainted");
        assert!(!all_text(&rebuilt).contains("GONE-PARAPHRASE"));
        assert_eq!(rebuilt.len(), 5, "{}", all_text(&rebuilt));
        assert!(all_text(&rebuilt).contains("newer"));
        scrub_messages(&mut rebuilt, &withdrawn(&[("skill_g", "gone")]));
        assert!(!all_text(&rebuilt).contains("GONE-SECRET"));
        // A summary that never saw the skill stays.
        let other: HashSet<String> = ["skill_other".to_string()].into();
        assert!(expand_tainted_summary(&active, &snaps, &other).is_none());
    }

    #[test]
    fn a_summary_of_a_tainted_summary_is_tainted_too() {
        let gone = activation("skill_g", "gone", "GONE-SECRET");
        let first = vec![
            Message::user("a"),
            tool_result("c", gone),
            Message::user("b"),
        ];
        // The second compaction summarised the first summary (no marker
        // left in it) and kept nothing.
        let second = vec![
            Message::user_blocks(vec![
                summary("paraphrase one"),
                ContentBlock::Text { text: "b".into() },
            ]),
            Message::assistant("c"),
        ];
        let active = vec![Message::user_blocks(vec![summary("paraphrase two")])];
        let snaps = vec![snapshot(1, first), snapshot(2, second)];
        let ids: HashSet<String> = ["skill_g".to_string()].into();
        let rebuilt = expand_tainted_summary(&active, &snaps, &ids).expect("tainted");
        let text = all_text(&rebuilt);
        assert!(
            !text.contains("paraphrase one") && !text.contains("paraphrase two"),
            "{text}"
        );
        assert!(text.contains("\"a\"") || text.contains("a"));
        assert_eq!(rebuilt.len(), 4, "{text}");
    }
}
