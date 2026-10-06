//! Persistent shells, supervised execution and background tasks.
//!
//! One [`ShellSessions`] per tool-context session id (an agent session; a
//! sub-agent gets its own id and therefore its own shells). It owns:
//!
//! * the persistent bash ([`bash::BashSession`], Unix) — foreground commands
//!   are serialised in it and share its state for its whole life;
//! * the persistent PowerShell ([`powershell::PwshSession`]);
//! * the background tasks ([`background::Task`]).
//!
//! State persists while the interpreter lives. When it ends (`exit`, `exec`,
//! a crash, a timeout that required destroying it), the next command starts
//! a fresh one and says so; nothing is restored behind the user's back.
//! [`close_session`] stops everything, awaitably and within bounds; dropping
//! the registry entry kills what is left (synchronous last resort).
//!
//! See `docs/shell.md` for the contracts and the per-OS guarantees.

pub mod background;
#[cfg(unix)]
pub mod bash;
pub mod capture;
pub mod clean;
pub mod powershell;
pub mod procs;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Called with progress messages while a command runs.
pub type Progress = Arc<dyn Fn(String) + Send + Sync>;

/// Installed in `ToolContext::extensions` by the agent runner to receive
/// progress of long-running tool calls: `(tool name, message)`.
#[derive(Clone)]
pub struct ProgressSink(pub Arc<ProgressSinkFn>);

/// `(tool name, message)`.
pub type ProgressSinkFn = dyn Fn(&str, &str) + Send + Sync;

/// Settings of shell execution. Put one in `ToolContext::extensions` to
/// override the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfig {
    /// `bash` to use (default: the first on `PATH`).
    pub bash_path: Option<PathBuf>,
    /// `pwsh`/`powershell` to use (default: found on `PATH`).
    pub pwsh_path: Option<PathBuf>,
    /// Foreground timeout when a call gives none.
    pub default_timeout: Duration,
    /// Upper bound of a requested timeout.
    pub max_timeout: Duration,
    /// Between the graceful and the forced signal.
    pub term_grace: Duration,
    /// How long to keep reading outputs after a command ended.
    pub drain: Duration,
    /// Interval of progress messages.
    pub progress_every: Duration,
    pub capture: capture::CaptureLimits,
    pub tasks: background::TaskLimits,
    /// Where session files (FIFOs, scripts, spilled outputs, task logs) go.
    pub base_dir: PathBuf,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            bash_path: None,
            pwsh_path: None,
            default_timeout: Duration::from_secs(120),
            max_timeout: Duration::from_secs(600),
            term_grace: Duration::from_secs(2),
            drain: Duration::from_millis(500),
            progress_every: Duration::from_secs(10),
            capture: capture::CaptureLimits::default(),
            tasks: background::TaskLimits::default(),
            base_dir: std::env::temp_dir().join("bricks-shell"),
        }
    }
}

/// The shells and tasks of one session.
pub struct ShellSessions {
    pub id: String,
    dir: PathBuf,
    #[cfg(unix)]
    bash: tokio::sync::Mutex<Option<Arc<bash::BashSession>>>,
    pwsh: tokio::sync::Mutex<Option<Arc<powershell::PwshSession>>>,
    /// Set when a shell of this session ended; reported once by the next
    /// command, which runs in a new shell.
    reset_notice: parking_lot::Mutex<Option<String>>,
    tasks: parking_lot::Mutex<Vec<Arc<background::Task>>>,
    next_task: AtomicU64,
    generation: AtomicU64,
}

static REGISTRY: once_cell::sync::Lazy<dashmap::DashMap<String, Arc<ShellSessions>>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);
static INSTANCE: AtomicU64 = AtomicU64::new(0);

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// The shells of session `id` (created empty on first use).
pub fn session(id: &str, config: &ShellConfig) -> Arc<ShellSessions> {
    REGISTRY
        .entry(id.to_string())
        .or_insert_with(|| {
            let n = INSTANCE.fetch_add(1, Ordering::SeqCst);
            Arc::new(ShellSessions {
                id: id.to_string(),
                dir: config
                    .base_dir
                    .join(format!("{}-{}-{n}", sanitize(id), std::process::id())),
                #[cfg(unix)]
                bash: tokio::sync::Mutex::new(None),
                pwsh: tokio::sync::Mutex::new(None),
                reset_notice: parking_lot::Mutex::new(None),
                tasks: parking_lot::Mutex::new(Vec::new()),
                next_task: AtomicU64::new(1),
                generation: AtomicU64::new(0),
            })
        })
        .clone()
}

/// Close the shells and tasks of session `id`: graceful, bounded, awaited.
pub async fn close_session(id: &str) {
    if let Some((_, s)) = REGISTRY.remove(id) {
        s.close().await;
    }
}

/// Synchronous last resort (drop paths): kill everything of session `id`.
pub fn close_session_now(id: &str) {
    if let Some((_, s)) = REGISTRY.remove(id) {
        s.kill_now();
    }
}

impl ShellSessions {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Take the pending reset notice, if a shell ended since the last call.
    pub fn take_reset_notice(&self) -> Option<String> {
        self.reset_notice.lock().take()
    }

    pub fn note_reset(&self, what: &str) {
        *self.reset_notice.lock() = Some(format!(
            "{what} ended before this command; this command runs in a new {what} — the previous \
             working directory, variables, functions and aliases are gone"
        ));
    }

    /// The live bash of this session, started in `cwd` if there is none.
    #[cfg(unix)]
    pub async fn bash(
        &self,
        cwd: &Path,
        config: &ShellConfig,
    ) -> std::io::Result<Arc<bash::BashSession>> {
        let mut slot = self.bash.lock().await;
        if let Some(b) = slot.as_ref() {
            if !b.is_dead() {
                return Ok(b.clone());
            }
            b.close().await;
            *slot = None;
            self.note_reset("shell");
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst);
        let b = bash::BashSession::start(
            cwd,
            config.clone(),
            self.dir.join(format!("bash-{generation}")),
        )
        .await?;
        *slot = Some(b.clone());
        Ok(b)
    }

    /// The live PowerShell of this session.
    pub async fn pwsh(
        &self,
        cwd: &Path,
        config: &ShellConfig,
    ) -> std::io::Result<Arc<powershell::PwshSession>> {
        let mut slot = self.pwsh.lock().await;
        if let Some(p) = slot.as_ref() {
            if !p.is_dead() {
                return Ok(p.clone());
            }
            p.close().await;
            *slot = None;
            self.note_reset("PowerShell session");
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst);
        let p = powershell::PwshSession::start(
            cwd,
            config.clone(),
            self.dir.join(format!("pwsh-{generation}")),
        )
        .await?;
        *slot = Some(p.clone());
        Ok(p)
    }

    /// Start a background task with the current state of the bash.
    #[cfg(unix)]
    pub async fn start_task(
        &self,
        command: &str,
        stdin: Option<&str>,
        ready_pattern: Option<regex::Regex>,
        cwd: &Path,
        config: &ShellConfig,
    ) -> std::io::Result<Arc<background::Task>> {
        let shell = self.bash(cwd, config).await?;
        let n = self.next_task.fetch_add(1, Ordering::SeqCst);
        let id = format!("bg-{n}");
        let dir = self.dir.join(format!("task-{n}"));
        std::fs::create_dir_all(&dir)?;
        let snapshot = dir.join("state.sh");
        shell.snapshot(&snapshot).await?;
        let bash = config
            .bash_path
            .clone()
            .or_else(|| which::which("bash").ok())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "bash was not found on PATH")
            })?;
        match background::Task::start(background::TaskSpec {
            id,
            command,
            snapshot: &snapshot,
            fallback_cwd: cwd,
            stdin,
            ready_pattern,
            dir: dir.clone(),
            bash: &bash,
            limits: config.tasks.clone(),
        })
        .await
        {
            Ok(task) => {
                self.tasks.lock().push(task.clone());
                Ok(task)
            }
            Err(e) => {
                // A launch that failed leaves nothing behind.
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    /// A task of this session (and only of this session).
    pub fn task(&self, id: &str) -> Option<Arc<background::Task>> {
        self.tasks.lock().iter().find(|t| t.id == id).cloned()
    }

    pub fn tasks(&self) -> Vec<Arc<background::Task>> {
        self.tasks.lock().clone()
    }

    pub async fn close(&self) {
        let tasks = self.tasks.lock().clone();
        for t in tasks {
            t.stop(Duration::from_secs(2)).await;
        }
        #[cfg(unix)]
        if let Some(b) = self.bash.lock().await.take() {
            b.close().await;
        }
        if let Some(p) = self.pwsh.lock().await.take() {
            p.close().await;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    pub fn kill_now(&self) {
        for t in self.tasks.lock().iter() {
            t.kill_now();
        }
        #[cfg(unix)]
        if let Ok(mut slot) = self.bash.try_lock() {
            if let Some(b) = slot.take() {
                b.kill_now();
            }
        }
        if let Ok(mut slot) = self.pwsh.try_lock() {
            if let Some(p) = slot.take() {
                p.kill_now();
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for ShellSessions {
    fn drop(&mut self) {
        for t in self.tasks.lock().iter() {
            t.kill_now();
        }
    }
}
