//! The histories of a run: appended, repaired, saved, loaded, revised and cleared.

use super::tools::{interrupted_text, tool_result_block};
use crate::Agent;
use crate::compact;
use bricks_types::*;

/// Append to the active history and to the raw history.
pub(super) fn push_message(agent: &Agent, msg: Message) {
    agent.raw_history.lock().push(msg.clone());
    agent.messages.lock().push(msg);
}

/// Preserve injected input in both histories at a valid conversation boundary.
pub(super) fn inject_pending(agent: &Agent, controls: Option<&crate::events::StreamControls>) {
    if let Some(controls) = controls {
        for message in controls.drain_messages() {
            push_message(agent, Message::user(message));
            agent
                .context
                .lock()
                .invalidate("a user message was injected");
        }
    }
}

/// Answer every tool call left without a result (a run stopped while tools
/// were running), so the history stays valid for the next request. Effects
/// the tools already had are not undone; the result says so.
pub(super) fn repair_interrupted(agent: &Agent, why: &str) {
    let unanswered = compact::find_unanswered_tool_uses(&agent.messages.lock());
    if unanswered.is_empty() {
        return;
    }
    let blocks: Vec<ContentBlock> = unanswered
        .into_iter()
        .map(|id| tool_result_block(&id, interrupted_text(why), true))
        .collect();
    push_message(agent, Message::user_blocks(blocks));
}

/// Store the active and raw histories under the session id, as one pair
/// (one checkpoint for backends that publish pairs atomically, see
/// `Memory::histories_are_atomic`). Both are copied together, then the
/// locks are released before any serialization or I/O. Returns once the
/// backend has published the pair (or found it already published).
pub(crate) async fn save_session(agent: &Agent) -> Result<bool> {
    let (Some(memory), Some(session_id)) = (&agent.memory, &agent.session_id) else {
        return Ok(false);
    };
    let (histories, revision) = {
        let active = agent.messages.lock();
        let raw = agent.raw_history.lock();
        (
            bricks_memory::SessionHistories {
                active: active.clone(),
                raw: raw.clone(),
            },
            bricks_memory::SaveRevision {
                owner: agent.save_owner,
                seq: agent
                    .save_revision
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1,
            },
        )
    };
    memory
        .store_histories(session_id, histories, revision)
        .await?;
    Ok(true)
}

/// Empty the active history, keeping it as a snapshot (stored with the
/// session) and in the raw history.
pub(crate) async fn clear_context(agent: &Agent) -> Result<usize> {
    let cleared = std::mem::take(&mut *agent.messages.lock());
    let n = cleared.len();
    if n == 0 {
        return Ok(0);
    }
    let number = agent.snapshots.lock().len() + 1;
    let memory_key = match (&agent.memory, &agent.session_id) {
        (Some(memory), Some(session_id)) => {
            let key = bricks_memory::session_keys::snapshot(session_id, number);
            memory.store(&key, &cleared).await?;
            Some(key)
        }
        _ => None,
    };
    agent.snapshots.lock().push(crate::CompactionSnapshot {
        number,
        messages: cleared,
        memory_key,
    });
    agent.compaction_state.lock().applied = number;
    // The instructions of active skills left with the context.
    agent.clear_skills();
    agent
        .context
        .lock()
        .invalidate("the active context was cleared");
    save_session(agent).await?;
    Ok(n)
}

/// Cut the active history before `position` (a past user message is being
/// revised). The history as it was is kept as a snapshot (stored with the
/// session) and stays in the raw history. Returns the messages removed.
pub(crate) async fn revise_context(agent: &Agent, position: usize) -> Result<usize> {
    let before = agent.messages.lock().clone();
    if position > before.len() {
        return Err(BricksError::InvalidInput(format!(
            "the revised message is not in the active context ({position} of {})",
            before.len()
        )));
    }
    let removed = before.len() - position;
    let number = agent.snapshots.lock().len() + 1;
    let memory_key = match (&agent.memory, &agent.session_id) {
        (Some(memory), Some(session_id)) => {
            let key = bricks_memory::session_keys::snapshot(session_id, number);
            memory.store(&key, &before).await?;
            Some(key)
        }
        _ => None,
    };
    agent.snapshots.lock().push(crate::CompactionSnapshot {
        number,
        messages: before,
        memory_key,
    });
    agent.compaction_state.lock().applied = number;
    let kept = {
        let mut messages = agent.messages.lock();
        messages.truncate(position);
        messages.clone()
    };
    agent.forget_skills_absent_from(&kept);
    agent
        .context
        .lock()
        .invalidate("a past message was revised");
    save_session(agent).await?;
    Ok(removed)
}

/// Load the stored history of the agent's session into it, when it has none
/// yet: the active and raw histories (one stored pair) and the
/// pre-compaction snapshots. The context is then an estimate of the whole
/// history until the next measurement. Returns the number of messages
/// loaded, `None` when there was nothing to load.
pub(crate) async fn load_session_history(agent: &Agent) -> Result<Option<usize>> {
    let (Some(memory), Some(session_id)) = (&agent.memory, &agent.session_id) else {
        return Ok(None);
    };
    if !agent.messages.lock().is_empty() {
        return Ok(None);
    }
    // Both histories from one stored pair (one generation).
    let stored = memory.load_histories(session_id).await?;
    let history = stored.active;
    if history.is_empty() {
        return Ok(None);
    }
    let count = history.len();
    // An older session without a raw history starts from the active one.
    let raw = if stored.raw.is_empty() {
        history.clone()
    } else {
        stored.raw
    };
    // Pre-compaction snapshots, numbered from 1 without gaps; later
    // compactions continue the numbering instead of overwriting.
    let mut snapshots = Vec::new();
    loop {
        let number = snapshots.len() + 1;
        let key = bricks_memory::session_keys::snapshot(session_id, number);
        match memory.load(&key).await {
            Ok(messages) if !messages.is_empty() => snapshots.push(crate::CompactionSnapshot {
                number,
                messages,
                memory_key: Some(key),
            }),
            _ => break,
        }
    }
    *agent.raw_history.lock() = raw;
    agent.compaction_state.lock().applied = snapshots.len();
    *agent.snapshots.lock() = snapshots;
    agent.messages.lock().extend(history);
    agent
        .context
        .lock()
        .invalidate("the session was restored from storage");
    Ok(Some(count))
}
