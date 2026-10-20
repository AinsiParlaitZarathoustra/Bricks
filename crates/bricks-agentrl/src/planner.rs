//! Planner-side helpers: turning a [`FailureTrace`] into proposals and turning a
//! winning proposal into a [`RegistryEntry`]. The actual proposal *generation*
//! and *execution* are performed by an [`crate::orchestrator::AgentRlRunner`]
//! implementation; these are the pure, testable transforms around it.

use crate::graph::FailureTrace;
use crate::orchestrator::Proposal;
use crate::registry::{RegistryEntry, SolutionSpec};
use crate::scrub::redact;

/// Build the directed prompt context handed to a proposal sub-agent.
pub fn proposal_context(trace: &FailureTrace, angle: &str) -> String {
    format!(
        "{directionality}\n\nApproach this from the angle: {angle}.",
        directionality = trace.directionality(),
        angle = angle
    )
}

/// A few default proposal "angles" to diversify recovery attempts.
pub fn default_angles(n: usize) -> Vec<String> {
    let base = [
        "fix the root cause directly",
        "work around the failing step with an alternative tool",
        "add missing setup/preconditions before retrying",
        "decompose the task into smaller verified steps",
    ];
    base.iter().take(n).map(|s| s.to_string()).collect()
}

/// Construct proposals from a trace using the default angles.
pub fn proposals_from_trace(trace: &FailureTrace, n: usize) -> Vec<Proposal> {
    default_angles(n)
        .into_iter()
        .enumerate()
        .map(|(i, angle)| Proposal {
            id: format!("proposal-{i}"),
            goal: trace.problem_statement.clone(),
            context: proposal_context(trace, &angle),
        })
        .collect()
}

/// Slugify a task into a tool name like `solve_build_the_parser`.
pub fn tool_name_for(task: &str) -> String {
    let slug: String = task
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("_");
    format!("solve_{slug}")
}

/// Assemble a registry entry from a solved problem. All free text is scrubbed
/// (again) by `ToolRegistry::register`, but we scrub the description here too.
pub fn build_entry(
    tool_id: String,
    task: &str,
    trace: &FailureTrace,
    solution: SolutionSpec,
    created_at: i64,
) -> RegistryEntry {
    RegistryEntry {
        tool_id,
        name: tool_name_for(task),
        description: redact(&format!(
            "Auto-built tool that solves: {task}. Derived from a failed first attempt and a passing sandboxed proposal."
        )),
        problem_domain: redact(task),
        failure_trace: trace.clone(),
        solution,
        created_at,
        success_count: 0,
    }
}
