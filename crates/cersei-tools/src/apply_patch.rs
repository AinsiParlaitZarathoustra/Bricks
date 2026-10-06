//! ApplyPatch tool: apply unified diffs (`diff -u`, `git diff`).
//!
//! Supported: several files and several hunks per file, file creation and
//! deletion through `/dev/null`, `a/`/`b/` prefixes, timestamps after a tab,
//! C-quoted paths (`"with \"quote\""`) and plain paths with spaces, CRLF
//! files, and `\ No newline at end of file`. Refused explicitly: binary
//! patches, renames, copies and mode changes.
//!
//! Everything is validated before anything is written: headers, hunk line
//! counts, the context of every hunk against the file (exact, at the stated
//! line or the nearest place it matches), and every path — relative, inside
//! the working directory, through no symlink. Files are then written one by
//! one, atomically each; if a write fails, the files already written are
//! restored and the result says so. This is not a multi-file transaction:
//! the result names exactly what changed.

use super::*;
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

pub struct ApplyPatchTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    patch: String,
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "ApplyPatch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch (as produced by `diff -u` or `git diff`) to one or more files, \
         relative to the working directory. Supports several hunks and files, creating files \
         (`--- /dev/null`) and deleting them (`+++ /dev/null`). Context lines must match the files \
         exactly; nothing is written unless the whole patch applies."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::FileSystem
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "patch": { "type": "string", "description": "Unified diff patch content" }
            },
            "required": ["patch"]
        })
    }

    async fn preview(
        &self,
        input: &Value,
        ctx: &ToolContext,
    ) -> Option<crate::preview::ChangePreview> {
        let input: Input = match crate::tool_feedback::parse_input(self, input) {
            Ok(i) => i,
            Err(e) => return Some(crate::preview::ChangePreview::refused(e.content)),
        };
        let actions = match plan(&input.patch, &ctx.working_dir) {
            Ok(a) => a,
            Err(e) => {
                return Some(crate::preview::ChangePreview::refused(format!(
                    "The patch would not be applied: {e}"
                )))
            }
        };
        let files = actions
            .iter()
            .map(|a| {
                let (path, after) = match a {
                    Action::Write { path, content, .. } => {
                        (path, Some(String::from_utf8_lossy(content).into_owned()))
                    }
                    Action::Delete { path, .. } => (path, None),
                };
                let before = std::fs::read(path)
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).into_owned());
                let shown = path
                    .strip_prefix(&ctx.working_dir)
                    .unwrap_or(path)
                    .display()
                    .to_string();
                crate::preview::file_change(&shown, path, before.as_deref(), after.as_deref())
            })
            .collect();
        Some(crate::preview::ChangePreview {
            files,
            refusal: None,
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        match apply_unified_patch(&input.patch, &ctx.working_dir) {
            Ok(changes) if changes.is_empty() => {
                ToolResult::success("The patch contains no file changes.")
            }
            Ok(changes) => ToolResult::success(format!(
                "Patch applied to {} file(s):\n{}",
                changes.len(),
                changes
                    .iter()
                    .map(|c| format!("  {c}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
            Err(e) => ToolResult::error(format!("The patch was not applied: {e}")),
        }
    }
}

// ─── Parsing ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct HunkLine {
    kind: char,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_count: usize,
    header: String,
    lines: Vec<HunkLine>,
    /// `\ No newline at end of file` after the last old / new line.
    old_no_eol: bool,
    new_no_eol: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePatch {
    /// `None` for `/dev/null`.
    old: Option<String>,
    new: Option<String>,
    hunks: Vec<Hunk>,
}

/// Decode a header path: C-quoted or plain (up to a tab, i.e. before a
/// timestamp).
fn header_path(rest: &str) -> std::result::Result<Option<String>, String> {
    let rest = rest.strip_suffix('\r').unwrap_or(rest);
    let raw = if let Some(q) = rest.strip_prefix('"') {
        let mut out = Vec::new();
        let mut chars = q.chars();
        loop {
            match chars.next() {
                None => return Err(format!("unterminated quoted path in `{rest}`")),
                Some('"') => break,
                Some('\\') => match chars.next() {
                    Some('n') => out.push(b'\n'),
                    Some('t') => out.push(b'\t'),
                    Some('"') => out.push(b'"'),
                    Some('\\') => out.push(b'\\'),
                    Some(d @ '0'..='7') => {
                        let mut v = d.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            if let Some(n) = chars.clone().next().and_then(|c| c.to_digit(8)) {
                                chars.next();
                                v = v * 8 + n;
                            }
                        }
                        out.push(v as u8);
                    }
                    other => {
                        return Err(format!(
                            "unsupported escape `\\{}` in a quoted path",
                            other.unwrap_or(' ')
                        ))
                    }
                },
                Some(c) => {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
        String::from_utf8(out).map_err(|_| "a quoted path is not valid UTF-8".to_string())?
    } else {
        // Timestamps follow a tab; spaces belong to the name.
        rest.split('\t').next().unwrap_or(rest).to_string()
    };
    if raw == "/dev/null" {
        return Ok(None);
    }
    if raw.is_empty() {
        return Err("empty path in a file header".into());
    }
    Ok(Some(raw))
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let body = line.strip_prefix("@@ -")?;
    let (ranges, _) = body.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |s: &str| -> Option<(usize, usize)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (os, oc) = range(old)?;
    let (ns, nc) = range(new)?;
    Some((os, oc, ns, nc))
}

fn parse_patch(patch: &str) -> std::result::Result<Vec<FilePatch>, String> {
    let lines: Vec<&str> = patch.split('\n').collect();
    let mut files: Vec<FilePatch> = Vec::new();
    let mut git_header = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].strip_suffix('\r').unwrap_or(lines[i]);
        if line.starts_with("diff --git ") {
            git_header = true;
            i += 1;
            continue;
        }
        if line.starts_with("GIT binary patch")
            || (line.starts_with("Binary files ") && line.ends_with(" differ"))
        {
            return Err("binary patches are not supported; edit binary files another way".into());
        }
        for (prefix, what) in [
            ("rename from ", "renames"),
            ("rename to ", "renames"),
            ("copy from ", "copies"),
            ("copy to ", "copies"),
            ("old mode ", "mode changes"),
            ("new mode ", "mode changes"),
        ] {
            if line.starts_with(prefix) {
                return Err(format!(
                    "{what} are not supported by ApplyPatch; use Bash (`git mv`, `chmod`) for that part"
                ));
            }
        }
        if let Some(rest) = line.strip_prefix("--- ") {
            let next = lines
                .get(i + 1)
                .map(|l| l.strip_suffix('\r').unwrap_or(l))
                .unwrap_or("");
            let Some(new_rest) = next.strip_prefix("+++ ") else {
                return Err(format!(
                    "line {}: `--- ` must be followed by a `+++ ` line",
                    i + 1
                ));
            };
            let mut old = header_path(rest)?;
            let mut new = header_path(new_rest)?;
            let both_git = old.as_deref().is_some_and(|o| o.starts_with("a/"))
                && new.as_deref().is_some_and(|n| n.starts_with("b/"));
            let one_sided = old.is_none() || new.is_none();
            if git_header || both_git || one_sided {
                if let Some(o) = old.as_mut() {
                    if let Some(s) = o.strip_prefix("a/") {
                        *o = s.to_string();
                    }
                }
                if let Some(n) = new.as_mut() {
                    if let Some(s) = n.strip_prefix("b/") {
                        *n = s.to_string();
                    }
                }
            }
            if old.is_none() && new.is_none() {
                return Err(format!("line {}: both sides are /dev/null", i + 1));
            }
            files.push(FilePatch {
                old,
                new,
                hunks: Vec::new(),
            });
            git_header = false;
            i += 2;
            continue;
        }
        if line.starts_with("@@ ") {
            let file = files
                .last_mut()
                .ok_or_else(|| format!("line {}: a hunk before any `---`/`+++` header", i + 1))?;
            let (os, oc, _ns, nc) = parse_hunk_header(line)
                .ok_or_else(|| format!("line {}: malformed hunk header `{line}`", i + 1))?;
            let mut hunk = Hunk {
                old_start: os,
                old_count: oc,
                new_count: nc,
                header: line.to_string(),
                lines: Vec::new(),
                old_no_eol: false,
                new_no_eol: false,
            };
            let (mut seen_old, mut seen_new) = (0usize, 0usize);
            i += 1;
            while seen_old < oc || seen_new < nc {
                // The empty string after the patch's final newline is the end
                // of the text, not an empty context line.
                let at_end = i + 1 == lines.len() && lines[i].is_empty();
                let Some(raw) = lines.get(i).filter(|_| !at_end) else {
                    return Err(format!(
                        "{}: the hunk ends early ({seen_old}/{oc} old and {seen_new}/{nc} new lines)",
                        hunk.header
                    ));
                };
                let (kind, text) = match raw.chars().next() {
                    Some(c @ (' ' | '-' | '+')) => (c, raw[1..].to_string()),
                    // An empty context line whose space was stripped.
                    None => (' ', String::new()),
                    Some('\\') => {
                        i += 1;
                        continue;
                    }
                    Some(_) => {
                        return Err(format!(
                            "{}: line {} is not part of the hunk (counts say {oc} old / {nc} new lines, \
                             found {seen_old} / {seen_new})",
                            hunk.header,
                            i + 1
                        ))
                    }
                };
                match kind {
                    ' ' => {
                        seen_old += 1;
                        seen_new += 1;
                    }
                    '-' => seen_old += 1,
                    _ => seen_new += 1,
                }
                if seen_old > oc || seen_new > nc {
                    return Err(format!(
                        "{}: more lines than the header announces",
                        hunk.header
                    ));
                }
                hunk.lines.push(HunkLine { kind, text });
                i += 1;
                // `\ No newline at end of file` refers to the line just read.
                if lines.get(i).is_some_and(|l| l.starts_with('\\')) {
                    match kind {
                        '-' => hunk.old_no_eol = true,
                        '+' => hunk.new_no_eol = true,
                        _ => {
                            hunk.old_no_eol = true;
                            hunk.new_no_eol = true;
                        }
                    }
                    i += 1;
                }
            }
            file.hunks.push(hunk);
            continue;
        }
        i += 1;
    }
    Ok(files)
}

/// Paths a patch writes (created, modified or deleted), as the tool resolves
/// them relative to the working directory. Used by the agent's
/// read-before-edit guard so both agree on targets.
pub fn patch_targets(patch: &str) -> Vec<String> {
    parse_patch(patch)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.new.or(f.old))
        .collect()
}

// ─── Paths ───────────────────────────────────────────────────────────────────

/// Resolve `rel` inside `root`: relative, no `..` escape, no symlink on the
/// way, and the nearest existing ancestor inside the root.
fn resolve(root: &Path, rel: &str) -> std::result::Result<PathBuf, String> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(format!(
            "`{rel}`: absolute paths are refused; use a path relative to the working directory"
        ));
    }
    let mut depth = 0isize;
    for c in p.components() {
        match c {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!("`{rel}`: the path leaves the working directory"));
                }
            }
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return Err(format!("`{rel}`: unsupported path component")),
        }
    }
    let root_canon = std::fs::canonicalize(root).map_err(|e| format!("working directory: {e}"))?;
    let full = root.join(p);
    // Walk every existing prefix: none may be a symlink, all stay inside.
    let mut cur = root.to_path_buf();
    for c in p.components() {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!(
                    "`{rel}`: goes through a symbolic link ({}), which is refused",
                    cur.display()
                ))
            }
            Ok(_) => {
                let canon = std::fs::canonicalize(&cur).map_err(|e| format!("`{rel}`: {e}"))?;
                if !canon.starts_with(&root_canon) {
                    return Err(format!("`{rel}`: resolves outside the working directory"));
                }
            }
            Err(_) => break,
        }
    }
    Ok(full)
}

// ─── Application ─────────────────────────────────────────────────────────────

/// What will be written for one file.
enum Action {
    Write {
        path: PathBuf,
        content: Vec<u8>,
        created: bool,
        summary: String,
    },
    Delete {
        path: PathBuf,
        summary: String,
    },
}

fn split_keep(content: &str) -> (Vec<String>, bool) {
    let ends_nl = content.ends_with('\n');
    let mut v: Vec<String> = content.split('\n').map(|s| s.to_string()).collect();
    if ends_nl {
        v.pop();
    }
    (v, ends_nl)
}

fn strip_cr(s: &str) -> &str {
    s.strip_suffix('\r').unwrap_or(s)
}

/// Apply the hunks of one file to its lines.
fn apply_hunks(
    name: &str,
    original: &str,
    hunks: &[Hunk],
) -> std::result::Result<(String, usize, usize), String> {
    let crlf = original.contains("\r\n");
    let (lines, mut ends_nl) = split_keep(original);
    let mut out: Vec<String> = Vec::new();
    let mut pos = 0usize; // next unconsumed original line
    let (mut added, mut removed) = (0, 0);
    for (n, h) in hunks.iter().enumerate() {
        let old_block: Vec<&str> = h
            .lines
            .iter()
            .filter(|l| l.kind != '+')
            .map(|l| l.text.as_str())
            .collect();
        let expected = if h.old_count == 0 {
            h.old_start
        } else {
            h.old_start.saturating_sub(1)
        };
        let matches_at = |at: usize| -> bool {
            at >= pos
                && at + old_block.len() <= lines.len()
                && old_block
                    .iter()
                    .enumerate()
                    .all(|(k, t)| strip_cr(&lines[at + k]) == strip_cr(t))
        };
        // The stated line first, then the nearest place, never before the
        // previous hunk.
        let mut found = None;
        for d in 0..=lines.len() {
            let below = expected + d;
            if matches_at(below) {
                found = Some(below);
                break;
            }
            if d > 0 && expected >= d && matches_at(expected - d) {
                found = Some(expected - d);
                break;
            }
            if below > lines.len() && expected < d {
                break;
            }
        }
        let Some(at) = found else {
            let show: Vec<String> = lines
                .iter()
                .enumerate()
                .skip(expected.saturating_sub(1))
                .take(old_block.len().clamp(3, 8))
                .map(|(i, l)| format!("{:>6} | {}", i + 1, strip_cr(l)))
                .collect();
            return Err(format!(
                "{name}: hunk {} ({}) does not match the file — its context/removed lines are not \
                 at line {} or anywhere after the previous hunk. File around line {}:\n{}",
                n + 1,
                h.header,
                expected + 1,
                expected + 1,
                show.join("\n")
            ));
        };
        out.extend(lines[pos..at].iter().cloned());
        // Index in the old block: context lines keep the file's exact text
        // (its `\r` included).
        let mut k = 0usize;
        for l in &h.lines {
            match l.kind {
                ' ' => {
                    out.push(lines[at + k].clone());
                    k += 1;
                }
                '+' => {
                    let mut t = l.text.clone();
                    if crlf && !t.ends_with('\r') {
                        t.push('\r');
                    }
                    out.push(t);
                    added += 1;
                }
                _ => {
                    removed += 1;
                    k += 1;
                }
            }
        }
        pos = at + old_block.len();
        if pos == lines.len() {
            if h.new_no_eol {
                ends_nl = false;
            } else if h.old_no_eol {
                ends_nl = true;
            }
        }
    }
    out.extend(lines[pos..].iter().cloned());
    let mut content = out.join("\n");
    if ends_nl && !out.is_empty() {
        content.push('\n');
    }
    Ok((content, added, removed))
}

fn plan(patch: &str, root: &Path) -> std::result::Result<Vec<Action>, String> {
    let files = parse_patch(patch)?;
    let mut seen = std::collections::HashSet::new();
    let mut actions = Vec::new();
    for f in &files {
        let rel = f
            .new
            .clone()
            .or(f.old.clone())
            .expect("checked while parsing");
        if !seen.insert(rel.clone()) {
            return Err(format!("`{rel}` appears more than once in the patch"));
        }
        if f.old.is_some() && f.new.is_some() && f.old != f.new {
            return Err(format!(
                "`{}` → `{}`: a different old and new path is a rename, which is not supported",
                f.old.as_deref().unwrap_or(""),
                f.new.as_deref().unwrap_or("")
            ));
        }
        let path = resolve(root, &rel)?;
        let exists = path.exists();
        match (&f.old, &f.new) {
            (None, Some(_)) => {
                if exists {
                    return Err(format!(
                        "`{rel}` is created by the patch but already exists"
                    ));
                }
                if f.hunks.iter().any(|h| h.old_count != 0) {
                    return Err(format!("`{rel}`: a created file cannot have old lines"));
                }
                let (content, added, _) = apply_hunks(&rel, "", &f.hunks)?;
                actions.push(Action::Write {
                    path,
                    content: content.into_bytes(),
                    created: true,
                    summary: format!("{rel}: created (+{added})"),
                });
            }
            (Some(_), new) => {
                if !exists {
                    return Err(format!("`{rel}` does not exist"));
                }
                let bytes = std::fs::read(&path).map_err(|e| format!("`{rel}`: {e}"))?;
                let original = String::from_utf8(bytes).map_err(|_| {
                    format!("`{rel}` is not UTF-8 text; binary files are not patched")
                })?;
                let (content, added, removed) = apply_hunks(&rel, &original, &f.hunks)?;
                if new.is_none() {
                    if !content.is_empty() {
                        return Err(format!(
                            "`{rel}` is deleted by the patch, but its hunks do not remove every line"
                        ));
                    }
                    actions.push(Action::Delete {
                        path,
                        summary: format!("{rel}: deleted (−{removed})"),
                    });
                } else {
                    actions.push(Action::Write {
                        path,
                        content: content.into_bytes(),
                        created: false,
                        summary: format!(
                            "{rel}: modified ({} hunk(s), +{added} −{removed})",
                            f.hunks.len()
                        ),
                    });
                }
            }
            (None, None) => unreachable!("rejected while parsing"),
        }
    }
    Ok(actions)
}

/// Apply a unified diff. Returns one line per changed file.
pub fn apply_unified_patch(
    patch: &str,
    working_dir: &Path,
) -> std::result::Result<Vec<String>, String> {
    let actions = plan(patch, working_dir)?;
    // Write phase, with what is needed to undo each step.
    let mut done: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    let mut summaries = Vec::new();
    for a in &actions {
        let res = match a {
            Action::Write {
                path,
                content,
                created,
                summary,
            } => {
                let before = if *created {
                    None
                } else {
                    std::fs::read(path).ok()
                };
                let r = (|| -> std::io::Result<()> {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    write_atomic_sync(path, content)
                })();
                r.map(|_| {
                    done.push((path.clone(), before));
                    summaries.push(summary.clone());
                })
            }
            Action::Delete { path, summary } => {
                let before = std::fs::read(path).ok();
                std::fs::remove_file(path).map(|_| {
                    done.push((path.clone(), before));
                    summaries.push(summary.clone());
                })
            }
        };
        if let Err(e) = res {
            let mut failed_restores = Vec::new();
            for (p, before) in done.iter().rev() {
                let r = match before {
                    Some(b) => write_atomic_sync(p, b),
                    None => std::fs::remove_file(p),
                };
                if r.is_err() {
                    failed_restores.push(p.display().to_string());
                }
            }
            return Err(if failed_restores.is_empty() {
                format!(
                    "writing failed ({e}); the {} file(s) already written were restored, so no file is changed",
                    done.len()
                )
            } else {
                format!(
                    "writing failed ({e}); these files could NOT be restored and are left changed: {}",
                    failed_restores.join(", ")
                )
            });
        }
    }
    Ok(summaries)
}

fn write_atomic_sync(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.bricks-{}.tmp", std::process::id()));
    let perms = std::fs::metadata(path).ok().map(|m| m.permissions());
    std::fs::write(&tmp, content)?;
    if let Some(p) = perms {
        let _ = std::fs::set_permissions(&tmp, p);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (n, c) in files {
            let p = d.path().join(n);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        d
    }

    fn read(d: &tempfile::TempDir, n: &str) -> String {
        std::fs::read_to_string(d.path().join(n)).unwrap()
    }

    #[test]
    fn multiple_hunks_with_offset_and_verified_context() {
        let d = dir_with(&[("f.txt", "a\nb\nc\nd\ne\nf\n")]);
        let patch =
            "--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,4 @@\n a\n+a2\n b\n c\n@@ -5,2 +6,1 @@\n-e\n f\n";
        let out = apply_unified_patch(patch, d.path()).unwrap();
        assert_eq!(out, vec!["f.txt: modified (2 hunk(s), +1 −1)"]);
        assert_eq!(read(&d, "f.txt"), "a\na2\nb\nc\nd\nf\n");
    }

    #[test]
    fn a_hunk_whose_context_does_not_match_changes_nothing() {
        let d = dir_with(&[("f.txt", "a\nb\nc\n"), ("g.txt", "1\n")]);
        let patch = "--- a/g.txt\n+++ b/g.txt\n@@ -1 +1 @@\n-1\n+2\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,2 @@\n a\n-B\n+x\n";
        let err = apply_unified_patch(patch, d.path()).unwrap_err();
        assert!(
            err.contains("hunk 1") && err.contains("does not match"),
            "{err}"
        );
        assert_eq!(
            read(&d, "g.txt"),
            "1\n",
            "nothing is written when any hunk fails"
        );
    }

    #[test]
    fn counts_are_checked() {
        let d = dir_with(&[("f.txt", "a\nb\n")]);
        let err = apply_unified_patch(
            "--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+c\n",
            d.path(),
        )
        .unwrap_err();
        assert!(err.contains("ends early"), "{err}");
        let err = apply_unified_patch("--- f.txt\n@@ -1 +1 @@\n-x\n+y\n", d.path()).unwrap_err();
        assert!(err.contains("followed by a `+++ ` line"), "{err}");
    }

    #[test]
    fn creation_and_deletion_through_dev_null() {
        let d = dir_with(&[("old.txt", "x\ny\n")]);
        let patch = "--- /dev/null\n+++ b/new dir/new file.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n\
                     --- a/old.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n";
        let out = apply_unified_patch(patch, d.path()).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(read(&d, "new dir/new file.txt"), "hello\nworld\n");
        assert!(!d.path().join("old.txt").exists());
        assert!(!d.path().join("dev/null").exists() && !d.path().join("null").exists());
        // Creating an existing file is refused.
        let err = apply_unified_patch(
            "--- /dev/null\n+++ b/new dir/new file.txt\n@@ -0,0 +1 @@\n+z\n",
            d.path(),
        )
        .unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn timestamps_spaces_and_quoted_paths() {
        let d = dir_with(&[("my file.txt", "a\n"), ("q\"x.txt", "b\n")]);
        let patch = "--- my file.txt\t2026-10-05 10:00:00.000000000 +0200\n+++ my file.txt\t2026-10-05 10:01:00.000000000 +0200\n@@ -1 +1 @@\n-a\n+A\n\
                     diff --git \"a/q\\\"x.txt\" \"b/q\\\"x.txt\"\n--- \"a/q\\\"x.txt\"\n+++ \"b/q\\\"x.txt\"\n@@ -1 +1 @@\n-b\n+B\n";
        apply_unified_patch(patch, d.path()).unwrap();
        assert_eq!(read(&d, "my file.txt"), "A\n");
        assert_eq!(read(&d, "q\"x.txt"), "B\n");
    }

    #[test]
    fn missing_final_newline_both_ways() {
        let d = dir_with(&[("f.txt", "a\nb")]);
        let patch =
            "--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n";
        apply_unified_patch(patch, d.path()).unwrap();
        assert_eq!(read(&d, "f.txt"), "a\nc\n");
        let patch =
            "--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,2 @@\n a\n-c\n+d\n\\ No newline at end of file\n";
        apply_unified_patch(patch, d.path()).unwrap();
        assert_eq!(read(&d, "f.txt"), "a\nd");
    }

    #[test]
    fn crlf_files_stay_crlf() {
        let d = dir_with(&[("w.txt", "one\r\ntwo\r\n")]);
        apply_unified_patch(
            "--- a/w.txt\n+++ b/w.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+deux\n",
            d.path(),
        )
        .unwrap();
        assert_eq!(read(&d, "w.txt"), "one\r\ndeux\r\n");
    }

    #[test]
    fn dangerous_paths_are_refused() {
        let d = dir_with(&[("in.txt", "a\n")]);
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("t.txt"), "a\n").unwrap();
        for p in ["../t.txt", "/etc/passwd", "x/../../t.txt"] {
            let patch = format!("--- a/{p}\n+++ b/{p}\n@@ -1 +1 @@\n-a\n+b\n");
            let patch = patch.replace("a//", "/").replace("b//", "/");
            let err = apply_unified_patch(&patch, d.path()).unwrap_err();
            assert!(
                err.contains("leaves") || err.contains("absolute"),
                "{p}: {err}"
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
            let err = apply_unified_patch(
                "--- a/link/t.txt\n+++ b/link/t.txt\n@@ -1 +1 @@\n-a\n+b\n",
                d.path(),
            )
            .unwrap_err();
            assert!(err.contains("symbolic link"), "{err}");
            assert_eq!(
                std::fs::read_to_string(outside.path().join("t.txt")).unwrap(),
                "a\n"
            );
        }
    }

    #[test]
    fn unsupported_formats_are_refused_explicitly() {
        let d = dir_with(&[("a.bin", "x")]);
        for (patch, word) in [
            (
                "diff --git a/a.bin b/a.bin\nBinary files a/a.bin and b/a.bin differ\n",
                "binary",
            ),
            (
                "diff --git a/x b/x\nGIT binary patch\nliteral 3\n",
                "binary",
            ),
            (
                "diff --git a/x b/y\nsimilarity index 90%\nrename from x\nrename to y\n",
                "renames",
            ),
            (
                "diff --git a/x b/x\nold mode 100644\nnew mode 100755\n",
                "mode",
            ),
        ] {
            let err = apply_unified_patch(patch, d.path()).unwrap_err();
            assert!(err.contains(word), "{err}");
        }
    }

    #[test]
    fn targets_match_what_is_written() {
        let patch = "--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-a\n+b\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+n\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-g\n";
        assert_eq!(patch_targets(patch), vec!["x.rs", "new.rs", "gone.rs"]);
    }
}
