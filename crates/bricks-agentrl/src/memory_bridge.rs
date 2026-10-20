//! Bridges solved problems into the GeneralAgent's memory so future runs recall
//! them. The registry holds the *executable* tool; memory holds a human-readable
//! recall hint tying the problem statement to the agent's session.

use crate::scrub::redact;
use bricks_memory::Memory;
use bricks_types::{Message, Result};
use std::sync::Arc;

/// Record that `problem` was solved and a reusable tool `tool_name` (`tool_id`)
/// is registered. Stored as a short hint the agent will surface via memory search.
pub async fn record_solution(
    memory: &Arc<dyn Memory>,
    session_id: &str,
    problem: &str,
    tool_name: &str,
    tool_id: &str,
) -> Result<()> {
    let hint = format!(
        "Solved problem: \"{}\". A reusable tool `{}` (id {}) is registered — recall it via registry_search before re-solving similar problems.",
        redact(problem),
        redact(tool_name),
        tool_id
    );
    let msg = Message::assistant(&hint);
    memory.store(session_id, &[msg]).await
}
