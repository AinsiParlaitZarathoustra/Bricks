//! Approvals: how a person is asked, shared by every frontend.
//!
//! The session's permission engine (`crate::permission`) decides each call.
//! A call it says to ask goes through [`BrokerApprover`]: it becomes an
//! [`ApprovalRequest`] (with the change preview when the tool can compute
//! one) and waits for a decision sent back through
//! [`ApprovalBroker::respond`]. Nothing is written before the decision, and
//! an answer covers that one call.
//!
//! Without anyone to ask (non-interactive), the call is refused, recorded as
//! unsatisfied and the run is stopped: it ends with an explicit result
//! instead of going on without the step that needed approval.

use crate::events::AgentEvent;
use async_trait::async_trait;
use bricks_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use bricks_tools::preview::ChangePreview;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// The revision of the session's policy that asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_revision: Option<u64>,
}

/// A decision on an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// This call only.
    Allow,
    Deny,
}

/// Who decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecidedBy {
    /// A person, through a frontend.
    User,
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

    /// Asks a person (or refuses, non-interactive).
    async fn ask(&self, request: ApprovalRequest) -> (Decision, DecidedBy, Option<String>) {
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
                Ok((decision, reason)) => (decision, DecidedBy::User, reason),
                Err(_) => (Decision::Deny, DecidedBy::Cancelled, Some("the request was dropped".into())),
            },
            _ = cancelled => {
                self.pending.lock().remove(&id);
                (Decision::Deny, DecidedBy::Cancelled, Some("the run was cancelled".into()))
            }
        }
    }
}

/// The approver of a session: asks a person through the broker. It is only
/// consulted for the calls the session's policy says to ask.
pub struct BrokerApprover {
    broker: Arc<ApprovalBroker>,
}

impl BrokerApprover {
    pub fn new(broker: Arc<ApprovalBroker>) -> Self {
        Self { broker }
    }
}

fn level_name(level: bricks_tools::PermissionLevel) -> &'static str {
    use bricks_tools::PermissionLevel as L;
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
impl PermissionPolicy for BrokerApprover {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        let req = ApprovalRequest {
            approval_id: format!("ap_{}", uuid::Uuid::new_v4().simple()),
            tool_call_id: request.id.clone(),
            tool: request.tool_name.clone(),
            level: level_name(request.permission_level).to_string(),
            description: request.description.clone(),
            input: request.tool_input.clone(),
            preview: request.preview.clone(),
            agent_id: request.agent_id.clone(),
            policy_revision: request.policy_revision,
        };
        let (approval_id, tool_call_id) = (req.approval_id.clone(), req.tool_call_id.clone());
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
            Decision::Deny => {
                PermissionDecision::Deny(reason.unwrap_or_else(|| "rejected by the user".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{PermissionEngine, PermissionProfile, PolicyCell, SessionPolicy};
    use bricks_tools::{PermissionLevel, ToolCategory};
    use std::time::Duration;

    fn request(tool: &str, level: PermissionLevel, id: &str) -> PermissionRequest {
        PermissionRequest {
            tool_name: tool.into(),
            tool_input: serde_json::json!({}),
            permission_level: level,
            category: ToolCategory::FileSystem,
            description: format!("run {tool}"),
            id: id.into(),
            preview: None,
            agent_id: None,
            policy_revision: None,
        }
    }

    /// An interactive broker with a live run, and an engine on `cell`.
    fn setup(
        cell: PolicyCell,
    ) -> (
        Arc<ApprovalBroker>,
        Arc<PermissionEngine>,
        mpsc::Receiver<AgentEvent>,
    ) {
        let broker = ApprovalBroker::new(true);
        let (tx, rx) = mpsc::channel(64);
        broker.begin_run(tx, CancellationToken::new());
        let engine = PermissionEngine::new(cell, Arc::new(BrokerApprover::new(broker.clone())));
        (broker, Arc::new(engine), rx)
    }

    /// Waits until a request is pending, and returns it.
    async fn wait_pending(broker: &ApprovalBroker) -> ApprovalRequest {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(r) = broker.pending().first() {
                    return r.clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a request should be pending")
    }

    fn spawn_check(
        engine: &Arc<PermissionEngine>,
        req: PermissionRequest,
    ) -> tokio::task::JoinHandle<PermissionDecision> {
        let engine = Arc::clone(engine);
        tokio::spawn(async move { engine.check(&req).await })
    }

    #[tokio::test]
    async fn an_answer_covers_one_call_only() {
        let cell = PolicyCell::new(SessionPolicy::chosen(PermissionProfile::Ask), 3);
        let (broker, engine, _rx) = setup(cell);
        let first = spawn_check(&engine, request("Write", PermissionLevel::Write, "c1"));
        let pending = wait_pending(&broker).await;
        assert_eq!(
            pending.policy_revision,
            Some(3),
            "the request says which revision asked"
        );
        broker
            .respond(&pending.approval_id, Decision::Allow, None)
            .unwrap();
        assert!(matches!(
            first.await.unwrap(),
            PermissionDecision::AllowOnce
        ));
        // The same tool again: asked again.
        let second = spawn_check(&engine, request("Write", PermissionLevel::Write, "c2"));
        let pending = wait_pending(&broker).await;
        broker
            .respond(&pending.approval_id, Decision::Deny, Some("no".into()))
            .unwrap();
        assert!(matches!(second.await.unwrap(), PermissionDecision::Deny(_)));
    }

    #[tokio::test]
    async fn a_pending_request_is_not_resolved_by_a_change_of_policy() {
        let cell = PolicyCell::new(SessionPolicy::chosen(PermissionProfile::Ask), 0);
        let (broker, engine, _rx) = setup(cell.clone());
        let mut check = spawn_check(&engine, request("Write", PermissionLevel::Write, "p"));
        let pending = wait_pending(&broker).await;
        cell.set(SessionPolicy::chosen(PermissionProfile::Execute));
        let still = tokio::time::timeout(Duration::from_millis(100), &mut check).await;
        assert!(still.is_err(), "the change must not answer the request");
        broker
            .respond(&pending.approval_id, Decision::Deny, None)
            .unwrap();
        assert!(matches!(check.await.unwrap(), PermissionDecision::Deny(_)));
        // The next call follows the new policy.
        let next = engine
            .check(&request("Write", PermissionLevel::Write, "q"))
            .await;
        assert!(matches!(next, PermissionDecision::Allow));
    }

    #[tokio::test]
    async fn without_anyone_to_ask_the_call_is_refused_and_recorded() {
        let broker = ApprovalBroker::new(false);
        let (tx, _rx) = mpsc::channel(64);
        let run = CancellationToken::new();
        broker.begin_run(tx, run.clone());
        let engine = PermissionEngine::new(
            PolicyCell::new(SessionPolicy::unchosen(), 0),
            Arc::new(BrokerApprover::new(broker.clone())),
        );
        let d = engine
            .check(&request("Edit", PermissionLevel::Write, "e"))
            .await;
        assert!(matches!(d, PermissionDecision::Deny(r) if r.contains("non-interactive")));
        assert_eq!(broker.unsatisfied().len(), 1);
        assert!(run.is_cancelled(), "the run stops with approval required");
        // Reads still run without asking.
        let r = engine
            .check(&request("Read", PermissionLevel::ReadOnly, "r"))
            .await;
        assert!(matches!(r, PermissionDecision::Allow));
    }
}
