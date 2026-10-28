//! Skills of a run: admitted to the context and activated.

use super::context::{context_status, model_view, run_compaction};
use super::history::push_message;
use super::send;
use super::tools::FinishedCall;
use crate::Agent;
use crate::events::{AgentEvent, CompactReason};
use bricks_tools::ToolResult;
use bricks_types::*;
use tokio::sync::mpsc;

/// A successful `Skill` result proposing an activation (whether it is one
/// of this agent's own is checked, once, when it is committed).
pub(super) fn is_skill_proposal(agent: &Agent, call: &FinishedCall) -> bool {
    call.name == "Skill"
        && !call.result.is_error
        && agent.skill_session().is_some()
        && call
            .result
            .metadata
            .as_ref()
            .and_then(|m| m.get(bricks_tools::skill_tool::ACTIVATION_KEY))
            .is_some()
}

/// Whether `text` can be added to the context now, compacting once (by
/// the existing rules) when it cannot. `Err` says why it cannot.
async fn admit_skill_text(
    agent: &Agent,
    text: &str,
    tx: Option<&mpsc::Sender<AgentEvent>>,
) -> std::result::Result<(), String> {
    let check = |agent: &Agent| {
        let status = context_status(agent);
        let est = agent
            .context
            .lock()
            .estimator(&model_view(agent))
            .text(text)
            .upper;
        let limit = status.input_limit.saturating_sub(status.margin);
        let left = limit.saturating_sub(status.context_used.upper_bound);
        (est <= left, est, left)
    };
    if check(agent).0 {
        return Ok(());
    }
    // Within the current run: its cancellation stops the compaction.
    let cancel = agent.run_cancellation();
    let outcome = run_compaction(agent, CompactReason::BudgetExceeded, true, tx, &cancel).await;
    if outcome == crate::compact::CompactionOutcome::Cancelled {
        return Err("the run was cancelled".into());
    }
    let (ok, est, left) = check(agent);
    if ok {
        return Ok(());
    }
    Err(format!(
        "its instructions (about {est} tokens) do not fit in the context ({left} tokens left{})",
        if outcome.is_compacted() {
            " after a compaction"
        } else {
            "; a compaction could not free enough"
        }
    ))
}

/// Commit the activation a `Skill` result proposes, if it is one of this
/// agent's own proposals. Returns whether an activation was committed (its
/// text then bypasses output reduction).
pub(super) async fn commit_skill_activation(
    agent: &Agent,
    tool_name: &str,
    result: &mut ToolResult,
    tx: &mpsc::Sender<AgentEvent>,
) -> bool {
    if tool_name != "Skill" || result.is_error {
        return false;
    }
    let Some(session) = agent.skill_session() else {
        return false;
    };
    let Some(id) = result
        .metadata
        .as_ref()
        .and_then(|m| m.get(bricks_tools::skill_tool::ACTIVATION_KEY))
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return false;
    };
    // Only a proposal this agent's tool made counts; anything else is an
    // ordinary output.
    let Some((activation, origin)) = session.ledger.lock().take_pending(&id) else {
        return false;
    };
    match admit_skill_text(agent, &result.content, Some(tx)).await {
        Ok(()) => {
            send(
                agent,
                Some(tx),
                AgentEvent::SkillActivated {
                    agent_id: None,
                    name: activation.name.clone(),
                    source: activation.source.clone(),
                    revision: activation.revision.clone(),
                    origin: origin.as_str().into(),
                },
            )
            .await;
            session.ledger.lock().commit(id, activation, origin);
            agent.save_skills();
            true
        }
        Err(why) => {
            send(
                agent,
                Some(tx),
                AgentEvent::SkillNotActivated {
                    agent_id: None,
                    name: activation.name.clone(),
                    reason: why.clone(),
                },
            )
            .await;
            *result = ToolResult::error(format!(
                "Skill `{}` was not activated: {why}. Its instructions were not shortened; \
                 continue without them, or ask the user to free context (`/compact`, `/clear`).",
                activation.name
            ));
            false
        }
    }
}

async fn user_skill_failed(
    agent: &Agent,
    tx: &mpsc::Sender<AgentEvent>,
    name: &str,
    why: String,
) -> BricksError {
    send(
        agent,
        Some(tx),
        AgentEvent::SkillNotActivated {
            agent_id: None,
            name: name.to_string(),
            reason: why.clone(),
        },
    )
    .await;
    BricksError::InvalidInput(format!(
        "skill `{name}` was not activated: {why}; the request was not sent"
    ))
}

/// Activate the skills a person chose for this run, before their request.
pub(super) async fn activate_user_skills(
    agent: &Agent,
    names: &[String],
    tx: &mpsc::Sender<AgentEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let Some(session) = agent.skill_session() else {
        return Err(BricksError::InvalidInput(
            "skills are not configured for this session; the request was not sent".into(),
        ));
    };
    // Everything is read and checked first.
    let mut todo = Vec::new();
    for name in names {
        let a = match session.access.activate(name) {
            Ok(a) => a,
            Err(e) => return Err(user_skill_failed(agent, tx, name, e.to_string()).await),
        };
        let already = session
            .ledger
            .lock()
            .active_revision(&a.source, &a.revision)
            .is_some();
        if !already
            && !todo
                .iter()
                .any(|(_, t): &(String, bricks_skills::Activation)| {
                    t.same_revision(&a.source, &a.revision)
                })
        {
            todo.push((bricks_skills::Ledger::new_id(), a));
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    let texts: Vec<String> = todo
        .iter()
        .map(|(id, a)| agent.render_skill(a, id))
        .collect();
    if let Err(why) = admit_skill_text(agent, &texts.join("\n\n"), Some(tx)).await {
        let names: Vec<&str> = todo.iter().map(|(_, a)| a.name.as_str()).collect();
        return Err(user_skill_failed(agent, tx, &names.join(", "), why).await);
    }
    if cancel.is_cancelled() {
        return Err(BricksError::Cancelled);
    }
    let blocks: Vec<ContentBlock> = texts
        .into_iter()
        .map(|text| ContentBlock::Text { text })
        .collect();
    push_message(agent, Message::user_blocks(blocks));
    for (id, a) in todo {
        send(
            agent,
            Some(tx),
            AgentEvent::SkillActivated {
                agent_id: None,
                name: a.name.clone(),
                source: a.source.clone(),
                revision: a.revision.clone(),
                origin: "user".into(),
            },
        )
        .await;
        session
            .ledger
            .lock()
            .commit(id, a, bricks_skills::Origin::User);
    }
    agent.save_skills();
    Ok(())
}
