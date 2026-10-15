//! Git operations against real repositories in temporary folders.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::ops;
use crate::contract::{GitArea, GitEntry, GitStatus, IpcErrorCode};

fn git_raw(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .expect("git is installed")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_raw(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn repo() -> TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    git(dir.path(), &["init", "-q"]);
    dir
}

fn write(dir: &Path, rel: &str, bytes: impl AsRef<[u8]>) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create dirs");
    }
    std::fs::write(path, bytes).expect("write file");
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--no-verify", "-m", message]);
}

fn area_name(area: GitArea) -> &'static str {
    match area {
        GitArea::Staged => "staged",
        GitArea::Unstaged => "unstaged",
        GitArea::Untracked => "untracked",
        GitArea::Conflicted => "conflicted",
    }
}

fn find<'a>(status: &'a GitStatus, area: &str, path: &str) -> &'a GitEntry {
    status
        .entries
        .iter()
        .find(|e| area_name(e.area) == area && e.path == path)
        .unwrap_or_else(|| panic!("no {area} entry for {path}: {:#?}", status.entries))
}

#[tokio::test]
async fn rename_is_staged_with_original_path_and_counts() {
    let dir = repo();
    let d = dir.path();
    write(d, "a.txt", "one\ntwo\nthree\n");
    commit_all(d, "init");
    git(d, &["mv", "a.txt", "b.txt"]);

    let status = ops::status(d).await.unwrap();
    let staged = find(&status, "staged", "b.txt");
    assert_eq!(staged.code, "R");
    assert_eq!(staged.orig_path.as_deref(), Some("a.txt"));
    assert_eq!((staged.added, staged.removed, staged.binary), (Some(0), Some(0), false));

    // Edit the renamed file: the worktree change is listed on its own.
    write(d, "b.txt", "one\ntwo\nthree\nfour\n");
    let status = ops::status(d).await.unwrap();
    let unstaged = find(&status, "unstaged", "b.txt");
    assert_eq!(unstaged.code, "M");
    assert_eq!((unstaged.added, unstaged.removed), (Some(1), Some(0)));

    // The staged rename diffs as a rename, not as a new file.
    let diff = ops::diff(d, "b.txt", GitArea::Staged).await.unwrap();
    assert!(diff.patch.contains("rename from a.txt"), "{}", diff.patch);
    assert!(diff.patch.contains("rename to b.txt"), "{}", diff.patch);
    assert!(!diff.truncated);
}

#[tokio::test]
async fn paths_with_spaces_and_unicode_round_trip() {
    let dir = repo();
    let d = dir.path();
    write(d, "dossier é/nom avec espaces.txt", "hello\n");
    commit_all(d, "init");
    write(d, "dossier é/nom avec espaces.txt", "hello\nworld\n");
    write(d, "nouveau fichier é.txt", "neuf\n");

    let status = ops::status(d).await.unwrap();
    let modified = find(&status, "unstaged", "dossier é/nom avec espaces.txt");
    assert_eq!((modified.added, modified.removed), (Some(1), Some(0)));
    let untracked = find(&status, "untracked", "nouveau fichier é.txt");
    assert_eq!(untracked.code, "?");
    assert_eq!((untracked.added, untracked.removed), (Some(1), Some(0)));

    let diff = ops::diff(d, "dossier é/nom avec espaces.txt", GitArea::Unstaged)
        .await
        .unwrap();
    assert!(diff.patch.contains("+world"), "{}", diff.patch);
}

#[tokio::test]
async fn binary_files_have_no_counts() {
    let dir = repo();
    let d = dir.path();
    write(d, "data.bin", b"\0\x01\x02 v1");
    commit_all(d, "init");
    write(d, "data.bin", b"\0\x01\x02 v2, changed");
    write(d, "new.bin", [0u8, 1, 2, 3]);

    let status = ops::status(d).await.unwrap();
    let tracked = find(&status, "unstaged", "data.bin");
    assert!(tracked.binary);
    assert_eq!((tracked.added, tracked.removed), (None, None));
    let added = find(&status, "untracked", "new.bin");
    assert!(added.binary);
    assert_eq!(added.added, None);

    let diff = ops::diff(d, "data.bin", GitArea::Unstaged).await.unwrap();
    assert!(diff.binary, "{}", diff.patch);
    let diff = ops::diff(d, "new.bin", GitArea::Untracked).await.unwrap();
    assert!(diff.binary, "{}", diff.patch);
}

#[tokio::test]
async fn untracked_file_is_shown_as_whole_file_added() {
    let dir = repo();
    let d = dir.path();
    write(d, "notes.txt", "alpha\nbeta\n");

    let status = ops::status(d).await.unwrap();
    let entry = find(&status, "untracked", "notes.txt");
    assert_eq!((entry.added, entry.removed), (Some(2), Some(0)));

    let diff = ops::diff(d, "notes.txt", GitArea::Untracked).await.unwrap();
    assert!(diff.patch.contains("new file mode"), "{}", diff.patch);
    assert!(diff.patch.contains("+alpha"), "{}", diff.patch);
    assert!(diff.patch.contains("+beta"), "{}", diff.patch);
    assert!(!diff.binary);
}

#[tokio::test]
async fn unborn_branch_has_status_but_no_log() {
    let dir = repo();
    let d = dir.path();
    write(d, "first.txt", "x\n");
    git(d, &["add", "first.txt"]);

    let status = ops::status(d).await.unwrap();
    assert!(status.is_repo);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.head, None);
    find(&status, "staged", "first.txt");

    assert!(ops::log(d, 10, 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn not_a_repository_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let status = ops::status(dir.path()).await.unwrap();
    assert!(!status.is_repo);
    assert!(status.entries.is_empty());
    assert!(ops::log(dir.path(), 10, 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn detached_head_is_reported() {
    let dir = repo();
    let d = dir.path();
    write(d, "a.txt", "1\n");
    commit_all(d, "first");
    write(d, "a.txt", "2\n");
    commit_all(d, "second");
    let first = git(d, &["rev-parse", "HEAD~1"]).trim().to_string();
    git(d, &["checkout", "-q", "--detach", &first]);

    let status = ops::status(d).await.unwrap();
    assert!(status.detached);
    assert_eq!(status.branch, None);
    assert_eq!(status.head.as_deref(), Some(&first[..7]));
}

#[tokio::test]
async fn oversized_patch_is_truncated_on_a_line_boundary() {
    let dir = repo();
    let d = dir.path();
    let big: String = (0..60_000)
        .map(|i| format!("line number {i} with some padding text\n"))
        .collect();
    write(d, "big.txt", &big);

    let diff = ops::diff(d, "big.txt", GitArea::Untracked).await.unwrap();
    assert!(diff.truncated);
    assert!(diff.patch.len() <= 512 * 1024, "{} bytes", diff.patch.len());
    assert!(diff.patch.ends_with('\n'));
    assert!(diff.patch.starts_with("diff --git"));
}

#[tokio::test]
async fn log_is_paginated_newest_first() {
    let dir = repo();
    let d = dir.path();
    for i in 1..=5 {
        write(d, "f.txt", format!("v{i}\n"));
        commit_all(d, &format!("commit {i}"));
    }

    let subjects = |commits: Vec<crate::contract::GitCommit>| -> Vec<String> {
        commits.into_iter().map(|c| c.subject).collect()
    };
    assert_eq!(
        subjects(ops::log(d, 2, 0).await.unwrap()),
        vec!["commit 5", "commit 4"]
    );
    assert_eq!(
        subjects(ops::log(d, 2, 2).await.unwrap()),
        vec!["commit 3", "commit 2"]
    );
    assert_eq!(subjects(ops::log(d, 2, 4).await.unwrap()), vec!["commit 1"]);
    assert_eq!(ops::log(d, 0, 0).await.unwrap().len(), 0);

    let commit = ops::log(d, 1, 0).await.unwrap().remove(0);
    assert_eq!(commit.author, "Test");
    assert!(commit.at > 0);
    assert_eq!(commit.short.len(), 7);
    assert_eq!(commit.hash.len(), 40);
}

#[tokio::test]
async fn show_lists_one_patch_per_file() {
    let dir = repo();
    let d = dir.path();
    write(d, "a.txt", "a\n");
    write(d, "dir/b c.txt", "b\n");
    commit_all(d, "add files");

    let files = ops::show(d, "HEAD").await.unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.txt", "dir/b c.txt"]);
    assert!(files.iter().all(|f| f.patch.starts_with("diff --git")));
    assert!(files[1].patch.contains("+b"));

    git(d, &["mv", "a.txt", "renamed.txt"]);
    commit_all(d, "rename");
    let files = ops::show(d, "HEAD").await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "renamed.txt");
    assert!(files[0].patch.contains("rename from a.txt"), "{}", files[0].patch);
}

#[tokio::test]
async fn show_rejects_option_like_and_unknown_revisions() {
    let dir = repo();
    let d = dir.path();
    write(d, "a.txt", "a\n");
    commit_all(d, "init");

    let err = ops::show(d, "--output=/tmp/x").await.unwrap_err();
    assert_eq!(err.code, IpcErrorCode::Invalid);
    let err = ops::show(d, "no-such-rev").await.unwrap_err();
    assert_eq!(err.code, IpcErrorCode::NotFound);
}

#[tokio::test]
async fn project_inside_a_repository_sees_only_its_folder() {
    let dir = repo();
    let d = dir.path();
    write(d, "app/src/main.txt", "1\n");
    write(d, "other/x.txt", "1\n");
    commit_all(d, "initial");
    write(d, "other/y.txt", "1\n");
    commit_all(d, "other only");
    write(d, "app/src/main.txt", "2\n");
    write(d, "other/x.txt", "2\n");
    write(d, "root.txt", "r\n");

    let project = d.join("app");
    let status = ops::status(&project).await.unwrap();
    assert!(status.is_repo);
    assert_eq!(status.entries.len(), 1, "{:#?}", status.entries);
    let entry = find(&status, "unstaged", "src/main.txt");
    assert_eq!((entry.added, entry.removed), (Some(1), Some(1)));

    let diff = ops::diff(&project, "src/main.txt", GitArea::Unstaged).await.unwrap();
    assert!(diff.patch.contains("+2"), "{}", diff.patch);

    let log = ops::log(&project, 10, 0).await.unwrap();
    let subjects: Vec<&str> = log.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, vec!["initial"]);
}

#[tokio::test]
async fn merge_conflict_is_reported_as_conflicted() {
    let dir = repo();
    let d = dir.path();
    write(d, "a.txt", "base\n");
    commit_all(d, "base");
    git(d, &["checkout", "-q", "-b", "side"]);
    write(d, "a.txt", "side\n");
    commit_all(d, "side");
    git(d, &["checkout", "-q", "main"]);
    write(d, "a.txt", "main\n");
    commit_all(d, "main");
    let merge = git_raw(d, &["merge", "--no-edit", "side"]);
    assert!(!merge.status.success(), "the merge was expected to conflict");

    let status = ops::status(d).await.unwrap();
    let entry = find(&status, "conflicted", "a.txt");
    assert_eq!(entry.code, "U");
    assert_eq!((entry.added, entry.removed), (None, None));
}

#[tokio::test]
async fn paths_outside_the_project_are_rejected() {
    let dir = repo();
    let d = dir.path();
    write(d, "a.txt", "a\n");
    for bad in ["../secret.txt", "/etc/passwd", "", "./a.txt", "a/../../b"] {
        let err = ops::diff(d, bad, GitArea::Unstaged).await.unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Invalid, "accepted {bad:?}");
    }
}

#[tokio::test]
async fn missing_project_folder_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("removed");
    let err = ops::status(&gone).await.unwrap_err();
    assert_eq!(err.code, IpcErrorCode::FolderMissing);
}
