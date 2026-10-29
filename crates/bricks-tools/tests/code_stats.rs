//! CodeStats contract: languages, mixed files and notebooks, ignores, scope
//! and physical identity, faithful `partial`, and display limits that never
//! limit the analysis.

use bricks_tools::code_stats::{CodeStatsProbe, CodeStatsTool};
use bricks_tools::permissions::AllowAll;
use bricks_tools::{CostTracker, Extensions, Tool, ToolContext, ToolResult};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use tokei::{Config, LanguageType};

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        working_dir: dir.to_path_buf(),
        session_id: format!("code-stats-{}", uuid::Uuid::new_v4()),
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

fn totals(v: &Value) -> (u64, u64, u64, u64, u64) {
    let t = &v["totals"];
    let n = |k: &str| t[k].as_u64().unwrap();
    (
        n("files"),
        n("code"),
        n("comments"),
        n("blanks"),
        n("lines"),
    )
}

fn causes(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["cause"].as_str().unwrap().to_string())
        .collect()
}

fn language_row<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["language"] == name)
}

fn summarised(lang: LanguageType, text: &str) -> (u64, u64, u64) {
    let s = lang.parse_from_str(text, &Config::default()).summarise();
    (s.code as u64, s.comments as u64, s.blanks as u64)
}

#[cfg(unix)]
fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

const RUST: &str = "// comment\nfn main() {\n\n    println!(\"hi\");\n}\n";

// ─── 2. Languages, names, shebangs, encodings and parameters ────────────────

#[tokio::test]
async fn counts_common_languages_names_and_shebangs() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "main.rs", RUST);
    write(p, "app.ts", "// c\nconst a: number = 1;\n");
    write(p, "lib.js", "/* c */\nlet b = 2;\n\n");
    write(p, "tool.py", "# c\nx = 1\n");
    write(p, "Makefile", "all:\n\techo hi\n");
    write(p, "Dockerfile", "FROM alpine\n# c\n");
    write(p, "run", "#!/usr/bin/env python3\nprint(1)\n");
    write(p, "deploy", "#!/bin/bash\necho ok\n");
    write(p, "notes", "no shebang here\n");
    write(p, "data.xyz", "1\n2\n3\n");
    write(p, "empty.rs", "");
    write(
        p,
        "crlf.go",
        "package main\r\n\r\n// c\r\nfunc main() {}\r\n",
    );
    write(p, "unicode.js", "// café ☕\nlet s = \"é\";\n");
    // UTF-16 with a byte order mark is decoded before counting.
    let mut utf16 = vec![0xFF, 0xFE];
    for u in "x = 1\n# c\n".encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    write(p, "utf16.py", utf16);

    let v = stats(&ctx(p), json!({})).await;
    assert_eq!(v["detail"], "languages");
    assert_eq!(v["partial"], false, "{v}");
    let row = |n: &str| language_row(&v, n).unwrap_or_else(|| panic!("{n} missing: {v}"));
    assert_eq!(row("Rust")["files"], 2, "main.rs and empty.rs");
    assert_eq!(row("Rust")["code"], 3);
    assert_eq!(row("Rust")["comments"], 1);
    assert_eq!(row("Rust")["blanks"], 1);
    assert_eq!(row("TypeScript")["code"], 1);
    assert_eq!(row("JavaScript")["files"], 2);
    assert_eq!(
        row("Python")["files"],
        3,
        "tool.py, run (shebang), utf16.py"
    );
    assert_eq!(row("Python")["code"], 3);
    assert_eq!(
        row("Python")["comments"],
        3,
        "a shebang line is a comment for Tokei"
    );
    assert!(language_row(&v, "Makefile").is_some(), "{v}");
    assert!(language_row(&v, "Dockerfile").is_some(), "{v}");
    assert_eq!(row("BASH")["code"], 1);
    let go = row("Go");
    assert_eq!(
        (
            go["code"].clone(),
            go["comments"].clone(),
            go["blanks"].clone()
        ),
        (json!(2), json!(1), json!(1)),
        "CRLF"
    );
    // Unknown extensions and files without a shebang are not code.
    let (files, code, comments, blanks, lines) = totals(&v);
    assert_eq!(files, 12);
    assert_eq!(lines, code + comments + blanks);
    let sum = |k: &str| -> u64 {
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[k].as_u64().unwrap())
            .sum()
    };
    assert_eq!(
        (sum("files"), sum("code"), sum("lines")),
        (files, code, lines)
    );

    // A filter keeps one primary language, any case or identifier.
    for name in ["rust", "Rust", "RUST"] {
        let v = stats(&ctx(p), json!({"language": name})).await;
        assert_eq!(v["language"], "Rust");
        assert_eq!(totals(&v).0, 2);
        assert_eq!(v["results"].as_array().unwrap().len(), 1);
    }
    let v = stats(&ctx(p), json!({"language": "python"})).await;
    assert_eq!(
        totals(&v).0,
        3,
        "shebang files are filtered by their language too"
    );
    // A recognised language absent from the tree: complete zeros.
    let v = stats(&ctx(p), json!({"language": "C++"})).await;
    assert_eq!(totals(&v), (0, 0, 0, 0, 0));
    assert_eq!(v["partial"], false);
}

#[tokio::test]
async fn rejects_bad_parameters_with_a_usable_message() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.rs", RUST);
    let c = ctx(d.path());
    for (input, needle) in [
        (json!({"top_n": 0}), "top_n"),
        (json!({"top_n": 101}), "top_n"),
        (json!({"detail": "bogus"}), "detail"),
        (json!({"bogus": 1}), "bogus"),
        (json!({"language": "Klingon"}), "unknown language"),
        (json!({"exclude": ["!keep.rs"]}), "only excludes"),
        (json!({"exclude": ["a[b"]}), "exclude"),
        (json!({"exclude": [""]}), "empty"),
        (json!({"path": "missing"}), "missing"),
    ] {
        let r = call(&c, input.clone()).await;
        assert!(r.is_error, "{input} accepted: {}", r.content);
        assert!(r.content.contains(needle), "{input}: {}", r.content);
    }
    let v = stats(&c, json!({"top_n": 1})).await;
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
    let v = stats(&c, json!({"top_n": 100, "detail": "files"})).await;
    assert_eq!(v["results"][0]["path"], "a.rs");
}

// ─── 3. Mixed files and notebooks ───────────────────────────────────────────

const HTML: &str = "<html>\n<body>\n<!-- c -->\n<script>\nlet a = 1;\n// js comment\nlet b = 2;\n</script>\n</body>\n</html>\n";
const SVELTE: &str = "<script lang=\"ts\">\nlet x: number = 1;\n// ts\n</script>\n\n<style>\np {\n  color: red;\n}\n</style>\n\n<p>{x}</p>\n";
const MARKDOWN: &str = "# Title\n\nSome text.\n\n```rust\nfn f() {}\n// c\n```\n";

#[tokio::test]
async fn a_mixed_file_counts_once_for_its_primary_language() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "page.html", HTML);
    write(p, "App.svelte", SVELTE);
    write(p, "README.md", MARKDOWN);
    write(p, "app.js", "let c = 3;\n");

    let v = stats(&ctx(p), json!({"detail": "files"})).await;
    assert_eq!(totals(&v).0, 4, "a nested block is not a file: {v}");
    for (path, lang, tokei_lang, text) in [
        ("page.html", "HTML", LanguageType::Html, HTML),
        ("App.svelte", "Svelte", LanguageType::Svelte, SVELTE),
        ("README.md", "Markdown", LanguageType::Markdown, MARKDOWN),
    ] {
        let row = v["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["path"] == path)
            .unwrap();
        assert_eq!(row["language"], lang);
        let (code, comments, blanks) = summarised(tokei_lang, text);
        assert_eq!(
            (
                row["code"].as_u64().unwrap(),
                row["comments"].as_u64().unwrap(),
                row["blanks"].as_u64().unwrap()
            ),
            (code, comments, blanks),
            "{path}"
        );
        // The nested blocks are in, once.
        let root = tokei_lang.parse_from_str(text, &Config::default());
        assert!(!root.blobs.is_empty(), "{path} has nested blocks");
        assert!(code > root.code as u64, "{path}: nested code counted");
    }
    let sum: u64 = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["lines"].as_u64().unwrap())
        .sum();
    assert_eq!(sum, totals(&v).4);

    // JavaScript means JavaScript files, not the script of an HTML page.
    let v = stats(
        &ctx(p),
        json!({"language": "JavaScript", "detail": "files"}),
    )
    .await;
    assert_eq!(totals(&v).0, 1);
    assert_eq!(v["results"][0]["path"], "app.js");
}

fn notebook(cells: Value) -> String {
    json!({
        "cells": cells,
        "metadata": {
            "kernelspec": {"language": "python", "name": "python3"},
            "language_info": {"name": "python", "file_extension": ".py"}
        },
        "nbformat": 4,
        "nbformat_minor": 5
    })
    .to_string()
}

#[tokio::test]
async fn notebooks_count_once_and_an_invalid_one_is_not_an_empty_one() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(
        p,
        "one.ipynb",
        notebook(
            json!([{"cell_type": "code", "source": ["x = 1\n"], "metadata": {}, "outputs": [], "execution_count": null}]),
        ),
    );
    let v = stats(&ctx(p), json!({"path": "one.ipynb"})).await;
    assert_eq!(v["results"][0]["language"], "Jupyter Notebooks");
    assert_eq!(totals(&v).1, 1, "one line of code, not two: {v}");
    assert_eq!(v["partial"], false);

    // A Markdown cell with fenced blocks: the cells add up once each, their
    // nested blocks included.
    // Nested twice (a script in an HTML block) and in the kernel's own
    // language: still once each.
    let md = "# T\n\n```rust\nfn f() {}\n```\n\n```html\n<p>x</p>\n<script>\nlet a = 1;\n</script>\n```\n\n```python\nz = 3\n```\n";
    write(
        p,
        "two.ipynb",
        notebook(json!([
            {"cell_type": "code", "source": ["x = 1\n", "# c\n", "y = 2\n"], "metadata": {}, "outputs": [], "execution_count": null},
            {"cell_type": "markdown", "source": [md], "metadata": {}}
        ])),
    );
    let v = stats(&ctx(p), json!({"path": "two.ipynb"})).await;
    let py = summarised(LanguageType::Python, "x = 1\n# c\ny = 2\n");
    let mdc = summarised(LanguageType::Markdown, md);
    let t = totals(&v);
    assert_eq!(
        (t.1, t.2, t.3),
        (py.0 + mdc.0, py.1 + mdc.1, py.2 + mdc.2),
        "{v}"
    );

    // Valid and empty: a complete zero. Invalid: a diagnostic and partial.
    write(p, "empty.ipynb", notebook(json!([])));
    write(p, "broken.ipynb", "{ not json");
    write(
        p,
        "raw.ipynb",
        notebook(json!([{"cell_type": "raw", "source": ["x"], "metadata": {}}])),
    );
    let v = stats(&ctx(p), json!({"path": "empty.ipynb"})).await;
    assert_eq!(totals(&v), (1, 0, 0, 0, 0));
    assert_eq!(v["partial"], false);
    for bad in ["broken.ipynb", "raw.ipynb"] {
        let v = stats(&ctx(p), json!({"path": bad})).await;
        assert_eq!(totals(&v).0, 0, "{v}");
        assert_eq!(v["partial"], true, "{v}");
        assert!(causes(&v).contains(&"invalid_notebook".to_string()), "{v}");
    }
    // In a tree, the valid ones are counted and the invalid ones said.
    let v = stats(&ctx(p), json!({"language": "Jupyter Notebooks"})).await;
    assert_eq!(totals(&v).0, 3);
    assert_eq!(v["partial"], true);
    let w = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["cause"] == "invalid_notebook")
        .unwrap();
    assert_eq!(w["count"], 2);
}

// ─── 4. Ignores, hidden files, exclusions ───────────────────────────────────

#[tokio::test]
async fn each_ignore_file_applies_alone_and_together() {
    for name in [".gitignore", ".ignore", ".tokeignore"] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        write(p, name, "skipped.rs\n");
        write(p, "skipped.rs", RUST);
        write(p, "kept.rs", RUST);
        let v = stats(&ctx(p), json!({"detail": "files"})).await;
        assert_eq!(totals(&v).0, 1, "{name}: {v}");
        assert_eq!(v["results"][0]["path"], "kept.rs");
        assert_eq!(v["partial"], false);
    }

    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, ".gitignore", "a.rs\n");
    write(p, ".ignore", "b.rs\n");
    write(p, ".tokeignore", "c.rs\n");
    write(p, "sub/.gitignore", "local.rs\n");
    for f in [
        "a.rs",
        "b.rs",
        "c.rs",
        "d.rs",
        "local.rs",
        "sub/local.rs",
        "sub/e.rs",
    ] {
        write(p, f, RUST);
    }
    // Hidden files are counted, generated folders too unless a rule says.
    write(p, ".github/workflows/ci.yml", "on: push\njobs: {}\n");
    write(p, "target/gen.rs", RUST);
    write(p, "node_modules/x/index.js", "let a = 1;\n");
    let v = stats(&ctx(p), json!({"detail": "files", "top_n": 100})).await;
    let paths: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            ".github/workflows/ci.yml",
            "d.rs",
            "local.rs",
            "node_modules/x/index.js",
            "sub/e.rs",
            "target/gen.rs"
        ],
        "nested rules apply to their folder only: {v}"
    );
    assert_eq!(
        v["partial"], false,
        "generated folders do not make it partial"
    );
    let g = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["cause"] == "generated_dirs")
        .unwrap();
    assert_eq!(g["count"], 2);

    // `exclude` is relative to the analysed root and only removes.
    let v = stats(
        &ctx(p),
        json!({"exclude": ["sub/*.rs", "target/", "node_modules"], "detail": "files"}),
    )
    .await;
    let paths: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert!(
        !paths.iter().any(|p| p.starts_with("sub/")
            || p.starts_with("target/")
            || p.starts_with("node_modules/")),
        "{paths:?}"
    );
    assert!(paths.contains(&"local.rs"));
    let v = stats(&ctx(p), json!({"path": "sub", "exclude": ["e.rs"]})).await;
    assert_eq!(totals(&v).0, 0, "anchored at the analysed folder: {v}");
}

#[tokio::test]
async fn a_requested_file_still_obeys_the_ignore_rules() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, ".tokeignore", "secret.rs\n");
    write(p, "sub/.gitignore", "gen.rs\n");
    write(p, "secret.rs", RUST);
    write(p, "sub/gen.rs", RUST);
    write(p, "sub/kept.rs", RUST);
    for path in ["secret.rs", "sub/gen.rs"] {
        let v = stats(&ctx(p), json!({"path": path})).await;
        assert_eq!(totals(&v).0, 0, "{path}: {v}");
        assert!(causes(&v).contains(&"ignored".to_string()), "{v}");
        assert_eq!(v["partial"], false);
    }
    let v = stats(&ctx(p), json!({"path": "sub/kept.rs"})).await;
    assert_eq!(totals(&v).0, 1);
    assert_eq!(v["path"], "sub/kept.rs");
    // Exclusions for a file are anchored at its folder.
    let v = stats(
        &ctx(p),
        json!({"path": "sub/kept.rs", "exclude": ["kept.rs"]}),
    )
    .await;
    assert_eq!(totals(&v).0, 0);
    let v = stats(
        &ctx(p),
        json!({"path": "sub/kept.rs", "exclude": ["sub/kept.rs"]}),
    )
    .await;
    assert_eq!(totals(&v).0, 1, "not anchored at the working directory");
    // An unknown kind of file is said, not silently zero.
    write(p, "data.xyz", "1\n");
    let v = stats(&ctx(p), json!({"path": "data.xyz"})).await;
    assert!(causes(&v).contains(&"unrecognized".to_string()), "{v}");
}

// ─── 5. Links, hard links, non-regular entries and scope ────────────────────

#[cfg(unix)]
#[tokio::test]
async fn links_stay_in_scope_and_each_file_counts_once() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    let outside = d.path().join("outside");
    write(&root, "a.rs", RUST);
    write(&outside, "secret.rs", RUST);
    write(&outside, "script", "#!/usr/bin/env python3\nprint(1)\n");
    write(&outside, "deep/x.rs", RUST);
    symlink(root.join("a.rs"), root.join("link_in.rs")).unwrap();
    symlink(outside.join("secret.rs"), root.join("link_out.rs")).unwrap();
    symlink(outside.join("script"), root.join("tool")).unwrap();
    symlink(&outside, root.join("dirlink")).unwrap();
    symlink(root.join("missing.rs"), root.join("broken.rs")).unwrap();
    std::fs::hard_link(root.join("a.rs"), root.join("hard.rs")).unwrap();
    mkfifo(&root.join("pipe.rs"));

    let v = stats(&ctx(&root), json!({"detail": "files"})).await;
    assert_eq!(totals(&v).0, 1, "one physical file: {v}");
    assert_eq!(
        v["results"][0]["path"], "a.rs",
        "the first path in walk order"
    );
    let c = causes(&v);
    for cause in [
        "duplicate",
        "link_outside_root",
        "directory_link",
        "broken_link",
        "not_regular",
    ] {
        assert!(c.contains(&cause.to_string()), "{cause}: {v}");
    }
    assert_eq!(v["partial"], false, "refused entries are not missing code");

    // A root link is resolved once; links inside still cannot leave it.
    symlink(&root, d.path().join("rootlink")).unwrap();
    let v = stats(
        &ctx(d.path()),
        json!({"path": "rootlink", "detail": "files"}),
    )
    .await;
    assert_eq!(totals(&v).0, 1, "{v}");
    // A FIFO or a missing root is a request error.
    let r = call(&ctx(&root), json!({"path": "pipe.rs"})).await;
    assert!(r.is_error && r.content.contains("neither"), "{}", r.content);
    // Separate scopes do not see each other.
    let v = stats(&ctx(d.path()), json!({"path": "outside"})).await;
    assert_eq!(totals(&v).0, 3);
}

/// Swaps made between the walk and the reads: never read outside, never
/// counted as the file the walk saw.
#[cfg(unix)]
#[tokio::test]
async fn entries_changed_after_the_walk_are_not_read() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    let outside = d.path().join("outside");
    write(
        &outside,
        "secret.rs",
        "fn secret() {}\nfn secret2() {}\nfn secret3() {}\n",
    );
    for f in ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs", "g.rs"] {
        write(&root, f, RUST);
    }
    let c = ctx(&root);
    let r2 = root.clone();
    let o2 = outside.clone();
    let probe = CodeStatsProbe {
        walked: Some(Arc::new(move || {
            // b: now a link out; c: another file; d: gone; e: a FIFO;
            // f: rewritten in place (same file, new content).
            std::fs::remove_file(r2.join("b.rs")).unwrap();
            symlink(o2.join("secret.rs"), r2.join("b.rs")).unwrap();
            std::fs::write(r2.join("c.new"), RUST).unwrap();
            std::fs::rename(r2.join("c.new"), r2.join("c.rs")).unwrap();
            std::fs::remove_file(r2.join("d.rs")).unwrap();
            std::fs::remove_file(r2.join("e.rs")).unwrap();
            mkfifo(&r2.join("e.rs"));
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(r2.join("f.rs"))
                .unwrap();
            std::io::Write::write_all(&mut f, b"fn more() {}\n").unwrap();
        })),
        opened: Some(Arc::new(|path: &Path| {
            // g changes while it is being read.
            if path.ends_with("g.rs") {
                let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
                std::io::Write::write_all(&mut f, b"fn late() {}\n").unwrap();
            }
        })),
        counted: None,
    };
    c.extensions.insert(probe);
    let v = stats(&c, json!({"detail": "files"})).await;
    let paths: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths
            .iter()
            .filter(|p| ["b.rs", "c.rs", "d.rs", "e.rs"].contains(p))
            .count(),
        0,
        "{v}"
    );
    assert!(
        paths.contains(&"a.rs") && paths.contains(&"f.rs") && paths.contains(&"g.rs"),
        "{v}"
    );
    let f = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == "f.rs")
        .unwrap();
    assert_eq!(f["code"], 4, "the content read, not the content walked");
    assert_eq!(v["partial"], true);
    let w = |cause: &str| {
        v["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["cause"] == cause)
            .cloned()
    };
    assert_eq!(w("changed").unwrap()["count"], 3, "b, c, e: {v}");
    assert_eq!(w("vanished").unwrap()["count"], 1);
    assert_eq!(
        w("changed_during_read").unwrap()["examples"],
        json!(["g.rs"])
    );
    assert!(
        !v.to_string().contains("secret"),
        "nothing outside was read: {v}"
    );
}

// ─── 6. Read and walk errors ────────────────────────────────────────────────

#[cfg(unix)]
#[tokio::test]
async fn unreadable_entries_make_the_analysis_partial() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        return; // permissions do not stop root
    }
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    write(p, "ok.rs", RUST);
    write(p, "locked.rs", RUST);
    write(p, "closed/inner.rs", RUST);
    std::fs::set_permissions(p.join("locked.rs"), std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::set_permissions(p.join("closed"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let v = stats(&ctx(p), json!({})).await;
    std::fs::set_permissions(p.join("closed"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(totals(&v).0, 1, "{v}");
    assert_eq!(v["partial"], true);
    let c = causes(&v);
    assert!(
        c.contains(&"unreadable".to_string()) && c.contains(&"walk_error".to_string()),
        "{v}"
    );
}

// ─── 9. Display limits never limit the analysis ─────────────────────────────

#[tokio::test]
async fn top_n_only_limits_the_rows_shown() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    let mut expected: Vec<(String, u64)> = Vec::new();
    for i in 0..60u64 {
        let n = i % 10 + 1;
        let name = format!("src/f{i:02}.rs");
        write(p, &name, "fn x() {}\n".repeat(n as usize));
        expected.push((name, n));
    }
    for i in 0..5u64 {
        write(p, &format!("py/m{i}.py"), "x = 1\n".repeat(i as usize + 1));
        expected.push((format!("py/m{i}.py"), i + 1));
    }
    expected.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total_code: u64 = expected.iter().map(|e| e.1).sum();

    let c = ctx(p);
    let summary = stats(&c, json!({"detail": "summary"})).await;
    let languages = stats(&c, json!({"detail": "languages"})).await;
    let files = stats(&c, json!({"detail": "files"})).await;
    let all = stats(&c, json!({"detail": "files", "top_n": 100})).await;
    for v in [&summary, &languages, &files, &all] {
        assert_eq!(
            v["totals"], summary["totals"],
            "same totals in every detail"
        );
        assert_eq!(v["partial"], false);
    }
    assert_eq!(totals(&summary).0, 65);
    assert_eq!(totals(&summary).1, total_code);
    assert_eq!(summary["results"], json!([]));
    assert_eq!(
        (summary["omitted"].clone(), summary["truncated"].clone()),
        (json!(0), json!(false))
    );

    let shown: Vec<(String, u64)> = files["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["path"].as_str().unwrap().to_string(),
                r["code"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(shown, expected[..20].to_vec(), "exact top 20, ties by path");
    assert_eq!(
        (files["omitted"].clone(), files["truncated"].clone()),
        (json!(45), json!(true))
    );
    assert_eq!(all["results"].as_array().unwrap().len(), 65);
    assert_eq!(
        (all["omitted"].clone(), all["truncated"].clone()),
        (json!(0), json!(false))
    );

    assert_eq!(languages["results"][0]["language"], "Rust");
    assert_eq!(languages["results"][1]["language"], "Python");
    let one = stats(&c, json!({"top_n": 1})).await;
    assert_eq!(
        (one["omitted"].clone(), one["truncated"].clone()),
        (json!(1), json!(true))
    );
    assert_eq!(one["totals"], summary["totals"]);
}

#[tokio::test]
async fn an_empty_tree_is_a_complete_zero() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "notes.txt.unknown", "x\n");
    let v = stats(&ctx(d.path()), json!({})).await;
    assert_eq!(totals(&v), (0, 0, 0, 0, 0));
    assert_eq!(v["partial"], false);
    assert_eq!(v["warnings"], json!([]));
    assert_eq!(v["path"], ".");
}
