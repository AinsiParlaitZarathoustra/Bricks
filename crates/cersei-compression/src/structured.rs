//! Structured outputs, rendered back to the text the tool would have printed.
//!
//! When a tool emits machine-readable records (`cargo --message-format=json`,
//! `go test -json`), the records are more reliable than scraping text: each
//! diagnostic is delimited and carries its own rendering. They are converted
//! here, losslessly as far as the reader is concerned, into the plain lines
//! the ordinary rules and diagnostic detectors understand.

use crate::rules::Structured;
use serde_json::Value;

/// Result of a conversion: the lines and a note describing it.
pub struct Converted {
    pub lines: Vec<String>,
    pub note: String,
}

/// Detect a structured format in `lines` (or use the rule's hint) and convert.
pub fn convert(lines: &[String], hint: Option<Structured>) -> Option<Converted> {
    let json_lines = lines
        .iter()
        .filter(|l| l.trim_start().starts_with('{'))
        .take(64)
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .collect::<Vec<_>>();
    if json_lines.len() < 2 {
        return None;
    }
    let looks_cargo = json_lines
        .iter()
        .filter(|v| v.get("reason").is_some())
        .count()
        * 2
        >= json_lines.len();
    let looks_go = json_lines
        .iter()
        .filter(|v| {
            v.get("Action").is_some()
                && (v.get("Package").is_some() || v.get("ImportPath").is_some())
        })
        .count()
        * 2
        >= json_lines.len();
    match (hint, looks_cargo, looks_go) {
        (_, true, false) => Some(cargo(lines)),
        (_, false, true) => Some(go_test(lines)),
        (Some(Structured::CargoJson), true, true) => Some(cargo(lines)),
        (Some(Structured::GoTestJson), true, true) => Some(go_test(lines)),
        _ => None,
    }
}

/// `cargo --message-format=json`: each `compiler-message` contributes its
/// `rendered` text; artifacts and build-script records are counted, not shown.
fn cargo(lines: &[String]) -> Converted {
    let mut out = Vec::new();
    let (mut messages, mut artifacts, mut other) = (0usize, 0usize, 0usize);
    for line in lines {
        let parsed = line
            .trim_start()
            .starts_with('{')
            .then(|| serde_json::from_str::<Value>(line.trim()).ok())
            .flatten();
        let Some(v) = parsed else {
            // Test-harness output and anything else printed between records.
            out.push(line.clone());
            continue;
        };
        match v.get("reason").and_then(Value::as_str) {
            Some("compiler-message") => {
                messages += 1;
                let rendered = v
                    .pointer("/message/rendered")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                out.extend(rendered.trim_end_matches('\n').lines().map(str::to_string));
                out.push(String::new());
            }
            Some("compiler-artifact") | Some("build-script-executed") => artifacts += 1,
            Some("build-finished") => {
                let ok = v.get("success").and_then(Value::as_bool).unwrap_or(false);
                out.push(format!(
                    "build finished: {}",
                    if ok { "success" } else { "FAILED" }
                ));
            }
            _ => other += 1,
        }
    }
    Converted {
        lines: out,
        note: format!(
            "cargo JSON records rendered as text ({messages} diagnostics; {artifacts} artifact and {other} other records not shown)"
        ),
    }
}

/// `go test -json`: the `Output` fields, in order, are exactly the text
/// `go test -v` prints; build output records are included.
fn go_test(lines: &[String]) -> Converted {
    let mut text = String::new();
    let (mut records, mut passed, mut failed) = (0usize, 0usize, 0usize);
    for line in lines {
        let parsed = line
            .trim_start()
            .starts_with('{')
            .then(|| serde_json::from_str::<Value>(line.trim()).ok())
            .flatten();
        let Some(v) = parsed else {
            text.push_str(line);
            text.push('\n');
            continue;
        };
        records += 1;
        let is_test = v.get("Test").is_some();
        match v.get("Action").and_then(Value::as_str) {
            Some("output") | Some("build-output") => {
                if let Some(o) = v.get("Output").and_then(Value::as_str) {
                    text.push_str(o);
                }
            }
            Some("pass") if is_test => passed += 1,
            Some("fail") if is_test => failed += 1,
            _ => {}
        }
    }
    Converted {
        lines: text.lines().map(str::to_string).collect(),
        note: format!(
            "go test JSON records rendered as text ({records} records; {passed} tests passed, {failed} failed)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn cargo_messages_are_rendered() {
        let src = lines(concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"a"}}"#,
            "\n",
            r#"{"reason":"compiler-message","message":{"rendered":"error[E0308]: mismatched types\n  --> src/lib.rs:1:1\n\n","level":"error"}}"#,
            "\n",
            r#"{"reason":"build-finished","success":false}"#,
        ));
        let c = convert(&src, None).unwrap();
        assert_eq!(c.lines[0], "error[E0308]: mismatched types");
        assert!(c.lines.contains(&"build finished: FAILED".to_string()));
        assert!(c.note.contains("1 diagnostics"));
    }

    #[test]
    fn go_outputs_are_concatenated() {
        let src = lines(concat!(
            r#"{"Action":"run","Package":"p","Test":"TestA"}"#,
            "\n",
            r#"{"Action":"output","Package":"p","Test":"TestA","Output":"=== RUN   TestA\n"}"#,
            "\n",
            r#"{"Action":"output","Package":"p","Test":"TestA","Output":"--- FAIL: TestA (0.00s)\n"}"#,
            "\n",
            r#"{"Action":"fail","Package":"p","Test":"TestA"}"#,
        ));
        let c = convert(&src, None).unwrap();
        assert_eq!(c.lines, vec!["=== RUN   TestA", "--- FAIL: TestA (0.00s)"]);
        assert!(c.note.contains("1 failed"));
    }

    #[test]
    fn plain_text_is_not_structured() {
        assert!(convert(&lines("hello\n{not json\nworld"), None).is_none());
    }
}
