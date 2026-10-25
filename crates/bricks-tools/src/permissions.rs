//! The approver of tool calls: who answers when the permission engine says
//! a call must be asked.
//!
//! The decision itself belongs to the engine (`bricks_agent::permission`):
//! the session's policy says allow, ask or deny. A [`PermissionPolicy`] is
//! only consulted for the calls it says to ask — a person through a
//! frontend, a terminal, or an embedder's callback. An answer covers the one
//! call it was given for.

use super::{PermissionLevel, ToolCategory};
use async_trait::async_trait;

// ─── Approver trait ──────────────────────────────────────────────────────────

#[async_trait]
pub trait PermissionPolicy: Send + Sync {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision;

    /// Whether the agent must bind this approver to an interactive stream.
    fn requires_stream(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    /// The level of this call (for its input).
    pub permission_level: PermissionLevel,
    /// The tool's category (a read on the network is not a local read).
    pub category: ToolCategory,
    pub description: String,
    pub id: String,
    /// The file changes the call would make, when the tool can preview them.
    pub preview: Option<crate::preview::ChangePreview>,
    /// The sub-agent making the call (`None`: the top-level agent).
    pub agent_id: Option<String>,
    /// The revision of the policy that asked for this decision.
    pub policy_revision: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum PermissionDecision {
    Allow,
    Deny(String),
    /// Allowed by a person, for this call only.
    AllowOnce,
}

// ─── Built-in approvers ──────────────────────────────────────────────────────

/// Agrees to every call it is asked about (tests, trusted automation).
pub struct AllowAll;

#[async_trait]
impl PermissionPolicy for AllowAll {
    async fn check(&self, _request: &PermissionRequest) -> PermissionDecision {
        PermissionDecision::Allow
    }
}

/// Refuses every call it is asked about.
pub struct DenyAll;

#[async_trait]
impl PermissionPolicy for DenyAll {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        PermissionDecision::Deny(format!(
            "`{}` needs an approval and nobody approves here",
            request.tool_name
        ))
    }
}

/// An approver that defers to a callback.
pub struct InteractivePolicy {
    pub handler: Box<dyn Fn(&PermissionRequest) -> PermissionDecision + Send + Sync>,
}

impl InteractivePolicy {
    pub fn new(
        handler: impl Fn(&PermissionRequest) -> PermissionDecision + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Box::new(handler),
        }
    }

    /// An approver answered through the AgentStream.
    pub fn via_stream() -> StreamDeferredPolicy {
        StreamDeferredPolicy
    }
}

#[async_trait]
impl PermissionPolicy for InteractivePolicy {
    async fn check(&self, request: &PermissionRequest) -> PermissionDecision {
        (self.handler)(request)
    }
}

/// Approver bound to permission events by `Agent::run_stream`.
/// Without a stream it refuses rather than silently allowing.
pub struct StreamDeferredPolicy;

#[async_trait]
impl PermissionPolicy for StreamDeferredPolicy {
    async fn check(&self, _request: &PermissionRequest) -> PermissionDecision {
        PermissionDecision::Deny("Interactive permissions require Agent::run_stream".into())
    }

    fn requires_stream(&self) -> bool {
        true
    }
}
