//! Deterministic routing of `auto` queries. No model is consulted: the
//! rules below are the whole decision, and the response says which one
//! applied.

use crate::query::{CodeQuery, Intent, MatchMode};

/// `foo`, `Foo::bar`, `a.b`, `$x`: something a definition can be named.
pub fn is_identifier_like(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 200 {
        return false;
    }
    s.split("::").flat_map(|p| p.split('.')).all(|seg| {
        let mut chars = seg.chars();
        match chars.next() {
            Some(c) if c.is_alphabetic() || c == '_' || c == '$' => {
                chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            }
            _ => false,
        }
    })
}

/// The intent to run and why.
pub fn route(q: &CodeQuery) -> (Intent, String) {
    if q.intent != Intent::Auto {
        return (
            q.intent,
            format!("intent `{}` requested", q.intent.as_str()),
        );
    }
    if q.target.is_some() {
        return (
            Intent::Understand,
            "auto: a target was given → understand that place".into(),
        );
    }
    if q.mode == MatchMode::Literal && is_identifier_like(&q.text) {
        return (
            Intent::FindSymbol,
            "auto: the text is an identifier → look for its definitions first (text search if none)".into(),
        );
    }
    (
        Intent::TextSearch,
        "auto: free text or a pattern → text search".into(),
    )
}

/// The last segment of a qualified name, and its qualifier.
pub fn split_qualified(name: &str) -> (String, Option<String>) {
    let name = name.trim();
    let parts: Vec<&str> = name.split("::").flat_map(|p| p.split('.')).collect();
    match parts.as_slice() {
        [] => (String::new(), None),
        [one] => (one.to_string(), None),
        [.., q, last] => (last.to_string(), Some(q.to_string())),
    }
}

/// Escape a literal for the regex matcher.
pub fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Target;

    #[test]
    fn routing_is_explainable() {
        let r = |q: CodeQuery| route(&q).0;
        assert_eq!(r(CodeQuery::text("parse_config")), Intent::FindSymbol);
        assert_eq!(r(CodeQuery::text("Engine::query")), Intent::FindSymbol);
        assert_eq!(
            r(CodeQuery::text("thread 'main' panicked at")),
            Intent::TextSearch
        );
        let mut q = CodeQuery::text("fo+");
        q.mode = MatchMode::Regex;
        assert_eq!(r(q), Intent::TextSearch);
        assert_eq!(
            r(CodeQuery::text("").target(Target::File {
                path: "a.rs".into()
            })),
            Intent::Understand
        );
        assert_eq!(
            r(CodeQuery::text("x").intent(Intent::References)),
            Intent::References
        );
    }

    #[test]
    fn qualified_names() {
        assert_eq!(split_qualified("a::B::c"), ("c".into(), Some("B".into())));
        assert_eq!(split_qualified("x"), ("x".into(), None));
        assert_eq!(regex_escape("$a.b"), "\\$a\\.b");
    }
}
