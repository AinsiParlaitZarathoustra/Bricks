//! Secret scrubbing — the hard rule.
//!
//! Every string that enters a persisted artifact (FailureTrace, RegistryEntry,
//! serialized graph, jsonl line) MUST pass through [`redact`] first. Keys have
//! leaked from this repo before; we sanitize at the source, never the output.

use once_cell::sync::Lazy;
use regex::Regex;

const PLACEHOLDER: &str = "[REDACTED]";

/// Provider/key token shapes we never want persisted.
static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // OpenAI-style: sk-..., sk-proj-...
        Regex::new(r"sk-[A-Za-z0-9_-]{20,}").unwrap(),
        // Google API keys: AIza + 35 chars
        Regex::new(r"AIza[A-Za-z0-9_\-]{35}").unwrap(),
        // Anthropic: sk-ant-...
        Regex::new(r"sk-ant-[A-Za-z0-9_-]{20,}").unwrap(),
        // Bearer tokens in headers / urls
        Regex::new(r"(?i)bearer\s+[A-Za-z0-9._\-]{12,}").unwrap(),
        // Generic ?key= / &key= / api_key= URL params
        Regex::new(r"(?i)(api[_-]?key|access[_-]?token)=[A-Za-z0-9._\-]{8,}").unwrap(),
    ]
});

/// Values of process-env vars whose name looks secret — redacted verbatim if
/// they appear in text. Captured once at first use.
static ENV_SECRETS: Lazy<Vec<String>> = Lazy::new(|| {
    let name_re = Regex::new(r"(?i)(key|token|secret|password|passwd|auth|credential)").unwrap();
    std::env::vars()
        .filter(|(name, value)| name_re.is_match(name) && value.len() >= 8)
        .map(|(_, value)| value)
        .collect()
});

/// Redact secrets from a string. Idempotent and safe to call on any text.
pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    // 1) literal env-secret values (most specific)
    for secret in ENV_SECRETS.iter() {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), PLACEHOLDER);
        }
    }
    // 2) structural token patterns
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, PLACEHOLDER).into_owned();
    }
    out
}

/// Redact and truncate to `max` chars (for bounded excerpts in traces).
pub fn redact_excerpt(input: &str, max: usize) -> String {
    let r = redact(input);
    if r.chars().count() <= max {
        r
    } else {
        let truncated: String = r.chars().take(max).collect();
        format!("{truncated}… (truncated)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_openai_key() {
        let s = "error: auth failed for sk-proj-abcdefghijklmnopqrstuvwxyz0123";
        let out = redact(s);
        assert!(!out.contains("sk-proj-abcdefghij"), "{out}");
        assert!(out.contains(PLACEHOLDER));
    }

    #[test]
    fn redacts_google_key_and_bearer() {
        let s = "url ?key=AIzaSyA1234567890123456789012345678901234 Authorization: Bearer abc123def456ghi";
        let out = redact(s);
        assert!(!out.contains("AIzaSyA12345"), "{out}");
        assert!(!out.to_lowercase().contains("bearer abc123"), "{out}");
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let s = "the build failed at cargo test with exit code 101";
        assert_eq!(redact(s), s);
    }

    #[test]
    fn excerpt_truncates() {
        let s = "x".repeat(100);
        let out = redact_excerpt(&s, 10);
        assert!(out.starts_with(&"x".repeat(10)));
        assert!(out.contains("truncated"));
    }
}
