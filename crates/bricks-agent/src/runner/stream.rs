//! The answer of a request as it streams, and the calls started from it
//! before it ends.

use super::tools::{FinishedCall, UNPLACED, call_future};
use crate::Agent;
use crate::events::AgentEvent;
use bricks_provider::StreamAccumulator;
use bricks_tools::ToolContext;
use bricks_tools::permissions::PermissionPolicy;
use bricks_types::*;
use std::sync::Arc;
use tokio::sync::mpsc;

/// How the reception of an answer ended.
pub(super) enum Received {
    /// The stream ended (complete or not: the accumulator says).
    Ended,
    /// The run was cancelled.
    Cancelled,
    /// The provider reported an error in the stream.
    Failed(BricksError),
}

/// The calls started while the answer streamed: those still running, those
/// finished, and the ids of all of them.
pub(super) type EarlyCalls<'a> = (
    futures::stream::FuturesUnordered<futures::future::BoxFuture<'a, FinishedCall>>,
    Vec<FinishedCall>,
    std::collections::HashSet<String>,
);

/// Receive the answer into `accumulator`, forwarding its text and thinking.
///
/// With `async_tool_calls` (`early_start`), each call starts as soon as it is
/// complete in the stream, while the model still writes the rest. Those calls
/// are awaited with the others by the round and stop with the turn. A
/// `Skill` call waits for the answer: its activation is committed in the
/// order of the calls.
#[allow(clippy::too_many_arguments)]
pub(super) async fn receive<'a>(
    agent: &'a Agent,
    event_tx: &mpsc::Sender<AgentEvent>,
    tool_ctx: &ToolContext,
    permissions: &Arc<dyn PermissionPolicy>,
    cancel: &tokio_util::sync::CancellationToken,
    turn: u32,
    early_start: bool,
    rx: &mut mpsc::Receiver<StreamEvent>,
    accumulator: &mut StreamAccumulator,
) -> (EarlyCalls<'a>, Received) {
    let mut early: futures::stream::FuturesUnordered<futures::future::BoxFuture<'_, FinishedCall>> =
        futures::stream::FuturesUnordered::new();
    let mut early_done: Vec<FinishedCall> = Vec::new();
    let mut early_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Process stream events (with cancellation support)
    let mut received = Received::Ended;
    loop {
        tokio::select! {
            // A cancellation is seen first: no call starts after it.
            biased;
            _ = cancel.cancelled() => {
                received = Received::Cancelled;
                break;
            }
            Some(call) = futures::StreamExt::next(&mut early), if !early.is_empty() => {
                early_done.push(call);
            }
            event = rx.recv() => {
                match event {
                    Some(event) => {
                        let closed = match &event {
                            StreamEvent::ContentBlockStop { index } if early_start => Some(*index),
                            _ => None,
                        };
                        match &event {
                            StreamEvent::TextDelta { text, .. } => {
                                let _ = event_tx.send(AgentEvent::TextDelta(text.clone())).await;
                                agent.emit(AgentEvent::TextDelta(text.clone()));
                            }
                            StreamEvent::ThinkingDelta { thinking, .. } => {
                                let _ = event_tx
                                    .send(AgentEvent::ThinkingDelta(thinking.clone()))
                                    .await;
                                agent.emit(AgentEvent::ThinkingDelta(thinking.clone()));
                            }
                            StreamEvent::Error { message } => {
                                received = Received::Failed(BricksError::Provider(message.clone()));
                                break;
                            }
                            _ => {}
                        }
                        accumulator.process_event(event);
                        if let Some((id, name, input)) =
                            closed.and_then(|index| accumulator.tool_use(index))
                            && name != "Skill"
                            && early_ids.insert(id.clone())
                        {
                            let start = AgentEvent::ToolStart {
                                name: name.clone(),
                                id: id.clone(),
                                input: input.clone(),
                            };
                            let _ = event_tx.send(start.clone()).await;
                            agent.emit(start);
                            let names: Vec<String> =
                                agent.tool_list().iter().map(|t| t.name().to_string()).collect();
                            let cost = agent.cumulative_usage.lock().cost_usd.unwrap_or(0.0);
                            // The answer is not in the history yet: it will be.
                            let message_count = agent.messages.lock().len() + 1;
                            early.push(call_future(
                                agent,
                                tool_ctx,
                                permissions,
                                &names,
                                UNPLACED,
                                (id, name, input),
                                turn,
                                cost,
                                message_count,
                            ));
                        }
                    }
                    None => break, // Stream ended
                }
            }
        }
    }
    ((early, early_done, early_ids), received)
}
