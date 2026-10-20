//! Git utilities: repo detection, status, diff, and history.
//!
//! Used by the system prompt builder to inject git context.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Run `git <args>` in `path`. `None` when git cannot be started.
fn run_git(path: &Path, args: &[&str]) -> Option<Output> {
    Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .ok()
}

/// Trimmed stdout of `git <args>`, when git succeeds.
fn git_stdout(path: &Path, args: &[&str]) -> Option<String> {
    run_git(path, args)
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Trimmed stdout of `git <args>` whatever its status; empty when git cannot
/// be started.
fn git_stdout_or_empty(path: &Path, args: &[&str]) -> String {
    run_git(path, args)
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Check if a path is inside a git repository.
pub fn is_git_repo(path: &Path) -> bool {
    run_git(path, &["rev-parse", "--is-inside-work-tree"]).is_some_and(|o| o.status.success())
}

/// Get the root of the git repository.
pub fn get_repo_root(path: &Path) -> Option<PathBuf> {
    git_stdout(path, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Get current branch name.
pub fn current_branch(path: &Path) -> Option<String> {
    git_stdout(path, &["rev-parse", "--abbrev-ref", "HEAD"])
}

/// Get git status (short format).
pub fn git_status(path: &Path) -> Option<String> {
    git_stdout(path, &["status", "--short"])
}

/// Get git diff (staged + unstaged).
pub fn git_diff(path: &Path) -> Option<String> {
    let staged = git_stdout_or_empty(path, &["diff", "--cached", "--stat"]);
    let unstaged = git_stdout_or_empty(path, &["diff", "--stat"]);

    let combined = format!("{}\n{}", staged, unstaged).trim().to_string();
    if combined.is_empty() {
        None
    } else {
        Some(combined)
    }
}

/// Get recent commit history (one-line format).
pub fn recent_commits(path: &Path, count: usize) -> Option<String> {
    let count = format!("-{}", count);
    git_stdout(path, &["log", "--oneline", &count])
}

/// List modified files (both staged and unstaged).
pub fn list_modified_files(path: &Path) -> Vec<String> {
    run_git(path, &["diff", "--name-only", "HEAD"])
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Build a git context string for the system prompt.
pub fn build_git_context(working_dir: &Path) -> Option<String> {
    if !is_git_repo(working_dir) {
        return None;
    }

    let mut parts = Vec::new();

    if let Some(branch) = current_branch(working_dir) {
        parts.push(format!("Current branch: {}", branch));
    }

    if let Some(status) = git_status(working_dir) {
        if !status.is_empty() {
            parts.push(format!("Status:\n{}", status));
        } else {
            parts.push("Status: (clean)".to_string());
        }
    }

    if let Some(commits) = recent_commits(working_dir, 5) {
        parts.push(format!("Recent commits:\n{}", commits));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_git_repo() {
        // The project root should be a git repo
        let cwd = std::env::current_dir().unwrap();
        // This might not be true in all test environments
        let _ = is_git_repo(&cwd); // just verify it doesn't panic
    }

    #[test]
    fn test_not_git_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(tmp.path()));
    }

    #[test]
    fn test_build_git_context_non_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(build_git_context(tmp.path()).is_none());
    }

    #[test]
    fn test_git_context_real_repo() {
        // Try on the actual project repo
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        if is_git_repo(root) {
            let ctx = build_git_context(root);
            assert!(ctx.is_some());
            let ctx = ctx.unwrap();
            assert!(ctx.contains("branch") || ctx.contains("Status"));
        }
    }

    #[test]
    fn queries_read_branch_status_log_and_changes_of_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args([
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "first"]);

        assert!(is_git_repo(dir));
        assert!(get_repo_root(dir).is_some());
        assert_eq!(current_branch(dir).as_deref(), Some("main"));
        assert_eq!(git_status(dir).as_deref(), Some(""));
        assert!(recent_commits(dir, 5).unwrap().contains("first"));
        assert!(list_modified_files(dir).is_empty());
        assert_eq!(git_diff(dir), None);

        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        assert_eq!(git_status(dir).as_deref(), Some("M a.txt"));
        assert_eq!(list_modified_files(dir), vec!["a.txt".to_string()]);
        assert!(git_diff(dir).unwrap().contains("a.txt"));

        let ctx = build_git_context(dir).unwrap();
        assert!(ctx.contains("Current branch: main"));
        assert!(ctx.contains("M a.txt"));
    }
}
