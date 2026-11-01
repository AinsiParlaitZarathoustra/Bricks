//! A scripted model follows the handles it is given. `Agent` returns the ids
//! of its sub-agents at once; a model then waits for each of them with
//! `AgentControl wait` (agent_id), in the order they were started. This
//! module makes that wait for a scripted run: before a request is answered
//! from the script, every started sub-agent whose end is not reported yet is
//! waited for, one `AgentControl wait` per request. A wait may return earlier
//! (a delivery for the model, such as another sub-agent's report): the
//! sub-agent is then waited for again, until its end is reported.
//!
//! The waits are the harness's own answers, not scripted replies: they never
//! take a reply of the script, and they are answered with the provider's
//! default usage, as any other response of the model would be.

#![allow(dead_code)]

use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
use bricks_provider::{CompletionRequest, CompletionStream, ModelInfo, Provider};
use bricks_types::{ContentBlock, MessageContent, ToolResultContent};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The waits asked so far, per sub-agent (shared by every model of one test).
#[derive(Default)]
pub struct Waited(parking_lot::Mutex<HashMap<String, u32>>);

/// The ids announced by the tool results of `req` (the handles of `Agent`:
/// lines `- sub_0000 (…)`), in the order they appear.
pub fn started_ids(req: &CompletionRequest) -> Vec<String> {
    let mut ids = Vec::new();
    for m in &req.messages {
        let MessageContent::Blocks(blocks) = &m.content else {
            continue;
        };
        for b in blocks {
            let ContentBlock::ToolResult { content, .. } = b else {
                continue;
            };
            let ToolResultContent::Text(text) = content else {
                continue;
            };
            if !text.contains("sub-agent(s) started") {
                continue;
            }
            for line in text.lines() {
                if let Some(id) = line
                    .strip_prefix("- ")
                    .and_then(|rest| rest.split(' ').next())
                    .filter(|id| id.starts_with("sub_"))
                {
                    ids.push(id.to_string());
                }
            }
        }
    }
    ids
}

/// The sub-agent whose end a `wait` result of `req` reported, if any (the
/// wait's answer: `<id> has ended; AgentControl result <id> gives its report.`).
fn ended_in(text: &str) -> Option<String> {
    let at = text.find(" has ended; AgentControl result ")?;
    let id = text[..at].rsplit(char::is_whitespace).next()?;
    id.starts_with("sub_").then(|| id.to_string())
}

/// The ids whose end is reported by a wait result of `req`.
fn ended_ids(req: &CompletionRequest) -> HashSet<String> {
    let mut ids = HashSet::new();
    for m in &req.messages {
        let MessageContent::Blocks(blocks) = &m.content else {
            continue;
        };
        for b in blocks {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content: ToolResultContent::Text(text),
                ..
            } = b
                && tool_use_id.starts_with("wait_")
                && let Some(id) = ended_in(text)
            {
                ids.insert(id);
            }
        }
    }
    ids
}

/// The next `AgentControl wait` for `req`, if a started sub-agent has not
/// reported its end yet. The first wait for a sub-agent is `wait_<id>`; a
/// later one, `wait_<id>_<n>`.
pub fn next_wait(req: &CompletionRequest, waited: &Waited) -> Option<Reply> {
    let ended = ended_ids(req);
    let id = started_ids(req)
        .into_iter()
        .find(|id| !ended.contains(id))?;
    let mut counts = waited.0.lock();
    let n = counts
        .entry(id.clone())
        .and_modify(|n| *n += 1)
        .or_insert(1);
    let call = if *n == 1 {
        format!("wait_{id}")
    } else {
        format!("wait_{id}_{n}")
    };
    Some(Reply::tool(
        &call,
        "AgentControl",
        json!({ "action": "wait", "agent_id": id }),
    ))
}

/// A provider that waits for started sub-agents while `enabled`, and
/// otherwise answers as `inner` does. Used where the scripted model's
/// children are built by a catalog (the parent and every child).
pub struct FollowHandles {
    pub inner: Box<dyn Provider>,
    pub selection: String,
    pub waited: Arc<Waited>,
    pub enabled: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Provider for FollowHandles {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn context_window(&self, model: &str) -> u64 {
        self.inner.context_window(model)
    }

    fn model_info(&self) -> Option<ModelInfo> {
        self.inner.model_info()
    }

    async fn complete(&self, request: CompletionRequest) -> bricks_types::Result<CompletionStream> {
        if self.enabled.load(Ordering::SeqCst)
            && let Some(reply) = next_wait(&request, &self.waited)
        {
            return ScriptedProvider::new(&self.selection, Script::new(vec![reply]))
                .complete(request)
                .await;
        }
        self.inner.complete(request).await
    }
}
