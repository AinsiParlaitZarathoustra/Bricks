//! Source locations from compiler output, to feed `explain_location`.
//!
//! Cargo's JSON messages (`--message-format=json`) are structured and
//! preferred; the text parser is a heuristic and says so. Nothing here
//! runs a compiler.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationSource {
    /// From a structured diagnostic (cargo JSON).
    Structured,
    /// Matched as `path:line:col` in text: may be wrong.
    TextHeuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerLocation {
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 1-based, in characters.
    pub column: u32,
    pub level: Option<String>,
    pub message: Option<String>,
    pub source: LocationSource,
}

/// Primary spans of cargo's `compiler-message` lines; other lines ignored.
pub fn from_cargo_json(output: &str) -> Vec<CompilerLocation> {
    let mut out = Vec::new();
    for line in output.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let msg = if v["reason"] == "compiler-message" {
            &v["message"]
        } else if v["spans"].is_array() {
            &v // rustc --error-format=json
        } else {
            continue;
        };
        for span in msg["spans"].as_array().into_iter().flatten() {
            if span["is_primary"].as_bool() != Some(true) {
                continue;
            }
            let (Some(path), Some(l), Some(c)) = (
                span["file_name"].as_str(),
                span["line_start"].as_u64(),
                span["column_start"].as_u64(),
            ) else {
                continue;
            };
            out.push(CompilerLocation {
                path: path.to_string(),
                line: l as u32,
                column: c as u32,
                level: msg["level"].as_str().map(String::from),
                message: msg["message"].as_str().map(String::from),
                source: LocationSource::Structured,
            });
        }
    }
    out.dedup();
    out
}

/// `path:line:col` (or `path:line`) occurrences in free text.
pub fn from_text(output: &str) -> Vec<CompilerLocation> {
    let mut out = Vec::new();
    for raw in output.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',') {
        let token = raw.trim_matches(|c: char| c == '\'' || c == '"' || c == '`');
        let mut parts = token.rsplitn(3, ':');
        let (a, b, c) = (parts.next(), parts.next(), parts.next());
        let (path, line, col) = match (c, b, a) {
            (Some(p), Some(l), Some(c)) if l.parse::<u32>().is_ok() && c.parse::<u32>().is_ok() => {
                (p, l.parse().unwrap(), c.parse().unwrap())
            }
            (_, Some(p), Some(l)) if l.parse::<u32>().is_ok() && c.is_none() => {
                (p, l.parse().unwrap(), 1)
            }
            _ => continue,
        };
        let has_ext = std::path::Path::new(path).extension().is_some_and(|e| {
            e.len() <= 5
                && e.to_str()
                    .is_some_and(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
        });
        if !has_ext || line == 0 || path.contains("://") {
            continue;
        }
        let loc = CompilerLocation {
            path: path.to_string(),
            line,
            column: col.max(1),
            level: None,
            message: None,
            source: LocationSource::TextHeuristic,
        };
        if !out.contains(&loc) {
            out.push(loc);
        }
    }
    out
}

/// Structured locations when the output has any, else the text heuristic.
pub fn locations(output: &str) -> Vec<CompilerLocation> {
    let s = from_cargo_json(output);
    if s.is_empty() {
        from_text(output)
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_json_primary_spans() {
        let line = r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","spans":[{"file_name":"src/a.rs","line_start":3,"column_start":9,"is_primary":true},{"file_name":"src/b.rs","line_start":1,"column_start":1,"is_primary":false}]}}"#;
        let out = from_cargo_json(&format!("{line}\n{{\"reason\":\"build-finished\"}}\n"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "src/a.rs");
        assert_eq!((out[0].line, out[0].column), (3, 9));
        assert_eq!(out[0].source, LocationSource::Structured);
    }

    #[test]
    fn text_is_heuristic() {
        let out = from_text(
            "error[E0308]: mismatched types\n --> src/main.rs:12:5\nsee https://x.y:80/z\n",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "src/main.rs");
        assert_eq!((out[0].line, out[0].column), (12, 5));
        assert_eq!(out[0].source, LocationSource::TextHeuristic);
        assert_eq!(locations("at lib/x.py:7")[0].line, 7);
    }
}
