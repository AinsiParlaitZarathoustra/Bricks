//! A scripted model and catalog: deterministic, local, free. For tests of
//! the engine and of frontends (headless, terminal interface), and for
//! reproducible demonstrations. Never used unless built explicitly.

use super::controller::{ModelCatalog, ModelChoice, ProfileChoice};
use async_trait::async_trait;
use cersei_provider::{CompletionRequest, CompletionStream, ModelInfo, ModelLimits, Provider};
use cersei_types::{CerseiError, Result, StopReason, StreamEvent, Usage};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// One model response.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub thinking: Option<String>,
    /// Streamed as these chunks.
    pub text: Vec<String>,
    /// Tool calls: (id, name, input).
    pub tools: Vec<(String, String, serde_json::Value)>,
    /// Pause before each chunk.
    pub delay: Duration,
    /// Never finish (until the request is dropped): for cancellation tests.
    pub hang: bool,
    /// Fail with this provider error.
    pub error: Option<String>,
    pub usage: Option<Usage>,
}

impl Reply {
    pub fn text(t: &str) -> Self {
        Self {
            text: vec![t.to_string()],
            ..Default::default()
        }
    }

    pub fn chunks(chunks: &[&str]) -> Self {
        Self {
            text: chunks.iter().map(|c| c.to_string()).collect(),
            ..Default::default()
        }
    }

    pub fn tool(id: &str, name: &str, input: serde_json::Value) -> Self {
        Self {
            tools: vec![(id.into(), name.into(), input)],
            ..Default::default()
        }
    }

    pub fn tools(calls: Vec<(&str, &str, serde_json::Value)>) -> Self {
        Self {
            tools: calls
                .into_iter()
                .map(|(i, n, v)| (i.to_string(), n.to_string(), v))
                .collect(),
            ..Default::default()
        }
    }

    pub fn hang() -> Self {
        Self {
            hang: true,
            ..Default::default()
        }
    }
}

/// The replies, shared by every provider built from one catalog, and the
/// requests seen (model selection, request).
#[derive(Default)]
pub struct Script {
    replies: parking_lot::Mutex<VecDeque<Reply>>,
    seen: parking_lot::Mutex<Vec<(String, CompletionRequest)>>,
}

impl Script {
    pub fn new(replies: Vec<Reply>) -> Arc<Self> {
        Arc::new(Self {
            replies: parking_lot::Mutex::new(replies.into()),
            seen: parking_lot::Mutex::new(Vec::new()),
        })
    }

    pub fn push(&self, reply: Reply) {
        self.replies.lock().push_back(reply);
    }

    pub fn requests(&self) -> Vec<(String, CompletionRequest)> {
        self.seen.lock().clone()
    }
}

pub struct ScriptedProvider {
    selection: String,
    script: Arc<Script>,
}

impl ScriptedProvider {
    pub fn new(selection: &str, script: Arc<Script>) -> Self {
        Self {
            selection: selection.to_string(),
            script,
        }
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn context_window(&self, _model: &str) -> u64 {
        100_000
    }

    fn model_info(&self) -> Option<ModelInfo> {
        Some(ModelInfo {
            selection: self.selection.clone(),
            protocol: "chat_completions",
            limits: ModelLimits {
                max_input_tokens: 100_000,
                max_output_tokens: 8_000,
                context_window_tokens: None,
            },
            reasoning_resent: false,
            token_counting: false,
        })
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream> {
        self.script
            .seen
            .lock()
            .push((self.selection.clone(), request));
        let reply = self
            .script
            .replies
            .lock()
            .pop_front()
            .unwrap_or_else(|| Reply::text("(no more scripted replies)"));
        if let Some(e) = reply.error {
            return Err(CerseiError::Provider(e));
        }
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let _ = tx
                .send(StreamEvent::MessageStart {
                    id: "m".into(),
                    model: "scripted".into(),
                    usage: None,
                })
                .await;
            if reply.hang {
                // Keep the stream open until the consumer goes away.
                tx.closed().await;
                return;
            }
            let mut index = 0;
            if let Some(t) = reply.thinking {
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index,
                        block_type: "thinking".into(),
                        id: None,
                        name: None,
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::ThinkingDelta { index, thinking: t })
                    .await;
                let _ = tx.send(StreamEvent::ContentBlockStop { index }).await;
                index += 1;
            }
            if !reply.text.is_empty() {
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index,
                        block_type: "text".into(),
                        id: None,
                        name: None,
                    })
                    .await;
                for chunk in reply.text {
                    if !reply.delay.is_zero() {
                        tokio::time::sleep(reply.delay).await;
                    }
                    if tx
                        .send(StreamEvent::TextDelta { index, text: chunk })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx.send(StreamEvent::ContentBlockStop { index }).await;
                index += 1;
            }
            let has_tools = !reply.tools.is_empty();
            for (id, name, input) in reply.tools {
                let _ = tx
                    .send(StreamEvent::ContentBlockStart {
                        index,
                        block_type: "tool_use".into(),
                        id: Some(id),
                        name: Some(name),
                    })
                    .await;
                let _ = tx
                    .send(StreamEvent::InputJsonDelta {
                        index,
                        partial_json: input.to_string(),
                    })
                    .await;
                let _ = tx.send(StreamEvent::ContentBlockStop { index }).await;
                index += 1;
            }
            let _ = tx
                .send(StreamEvent::MessageDelta {
                    stop_reason: Some(if has_tools {
                        StopReason::ToolUse
                    } else {
                        StopReason::EndTurn
                    }),
                    usage: Some(reply.usage.unwrap_or(Usage {
                        input_tokens: 100,
                        output_tokens: 10,
                        ..Default::default()
                    })),
                })
                .await;
            let _ = tx.send(StreamEvent::MessageStop).await;
        });
        Ok(CompletionStream::new(rx))
    }
}

/// A catalog of scripted models sharing one [`Script`].
pub struct ScriptedCatalog {
    pub models: Vec<ModelChoice>,
    pub script: Arc<Script>,
}

impl ScriptedCatalog {
    /// Models named `test/<name>`, each with the profiles `fast` and `deep`.
    pub fn new(names: &[&str], script: Arc<Script>) -> Arc<Self> {
        Arc::new(Self {
            models: names
                .iter()
                .map(|n| ModelChoice {
                    selection: format!("test/{n}"),
                    name: n.to_string(),
                    provider: "test".into(),
                    profiles: ["fast", "deep"]
                        .iter()
                        .map(|p| ProfileChoice {
                            id: p.to_string(),
                            label: p.to_string(),
                        })
                        .collect(),
                    default_profile: None,
                    max_input_tokens: 100_000,
                    context_window_tokens: None,
                    input: vec!["text".into(), "image".into()],
                    priced: false,
                })
                .collect(),
            script,
        })
    }
}

impl ModelCatalog for ScriptedCatalog {
    fn models(&self) -> Vec<ModelChoice> {
        self.models.clone()
    }

    fn build(
        &self,
        selection: &str,
        reasoning: Option<&str>,
    ) -> std::result::Result<Box<dyn Provider>, String> {
        let m = self
            .models
            .iter()
            .find(|m| m.selection == selection)
            .ok_or_else(|| format!("unknown model `{selection}`"))?;
        if let Some(r) = reasoning {
            if !m.profiles.iter().any(|p| p.id == r) {
                return Err(format!(
                    "model `{selection}` has no reasoning profile `{r}`"
                ));
            }
        }
        Ok(Box::new(ScriptedProvider::new(
            selection,
            self.script.clone(),
        )))
    }
}
