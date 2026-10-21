//! A conversation's title, named by the model from its first prompt.
//!
//! The shortened first prompt is the title at once (`SessionMeta::title_from`);
//! this call replaces it when it answers in time. Any failure keeps the
//! shortened prompt: a title is never worth an error.

use bricks_provider::{
    CompletionRequest, Provider, ProviderOptions, REASONING_PROFILE_OPTION, StreamAccumulator,
};
use bricks_types::{Message, StreamEvent};
use std::time::Duration;

/// Absolute limit of the title call, from sending it to the end of its stream.
pub const TITLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Room for a short title, and for the reasoning a model may do first.
const TITLE_MAX_TOKENS: u32 = 1024;
/// Prompt characters sent: enough to know what the conversation is about.
const PROMPT_MAX_CHARS: usize = 2000;
const TITLE_MAX_CHARS: usize = 80;

const TITLE_SYSTEM_PROMPT: &str = "You name conversations. Reply with a title of 2 to 6 words \
saying what the user's message is about, written in the language of that message. Reply with \
the title only: no quotes, no final punctuation, no emoji, no explanation.";

/// Reasoning profiles light enough for a title, by preference.
const LIGHT_PROFILES: [&str; 5] = ["none", "off", "disabled", "minimal", "low"];

/// The lightest of a model's reasoning profiles, when it has a known light one.
pub fn light_profile<'a>(profiles: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let ids: Vec<&str> = profiles.into_iter().collect();
    LIGHT_PROFILES
        .iter()
        .find_map(|light| ids.iter().find(|id| id.eq_ignore_ascii_case(light)).copied())
}

/// The model's title for a conversation starting with `prompt`, or `None`
/// (failure, timeout, empty answer). `profile` is the reasoning profile of the
/// call (`None`: the provider's own).
pub async fn generate(provider: &dyn Provider, prompt: &str, profile: Option<&str>) -> Option<String> {
    let excerpt: String = prompt.trim().chars().take(PROMPT_MAX_CHARS).collect();
    if excerpt.is_empty() {
        return None;
    }
    let mut options = ProviderOptions::default();
    if let Some(p) = profile {
        options.set(REASONING_PROFILE_OPTION, p);
    }
    let request = CompletionRequest {
        model: String::new(),
        messages: vec![Message::user(format!(
            "<message>\n{excerpt}\n</message>\n\nTitle of this conversation:"
        ))],
        system: Some(TITLE_SYSTEM_PROMPT.into()),
        tools: Vec::new(),
        max_tokens: TITLE_MAX_TOKENS,
        temperature: None,
        stop_sequences: Vec::new(),
        options,
        output_modalities: Vec::new(),
    };
    let call = async {
        let mut rx = provider.complete(request).await.ok()?.into_receiver();
        let mut acc = StreamAccumulator::new();
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::Error { .. }) {
                return None;
            }
            acc.process_event(event);
        }
        acc.into_response().ok()
    };
    let response = tokio::time::timeout(TITLE_TIMEOUT, call).await.ok()??;
    clean(&response.message.get_all_text())
}

/// The first non-empty line of an answer, without the quotes, label or final
/// punctuation a model may add, shortened like a prompt title.
pub fn clean(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("Titre :"))
        .or_else(|| line.strip_prefix("Titre:"))
        .unwrap_or(line)
        .trim();
    // Quotes and final punctuation, in whatever order they close the line.
    let line = line
        .trim_matches(|c: char| {
            matches!(c, '"' | '\'' | '«' | '»' | '“' | '”' | '*' | '`') || c.is_whitespace()
        })
        .trim_end_matches(|c: char| {
            matches!(c, '.' | '!' | '?' | ':' | ';' | '…' | '"' | '\'' | '»' | '”' | '*' | '`')
                || c.is_whitespace()
        });
    if line.is_empty() {
        return None;
    }
    let mut title: String = line.chars().take(TITLE_MAX_CHARS).collect();
    if line.chars().count() > TITLE_MAX_CHARS {
        title.push('…');
    }
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::{clean, light_profile};

    #[test]
    fn the_lightest_known_profile_is_chosen() {
        assert_eq!(light_profile(["max", "low", "minimal"]), Some("minimal"));
        assert_eq!(light_profile(["high", "max"]), None);
    }

    #[test]
    fn answers_are_reduced_to_a_bare_title() {
        assert_eq!(
            clean("« Comparaison nutritionnelle de produits ».").as_deref(),
            Some("Comparaison nutritionnelle de produits")
        );
        assert_eq!(
            clean("\n  Title: \"Fix the login bug\"\nBecause…").as_deref(),
            Some("Fix the login bug")
        );
        assert_eq!(clean("**Recette du macérat**").as_deref(), Some("Recette du macérat"));
        assert_eq!(clean("  \n \"\" ").as_deref(), None);
        assert_eq!(clean(&"é".repeat(100)).unwrap().chars().count(), 81);
    }
}
