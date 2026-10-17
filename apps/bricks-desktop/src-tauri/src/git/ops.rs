//! Git operations on a project folder.
//!
//! Every path going in or out is relative to the project folder, which may
//! sit below the repository root. Commands run with the project folder as
//! their working directory and are scoped to it (`-- .`, `--relative`).

use std::collections::HashMap;
use std::path::{Component, Path};

use crate::contract::{
    FileDiff, GitArea, GitCommit, GitEntry, GitStatus, IpcError, IpcErrorCode, IpcResult,
};

use super::parse::{self, Counts};
use super::run::{self, Captured};

/// Whole changed-file listings are read up to this size.
const LIST_CAP: usize = 64 * 1024 * 1024;
/// One shown patch is cut at this size.
const PATCH_LIMIT: usize = 512 * 1024;
/// A commit's patches are read up to this size, then cut per file.
const SHOW_CAP: usize = 8 * 1024 * 1024;
const LOG_MAX: u32 = 500;
/// Untracked files get line counts up to this many files and bytes each.
const UNTRACKED_COUNT_FILES: u32 = 1000;
const UNTRACKED_COUNT_BYTES: u64 = 512 * 1024;

/// Status of the repository containing the project folder.
pub async fn status(dir: &Path) -> IpcResult<GitStatus> {
    ensure_folder(dir)?;
    let Some(prefix) = repo_prefix(dir).await? else {
        return Ok(not_a_repo());
    };

    let status_args = [
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--untracked-files=all",
        "--",
        ".",
    ];
    let staged_args = numstat_args(true);
    let unstaged_args = numstat_args(false);
    let (listing, staged, unstaged) = tokio::join!(
        run::capture(dir, &status_args, LIST_CAP),
        run::capture(dir, &staged_args, LIST_CAP),
        run::capture(dir, &unstaged_args, LIST_CAP),
    );
    let (listing, staged, unstaged) = (listing?, staged?, unstaged?);
    check_listing(&listing, "Lecture du statut Git impossible")?;
    check_listing(&staged, "Lecture des changements indexés impossible")?;
    check_listing(&unstaged, "Lecture des changements non indexés impossible")?;

    let parsed = parse::parse_status_v2(&listing.stdout);
    let staged_counts = by_path(parse::parse_numstat_z(&staged.stdout));
    let unstaged_counts = by_path(parse::parse_numstat_z(&unstaged.stdout));

    let mut entries = Vec::with_capacity(parsed.entries.len());
    let mut budget = UNTRACKED_COUNT_FILES;
    for raw in parsed.entries {
        // Status paths are repository-relative; drop the project prefix.
        let Some(path) = to_project_path(&raw.path, &prefix) else {
            continue;
        };
        let orig_path = raw
            .orig_path
            .as_deref()
            .and_then(|p| to_project_path(p, &prefix));
        let counts = match raw.area {
            GitArea::Staged => staged_counts.get(&path).copied().unwrap_or_default(),
            GitArea::Unstaged => unstaged_counts.get(&path).copied().unwrap_or_default(),
            GitArea::Untracked => untracked_counts(dir, &path, &mut budget).await,
            GitArea::Conflicted => Counts::default(),
        };
        entries.push(GitEntry {
            path,
            orig_path,
            area: raw.area,
            code: raw.code,
            added: counts.added,
            removed: counts.removed,
            binary: counts.binary,
        });
    }

    let header = parsed.header;
    Ok(GitStatus {
        is_repo: true,
        branch: header.branch,
        detached: header.detached,
        head: header.head,
        ahead: header.ahead,
        behind: header.behind,
        entries,
    })
}

/// Patch of one path. Staged: index against HEAD. Unstaged: worktree against
/// the index. Untracked: the whole file as added.
pub async fn diff(dir: &Path, path: &str, area: GitArea) -> IpcResult<FileDiff> {
    ensure_folder(dir)?;
    validate_relative_path(path)?;

    let common = [
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--relative",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    let out = match area {
        GitArea::Untracked => {
            let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
            let mut args = vec!["diff", "--no-index"];
            args.extend(common.iter().copied().filter(|a| *a != "--relative"));
            args.extend(["--", null_device, path]);
            let out = run::capture(dir, &args, PATCH_LIMIT).await?;
            // `--no-index` exits 1 when the files differ, which is the normal case.
            if !(out.success() || out.truncated || out.code() == Some(1)) {
                return Err(run::failure(&out, "Diff Git impossible"));
            }
            out
        }
        GitArea::Staged => {
            let source = staged_rename_source(dir, path).await?;
            let mut args = vec!["diff", "--cached", "-M"];
            args.extend(common);
            args.push("--");
            if let Some(source) = source.as_deref() {
                args.push(source);
            }
            args.push(path);
            let out = run::capture(dir, &args, PATCH_LIMIT).await?;
            check_exit(&out, "Diff Git impossible")?;
            out
        }
        GitArea::Unstaged | GitArea::Conflicted => {
            let mut args = vec!["diff", "-M"];
            args.extend(common);
            args.push("--");
            args.push(path);
            let out = run::capture(dir, &args, PATCH_LIMIT).await?;
            check_exit(&out, "Diff Git impossible")?;
            out
        }
    };

    let (text, truncated) = if out.truncated {
        (String::from_utf8_lossy(cut_at_line(&out.stdout)).into_owned(), true)
    } else {
        (String::from_utf8_lossy(&out.stdout).into_owned(), false)
    };
    Ok(FileDiff {
        path: path.to_string(),
        binary: parse::is_binary_patch(&text),
        patch: text,
        truncated,
    })
}

/// Commits touching the project folder, newest first, one page at a time.
pub async fn log(dir: &Path, limit: u32, skip: u32) -> IpcResult<Vec<GitCommit>> {
    ensure_folder(dir)?;
    if limit == 0 || repo_prefix(dir).await?.is_none() {
        return Ok(Vec::new());
    }
    // An unborn branch has no commits yet.
    let head = run::capture(dir, &["rev-parse", "--verify", "--quiet", "HEAD"], 4096).await?;
    if !head.success() {
        return Ok(Vec::new());
    }

    let max_count = format!("--max-count={}", limit.min(LOG_MAX));
    let offset = format!("--skip={skip}");
    let out = run::capture(
        dir,
        &[
            "log",
            &max_count,
            &offset,
            "-z",
            "--format=%H%x00%h%x00%an%x00%at%x00%s",
            "--",
            ".",
        ],
        LIST_CAP,
    )
    .await?;
    check_listing(&out, "Historique Git illisible")?;
    Ok(parse::parse_log_z(&out.stdout))
}

/// Per-file patches of one commit, limited to the project folder.
pub async fn show(dir: &Path, rev: &str) -> IpcResult<Vec<FileDiff>> {
    ensure_folder(dir)?;
    validate_rev(rev)?;
    if repo_prefix(dir).await?.is_none() {
        return Err(not_found("Ce dossier n'est pas un dépôt Git."));
    }

    let spec = format!("{rev}^{{commit}}");
    let resolved = run::capture(dir, &["rev-parse", "--verify", "--quiet", &spec], 4096).await?;
    if !resolved.success() {
        return Err(not_found(&format!("Révision introuvable : {rev}")));
    }
    let hash = String::from_utf8_lossy(&resolved.stdout).trim().to_string();

    let names_out = run::capture(
        dir,
        &[
            "show",
            "--first-parent",
            "--format=",
            "-z",
            "--name-status",
            "-M",
            "--relative",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            &hash,
            "--",
            ".",
        ],
        LIST_CAP,
    )
    .await?;
    let patch_out = run::capture(
        dir,
        &[
            "show",
            "--first-parent",
            "--format=",
            "-M",
            "--relative",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &hash,
            "--",
            ".",
        ],
        SHOW_CAP,
    )
    .await?;
    check_listing(&names_out, "Liste des fichiers du commit illisible")?;
    check_exit(&patch_out, "Diff du commit illisible")?;

    let names: Vec<String> = parse::parse_name_status_z(&names_out.stdout)
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    let patch = String::from_utf8_lossy(&patch_out.stdout).into_owned();
    let sections = parse::split_patch_sections(&patch);
    // Name-status and the patch list the same files in the same order.
    let aligned = names.len() == sections.len();

    let last = sections.len().saturating_sub(1);
    let mut files = Vec::with_capacity(sections.len());
    for (index, section) in sections.iter().enumerate() {
        let path = match names.get(index) {
            Some(name) if aligned || patch_out.truncated => name.clone(),
            _ => parse::header_path(section),
        };
        let forced = patch_out.truncated && index == last;
        let (text, truncated) = cut_section(section, forced);
        files.push(FileDiff {
            path,
            binary: parse::is_binary_patch(text),
            patch: text.to_string(),
            truncated,
        });
    }
    Ok(files)
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn numstat_args(staged: bool) -> Vec<&'static str> {
    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    args.extend([
        "--numstat",
        "-z",
        "-M",
        "--relative",
        "--no-ext-diff",
        "--no-textconv",
        "--",
        ".",
    ]);
    args
}

/// A listing must be complete: a cut listing would silently hide files.
fn check_listing(out: &Captured, what: &str) -> IpcResult<()> {
    if out.truncated {
        return Err(IpcError {
            code: IpcErrorCode::Rejected,
            message: "Trop de changements pour être affichés.".into(),
        });
    }
    check_exit(out, what)
}

/// A cut patch is accepted whatever the exit status: git was stopped on
/// purpose once the cap was reached.
fn check_exit(out: &Captured, what: &str) -> IpcResult<()> {
    if out.success() || out.truncated {
        Ok(())
    } else {
        Err(run::failure(out, what))
    }
}

fn by_path(entries: Vec<(String, Counts)>) -> HashMap<String, Counts> {
    entries.into_iter().collect()
}

fn to_project_path(path: &str, prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        Some(path.to_string())
    } else {
        path.strip_prefix(prefix).map(str::to_string)
    }
}

/// `Some(prefix)` when the folder is inside a work tree (the prefix is the
/// folder's path from the repository root, empty at the root), `None` when
/// it is not a repository.
async fn repo_prefix(dir: &Path) -> IpcResult<Option<String>> {
    let out = run::capture(
        dir,
        &["rev-parse", "--is-inside-work-tree", "--show-prefix"],
        4096,
    )
    .await?;
    if !out.success() {
        if out.stderr.contains("not a git repository") {
            return Ok(None);
        }
        return Err(run::failure(&out, "Dépôt Git inaccessible"));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines = text.lines();
    if lines.next() != Some("true") {
        return Ok(None);
    }
    Ok(Some(lines.next().unwrap_or("").to_string()))
}

/// Source path of a staged rename whose destination is `path`, if any.
/// Without it a rename would show up as a plain addition.
async fn staged_rename_source(dir: &Path, path: &str) -> IpcResult<Option<String>> {
    let out = run::capture(
        dir,
        &[
            "diff",
            "--cached",
            "--name-status",
            "-z",
            "-M",
            "--relative",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            ".",
        ],
        LIST_CAP,
    )
    .await?;
    check_listing(&out, "Diff Git impossible")?;
    Ok(parse::parse_name_status_z(&out.stdout)
        .into_iter()
        .find(|entry| entry.code == 'R' && entry.path == path)
        .and_then(|entry| entry.orig))
}

/// Line counts of an untracked file, read directly: git has no numstat for
/// files outside the index. Large or binary files get no line counts.
async fn untracked_counts(dir: &Path, path: &str, budget: &mut u32) -> Counts {
    if *budget == 0 {
        return Counts::default();
    }
    *budget -= 1;
    let full = dir.join(path);
    let Ok(meta) = tokio::fs::symlink_metadata(&full).await else {
        return Counts::default();
    };
    if !meta.is_file() || meta.len() > UNTRACKED_COUNT_BYTES {
        return Counts::default();
    }
    let Ok(bytes) = tokio::fs::read(&full).await else {
        return Counts::default();
    };
    if parse::is_binary_content(&bytes) {
        return Counts { added: None, removed: None, binary: true };
    }
    Counts {
        added: Some(parse::count_lines(&bytes)),
        removed: Some(0),
        binary: false,
    }
}

/// Cut a patch at the size limit, backing off to a line boundary.
fn cut_at_line(bytes: &[u8]) -> &[u8] {
    match bytes.iter().rposition(|b| *b == b'\n') {
        Some(index) => &bytes[..=index],
        None => bytes,
    }
}

/// Section of a multi-file patch within the per-file limit.
fn cut_section(text: &str, forced: bool) -> (&str, bool) {
    if text.len() <= PATCH_LIMIT && !forced {
        return (text, false);
    }
    let mut end = text.len().min(PATCH_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    let head = match head.rfind('\n') {
        Some(index) => &head[..=index],
        None => head,
    };
    (head, true)
}

fn validate_relative_path(path: &str) -> IpcResult<()> {
    let invalid = || IpcError {
        code: IpcErrorCode::Invalid,
        message: "Chemin invalide : il doit être relatif au projet.".into(),
    };
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err(invalid());
    }
    let p = Path::new(path);
    if p.is_absolute() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(invalid());
    }
    Ok(())
}

fn validate_rev(rev: &str) -> IpcResult<()> {
    let valid = !rev.is_empty()
        && rev.len() <= 256
        && !rev.starts_with('-')
        && !rev.chars().any(|c| c.is_whitespace() || c.is_control());
    if valid {
        Ok(())
    } else {
        Err(IpcError {
            code: IpcErrorCode::Invalid,
            message: "Révision invalide.".into(),
        })
    }
}

fn ensure_folder(dir: &Path) -> IpcResult<()> {
    if dir.is_dir() {
        Ok(())
    } else {
        Err(IpcError {
            code: IpcErrorCode::FolderMissing,
            message: "Le dossier du projet n'existe plus.".into(),
        })
    }
}

fn not_found(message: &str) -> IpcError {
    IpcError {
        code: IpcErrorCode::NotFound,
        message: message.to_string(),
    }
}

fn not_a_repo() -> GitStatus {
    GitStatus {
        is_repo: false,
        branch: None,
        detached: false,
        head: None,
        ahead: 0,
        behind: 0,
        entries: Vec::new(),
    }
}
