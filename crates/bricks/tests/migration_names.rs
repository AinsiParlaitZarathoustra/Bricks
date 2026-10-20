//! The rename from `cersei` to `bricks` stays done: no workspace package
//! keeps an old name, and no code or manifest imports or paths an old crate
//! (`cersei_*`, `cersei::`). External identities stay on purpose and are not
//! searched for: the `CERSEI_*` variables, the `cersei-envd` binary, the
//! docker labels and images, the user agent, and frozen captured fixtures.
//! Docs and wiki are not scanned.

use std::path::{Path, PathBuf};

/// Files that keep an old spelling for a reason the rename does not change:
/// a frozen captured output, and the source file name of the `cersei-envd`
/// binary (its binary name is the external identity, see Cargo.toml).
const ALLOWED: &[&str] = &[
    "crates/bricks-compression/tests/support/cases.rs",
    "crates/bricks-compression/tests/fixtures/",
    "crates/bricks-vms/Cargo.toml",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Every file under `dir` with one of `exts`, skipping build output, vcs,
/// generated trees and the documentation folders.
fn files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            let skip = name.starts_with('.')
                || matches!(
                    name.as_str(),
                    "target" | "node_modules" | "docs" | "wiki" | "dist" | "build"
                );
            if !skip {
                files(&p, exts, out);
            }
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| exts.contains(&x))
        {
            out.push(p);
        }
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn allowed(path: &str) -> bool {
    ALLOWED
        .iter()
        .any(|a| path == *a || (a.ends_with('/') && path.starts_with(a)))
}

/// The occurrences of `cersei_` or `cersei::` that start a word (not
/// `bricks_cersei_…`, not `CERSEI_…`).
fn old_paths(text: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (i, _) in text.match_indices("cersei") {
        let before = text[..i].chars().next_back();
        let after = &text[i + "cersei".len()..];
        let word_start = !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if word_start && (after.starts_with('_') || after.starts_with("::")) {
            let line = text[..i].lines().count().max(1);
            hits.push(format!(
                "line {line}: {}",
                after.chars().take(12).collect::<String>()
            ));
        }
    }
    hits
}

#[test]
fn no_workspace_package_keeps_an_old_name() {
    let root = repo_root();
    let mut manifests = Vec::new();
    files(&root, &["toml"], &mut manifests);
    let mut bad = Vec::new();
    for m in manifests
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "Cargo.toml"))
    {
        let text = std::fs::read_to_string(m).unwrap();
        let mut in_package = false;
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_package = t == "[package]";
            } else if in_package
                && let Some(v) = t.strip_prefix("name")
                && let Some(v) = v.trim_start().strip_prefix('=')
                && v.trim().trim_matches('"').starts_with("cersei")
            {
                bad.push(rel(&root, m));
            }
        }
    }
    assert!(bad.is_empty(), "packages with an old name: {bad:?}");
}

#[test]
fn no_active_import_or_path_of_an_old_crate_name() {
    let root = repo_root();
    let mut sources = Vec::new();
    files(
        &root,
        &["rs", "toml", "py", "sh", "ts", "tsx"],
        &mut sources,
    );
    let mut bad = Vec::new();
    for p in &sources {
        let r = rel(&root, p);
        if allowed(&r) || r.ends_with("migration_names.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        for hit in old_paths(&text) {
            bad.push(format!("{r}: {hit}"));
        }
    }
    assert!(bad.is_empty(), "old crate names still in use: {bad:#?}");
}

#[test]
fn the_external_identities_are_the_only_old_spellings_kept() {
    // The binary name is the identity that stays.
    let root = repo_root();
    let vms = std::fs::read_to_string(root.join("crates/bricks-vms/Cargo.toml")).unwrap();
    assert!(vms.contains("name = \"cersei-envd\""), "{vms}");
    assert!(old_paths("use cersei_envd;").len() == 1);
    assert!(old_paths("bricks_cersei_x").is_empty());
    assert!(old_paths("CERSEI_TEST_VAR").is_empty());
    assert!(old_paths("CerseiError").is_empty());
}
