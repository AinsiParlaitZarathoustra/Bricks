//! What an agent's requests describe of their environment: the date, the
//! system, the shell, the git state, the model and the plan. Read for each
//! request (the git state when it may have changed), bounded, never fatal.

use crate::system_prompt::{GitSnapshot, ModelIdentity, RequestContext};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// Programs worth knowing about, looked up on the `PATH` Bash runs with.
const EXECUTABLES: &[&str] = &[
    "git", "rg", "node", "npm", "python3", "cargo", "go", "java", "docker", "make",
];
/// Lines of `git status` kept.
const GIT_STATUS_LINES: usize = 20;
/// Commits of `git log` kept.
const GIT_COMMITS: usize = 5;
/// One git command at most this long.
const GIT_TIMEOUT: Duration = Duration::from_secs(3);

/// A fact of this process, looked up once for every agent: it does not change
/// while it runs.
static TIME_ZONE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// A directory of `PATH` and when its entries last changed (`None`: unreadable).
type SearchedDir = (PathBuf, Option<SystemTime>);

/// An agent's git state, read again when the workspace may have changed.
#[derive(Default)]
pub(crate) struct RequestEnv {
    git: parking_lot::Mutex<Option<GitSnapshot>>,
    /// The git state must be read again before the next request.
    git_stale: AtomicBool,
    git_read: AtomicBool,
    /// The programs found, and the directories they were looked up in.
    executables: parking_lot::Mutex<Option<(Vec<SearchedDir>, Vec<String>)>>,
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

    /// The programs found on `path` from `cwd`. Each call costs one `stat`
    /// per directory; they are looked up again only when a directory changed
    /// (another `PATH`, another `cwd` for a relative entry, or a program added
    /// to or removed from one of them).
    pub(crate) fn executables(&self, path: Option<&OsStr>, cwd: &Path) -> Vec<String> {
        // Read before the lookup: a change during it is seen by the next call.
        let dirs: Vec<SearchedDir> = search_dirs(path, cwd)
            .into_iter()
            .map(|dir| {
                let modified = std::fs::metadata(&dir).and_then(|m| m.modified()).ok();
                (dir, modified)
            })
            .collect();
        let mut cache = self.executables.lock();
        if let Some((seen, found)) = cache.as_ref()
            && *seen == dirs
        {
            return found.clone();
        }
        let found: Vec<String> = EXECUTABLES
            .iter()
            .filter(|e| {
                dirs.iter().any(|(dir, _)| {
                    // A name with a directory: `which` looks at that file only.
                    which::which_in(dir.join(e), None::<&OsStr>, cwd).is_ok()
                })
            })
            .map(|e| e.to_string())
            .collect();
        *cache = Some((dirs, found.clone()));
        found
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
    path: Option<OsString>,
    path_cwd: &Path,
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
        executables: env.executables(path.as_deref(), path_cwd),
        model: Some(model),
        plan,
    }
}

/// The directories of `path` as a shell in `cwd` searches them: a relative
/// entry (an empty one is `cwd` itself) from `cwd`, `~` as the home directory.
fn search_dirs(path: Option<&OsStr>, cwd: &Path) -> Vec<PathBuf> {
    let Some(path) = path else {
        return Vec::new();
    };
    std::env::split_paths(path)
        .map(|entry| match entry.strip_prefix("~") {
            Ok(rest) => match dirs::home_dir() {
                Some(home) => home.join(rest),
                None => entry,
            },
            Err(_) => cwd.join(entry),
        })
        .collect()
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

    /// A fake `cargo` in a temporary directory.
    #[cfg(unix)]
    fn fake_cargo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cargo");
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        make_executable(&file);
        dir
    }

    #[cfg(unix)]
    fn make_executable(file: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn cargo() -> Vec<String> {
        vec!["cargo".to_string()]
    }

    /// The list follows the `PATH` it is given, also after a first lookup.
    #[cfg(unix)]
    #[test]
    fn the_executables_follow_the_path() {
        let dir = fake_cargo();
        let empty = tempfile::tempdir().unwrap();
        let env = RequestEnv::default();
        let cwd = empty.path();
        let with = Some(dir.path().as_os_str());
        let without = Some(empty.path().as_os_str());
        assert!(env.executables(without, cwd).is_empty());
        assert_eq!(env.executables(with, cwd), cargo());
        assert_eq!(env.executables(with, cwd), cargo());
        assert!(env.executables(without, cwd).is_empty());
        assert!(env.executables(None, cwd).is_empty());
    }

    /// A program installed or removed in a directory already on `PATH`.
    #[cfg(unix)]
    #[test]
    fn a_program_added_or_removed_in_a_known_directory_is_seen() {
        let dir = fake_cargo();
        let file = dir.path().join("cargo");
        let path = Some(dir.path().as_os_str());
        let env = RequestEnv::default();
        assert_eq!(env.executables(path, dir.path()), cargo());
        // Past the resolution of a coarse file system clock.
        std::thread::sleep(Duration::from_millis(50));
        std::fs::remove_file(&file).unwrap();
        assert!(env.executables(path, dir.path()).is_empty());
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        make_executable(&file);
        assert_eq!(env.executables(path, dir.path()), cargo());
    }

    /// A directory that appears after the first lookup.
    #[cfg(unix)]
    #[test]
    fn a_directory_created_later_is_seen() {
        let base = tempfile::tempdir().unwrap();
        let later = base.path().join("later");
        let env = RequestEnv::default();
        assert!(
            env.executables(Some(later.as_os_str()), base.path())
                .is_empty()
        );
        std::fs::create_dir(&later).unwrap();
        std::fs::write(later.join("cargo"), "#!/bin/sh\n").unwrap();
        make_executable(&later.join("cargo"));
        assert_eq!(
            env.executables(Some(later.as_os_str()), base.path()),
            cargo()
        );
    }

    /// With the directories unchanged, nothing is looked up: the program
    /// slipped in here, under the directory's previous date, stays unseen.
    #[cfg(unix)]
    #[test]
    fn unchanged_directories_reuse_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = Some(dir.path().as_os_str());
        let date = std::fs::metadata(dir.path()).unwrap().modified().unwrap();
        let env = RequestEnv::default();
        assert!(env.executables(path, dir.path()).is_empty());
        let file = dir.path().join("cargo");
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        make_executable(&file);
        std::fs::File::open(dir.path())
            .unwrap()
            .set_modified(date)
            .unwrap();
        assert!(env.executables(path, dir.path()).is_empty());
    }

    /// A relative entry is searched from the directory given, never from the
    /// process's, and the list follows that directory.
    #[cfg(unix)]
    #[test]
    fn a_relative_entry_follows_the_directory_of_the_session() {
        let base = tempfile::tempdir().unwrap();
        let project = base.path().join("project");
        std::fs::create_dir_all(project.join("rel")).unwrap();
        std::fs::write(project.join("rel/cargo"), "#!/bin/sh\n").unwrap();
        make_executable(&project.join("rel/cargo"));
        let path = Some(OsStr::new("rel"));
        let env = RequestEnv::default();
        assert_eq!(env.executables(path, &project), cargo());
        assert!(env.executables(path, base.path()).is_empty());
        assert_eq!(env.executables(path, &project), cargo());
    }

    /// An empty entry of `PATH` is the current directory, as in a shell.
    #[cfg(unix)]
    #[test]
    fn an_empty_entry_is_the_directory_of_the_session() {
        let dir = fake_cargo();
        let other = tempfile::tempdir().unwrap();
        let path = Some(OsStr::new("/nonexistent-bricks:"));
        let env = RequestEnv::default();
        assert_eq!(env.executables(path, dir.path()), cargo());
        assert!(env.executables(path, other.path()).is_empty());
    }

    /// Absolute entries do not depend on the directory: changing it keeps
    /// the list, without a new lookup.
    #[cfg(unix)]
    #[test]
    fn absolute_entries_do_not_depend_on_the_directory() {
        let dir = fake_cargo();
        let other = tempfile::tempdir().unwrap();
        let path = Some(dir.path().as_os_str());
        assert_eq!(
            search_dirs(path, other.path()),
            search_dirs(path, dir.path())
        );
        let env = RequestEnv::default();
        assert_eq!(env.executables(path, other.path()), cargo());
        assert_eq!(env.executables(path, dir.path()), cargo());
    }

    /// A directory holding a `:` cannot be written in a `PATH`, but a
    /// relative entry may lead there.
    #[cfg(unix)]
    #[test]
    fn a_relative_entry_works_from_a_directory_with_a_separator() {
        let base = tempfile::tempdir().unwrap();
        let cwd = base.path().join("a:b");
        std::fs::create_dir_all(cwd.join("rel")).unwrap();
        std::fs::write(cwd.join("rel/cargo"), "#!/bin/sh\n").unwrap();
        make_executable(&cwd.join("rel/cargo"));
        let env = RequestEnv::default();
        assert_eq!(env.executables(Some(OsStr::new("rel")), &cwd), cargo());
    }

    #[test]
    fn a_tilde_entry_is_the_home_directory() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let dirs = search_dirs(Some(OsStr::new("~/bin")), Path::new("/elsewhere"));
        assert_eq!(dirs, vec![home.join("bin")]);
    }

    /// Two agents with different `PATH`s in one process do not share a list.
    #[cfg(unix)]
    #[test]
    fn the_executables_are_not_shared_between_agents() {
        let dir = fake_cargo();
        let empty = tempfile::tempdir().unwrap();
        let first = RequestEnv::default();
        let second = RequestEnv::default();
        let found = first.executables(Some(dir.path().as_os_str()), dir.path());
        assert_eq!(found, cargo());
        assert!(
            second
                .executables(Some(empty.path().as_os_str()), dir.path())
                .is_empty()
        );
    }

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
