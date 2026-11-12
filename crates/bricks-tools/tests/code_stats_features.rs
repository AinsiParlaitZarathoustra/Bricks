//! CodeStats features: the share of each row (`percent`), line bounds
//! (`min_lines` / `max_lines`), `.gitattributes` exclusions, and the Markdown
//! answer (`format`), with git's rules for `.gitattributes`.

use bricks_tools::code_stats::CodeStatsTool;
use bricks_tools::permissions::AllowAll;
use bricks_tools::{CostTracker, Extensions, Tool, ToolContext, ToolResult};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: format!("code-stats-features-{}", uuid::Uuid::new_v4()),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

async fn call(ctx: &ToolContext, input: Value) -> ToolResult {
    CodeStatsTool.execute(input, ctx).await
}

async fn stats(ctx: &ToolContext, input: Value) -> Value {
    let r = call(ctx, input).await;
    assert!(!r.is_error, "{}", r.content);
    serde_json::from_str(&r.content).expect("compact JSON")
}

fn write(dir: &Path, rel: &str, text: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// The result row whose `key` is `name` (`language` or `path`).
fn row<'a>(v: &'a Value, key: &str, name: &str) -> &'a Value {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r[key] == name)
        .unwrap_or_else(|| panic!("{name} missing: {v}"))
}

fn paths(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

fn percent(r: &Value) -> f64 {
    r["percent"].as_f64().unwrap_or_else(|| panic!("no percent: {r}"))
}

/// Rust (3 code, 1 comment, 1 blank) and Python (1 code): 4 code lines.
const RUST: &str = "// comment\nfn main() {\n\n    println!(\"hi\");\n}\n";
const PY: &str = "x = 1\n";

// ─── Percent of the code counted ────────────────────────────────────────────

#[tokio::test]
async fn percent_is_each_rows_share_of_the_code_counted() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "main.rs", RUST);
    write(p, "tool.py", PY);
    let c = ctx(p);

    let v = stats(&c, json!({})).await;
    assert_eq!(percent(row(&v, "language", "Rust")), 75.0, "{v}");
    assert_eq!(percent(row(&v, "language", "Python")), 25.0, "{v}");

    let files = stats(&c, json!({"detail": "files"})).await;
    assert_eq!(percent(row(&files, "path", "main.rs")), 75.0, "{files}");
    assert_eq!(percent(row(&files, "path", "tool.py")), 25.0, "{files}");

    // The share is of all the code, not of the rows shown.
    let one = stats(&c, json!({"top_n": 1})).await;
    assert_eq!(one["results"].as_array().unwrap().len(), 1);
    assert_eq!(percent(&one["results"][0]), 75.0, "{one}");

    // A language filter: that language is the whole analysis.
    let only = stats(&c, json!({"language": "python"})).await;
    assert_eq!(percent(&only["results"][0]), 100.0, "{only}");
}

#[tokio::test]
async fn percent_is_zero_when_no_code_is_counted() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "only_comments.rs", "// one\n// two\n");
    write(p, "blanks.py", "\n\n");
    let c = ctx(p);
    let v = stats(&c, json!({})).await;
    assert_eq!(v["totals"]["code"], 0, "{v}");
    for r in v["results"].as_array().unwrap() {
        assert_eq!(percent(r), 0.0, "{r}");
    }
    let files = stats(&c, json!({"detail": "files"})).await;
    for r in files["results"].as_array().unwrap() {
        assert_eq!(percent(r), 0.0, "{r}");
    }
    let empty = tempfile::tempdir().unwrap();
    let v = stats(&ctx(empty.path()), json!({})).await;
    assert_eq!(v["totals"]["code"], 0);
    assert_eq!(v["results"], json!([]));
}

#[tokio::test]
async fn percent_is_taken_after_the_line_bounds() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "main.rs", RUST); // 5 lines
    write(p, "tool.py", PY); // 1 line, left out below
    let v = stats(&ctx(p), json!({"min_lines": 2})).await;
    assert_eq!(v["totals"]["code"], 3, "{v}");
    assert_eq!(percent(row(&v, "language", "Rust")), 100.0, "{v}");
    assert!(
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["language"] != "Python"),
        "{v}"
    );
}

// ─── Line bounds ────────────────────────────────────────────────────────────

/// Three one-code-line files of 2, 5 and 10 lines.
fn sized_tree(p: &Path) {
    write(p, "a.rs", "fn a() {}\n".repeat(2));
    write(p, "b.rs", "fn b() {}\n".repeat(5));
    write(p, "c.rs", "fn c() {}\n".repeat(10));
}

#[tokio::test]
async fn line_bounds_leave_files_out_and_count_them() {
    let d = tempfile::tempdir().unwrap();
    sized_tree(d.path());
    let c = ctx(d.path());

    // Both bounds: only b.rs (5 lines) remains; a and c are outside.
    let v = stats(&c, json!({"min_lines": 3, "max_lines": 6})).await;
    assert_eq!(v["totals"]["files"], 1, "{v}");
    assert_eq!(v["totals"]["code"], 5, "{v}");
    assert_eq!(v["min_lines"], 3);
    assert_eq!(v["max_lines"], 6);
    assert_eq!(v["outside_line_bounds"], 2, "{v}");
    let files = stats(&c, json!({"min_lines": 3, "max_lines": 6, "detail": "files"})).await;
    assert_eq!(paths(&files), vec!["b.rs".to_string()], "{files}");
    assert_eq!(row(&files, "path", "b.rs")["code"], 5);

    // Bounds are inclusive: 2 and 10 lines stay in.
    let v = stats(&c, json!({"min_lines": 2, "max_lines": 10})).await;
    assert_eq!(v["totals"]["files"], 3, "{v}");
    assert_eq!(v["outside_line_bounds"], 0, "{v}");

    // One bound alone: the other is reported as null.
    let v = stats(&c, json!({"min_lines": 5})).await;
    assert_eq!(v["totals"]["files"], 2, "{v}");
    assert_eq!(v["outside_line_bounds"], 1);
    assert_eq!(v["max_lines"], Value::Null);
    let v = stats(&c, json!({"max_lines": 5})).await;
    assert_eq!(v["totals"]["files"], 2, "{v}");
    assert_eq!(v["outside_line_bounds"], 1);
    assert_eq!(v["min_lines"], Value::Null);

    // Zero is a valid bound and leaves nothing out.
    let v = stats(&c, json!({"min_lines": 0})).await;
    assert_eq!(v["totals"]["files"], 3, "{v}");

    // Without bounds, the answer has none of the bound fields.
    let v = stats(&c, json!({})).await;
    assert!(v.get("min_lines").is_none(), "{v}");
    assert!(v.get("max_lines").is_none(), "{v}");
    assert!(v.get("outside_line_bounds").is_none(), "{v}");
    assert_eq!(v["totals"]["files"], 3);
}

#[tokio::test]
async fn line_bounds_are_validated() {
    let d = tempfile::tempdir().unwrap();
    sized_tree(d.path());
    let c = ctx(d.path());
    for (input, needle) in [
        (json!({"min_lines": 5, "max_lines": 2}), "min_lines"),
        (json!({"min_lines": -1}), "min_lines"),
        (json!({"max_lines": -3}), "max_lines"),
        (json!({"min_lines": "3"}), "min_lines"),
    ] {
        let r = call(&c, input.clone()).await;
        assert!(r.is_error, "{input} accepted: {}", r.content);
        assert!(r.content.contains(needle), "{input}: {}", r.content);
    }
}

#[tokio::test]
async fn analyses_with_and_without_bounds_do_not_share_an_answer() {
    let d = tempfile::tempdir().unwrap();
    sized_tree(d.path());
    let c = ctx(d.path());
    // Two concurrent calls that differ only by a bound: a shared analysis
    // would give both the same totals.
    let (low, high) = tokio::join!(
        call(&c, json!({"min_lines": 6})),
        call(&c, json!({"max_lines": 2})),
    );
    let low: Value = serde_json::from_str(&low.content).unwrap();
    let high: Value = serde_json::from_str(&high.content).unwrap();
    assert_eq!(low["totals"]["code"], 10, "min 6 keeps c.rs only: {low}");
    assert_eq!(high["totals"]["code"], 2, "max 2 keeps a.rs only: {high}");
}

// ─── .gitattributes ─────────────────────────────────────────────────────────

#[tokio::test]
async fn gitattributes_marks_generated_vendored_and_documentation() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    std::fs::create_dir_all(p.join(".git")).unwrap();
    write(
        p,
        ".gitattributes",
        "# generated code\n\
         gen/*.rs linguist-generated\n\
         vendor/** linguist-vendored\n\
         docs/*.md linguist-documentation\n\
         *.min.js linguist-generated=true\n\
         keep_eq.rs linguist-generated=false\n\
         keep_unset.rs -linguist-generated\n\
         keep_zero.rs linguist-vendored=0\n",
    );
    write(p, "src/main.rs", RUST);
    write(p, "gen/out.rs", "fn g() {}\n");
    write(p, "vendor/lib/x.js", "let v = 1;\n");
    write(p, "docs/guide.md", "# Guide\n");
    write(p, "bundle.min.js", "let b=1;\n");
    write(p, "keep_eq.rs", "fn k() {}\n");
    write(p, "keep_unset.rs", "fn u() {}\n");
    write(p, "keep_zero.rs", "fn z() {}\n");
    let c = ctx(p);

    let v = stats(&c, json!({"detail": "files"})).await;
    assert_eq!(
        v["gitattributes_excluded"],
        json!(["gen/*.rs", "vendor/**", "docs/*.md", "*.min.js"]),
        "{v}"
    );
    assert_eq!(
        paths(&v),
        vec![
            "keep_eq.rs".to_string(),
            "keep_unset.rs".to_string(),
            "keep_zero.rs".to_string(),
            "src/main.rs".to_string(),
        ],
        "{v}"
    );
    assert_eq!(v["totals"]["files"], 4, "{v}");
    assert_eq!(v["partial"], false, "{v}");
    assert!(v.get("outside_line_bounds").is_none());
}

#[tokio::test]
async fn gitattributes_are_read_up_to_the_folder_holding_git_only() {
    let d = tempfile::tempdir().unwrap();
    let outer = d.path();
    // A .gitattributes above the repository must be ignored.
    write(outer, ".gitattributes", "outside.rs linguist-generated\n");
    let repo = outer.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    write(&repo, ".gitattributes", "inside.rs linguist-generated\n");
    write(&repo, "inside.rs", "fn i() {}\n");
    write(&repo, "outside.rs", "fn o() {}\n");
    write(&repo, "sub/inside.rs", "fn i() {}\n");
    write(&repo, "sub/outside.rs", "fn o() {}\n");
    // Paths are shown from the working directory: the repository here.
    let c = ctx(&repo);

    // The repository root: its own pattern applies (by name, at any depth),
    // the outer one does not.
    let v = stats(&c, json!({"detail": "files"})).await;
    assert_eq!(
        paths(&v),
        vec!["outside.rs".to_string(), "sub/outside.rs".to_string()],
        "{v}"
    );
    assert_eq!(v["gitattributes_excluded"], json!(["inside.rs"]), "{v}");

    // A subfolder: the repository's .gitattributes is read (it is the
    // parent up to `.git`), the outer one still is not.
    let v = stats(&c, json!({"path": "sub", "detail": "files"})).await;
    assert_eq!(paths(&v), vec!["sub/outside.rs".to_string()], "{v}");
    assert_eq!(v["gitattributes_excluded"], json!(["inside.rs"]), "{v}");

    // Without a repository, the folder's own .gitattributes applies.
    let plain = outer.join("plain");
    write(&plain, ".gitattributes", "gone.rs linguist-generated\n");
    write(&plain, "gone.rs", "fn g() {}\n");
    write(&plain, "kept.rs", "fn k() {}\n");
    let v = stats(&ctx(outer), json!({"path": "plain", "detail": "files"})).await;
    assert_eq!(paths(&v), vec!["plain/kept.rs".to_string()], "{v}");
}

#[tokio::test]
async fn gitattributes_patterns_of_a_parent_match_by_name_below_it() {
    // A basename pattern in the repository's .gitattributes applies to the
    // files of a subfolder, as git does.
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    write(&repo, ".gitattributes", "*.gen.rs linguist-generated\n");
    write(&repo, "sub/x.gen.rs", "fn x() {}\n");
    write(&repo, "sub/y.rs", "fn y() {}\n");
    let v = stats(&ctx(&repo), json!({"path": "sub", "detail": "files"})).await;
    assert_eq!(paths(&v), vec!["sub/y.rs".to_string()], "{v}");
}

#[tokio::test]
async fn bug_parent_slash_pattern_is_relative_to_the_repository() {
    // git: `gen/*.rs` in repo/.gitattributes means repo/gen/*.rs only, so
    // repo/sub/gen/two.rs is counted when analysing repo/sub.
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    write(&repo, ".gitattributes", "gen/*.rs linguist-generated\n");
    write(&repo, "gen/one.rs", "fn one() {}\n");
    write(&repo, "sub/gen/two.rs", "fn two() {}\n");
    write(&repo, "sub/keep.rs", "fn keep() {}\n");
    let v = stats(&ctx(&repo), json!({"path": "sub", "detail": "files"})).await;
    assert_eq!(
        paths(&v),
        vec!["sub/gen/two.rs".to_string(), "sub/keep.rs".to_string()],
        "{v}"
    );
}

#[tokio::test]
async fn bug_a_later_unset_line_undoes_an_earlier_one() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    std::fs::create_dir_all(p.join(".git")).unwrap();
    write(p, ".gitattributes", "*.rs linguist-generated\nkeep.rs -linguist-generated\n");
    write(p, "gen.rs", "fn g() {}\n");
    write(p, "keep.rs", "fn k() {}\n");
    let v = stats(&ctx(p), json!({"detail": "files"})).await;
    assert_eq!(paths(&v), vec!["keep.rs".to_string()], "{v}");
}

#[tokio::test]
async fn a_pattern_git_would_refuse_is_warned_and_not_listed() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    std::fs::create_dir_all(p.join(".git")).unwrap();
    // An unclosed alternation is refused by the glob builder; `a[b` is a
    // literal name for git, so it is listed and excludes nothing here.
    write(p, ".gitattributes", "a{b linguist-generated\na[b linguist-generated\n");
    write(p, "a.rs", "fn a() {}\n");
    let v = stats(&ctx(p), json!({})).await;
    let listed: Vec<&str> = v["gitattributes_excluded"].as_array().unwrap().iter().filter_map(|x| x.as_str()).collect();
    assert_eq!(listed, vec!["a[b"], "{v}");
    assert!(causes(&v).contains(&"gitattributes_pattern".to_string()), "{v}");
    assert_eq!(v["totals"]["files"], 1, "{v}");
}

fn causes(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["cause"].as_str().unwrap().to_string())
        .collect()
}

// ─── Markdown answer ────────────────────────────────────────────────────────

#[tokio::test]
async fn markdown_is_a_table_and_the_json_stays_as_metadata() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "main.rs", RUST);
    write(p, "tool.py", PY);
    let c = ctx(p);

    let md = call(&c, json!({"format": "markdown"})).await;
    assert!(!md.is_error, "{}", md.content);
    assert!(
        md.content
            .contains("| Language | Files | Code | Comments | Blanks | Lines | % |"),
        "{}",
        md.content
    );
    assert!(md.content.contains("| Rust | 1 | 3 | 1 | 1 | 5 | 75.0 |"), "{}", md.content);
    assert!(md.content.contains("| Python | 1 | 1 | 0 | 0 | 1 | 25.0 |"), "{}", md.content);
    assert!(md.content.contains("2 files, 6 lines"), "{}", md.content);
    assert!(!md.content.trim_start().starts_with('{'), "not JSON: {}", md.content);

    // The metadata is the same JSON the default format answers with.
    let json_answer = stats(&c, json!({})).await;
    assert_eq!(md.metadata.as_ref(), Some(&json_answer));
    let explicit = stats(&c, json!({"format": "json"})).await;
    assert_eq!(explicit, json_answer);
}

#[tokio::test]
async fn markdown_by_file_and_its_notes() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "main.rs", RUST);
    write(p, "tool.py", PY);
    write(p, "gen/out.rs", "fn g() {}\n");
    std::fs::create_dir_all(p.join(".git")).unwrap();
    write(p, ".gitattributes", "gen/*.rs linguist-generated\n");
    write(p, "big.rs", "fn b() {}\n".repeat(40));
    let c = ctx(p);

    let r = call(
        &c,
        json!({"format": "markdown", "detail": "files", "top_n": 1, "max_lines": 10}),
    )
    .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(
        r.content.contains("| File | Language | Code | Comments | Blanks | Lines | % |"),
        "{}",
        r.content
    );
    assert!(r.content.contains("| main.rs | Rust | 3 | 1 | 1 | 5 | 75.0 |"), "{}", r.content);
    assert!(r.content.contains("1 more rows not shown."), "{}", r.content);
    assert!(
        r.content.contains("Left out by .gitattributes: gen/*.rs."),
        "{}",
        r.content
    );
    assert!(
        r.content.contains("1 files outside the line bounds left out."),
        "{}",
        r.content
    );
}

#[tokio::test]
async fn markdown_summary_has_totals_and_no_table() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "main.rs", RUST);
    let r = call(&ctx(d.path()), json!({"format": "markdown", "detail": "summary"})).await;
    assert!(!r.is_error, "{}", r.content);
    assert!(r.content.contains("1 files") || r.content.contains("1 file"), "{}", r.content);
    assert!(!r.content.contains("| Language"), "{}", r.content);
}

#[tokio::test]
async fn format_is_json_or_markdown_only() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "main.rs", RUST);
    let c = ctx(d.path());
    for f in ["xml", "csv", "html"] {
        let r = call(&c, json!({"format": f})).await;
        assert!(r.is_error, "{f} accepted: {}", r.content);
        assert!(r.content.contains("format"), "{f}: {}", r.content);
    }
    // Trimmed, and the empty value is the default.
    assert!(!call(&c, json!({"format": " markdown "})).await.is_error);
    assert!(!call(&c, json!({"format": ""})).await.is_error);
}

#[tokio::test]
async fn bug_pipe_in_a_file_name_is_escaped_in_the_table() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a|b.rs", "fn a() {}\n");
    let r = call(
        &ctx(d.path()),
        json!({"format": "markdown", "detail": "files"}),
    )
    .await;
    assert!(r.content.contains(r"a\|b.rs"), "{}", r.content);
}

// ─── Tool description and schema ────────────────────────────────────────────

#[test]
fn schema_and_description_name_the_new_inputs() {
    let tool = CodeStatsTool;
    let schema = tool.input_schema();
    let props = &schema["properties"];
    assert_eq!(props["min_lines"]["minimum"], 0);
    assert_eq!(props["max_lines"]["minimum"], 0);
    assert_eq!(props["format"]["enum"], json!(["json", "markdown"]));
    assert_eq!(props["format"]["default"], "json");
    let text = tool.description();
    for word in ["percent", "min_lines", "max_lines", "gitattributes", "markdown", "top_n"] {
        assert!(text.contains(word), "description lacks {word}: {text}");
    }
}
