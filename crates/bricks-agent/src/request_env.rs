//! What an agent's requests describe of their environment: the date, the
//! system, the shell, the git state, the model and the plan. Read for each
//! request (the git state when it may have changed), bounded, never fatal.

use crate::system_prompt::{GitSnapshot, ModelIdentity, RequestContext};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Programs worth knowing about, looked up once on `PATH`.
const EXECUTABLES: &[&str] = &[
    "git", "rg", "node", "npm", "python3", "cargo", "go", "java", "docker", "make",
];
/// Lines of `git status` kept.
const GIT_STATUS_LINES: usize = 20;
/// Commits of `git log` kept.
const GIT_COMMITS: usize = 5;
/// One git command at most this long.
const GIT_TIMEOUT: Duration = Duration::from_secs(3);

/// Facts of this process, looked up once for every agent: they do not change
/// while it runs.
static EXECUTABLES_FOUND: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
static TIME_ZONE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// An agent's git state, read again when the workspace may have changed.
#[derive(Default)]
pub(crate) struct RequestEnv {
    git: parking_lot::Mutex<Option<GitSnapshot>>,
    /// The git state must be read again before the next request.
    git_stale: AtomicBool,
    git_read: AtomicBool,
}

impl RequestEnv {
    /// A user request starts, or the workspace may have changed: read git
    /// again before the next request.
    pub(crate) fn mark_git_stale(&self) {
        self.git_stale.store(true, Ordering::SeqCst);
    }

    /// The git state, read again when stale (or never read).
    pub(crate) async fn git(&self, dir: &Path) -> Option<GitSnapshot> {
        let first = !self.git_read.swap(true, Ordering::SeqCst);
        if first || self.git_stale.swap(false, Ordering::SeqCst) {
            let snap = read_git(dir).await;
            *self.git.lock() = snap;
        }
        self.git.lock().clone()
    }

    pub(crate) fn executables(&self) -> Vec<String> {
        EXECUTABLES_FOUND
            .get_or_init(|| {
                EXECUTABLES
                    .iter()
                    .filter(|e| which::which(e).is_ok())
                    .map(|e| e.to_string())
                    .collect()
            })
            .clone()
    }

    pub(crate) fn time_zone(&self) -> Option<String> {
        TIME_ZONE.get_or_init(local_time_zone).clone()
    }
}

/// The request context from its parts (everything else known locally).
#[allow(clippy::too_many_arguments)]
pub(crate) fn context(
    env: &RequestEnv,
    tools_dir: &Path,
    shell: Option<(Option<std::path::PathBuf>, bool)>,
    shell_dir: Option<std::path::PathBuf>,
    model: ModelIdentity,
    plan: Option<bricks_tools::todo_write::PlanState>,
) -> RequestContext {
    RequestContext {
        now: Some(chrono::Local::now().fixed_offset()),
        time_zone: env.time_zone(),
        os: Some(format!(
            "{} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        shell: shell
            .as_ref()
            .and_then(|(p, _)| p.as_ref())
            .map(|p| p.display().to_string()),
        shell_persistent: shell.as_ref().map(|(_, persistent)| *persistent),
        tools_directory: Some(tools_dir.display().to_string()),
        shell_directory: shell_dir
            .filter(|d| d != tools_dir)
            .map(|d| d.display().to_string()),
        executables: env.executables(),
        model: Some(model),
        plan,
    }
}

/// `TZ` when it names a zone, else the target of `/etc/localtime`.
fn local_time_zone() -> Option<String> {
    let named = |z: &str| {
        let z = z.trim().trim_start_matches(':');
        (z.contains('/') && !z.starts_with('/')).then(|| z.to_string())
    };
    if let Ok(tz) = std::env::var("TZ")
        && let Some(z) = named(&tz)
    {
        return Some(z);
    }
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let text = target.to_string_lossy();
    let (_, zone) = text.split_once("zoneinfo/")?;
    named(zone)
}

/// One git command in `dir`, without a shell: its output, or `None`.
async fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The repository's branch, status and last commits (`None`: not a
/// repository, or git unavailable). Two commands, run side by side.
async fn read_git(dir: &Path) -> Option<GitSnapshot> {
    // No `.git` above: not a repository, and no process is started.
    if !dir.ancestors().any(|d| d.join(".git").exists()) {
        return None;
    }
    let n = GIT_COMMITS.to_string();
    let status_args = [
        "status",
        "--porcelain=v1",
        "--branch",
        "--untracked-files=normal",
    ];
    let log_args = ["log", "--oneline", "-n", n.as_str()];
    let (status, log) = tokio::join!(git_out(dir, &status_args), git_out(dir, &log_args));
    // Outside a repository `status` fails: no git state.
    let status = status?;
    let mut lines = status.lines();
    let header = lines.next().unwrap_or_default();
    let branch = branch_of(header);
    let all: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
    let mut status_lines: Vec<String> = all
        .iter()
        .take(GIT_STATUS_LINES)
        .map(|l| l.to_string())
        .collect();
    if all.len() > GIT_STATUS_LINES {
        status_lines.push(format!("… {} more", all.len() - GIT_STATUS_LINES));
    }
    let recent_commits = log
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    Some(GitSnapshot {
        branch,
        recent_commits,
        status_lines,
        user: None,
        taken_at: Some(chrono::Local::now().fixed_offset()),
    })
}

/// The branch of a `status --branch` header (`## main...origin/main [ahead 1]`,
/// `## No commits yet on main`, `## HEAD (no branch)`).
fn branch_of(header: &str) -> String {
    let h = header.trim_start_matches("## ");
    if let Some(b) = h.strip_prefix("No commits yet on ") {
        return b.trim().to_string();
    }
    if h.starts_with("HEAD (no branch)") {
        return "(detached HEAD)".to_string();
    }
    h.split("...")
        .next()
        .unwrap_or(h)
        .split(' ')
        .next()
        .unwrap_or(h)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_is_read_from_the_status_header() {
        assert_eq!(branch_of("## main...origin/main [ahead 1]"), "main");
        assert_eq!(branch_of("## feature/x"), "feature/x");
        assert_eq!(branch_of("## No commits yet on main"), "main");
        assert_eq!(branch_of("## HEAD (no branch)"), "(detached HEAD)");
    }

    #[tokio::test]
    async fn outside_a_repository_there_is_no_git_state() {
        let dir = tempfile::tempdir().unwrap();
        let env = RequestEnv::default();
        assert!(env.git(dir.path()).await.is_none());
    }

    #[tokio::test]
    async fn the_git_state_is_read_again_only_when_stale() {
        if which::which("git").is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        let env = RequestEnv::default();
        let first = env.git(dir.path()).await.unwrap();
        assert_eq!(first.branch, "main");
        assert!(first.status_lines.is_empty());
        std::fs::write(dir.path().join("new.txt"), "x").unwrap();
        // Not stale: the cached state is kept.
        assert!(env.git(dir.path()).await.unwrap().status_lines.is_empty());
        env.mark_git_stale();
        let again = env.git(dir.path()).await.unwrap();
        assert_eq!(again.status_lines, vec!["?? new.txt".to_string()]);
        assert!(again.taken_at.is_some());
    }
}
