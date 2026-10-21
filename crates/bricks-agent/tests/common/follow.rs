//! A scripted model follows the handles it is given. `Agent` and `Agents`
//! return the ids of their sub-agents at once; a model then waits for each
//! of them with `AgentControl wait`, in the order they were started. This
//! module makes that wait for a scripted run: before a request is answered
//! from the script, every started sub-agent not waited for yet is waited for,
//! one `AgentControl wait` per request.
//!
//! The waits are the harness's own answers, not scripted replies: they never
//! take a reply of the script, and they are answered with the provider's
//! default usage, as any other response of the model would be.

#![allow(dead_code)]

use bricks_agent::control::scripted::{Reply, Script, ScriptedProvider};
use bricks_provider::{CompletionRequest, CompletionStream, ModelInfo, Provider};
use bricks_types::{ContentBlock, MessageContent, ToolResultContent};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The sub-agents already waited for (shared by every model of one test).
#[derive(Default)]
pub struct Waited(parking_lot::Mutex<HashSet<String>>);

/// The ids announced by the tool results of `req` (the handles of `Agent`
/// and `Agents`: lines `- agent_…`), in the order they appear.
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
                if let Some(rest) = line.strip_prefix("- agent_") {
                    let id = rest.split(' ').next().unwrap_or("");
                    ids.push(format!("agent_{id}"));
                }
            }
        }
    }
    ids
}

/// The next `AgentControl wait` for `req`, if a started sub-agent is not
/// waited for yet. Marks it waited: the next request asks for the following
/// one, so the results come back in the order the handles were given.
pub fn next_wait(req: &CompletionRequest, waited: &Waited) -> Option<Reply> {
    let mut seen = waited.0.lock();
    let id = started_ids(req)
        .into_iter()
        .find(|id| seen.insert(id.clone()))?;
    Some(Reply::tool(
        &format!("wait_{id}"),
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
