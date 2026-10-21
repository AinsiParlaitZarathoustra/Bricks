//! Conversation views built from the engine's stored sessions: history
//! blocks, search snippets and session items. Pure functions: the engine
//! (`crate::engine`) does the reading.

use crate::contract::{HistoryBlock, HistoryMessage, HistoryRole, SessionItem};
use bricks_agent::control::SessionSummary;
use bricks_types::{ContentBlock, Message, MessageContent, Role, ToolResultContent};
use std::path::Path;

/// How many characters of context a snippet keeps on each side of a match.
const SNIPPET_CONTEXT: usize = 60;

/// One stored message as the interface shows it.
pub fn history_message(m: &Message) -> HistoryMessage {
    let role = match m.role {
        Role::User => HistoryRole::User,
        Role::Assistant => HistoryRole::Assistant,
        Role::System => HistoryRole::System,
    };
    let blocks = match &m.content {
        MessageContent::Text(text) => vec![HistoryBlock::Text { text: text.clone() }],
        MessageContent::Blocks(blocks) => blocks.iter().filter_map(history_block).collect(),
    };
    HistoryMessage {
        role,
        blocks,
        attachments: Vec::new(),
    }
}

fn history_block(b: &ContentBlock) -> Option<HistoryBlock> {
    Some(match b {
        ContentBlock::Text { text } => HistoryBlock::Text { text: text.clone() },
        ContentBlock::Thinking { thinking, .. } => HistoryBlock::Thinking {
            text: thinking.clone(),
        },
        ContentBlock::ToolUse { id, name, input } => HistoryBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => HistoryBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: tool_result_text(content),
            is_error: is_error.unwrap_or(false),
        },
        ContentBlock::Image { source } => HistoryBlock::Image {
            path: None,
            media_type: source.media_type.clone(),
        },
        _ => return None,
    })
}

fn tool_result_text(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text(t) => t.clone(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// The text a person wrote or the model answered, for search. Tool inputs
/// and outputs are left out: they are not what someone remembers saying.
pub fn message_text(m: &Message) -> String {
    match &m.content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Case-insensitive, character-wise search. Both sides are mapped to one
/// lowercase character per character, so offsets stay valid for any text.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Whether `haystack` contains `needle` (case-insensitive).
pub fn contains_folded(haystack: &str, needle: &str) -> bool {
    let need: Vec<char> = needle.chars().map(fold).collect();
    if need.is_empty() {
        return false;
    }
    let hay: Vec<char> = haystack.chars().map(fold).collect();
    hay.windows(need.len()).any(|w| w == need.as_slice())
}

/// An excerpt of `text` around the first match of `needle`, with
/// whitespace collapsed. `None` when there is no match.
pub fn snippet(text: &str, needle: &str) -> Option<String> {
    let need: Vec<char> = needle.chars().map(fold).collect();
    if need.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().map(|c| fold(*c)).collect();
    let at = folded.windows(need.len()).position(|w| w == need.as_slice())?;
    let start = at.saturating_sub(SNIPPET_CONTEXT);
    let end = (at + need.len() + SNIPPET_CONTEXT).min(chars.len());
    let body: String = chars[start..end].iter().collect();
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!(
        "{}{body}{}",
        if start > 0 { "…" } else { "" },
        if end < chars.len() { "…" } else { "" }
    ))
}

/// A stored session as an item of the interface.
pub fn item_from_summary(
    s: &SessionSummary,
    project_id: Option<String>,
    open: bool,
    running: bool,
) -> SessionItem {
    SessionItem {
        id: s.id.clone(),
        project_id,
        title: s.title.clone(),
        created_at: s.created_at,
        updated_at: s.updated_at,
        message_count: u32::try_from(s.message_count).unwrap_or(u32::MAX),
        model: s.model.clone(),
        reasoning: s.reasoning.clone(),
        working_dir: s.working_dir.as_deref().map(path_text),
        open,
        running,
    }
}

pub fn path_text(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bricks_types::ImageSource;

    #[test]
    fn snippets_keep_the_match_and_its_context_on_character_boundaries() {
        let text = format!("{}ÉTÉ dernier{}", "é".repeat(80), "x".repeat(80));
        let s = snippet(&text, "été").expect("match, case-insensitive");
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.contains("ÉTÉ dernier"));
        assert!(snippet("rien ici", "absent").is_none());
        assert_eq!(snippet("a\n\n  b  cible", "cible").unwrap(), "a b cible");
        assert!(snippet("x", "").is_none());
    }

    #[test]
    fn folded_containment_is_case_insensitive() {
        assert!(contains_folded("Atlas Plan", "atlas"));
        assert!(!contains_folded("Atlas", "plan"));
        assert!(!contains_folded("abc", ""));
    }

    #[test]
    fn tool_results_and_images_become_history_blocks() {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text { text: "voici".into() },
                ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: ToolResultContent::Text("ok".into()),
                    is_error: None,
                },
                ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".into(),
                        media_type: Some("image/png".into()),
                        data: Some("AAAA".into()),
                        url: None,
                        file_id: None,
                    },
                },
            ]),
            id: None,
            metadata: None,
        };
        let h = history_message(&msg);
        assert!(matches!(h.role, HistoryRole::User));
        assert_eq!(h.blocks.len(), 3);
        assert!(matches!(&h.blocks[1], HistoryBlock::ToolResult { content, is_error: false, .. } if content == "ok"));
        assert_eq!(message_text(&msg), "voici");
    }
}
