//! What the two delegation paths (`Agent` and `delegate`) share: input
//! validation, the tools a child may have, its depth and its link to the
//! parent's run.
//!
//! Invariants, for both paths:
//! * an empty task is refused before a provider is built or a request sent;
//! * a child never has more permissions than its parent: it gets the
//!   parent's policy, never `AllowAll` by default;
//! * a child never gets a tool its parent did not give it, and never a
//!   delegation tool (no recursion, whatever the factories return);
//! * a child is cancelled with the parent's run, and no delegation starts
//!   once that run is cancelled;
//! * a child that stops at a limit, is cancelled or fails is reported as
//!   such, with its partial text, never as a success.

use crate::{AgentOutput, Termination};
use cersei_tools::{Extensions, Tool, ToolResult};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Tool names that start a sub-agent. A child never receives them.
pub const DELEGATION_TOOLS: &[&str] = &["Agent", "delegate"];

/// The current run's cancellation, put in the tool context by the runner
/// at the start of each run: children are cancelled with it.
#[derive(Clone)]
pub struct RunCancellation(pub CancellationToken);

/// Delegation depth of an agent (the top-level agent is 0), in its tool
/// context. Children are built at depth + 1.
#[derive(Clone, Copy, Debug)]
pub struct DelegationDepth(pub u32);

/// True when `s` has no visible character: empty, Unicode white space, or
/// only invisible format characters (zero-width space and joiners, word
/// joiner, byte-order mark, Mongolian vowel separator).
pub fn is_blank(s: &str) -> bool {
    s.chars().all(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '\u{180E}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
            )
    })
}

/// The depth of the agent whose tool context this is.
pub fn depth_of(ext: &Extensions) -> u32 {
    ext.get::<DelegationDepth>().map(|d| d.0).unwrap_or(0)
}

/// The cancellation of the run whose tool context this is, if the runner
/// put one there.
pub fn run_token(ext: &Extensions) -> Option<CancellationToken> {
    ext.get::<RunCancellation>().map(|r| r.0.clone())
}

/// The extensions a child starts with: its own map (a child never writes
/// into its parent's), its depth.
pub fn child_extensions(parent_depth: u32) -> Extensions {
    let ext = Extensions::default();
    ext.insert(DelegationDepth(parent_depth + 1));
    ext
}

/// Remove the delegation tools and the `blocked` names from a child's tools.
pub fn child_tools(tools: Vec<Box<dyn Tool>>, blocked: &[String]) -> Vec<Box<dyn Tool>> {
    tools
        .into_iter()
        .filter(|t| !DELEGATION_TOOLS.contains(&t.name()) && !blocked.iter().any(|b| b == t.name()))
        .collect()
}

/// How a child ended, as the parent sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildStatus {
    Completed,
    /// Stopped at a limit (turns, output, no progress) before a final answer.
    Incomplete(Termination),
    Cancelled,
    Failed(String),
}

impl ChildStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ChildStatus::Completed => "completed",
            ChildStatus::Incomplete(_) => "incomplete",
            ChildStatus::Cancelled => "cancelled",
            ChildStatus::Failed(_) => "failed",
        }
    }
}

/// The status of a finished child run. A final answer without any text
/// is not a completed task.
pub fn status_of(result: &cersei_types::Result<AgentOutput>) -> ChildStatus {
    match result {
        Ok(out) if !out.termination.is_completed() => {
            ChildStatus::Incomplete(out.termination.clone())
        }
        Ok(out) if is_blank(out.text()) => {
            ChildStatus::Failed("the sub-agent finished without an answer".into())
        }
        Ok(_) => ChildStatus::Completed,
        Err(cersei_types::CerseiError::Cancelled) => ChildStatus::Cancelled,
        Err(e) => ChildStatus::Failed(e.to_string()),
    }
}

/// The parent's tool result for one child run.
pub fn tool_result(result: cersei_types::Result<AgentOutput>, partial: String) -> ToolResult {
    let status = status_of(&result);
    let (text, mut meta) = match &result {
        Ok(out) => (
            out.text().to_string(),
            json!({
                "turns": out.turns,
                "tool_calls": out.tool_calls.len(),
                "input_tokens": out.usage.input_tokens,
                "output_tokens": out.usage.output_tokens,
                "termination": out.termination,
            }),
        ),
        Err(_) => (partial, json!({})),
    };
    meta["status"] = json!(status.label());
    match status {
        ChildStatus::Completed => ToolResult::success(text).with_metadata(meta),
        ChildStatus::Incomplete(t) => ToolResult::error(with_partial(
            &format!("Sub-agent incomplete: {}.", t.describe()),
            &text,
        ))
        .with_metadata(meta),
        ChildStatus::Cancelled => ToolResult::error(with_partial(
            "Sub-agent cancelled with the parent's run.",
            &text,
        ))
        .with_metadata(meta),
        ChildStatus::Failed(e) => {
            ToolResult::error(with_partial(&format!("Sub-agent failed: {e}"), &text))
                .with_metadata(meta)
        }
    }
}

/// The text of the last assistant message of `agent` (what a run that was
/// cancelled or failed had already said).
pub fn partial_text(agent: &crate::Agent) -> String {
    agent
        .messages()
        .iter()
        .rev()
        .find(|m| m.role == cersei_types::Role::Assistant)
        .and_then(|m| m.get_text().map(str::to_string))
        .unwrap_or_default()
}

fn with_partial(head: &str, partial: &str) -> String {
    if is_blank(partial) {
        format!("{head} No partial answer.")
    } else {
        format!("{head} Partial answer:\n{partial}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_covers_unicode_spaces_and_invisible_characters() {
        for s in [
            "",
            "   ",
            "\t\n\r",
            "\u{00A0}\u{2003}\u{3000}",
            "\u{200B}\u{FEFF}\u{2060}",
            " \u{200D} ",
        ] {
            assert!(is_blank(s), "{s:?}");
        }
        for s in ["a", " x ", "\u{200B}é", "0"] {
            assert!(!is_blank(s), "{s:?}");
        }
    }

    #[test]
    fn delegation_tools_are_never_given_to_a_child() {
        let tools = cersei_tools::filesystem();
        let n = tools.len();
        let kept = child_tools(tools, &["Read".to_string()]);
        assert_eq!(kept.len(), n - 1);
        assert!(kept.iter().all(|t| t.name() != "Read"));
    }
}
