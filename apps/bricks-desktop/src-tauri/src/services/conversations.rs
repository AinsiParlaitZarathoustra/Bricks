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

/// The first image the tool call `tool_call_id` showed, as a data URL: inside
/// its result, or next to it after its label in the same user turn.
pub fn tool_image(history: &[Message], tool_call_id: &str) -> Option<String> {
    let label = format!("{}{tool_call_id}]", bricks_tools::TOOL_IMAGE_MARK);
    let data_url = |b: &ContentBlock| match b {
        ContentBlock::Image { source } => source.data.as_ref().map(|data| {
            let media = source.media_type.as_deref().unwrap_or("image/png");
            format!("data:{media};base64,{data}")
        }),
        _ => None,
    };
    for message in history.iter().rev() {
        let MessageContent::Blocks(blocks) = &message.content else {
            continue;
        };
        // Only the engine's turns of tool results carry a call's image: a
        // person's message that looks like a label is not one.
        let engine_turn = message.role == Role::User
            && blocks.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }));
        if !engine_turn {
            continue;
        }
        for (i, block) in blocks.iter().enumerate() {
            match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content: ToolResultContent::Blocks(inner),
                    ..
                } if tool_use_id == tool_call_id => {
                    if let Some(url) = inner.iter().find_map(data_url) {
                        return Some(url);
                    }
                }
                ContentBlock::Text { text } if text.starts_with(&label) => {
                    if let Some(url) = blocks.get(i + 1).and_then(data_url) {
                        return Some(url);
                    }
                }
                _ => {}
            }
        }
    }
    None
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

    fn call_result(id: &str, content: ToolResultContent) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content,
            is_error: None,
        }
    }

    fn label(id: &str) -> ContentBlock {
        ContentBlock::Text {
            text: format!("{}{id}] a.png, image/png, 3 bytes", bricks_tools::TOOL_IMAGE_MARK),
        }
    }

    /// A turn of tool results, as the engine writes it: its results first.
    fn turn(blocks: Vec<ContentBlock>) -> Message {
        let mut all = vec![ContentBlock::ToolResult {
            tool_use_id: "earlier".into(),
            content: ToolResultContent::Text("ok".into()),
            is_error: None,
        }];
        all.extend(blocks);
        Message::user_blocks(all)
    }

    #[test]
    fn an_image_inside_the_result_of_its_call_is_read_as_a_data_url() {
        let history = vec![turn(vec![call_result(
            "v1",
            ToolResultContent::Blocks(vec![
                ContentBlock::Text { text: "Image a.png".into() },
                ContentBlock::image_base64("image/png", "QUJD"),
            ]),
        )])];
        assert_eq!(
            tool_image(&history, "v1").as_deref(),
            Some("data:image/png;base64,QUJD")
        );
    }

    #[test]
    fn an_image_after_its_label_in_the_same_turn_is_read_as_a_data_url() {
        let history = vec![turn(vec![
            call_result("v1", ToolResultContent::Text("Image a.png".into())),
            label("v1"),
            ContentBlock::image_base64("image/jpeg", "WFla"),
        ])];
        assert_eq!(
            tool_image(&history, "v1").as_deref(),
            Some("data:image/jpeg;base64,WFla")
        );
    }

    #[test]
    fn a_persons_message_that_looks_like_a_label_shows_no_call_image() {
        let typed = Message::user_blocks(vec![
            label("v1"),
            ContentBlock::image_base64("image/png", "AAAA"),
        ]);
        assert_eq!(tool_image(&[typed], "v1"), None);
    }

    #[test]
    fn a_missing_media_type_is_read_as_png() {
        let history = vec![turn(vec![
            label("v1"),
            ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".into(),
                    media_type: None,
                    data: Some("QUJD".into()),
                    url: None,
                    file_id: None,
                },
            },
        ])];
        assert_eq!(
            tool_image(&history, "v1").as_deref(),
            Some("data:image/png;base64,QUJD")
        );
    }

    #[test]
    fn a_label_only_matches_its_own_call_id_exactly() {
        let history = vec![turn(vec![
            label("v10"),
            ContentBlock::image_base64("image/png", "AAAA"),
        ])];
        assert_eq!(tool_image(&history, "v1"), None, "v10 is not v1");
        assert_eq!(
            tool_image(&history, "v10").as_deref(),
            Some("data:image/png;base64,AAAA")
        );
    }

    #[test]
    fn an_image_that_is_not_right_after_its_label_is_not_taken() {
        let history = vec![turn(vec![
            label("v1"),
            ContentBlock::Text { text: "autre chose".into() },
            ContentBlock::image_base64("image/png", "AAAA"),
        ])];
        assert_eq!(tool_image(&history, "v1"), None);
    }

    #[test]
    fn none_when_the_call_showed_no_image_or_the_image_has_no_data() {
        let url_only = ContentBlock::image_url("https://example.com/x.png");
        let history = vec![
            turn(vec![call_result("v1", ToolResultContent::Text("Image".into()))]),
            turn(vec![
                call_result("v2", ToolResultContent::Blocks(vec![url_only.clone()])),
            ]),
            turn(vec![label("v3"), url_only]),
        ];
        assert_eq!(tool_image(&history, "v1"), None, "a text result shows nothing");
        assert_eq!(tool_image(&history, "v2"), None, "an image without data is not read");
        assert_eq!(tool_image(&history, "v3"), None, "a label next to a URL is not read");
        assert_eq!(tool_image(&history, "unknown"), None);
        assert_eq!(tool_image(&[], "v1"), None);
    }
}
