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

use super::settings::{Action, ApprovalMode, ApprovalOverride, ApprovalRules};
use crate::events::AgentEvent;
use async_trait::async_trait;
use bricks_tools::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};
use bricks_tools::preview::ChangePreview;
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

    /// Asks a person (or refuses, non-interactive). `honour_grants`: whether
    /// session grants apply in this mode. When they do not (AlwaysAsk), an
    /// "allow for this session" answer is only a one-time allow and is not
    /// recorded.
    async fn ask(
        &self,
        request: ApprovalRequest,
        honour_grants: bool,
    ) -> (Decision, DecidedBy, Option<String>) {
        let tool = request.tool.clone();
        if honour_grants && self.session_allowed.lock().contains(&tool) {
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
                    // Under AlwaysAsk a grant is not kept: "allow for this
                    // session" is a one-time allow there, so nothing applies
                    // once the mode is back to standard.
                    let decision = match decision {
                        Decision::AllowForSession if !honour_grants => Decision::Allow,
                        other => other,
                    };
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
/// to ask. The runtime override ([`ApprovalMode`]) is read at each decision.
pub struct ApprovalGate {
    rules: ApprovalRules,
    broker: Arc<ApprovalBroker>,
    mode: Arc<ApprovalOverride>,
}

impl ApprovalGate {
    /// A gate with no override: the project's rules, as before.
    pub fn new(rules: ApprovalRules, broker: Arc<ApprovalBroker>) -> Self {
        Self {
            rules,
            broker,
            mode: ApprovalOverride::new(ApprovalMode::Standard),
        }
    }

    /// Reads `mode` at each decision (shared by every gate of one engine
    /// configuration, so a change reaches open sessions and sub-agents).
    pub fn with_override(mut self, mode: Arc<ApprovalOverride>) -> Self {
        self.mode = mode;
        self
    }

    pub fn rules(&self) -> &ApprovalRules {
        &self.rules
    }

    /// The override in force now.
    pub fn mode(&self) -> ApprovalMode {
        self.mode.get()
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
impl PermissionPolicy for ApprovalGate {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        let effective = self.rules.effective(
            self.mode.get(),
            &request.tool_name,
            request.permission_level,
        );
        match effective.action {
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
                let (decision, by, reason) =
                    self.broker.ask(req, effective.honour_session_grants).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use bricks_tools::PermissionLevel;
    use std::time::Duration;

    fn request(tool: &str, level: PermissionLevel, id: &str) -> PermissionRequest {
        PermissionRequest {
            tool_name: tool.into(),
            tool_input: serde_json::json!({}),
            permission_level: level,
            description: format!("run {tool}"),
            id: id.into(),
            preview: None,
            agent_id: None,
        }
    }

    /// An interactive broker with a live run, and a gate reading `mode`.
    fn setup(
        rules: ApprovalRules,
        mode: Arc<ApprovalOverride>,
    ) -> (
        Arc<ApprovalBroker>,
        Arc<ApprovalGate>,
        mpsc::Receiver<AgentEvent>,
    ) {
        let broker = ApprovalBroker::new(true);
        let (tx, rx) = mpsc::channel(64);
        broker.begin_run(tx, CancellationToken::new());
        let gate = ApprovalGate::new(rules, broker.clone()).with_override(mode);
        (broker, Arc::new(gate), rx)
    }

    /// Waits until a request is pending, and returns its id.
    async fn wait_pending(broker: &ApprovalBroker) -> String {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(r) = broker.pending().first() {
                    return r.approval_id.clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a request should be pending")
    }

    fn spawn_check(
        gate: &Arc<ApprovalGate>,
        req: PermissionRequest,
    ) -> tokio::task::JoinHandle<PermissionDecision> {
        let gate = Arc::clone(gate);
        tokio::spawn(async move { gate.check(&req).await })
    }

    fn is_allowed(d: &PermissionDecision) -> bool {
        matches!(
            d,
            PermissionDecision::Allow
                | PermissionDecision::AllowOnce
                | PermissionDecision::AllowForSession
        )
    }

    #[tokio::test]
    async fn a_session_grant_is_honoured_in_standard_and_ignored_under_always_ask() {
        let mode = ApprovalOverride::new(ApprovalMode::Standard);
        let (broker, gate, _rx) = setup(ApprovalRules::default(), Arc::clone(&mode));

        // The person allows Write for the session.
        let first = spawn_check(&gate, request("Write", PermissionLevel::Write, "c1"));
        let id = wait_pending(&broker).await;
        broker
            .respond(&id, Decision::AllowForSession, None)
            .unwrap();
        assert!(matches!(
            first.await.unwrap(),
            PermissionDecision::AllowForSession
        ));

        // Standard: the grant answers the next call without asking.
        let second = spawn_check(&gate, request("Write", PermissionLevel::Write, "c2"));
        assert!(is_allowed(&second.await.unwrap()));
        assert!(broker.pending().is_empty(), "no question under standard");

        // AlwaysAsk: the same grant is ignored, the person is asked again.
        mode.set(ApprovalMode::AlwaysAsk);
        let third = spawn_check(&gate, request("Write", PermissionLevel::Write, "c3"));
        let id = wait_pending(&broker).await;
        broker
            .respond(&id, Decision::Deny, Some("no".into()))
            .unwrap();
        assert!(matches!(third.await.unwrap(), PermissionDecision::Deny(_)));
    }

    /// Under AlwaysAsk, "allow for this session" is a one-time allow: it is
    /// not recorded, so nothing applies once the mode is back to standard.
    #[tokio::test]
    async fn an_always_ask_session_grant_is_one_time_and_not_recorded() {
        let mode = ApprovalOverride::new(ApprovalMode::AlwaysAsk);
        let (broker, gate, mut rx) = setup(ApprovalRules::default(), Arc::clone(&mode));

        let first = spawn_check(&gate, request("Write", PermissionLevel::Write, "c1"));
        let id = wait_pending(&broker).await;
        broker
            .respond(&id, Decision::AllowForSession, None)
            .unwrap();
        assert!(
            matches!(first.await.unwrap(), PermissionDecision::AllowOnce),
            "a one-time allow, not a session grant"
        );
        assert!(broker.session_allowed().is_empty(), "nothing is recorded");
        let resolved = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|e| match e {
            AgentEvent::ApprovalResolved { decision, .. } => Some(decision),
            _ => None,
        });
        assert_eq!(
            resolved,
            Some(Decision::Allow),
            "announced as a one-time allow"
        );

        // Back to standard: the earlier answer does not apply, the write is asked.
        mode.set(ApprovalMode::Standard);
        let second = spawn_check(&gate, request("Write", PermissionLevel::Write, "c2"));
        let id = wait_pending(&broker).await;
        broker
            .respond(&id, Decision::Deny, Some("non".into()))
            .unwrap();
        assert!(matches!(second.await.unwrap(), PermissionDecision::Deny(_)));
    }

    #[tokio::test]
    async fn always_ask_asks_a_read_only_call_the_rules_allow() {
        let mode = ApprovalOverride::new(ApprovalMode::AlwaysAsk);
        let (broker, gate, _rx) = setup(ApprovalRules::default(), mode);
        let check = spawn_check(&gate, request("Read", PermissionLevel::ReadOnly, "r1"));
        let id = wait_pending(&broker).await;
        broker.respond(&id, Decision::Allow, None).unwrap();
        assert!(is_allowed(&check.await.unwrap()));
    }

    #[tokio::test]
    async fn no_confirmation_runs_without_asking_but_forbidden_and_denied_stay_refused() {
        let mut rules = ApprovalRules::default();
        rules.tools.insert("Bash".into(), Action::Deny);
        let mode = ApprovalOverride::new(ApprovalMode::NoConfirmation);
        let (broker, gate, _rx) = setup(rules, mode);

        let write = gate
            .check(&request("Write", PermissionLevel::Write, "w"))
            .await;
        assert!(is_allowed(&write));
        let dangerous = gate
            .check(&request("Shell", PermissionLevel::Dangerous, "d"))
            .await;
        assert!(is_allowed(&dangerous));
        let bash = gate
            .check(&request("Bash", PermissionLevel::Execute, "b"))
            .await;
        assert!(
            matches!(bash, PermissionDecision::Deny(_)),
            "explicit deny wins"
        );
        let forbidden = gate
            .check(&request("Locked", PermissionLevel::Forbidden, "f"))
            .await;
        assert!(
            matches!(forbidden, PermissionDecision::Deny(_)),
            "forbidden wins"
        );
        assert!(broker.pending().is_empty(), "nothing was asked");
    }

    #[tokio::test]
    async fn a_pending_request_is_not_resolved_by_a_change_of_mode() {
        let mode = ApprovalOverride::new(ApprovalMode::Standard);
        let (broker, gate, _rx) = setup(ApprovalRules::default(), Arc::clone(&mode));
        let mut check = spawn_check(&gate, request("Write", PermissionLevel::Write, "p"));
        let id = wait_pending(&broker).await;

        mode.set(ApprovalMode::NoConfirmation);
        let still_waiting = tokio::time::timeout(Duration::from_millis(100), &mut check).await;
        assert!(
            still_waiting.is_err(),
            "the change must not answer the request"
        );
        assert_eq!(broker.pending().len(), 1, "still waiting for the person");

        broker.respond(&id, Decision::Deny, None).unwrap();
        assert!(matches!(check.await.unwrap(), PermissionDecision::Deny(_)));
    }

    #[test]
    fn a_gate_without_override_keeps_the_project_rules() {
        let broker = ApprovalBroker::new(false);
        let gate = ApprovalGate::new(ApprovalRules::default(), broker);
        assert_eq!(gate.mode(), ApprovalMode::Standard);
    }
}
