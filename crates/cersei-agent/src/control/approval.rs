//! Approvals: the engine's permission policy, shared by every frontend.
//!
//! [`ApprovalGate`] applies the configured [`ApprovalRules`]. A call that
//! must be approved becomes an [`ApprovalRequest`] (with the change preview
//! when the tool can compute one) and waits for a decision sent back through
//! [`ApprovalBroker::respond`]. Nothing is written before the decision.
//!
//! Without anyone to ask (non-interactive), the call is refused, recorded as
//! unsatisfied and the run is stopped: it ends with an explicit result
//! instead of going on without the step that needed approval.

use super::settings::{Action, ApprovalRules};
use crate::events::AgentEvent;
use async_trait::async_trait;
use cersei_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use cersei_tools::preview::ChangePreview;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// A tool call waiting for a decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub tool_call_id: String,
    pub tool: String,
    /// Permission level of the tool (`write`, `execute`, ...).
    pub level: String,
    /// What the call does, as the tool describes it (for a shell: the
    /// command, its directory, the session definitions it uses).
    pub description: String,
    pub input: serde_json::Value,
    /// The file changes, when the tool can compute them. Shell commands and
    /// MCP calls have none: a diff could not represent their effects.
    pub preview: Option<ChangePreview>,
    /// The sub-agent asking (absent: the session's own agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// A decision on an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// This call only.
    Allow,
    /// This call and every later call of the same tool in this session.
    AllowForSession,
    Deny,
}

/// Who decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecidedBy {
    /// A person, through a frontend.
    User,
    /// An earlier "allow for this session".
    Session,
    /// Nobody could be asked (non-interactive): refused.
    NonInteractive,
    /// The run was cancelled while the request waited.
    Cancelled,
}

type Waiter = oneshot::Sender<(Decision, Option<String>)>;

/// Where approval requests go and decisions come back. One per session.
#[derive(Default)]
pub struct ApprovalBroker {
    pending: parking_lot::Mutex<HashMap<String, (ApprovalRequest, Waiter)>>,
    /// Events of the current run (the same channel as the run's own events,
    /// so requests are ordered after the tool call that caused them).
    sink: parking_lot::Mutex<Option<mpsc::Sender<AgentEvent>>>,
    /// The current run's cancellation.
    run: parking_lot::Mutex<Option<CancellationToken>>,
    interactive: std::sync::atomic::AtomicBool,
    session_allowed: parking_lot::Mutex<HashSet<String>>,
    unsatisfied: parking_lot::Mutex<Vec<ApprovalRequest>>,
}

impl ApprovalBroker {
    pub fn new(interactive: bool) -> Arc<Self> {
        let b = Self::default();
        b.interactive
            .store(interactive, std::sync::atomic::Ordering::SeqCst);
        Arc::new(b)
    }

    pub fn is_interactive(&self) -> bool {
        self.interactive.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Route the requests of a new run.
    pub(crate) fn begin_run(&self, sink: mpsc::Sender<AgentEvent>, run: CancellationToken) {
        *self.sink.lock() = Some(sink);
        *self.run.lock() = Some(run);
        self.unsatisfied.lock().clear();
    }

    /// The run ended: nothing waits any more.
    pub(crate) fn end_run(&self) {
        *self.sink.lock() = None;
        *self.run.lock() = None;
        self.pending.lock().clear();
    }

    /// Requests refused because nobody could be asked, in this run.
    pub fn unsatisfied(&self) -> Vec<ApprovalRequest> {
        self.unsatisfied.lock().clone()
    }

    /// Requests waiting for a decision.
    pub fn pending(&self) -> Vec<ApprovalRequest> {
        let mut v: Vec<ApprovalRequest> = self
            .pending
            .lock()
            .values()
            .map(|(r, _)| r.clone())
            .collect();
        v.sort_by(|a, b| a.approval_id.cmp(&b.approval_id));
        v
    }

    /// Tools allowed for the rest of the session.
    /// Another session takes over: approvals given "for the session"
    /// belonged to the previous one.
    pub fn reset_session(&self) {
        self.session_allowed.lock().clear();
    }

    pub fn session_allowed(&self) -> Vec<String> {
        let mut v: Vec<String> = self.session_allowed.lock().iter().cloned().collect();
        v.sort();
        v
    }

    /// Answer a pending request. Unknown or already answered ids are an
    /// error (the caller reports it; nothing is decided twice).
    pub fn respond(
        &self,
        approval_id: &str,
        decision: Decision,
        reason: Option<String>,
    ) -> Result<(), String> {
        let (_, waiter) = self
            .pending
            .lock()
            .remove(approval_id)
            .ok_or_else(|| format!("no pending approval `{approval_id}`"))?;
        waiter
            .send((decision, reason))
            .map_err(|_| format!("approval `{approval_id}` is no longer waited for"))
    }

    async fn emit(&self, event: AgentEvent) {
        let sink = self.sink.lock().clone();
        if let Some(tx) = sink {
            let _ = tx.send(event).await;
        }
    }

    async fn ask(&self, request: ApprovalRequest) -> (Decision, DecidedBy, Option<String>) {
        let tool = request.tool.clone();
        if self.session_allowed.lock().contains(&tool) {
            return (Decision::Allow, DecidedBy::Session, None);
        }
        let run = self.run.lock().clone();
        let has_sink = self.sink.lock().is_some();
        if !self.is_interactive() || !has_sink {
            self.unsatisfied.lock().push(request.clone());
            self.emit(AgentEvent::ApprovalRequested(request)).await;
            // Stop the run: it ends with an explicit "approval required".
            if let Some(run) = &run {
                run.cancel();
            }
            return (
                Decision::Deny,
                DecidedBy::NonInteractive,
                Some("approval required, and nobody can be asked (non-interactive)".into()),
            );
        }
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .insert(request.approval_id.clone(), (request.clone(), tx));
        let id = request.approval_id.clone();
        self.emit(AgentEvent::ApprovalRequested(request)).await;
        let cancelled = async {
            match &run {
                Some(r) => r.cancelled().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            answer = rx => match answer {
                Ok((decision, reason)) => {
                    if decision == Decision::AllowForSession {
                        self.session_allowed.lock().insert(tool);
                    }
                    (decision, DecidedBy::User, reason)
                }
                Err(_) => (Decision::Deny, DecidedBy::Cancelled, Some("the request was dropped".into())),
            },
            _ = cancelled => {
                self.pending.lock().remove(&id);
                (Decision::Deny, DecidedBy::Cancelled, Some("the run was cancelled".into()))
            }
        }
    }
}

/// The engine's permission policy: rules, then a person when the rules say
/// to ask.
pub struct ApprovalGate {
    rules: ApprovalRules,
    broker: Arc<ApprovalBroker>,
}

impl ApprovalGate {
    pub fn new(rules: ApprovalRules, broker: Arc<ApprovalBroker>) -> Self {
        Self { rules, broker }
    }

    pub fn rules(&self) -> &ApprovalRules {
        &self.rules
    }
}

fn level_name(level: cersei_tools::PermissionLevel) -> &'static str {
    use cersei_tools::PermissionLevel as L;
    match level {
        L::None => "none",
        L::ReadOnly => "read_only",
        L::Write => "write",
        L::Execute => "execute",
        L::Dangerous => "dangerous",
        L::Forbidden => "forbidden",
    }
}

#[async_trait]
impl PermissionPolicy for ApprovalGate {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        match self
            .rules
            .action_for(&request.tool_name, request.permission_level)
        {
            Action::Allow => PermissionDecision::Allow,
            Action::Deny => PermissionDecision::Deny(format!(
                "`{}` is not allowed by the approval policy ([permissions] in bricks.toml)",
                request.tool_name
            )),
            Action::Ask => {
                let req = ApprovalRequest {
                    approval_id: format!("ap_{}", uuid::Uuid::new_v4().simple()),
                    tool_call_id: request.id.clone(),
                    tool: request.tool_name.clone(),
                    level: level_name(request.permission_level).to_string(),
                    description: request.description.clone(),
                    input: request.tool_input.clone(),
                    preview: request.preview.clone(),
                    agent_id: request.agent_id.clone(),
                };
                let (approval_id, tool_call_id) =
                    (req.approval_id.clone(), req.tool_call_id.clone());
                let (decision, by, reason) = self.broker.ask(req).await;
                self.broker
                    .emit(AgentEvent::ApprovalResolved {
                        approval_id,
                        tool_call_id,
                        decision,
                        by,
                    })
                    .await;
                match decision {
                    Decision::Allow => PermissionDecision::AllowOnce,
                    Decision::AllowForSession => PermissionDecision::AllowForSession,
                    Decision::Deny => PermissionDecision::Deny(
                        reason.unwrap_or_else(|| "rejected by the user".into()),
                    ),
                }
            }
        }
    }
}
