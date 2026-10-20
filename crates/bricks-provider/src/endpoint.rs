//! Endpoint assembly: base URL + protocol-relative path.
//!
//! A provider's `endpoint` is a *base*: it may already carry a version segment
//! (`https://api.example.com/v1`) or a gateway prefix
//! (`https://gw.example.com/team-a/openai`). The request path is then relative
//! to that base and chosen by the model's *effective* protocol
//! (`chat/completions`, `responses`, `messages`), or by an explicit override.
//!
//! Rules (all covered by tests):
//! * the base's path prefix is always kept;
//! * a version segment (`v1`, `v2`, …) present at the end of the base is not
//!   repeated if the relative path starts with the same one;
//! * the base's query string (for example `?api-version=…`) is preserved;
//! * a provider-level path override applies only to models speaking the
//!   provider's default protocol (so a protocol override never inherits another
//!   protocol's path);
//! * an override that is an absolute `http(s)://` URL is used as-is.

use crate::config::Protocol;
use url::Url;

/// Check a `path` override without a base.
pub fn validate_path_override(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("must not be empty".into());
    }
    if path.starts_with("http://") || path.starts_with("https://") {
        let url = Url::parse(path).map_err(|_| "not a valid URL".to_string())?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("must not embed credentials".into());
        }
        return Ok(());
    }
    if path.contains(['?', '#']) {
        return Err("must not contain `?` or `#`; put query parameters in `endpoint`".into());
    }
    if path.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("must not contain whitespace".into());
    }
    if path.split('/').any(|s| s == "..") {
        return Err("must not contain `..` segments".into());
    }
    Ok(())
}

fn is_version_segment(s: &str) -> bool {
    s.len() >= 2 && s.starts_with('v') && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Build the full request URL.
pub fn assemble(
    base: &str,
    protocol: Protocol,
    path_override: Option<&str>,
) -> Result<Url, String> {
    if let Some(p) = path_override {
        validate_path_override(p)?;
        if p.starts_with("http://") || p.starts_with("https://") {
            return Url::parse(p).map_err(|_| "not a valid URL".to_string());
        }
    }
    let mut url = Url::parse(base).map_err(|_| "not a valid URL".to_string())?;
    url.set_fragment(None);

    let relative = path_override.unwrap_or_else(|| protocol.default_path());
    let rel_segments: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();
    if rel_segments.is_empty() {
        return Err("the request path is empty".into());
    }

    let last_base = url
        .path_segments()
        .and_then(|mut s| s.rfind(|seg| !seg.is_empty()))
        .map(str::to_string);
    let skip_dup = match (&last_base, rel_segments.first()) {
        (Some(b), Some(r)) => is_version_segment(b) && b == r,
        _ => false,
    };

    {
        let mut segs = url
            .path_segments_mut()
            .map_err(|_| "URL cannot be a base for a request path".to_string())?;
        segs.pop_if_empty();
        for seg in rel_segments.iter().skip(usize::from(skip_dup)) {
            segs.push(seg);
        }
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(base: &str, p: Protocol, o: Option<&str>) -> String {
        assemble(base, p, o).unwrap().to_string()
    }

    #[test]
    fn default_paths_per_protocol() {
        assert_eq!(
            url(
                "https://api.example.com/v1",
                Protocol::ChatCompletions,
                None
            ),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            url("https://api.example.com/v1", Protocol::Responses, None),
            "https://api.example.com/v1/responses"
        );
        assert_eq!(
            url(
                "https://api.example.com/v1",
                Protocol::AnthropicMessages,
                None
            ),
            "https://api.example.com/v1/messages"
        );
    }

    #[test]
    fn trailing_slash_and_bare_host() {
        assert_eq!(
            url(
                "https://api.example.com/v1/",
                Protocol::ChatCompletions,
                None
            ),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            url("https://api.example.com", Protocol::Responses, None),
            "https://api.example.com/responses"
        );
        assert_eq!(
            url("https://api.example.com/", Protocol::Responses, None),
            "https://api.example.com/responses"
        );
    }

    #[test]
    fn version_is_not_doubled() {
        assert_eq!(
            url(
                "https://api.example.com/v1",
                Protocol::ChatCompletions,
                Some("/v1/chat/completions")
            ),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            url(
                "https://api.example.com/v2/",
                Protocol::AnthropicMessages,
                Some("v2/messages")
            ),
            "https://api.example.com/v2/messages"
        );
        // A different version segment is a real path, not a duplicate.
        assert_eq!(
            url(
                "https://api.example.com/v1",
                Protocol::ChatCompletions,
                Some("v2/chat")
            ),
            "https://api.example.com/v1/v2/chat"
        );
        // Non-version repeats are kept.
        assert_eq!(
            url(
                "https://gw.example.com/api",
                Protocol::ChatCompletions,
                Some("api/chat")
            ),
            "https://gw.example.com/api/api/chat"
        );
    }

    #[test]
    fn gateway_prefix_is_kept() {
        assert_eq!(
            url(
                "https://gw.example.com/team-a/openai/v1",
                Protocol::ChatCompletions,
                None
            ),
            "https://gw.example.com/team-a/openai/v1/chat/completions"
        );
        assert_eq!(
            url(
                "https://gw.example.com/team-a/anthropic",
                Protocol::AnthropicMessages,
                None
            ),
            "https://gw.example.com/team-a/anthropic/messages"
        );
    }

    #[test]
    fn query_string_is_preserved() {
        assert_eq!(
            url(
                "https://x.example.com/openai?api-version=2025-01-01",
                Protocol::ChatCompletions,
                None
            ),
            "https://x.example.com/openai/chat/completions?api-version=2025-01-01"
        );
    }

    #[test]
    fn absolute_override_replaces_everything() {
        assert_eq!(
            url(
                "https://api.example.com/v1",
                Protocol::Responses,
                Some("https://other.example.com/custom/path")
            ),
            "https://other.example.com/custom/path"
        );
    }

    #[test]
    fn overrides_are_validated() {
        for bad in ["", "  ", "a/../b", "chat?x=1", "chat#frag", "ch at"] {
            assert!(validate_path_override(bad).is_err(), "{bad:?}");
        }
        assert!(validate_path_override("openai/deployments/x/chat/completions").is_ok());
        assert!(assemble("https://api.example.com/v1", Protocol::Responses, Some("/")).is_err());
    }

    #[test]
    fn local_servers() {
        assert_eq!(
            url("http://localhost:11434/v1", Protocol::ChatCompletions, None),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            url("http://127.0.0.1:8080", Protocol::Responses, None),
            "http://127.0.0.1:8080/responses"
        );
    }
}
