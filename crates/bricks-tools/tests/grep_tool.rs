//! Grep tool, driven from outside the crate through `Tool::execute`.
//!
//! Covers scoping (`path`, `glob`), the match limit and its notice, binary
//! files, literal matching, and the ignore rules, all through the tool's
//! own output format (`file:line:content`).

use bricks_tools::grep_tool::{DEFAULT_LIMIT, GrepTool};
use bricks_tools::permissions::AllowAll;
use bricks_tools::{CostTracker, Extensions, Tool, ToolContext, ToolResult};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::sync::Arc;

fn ctx_in(dir: &Path) -> ToolContext {
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: "grep-tool-test".into(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

async fn grep(dir: &Path, input: Value) -> ToolResult {
    GrepTool.execute(input, &ctx_in(dir)).await
}

/// Splits one `file:line:content` line. The first `:digits:` marks the
/// line number, since the file path itself contains no colon.
fn parse_hit(line: &str) -> Option<(String, usize, String)> {
    for (i, _) in line.match_indices(':') {
        let rest = &line[i + 1..];
        if let Some(j) = rest.find(':') {
            let num = &rest[..j];
            if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
                let file = Path::new(&line[..i])
                    .file_name()?
                    .to_string_lossy()
                    .into_owned();
                return Some((file, num.parse().ok()?, rest[j + 1..].to_string()));
            }
        }
    }
    None
}

/// The match lines of a result (notice and blank lines are skipped).
fn hits(content: &str) -> Vec<(String, usize, String)> {
    content.lines().filter_map(parse_hit).collect()
}

/// Sorted basenames of the files that matched.
fn names(content: &str) -> Vec<String> {
    let mut v: Vec<String> = hits(content).into_iter().map(|h| h.0).collect();
    v.sort();
    v
}

fn has_notice(content: &str) -> bool {
    content.contains("more matches exist")
}

#[tokio::test]
async fn glob_and_relative_directory_scope_the_search() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("crates/a/src/deep")).unwrap();
    fs::write(root.join("crates/a/src/deep/lib.rs"), "NEEDLE\n").unwrap();
    fs::write(root.join("crates/b.txt"), "NEEDLE\n").unwrap();
    fs::write(root.join("outside.rs"), "NEEDLE\n").unwrap();

    let res = grep(
        root,
        json!({ "pattern": "NEEDLE", "path": "crates", "glob": "*.rs" }),
    )
    .await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(names(&res.content), ["lib.rs"], "got: {}", res.content);
}

#[tokio::test]
async fn nonexistent_relative_path_is_an_error_not_no_matches() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.txt"), "NEEDLE\n").unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "NEEDLE", "path": "nope" })).await;

    assert!(res.is_error, "got: {}", res.content);
    assert!(
        res.content.contains("does not exist"),
        "got: {}",
        res.content
    );
    assert_ne!(res.content, "No matches found.");
}

#[tokio::test]
async fn absolute_directory_path_works() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("crates/a")).unwrap();
    fs::write(root.join("crates/a/lib.rs"), "NEEDLE\n").unwrap();
    fs::write(root.join("skipped.rs"), "NEEDLE\n").unwrap();

    let abs = root.join("crates");
    let res = grep(
        root,
        json!({ "pattern": "NEEDLE", "path": abs.to_str().unwrap() }),
    )
    .await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(names(&res.content), ["lib.rs"], "got: {}", res.content);
}

#[tokio::test]
async fn single_file_path_searches_only_that_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("crates")).unwrap();
    fs::write(root.join("crates/b.txt"), "NEEDLE one\nNEEDLE two\n").unwrap();
    fs::write(root.join("crates/other.txt"), "NEEDLE\n").unwrap();

    let relative = grep(root, json!({ "pattern": "NEEDLE", "path": "crates/b.txt" })).await;
    assert!(!relative.is_error, "got: {}", relative.content);
    assert_eq!(
        hits(&relative.content)
            .iter()
            .map(|h| (h.0.as_str(), h.1))
            .collect::<Vec<_>>(),
        [("b.txt", 1), ("b.txt", 2)],
        "got: {}",
        relative.content
    );

    let abs = root.join("crates/b.txt");
    let absolute = grep(
        root,
        json!({ "pattern": "NEEDLE", "path": abs.to_str().unwrap() }),
    )
    .await;
    assert!(!absolute.is_error, "got: {}", absolute.content);
    assert_eq!(names(&absolute.content), ["b.txt", "b.txt"]);
    assert!(!absolute.content.contains("other.txt"));
}

#[tokio::test]
async fn default_limit_is_100_and_the_notice_says_so() {
    assert_eq!(DEFAULT_LIMIT, 100);
    let tmp = tempfile::tempdir().unwrap();
    let body: String = (1..=150).map(|i| format!("NEEDLE {i}\n")).collect();
    fs::write(tmp.path().join("many.txt"), body).unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "NEEDLE" })).await;

    assert!(!res.is_error, "got: {}", res.content);
    let found = hits(&res.content);
    assert_eq!(found.len(), 100, "got: {}", res.content);
    assert_eq!(found[0].1, 1);
    assert_eq!(found[99].1, 100);
    assert!(has_notice(&res.content));
    assert!(
        res.content
            .contains("[Showing the first 100 matches; more matches exist."),
        "got tail: {}",
        &res.content[res.content.len().saturating_sub(200)..]
    );
}

#[tokio::test]
async fn custom_limit_caps_the_matches() {
    let tmp = tempfile::tempdir().unwrap();
    let body: String = (1..=150).map(|i| format!("NEEDLE {i}\n")).collect();
    fs::write(tmp.path().join("many.txt"), body).unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "NEEDLE", "limit": 5 })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(hits(&res.content).len(), 5, "got: {}", res.content);
    assert!(
        res.content
            .contains("[Showing the first 5 matches; more matches exist."),
        "got: {}",
        res.content
    );
}

#[tokio::test]
async fn exactly_the_limit_of_matches_has_no_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let body: String = (1..=100).map(|i| format!("NEEDLE {i}\n")).collect();
    fs::write(tmp.path().join("hundred.txt"), body).unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "NEEDLE" })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(hits(&res.content).len(), 100);
    assert!(!has_notice(&res.content), "got: {}", res.content);
}

#[tokio::test]
async fn a_complete_small_result_has_no_notice() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("few.txt"), "NEEDLE a\nNEEDLE b\nNEEDLE c\n").unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "NEEDLE" })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(hits(&res.content).len(), 3);
    assert!(
        !res.content.contains("more matches exist"),
        "got: {}",
        res.content
    );
}

#[tokio::test]
async fn limit_zero_lists_no_match() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.txt"), "NEEDLE\n").unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "NEEDLE", "limit": 0 })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(res.content, "Limit is 0: no match listed.");
}

#[tokio::test]
async fn limit_zero_still_reports_a_missing_path() {
    let tmp = tempfile::tempdir().unwrap();

    let res = grep(
        tmp.path(),
        json!({ "pattern": "NEEDLE", "path": "nope", "limit": 0 }),
    )
    .await;

    assert!(res.is_error, "got: {}", res.content);
    assert!(
        res.content.contains("does not exist"),
        "got: {}",
        res.content
    );
}

#[tokio::test]
async fn limit_zero_still_reports_an_invalid_pattern() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.txt"), "x\n").unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "(unclosed", "limit": 0 })).await;

    assert!(res.is_error, "got: {}", res.content);
    assert!(
        res.content.contains("Search failed"),
        "got: {}",
        res.content
    );
}

#[tokio::test]
async fn matches_spread_over_many_files_are_capped_by_the_limit() {
    let tmp = tempfile::tempdir().unwrap();
    for f in 0..30 {
        let body: String = (1..=10).map(|i| format!("NEEDLE {f} {i}\n")).collect();
        fs::write(tmp.path().join(format!("f{f:02}.txt")), body).unwrap();
    }

    let res = grep(tmp.path(), json!({ "pattern": "NEEDLE", "limit": 20 })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(hits(&res.content).len(), 20, "got: {}", res.content);
    assert!(
        res.content
            .contains("[Showing the first 20 matches; more matches exist."),
        "got: {}",
        res.content
    );
}

#[tokio::test]
async fn binary_files_contribute_no_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // NUL before the match.
    fs::write(root.join("bin1.dat"), b"head\0 NEEDLE tail\n").unwrap();
    // NUL after the match.
    fs::write(root.join("bin2.dat"), b"NEEDLE first\n\0rest").unwrap();
    fs::write(root.join("ok.txt"), "NEEDLE in text\n").unwrap();

    let res = grep(root, json!({ "pattern": "NEEDLE", "limit": 100000 })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(names(&res.content), ["ok.txt"], "got: {}", res.content);
    assert!(!res.content.contains("bin1"), "got: {}", res.content);
    assert!(!res.content.contains("bin2"), "got: {}", res.content);
}

#[tokio::test]
async fn a_large_file_ending_in_nul_contributes_no_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // About 200 KB of matching text lines, then a NUL byte at the very end.
    let mut big: Vec<u8> = Vec::new();
    let mut i = 0;
    while big.len() < 200_000 {
        big.extend_from_slice(format!("NEEDLE line {i} {}\n", "x".repeat(40)).as_bytes());
        i += 1;
    }
    big.push(0);
    fs::write(root.join("big.bin"), &big).unwrap();
    fs::write(root.join("ok.txt"), "NEEDLE in text\n").unwrap();

    let res = grep(root, json!({ "pattern": "NEEDLE", "limit": 100000 })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(names(&res.content), ["ok.txt"], "got: {}", res.content);
    assert!(!res.content.contains("big.bin"));
}

#[tokio::test]
async fn literal_matches_exact_text_only() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.txt"), "config.json\nconfigXjson\n").unwrap();

    let literal = grep(
        tmp.path(),
        json!({ "pattern": "config.json", "literal": true }),
    )
    .await;
    assert!(!literal.is_error, "got: {}", literal.content);
    let found = hits(&literal.content);
    assert_eq!(found.len(), 1, "got: {}", literal.content);
    assert_eq!(found[0].2, "config.json");

    let regex = grep(tmp.path(), json!({ "pattern": "config.json" })).await;
    assert_eq!(hits(&regex.content).len(), 2, "got: {}", regex.content);
}

#[tokio::test]
async fn regex_syntax_still_works_without_literal() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.txt"), "config.json\nconfigXjson\n").unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "conf[a-z]+\\.json" })).await;

    assert!(!res.is_error, "got: {}", res.content);
    let found = hits(&res.content);
    assert_eq!(found.len(), 1, "got: {}", res.content);
    assert_eq!(found[0].2, "config.json");
}

#[tokio::test]
async fn literal_with_regex_metacharacters_does_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.txt"), "call a(b here\nplain line\n").unwrap();

    let res = grep(tmp.path(), json!({ "pattern": "a(b", "literal": true })).await;

    assert!(!res.is_error, "got: {}", res.content);
    let found = hits(&res.content);
    assert_eq!(found.len(), 1, "got: {}", res.content);
    assert_eq!(found[0].2, "call a(b here");
}

#[tokio::test]
async fn gitignored_directory_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join(".gitignore"), "build/\n").unwrap();
    fs::create_dir_all(root.join("build")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("build/gen.rs"), "NEEDLE\n").unwrap();
    fs::write(root.join("src/main.rs"), "NEEDLE\n").unwrap();

    let res = grep(root, json!({ "pattern": "NEEDLE" })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(names(&res.content), ["main.rs"], "got: {}", res.content);
}

#[tokio::test]
async fn dot_ignore_file_excludes_matching_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join(".ignore"), "gen.rs\n").unwrap();
    fs::write(root.join("gen.rs"), "NEEDLE\n").unwrap();
    fs::write(root.join("keep.rs"), "NEEDLE\n").unwrap();

    let res = grep(root, json!({ "pattern": "NEEDLE" })).await;

    assert!(!res.is_error, "got: {}", res.content);
    assert_eq!(names(&res.content), ["keep.rs"], "got: {}", res.content);
}

#[tokio::test]
async fn hidden_entries_are_skipped_unless_requested() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".hidden")).unwrap();
    fs::write(root.join(".hidden/x.txt"), "NEEDLE\n").unwrap();
    fs::write(root.join(".dotfile"), "NEEDLE\n").unwrap();
    fs::write(root.join("visible.txt"), "NEEDLE\n").unwrap();

    let default = grep(root, json!({ "pattern": "NEEDLE" })).await;
    assert!(!default.is_error, "got: {}", default.content);
    assert_eq!(
        names(&default.content),
        ["visible.txt"],
        "got: {}",
        default.content
    );

    let with_hidden = grep(root, json!({ "pattern": "NEEDLE", "hidden": true })).await;
    assert!(!with_hidden.is_error, "got: {}", with_hidden.content);
    assert_eq!(
        names(&with_hidden.content),
        [".dotfile", "visible.txt", "x.txt"],
        "got: {}",
        with_hidden.content
    );
}
