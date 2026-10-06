//! The three wire protocols Bricks speaks, as adapters.
//!
//! Each adapter turns a [`CompletionRequest`](crate::CompletionRequest) into the
//! protocol's request body, and turns the response (streamed or not) into
//! [`StreamEvent`](cersei_types::StreamEvent)s. Adapters know nothing about
//! providers, model names or prices: everything provider-specific arrives
//! through configuration.

pub mod anthropic_messages;
pub mod chat_completions;
pub mod responses;
pub mod sse;

use crate::config::ResolvedCompat;
use cersei_types::{
    CerseiError, ContentBlock, Message, MessageContent, Role, ToolResultContent,
    SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
};

/// Inputs every request builder needs besides the request itself.
pub(crate) struct BuildCtx<'a> {
    /// Exact model identifier sent to the server.
    pub api_model: &'a str,
    pub compat: &'a ResolvedCompat,
    /// Declared `max_output_tokens`; the request's limit is capped at it.
    pub max_output_tokens: u64,
    /// Whether the request is a streaming one.
    pub stream: bool,
}

impl BuildCtx<'_> {
    pub fn max_tokens(&self, requested: u32) -> u64 {
        (requested as u64).min(self.max_output_tokens)
    }
}

pub(crate) fn unsupported(msg: impl Into<String>) -> CerseiError {
    CerseiError::Unsupported(msg.into())
}

/// A `data:` URL for inline base64 media.
pub(crate) fn data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

/// The system prompt with the client-side cache boundary marker removed.
pub(crate) fn plain_system(system: &str) -> String {
    system.replace(SYSTEM_PROMPT_DYNAMIC_BOUNDARY, "")
}

/// Plain text of a tool result, or an error when it holds media this protocol
/// cannot carry inside a tool result (callers validated capabilities already;
/// this is the defence in depth that keeps media from being dropped).
pub(crate) fn tool_result_text(
    content: &ToolResultContent,
    protocol: &str,
) -> Result<String, CerseiError> {
    match content {
        ToolResultContent::Text(t) => Ok(t.clone()),
        ToolResultContent::Blocks(blocks) => {
            let mut out = String::new();
            for b in blocks {
                match b {
                    ContentBlock::Text { text } => out.push_str(text),
                    other => {
                        return Err(unsupported(format!(
                            "a tool result holds a {} block, which `{protocol}` cannot carry in a \
                             tool result",
                            other.modality().map(|m| m.as_str()).unwrap_or("non-text")
                        )))
                    }
                }
            }
            Ok(out)
        }
    }
}

/// Flatten a message's content into blocks (plain text becomes one block).
pub(crate) fn blocks_of(msg: &Message) -> Vec<ContentBlock> {
    match &msg.content {
        MessageContent::Text(t) => vec![ContentBlock::Text { text: t.clone() }],
        MessageContent::Blocks(b) => b.clone(),
    }
}

pub(crate) fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

/// File name for an inline document part: the title, else `document.<ext>`.
pub(crate) fn document_filename(title: Option<&str>, media_type: Option<&str>) -> String {
    if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
        return t.to_string();
    }
    let ext = match media_type {
        Some("application/pdf") => "pdf",
        Some("text/plain") => "txt",
        Some("text/markdown") => "md",
        Some("text/csv") => "csv",
        Some("text/html") => "html",
        Some("application/json") => "json",
        _ => "bin",
    };
    format!("document.{ext}")
}

/// Map a media type to the short audio `format` Chat Completions expects.
pub(crate) fn audio_format(media_type: &str) -> Option<&'static str> {
    match media_type.to_ascii_lowercase().as_str() {
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        _ => None,
    }
}

/// Parse tool-call arguments accumulated as text, as the accumulator will.
pub(crate) fn args_are_executable(args: &str) -> bool {
    args.trim().is_empty() || serde_json::from_str::<serde_json::Value>(args).is_ok()
}
