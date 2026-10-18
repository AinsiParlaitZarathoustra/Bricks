//! Shell execution of a session: ordinary commands, background tasks and
//! PowerShell.
//!
//! One [`ShellSessions`] per tool-context session id (an agent session; a
//! sub-agent gets its own id and therefore its own state). It owns:
//!
//! * the session's state ([`state::ShellState`]): working directory, exported
//!   variables, functions and aliases, shared by the session's commands;
//! * the ordinary commands ([`bash::Command`]): each one a separate `bash`
//!   process started from a copy of that state, running side by side (no
//!   command waits for another), with a deadline, and handed over as a task
//!   when a call stops waiting for it (`yield_time_ms`);
//! * the background tasks ([`background::Task`]): started from a copy of the
//!   state, never publishing into it;
//! * the persistent PowerShell ([`powershell::PwshSession`]), unchanged.
//!
//! State persists while the session lives; non-exported variables do not
//! (see [`state`]). [`close_session`] stops everything, awaitably and within
//! bounds; dropping the registry entry kills what is left (synchronous last
//! resort).
//!
//! See `docs/shell.md` for the contracts and the per-OS guarantees.

pub mod background;
#[cfg(unix)]
pub mod bash;
pub mod capture;
pub mod clean;
pub mod powershell;
pub mod procs;
pub mod state;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Called with progress messages while a command runs.
pub type Progress = Arc<dyn Fn(String) + Send + Sync>;

/// Installed in `ToolContext::extensions` by the agent runner to receive
/// progress of long-running tool calls: `(tool name, message)`.
#[derive(Clone)]
pub struct ProgressSink(pub Arc<ProgressSinkFn>);

/// `(tool name, message)`.
pub type ProgressSinkFn = dyn Fn(&str, &str) + Send + Sync;

/// `[bash]` in `bricks.toml`: settings of the Bash tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BashSettings {
    /// Milliseconds a foreground command runs before the call returns it as
    /// a task (`Running`, with a task id) while it goes on. A call's
    /// `yield_time_ms` overrides it; `timeout` still ends the command.
    pub yield_time_ms: u64,
}

impl Default for BashSettings {
    fn default() -> Self {
        Self {
            yield_time_ms: 10_000,
        }
    }
}

impl BashSettings {
    /// Applies these settings to a shell configuration.
    pub fn apply_to(&self, config: &mut ShellConfig) {
        config.yield_time = Duration::from_millis(self.yield_time_ms);
    }
}

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
    /// Foreground time before a still-running command is handed over as a
    /// task (see [`BashSettings`]).
    pub yield_time: Duration,
    /// Between the graceful and the forced signal.
    pub term_grace: Duration,
    /// How long to keep reading outputs after a command ended.
    pub drain: Duration,
    /// Interval of progress messages.
    pub progress_every: Duration,
    pub capture: capture::CaptureLimits,
    pub tasks: background::TaskLimits,
    /// Where session files (spilled outputs, task logs) go.
    pub base_dir: PathBuf,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            bash_path: None,
            pwsh_path: None,
            default_timeout: Duration::from_secs(120),
            max_timeout: Duration::from_secs(600),
            yield_time: Duration::from_millis(BashSettings::default().yield_time_ms),
            term_grace: Duration::from_secs(2),
            drain: Duration::from_millis(500),
            progress_every: Duration::from_secs(10),
            capture: capture::CaptureLimits::default(),
            tasks: background::TaskLimits::default(),
            base_dir: std::env::temp_dir().join("bricks-shell"),
        }
    }
}

/// The commands and tasks of one session.
pub struct ShellSessions {
    pub id: String,
    dir: PathBuf,
    state: Arc<state::ShellState>,
    pwsh: tokio::sync::Mutex<Option<Arc<powershell::PwshSession>>>,
    /// Set when the PowerShell of this session ended; reported once by the
    /// next command, which runs in a new one.
    reset_notice: parking_lot::Mutex<Option<String>>,
    /// Tasks the caller can read and stop: background tasks and handed-over
    /// commands.
    tasks: parking_lot::Mutex<Vec<Arc<background::Task>>>,
    /// Ordinary commands in flight (possibly handed over too).
    commands: parking_lot::Mutex<Vec<Arc<bash::Run>>>,
    /// Commands described for an approval: `(command, fingerprint)`.
    described: parking_lot::Mutex<Vec<(String, String)>>,
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
                state: Arc::new(state::ShellState::from_env()),
                pwsh: tokio::sync::Mutex::new(None),
                reset_notice: parking_lot::Mutex::new(None),
                tasks: parking_lot::Mutex::new(Vec::new()),
                commands: parking_lot::Mutex::new(Vec::new()),
                described: parking_lot::Mutex::new(Vec::new()),
                next_task: AtomicU64::new(1),
                generation: AtomicU64::new(0),
            })
        })
        .clone()
}

/// Close the commands and tasks of session `id`: graceful, bounded, awaited.
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

/// The `bash` to run: configured, or the first on `PATH`.
#[cfg(unix)]
pub(crate) fn bash_path(config: &ShellConfig) -> std::io::Result<PathBuf> {
    config
        .bash_path
        .clone()
        .or_else(|| which::which("bash").ok())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "bash was not found on PATH")
        })
}

impl ShellSessions {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The session's shell state (directory, exports, functions, aliases).
    pub fn state(&self) -> &state::ShellState {
        &self.state
    }

    /// Take the pending reset notice, if a PowerShell ended since the last call.
    pub fn take_reset_notice(&self) -> Option<String> {
        self.reset_notice.lock().take()
    }

    pub fn note_reset(&self, what: &str) {
        *self.reset_notice.lock() = Some(format!(
            "{what} ended before this command; this command runs in a new {what} — the previous \
             working directory, variables, functions and aliases are gone"
        ));
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

    /// Start a background task from a copy of the session state. It never
    /// publishes into the state.
    #[cfg(unix)]
    pub async fn start_task(
        &self,
        command: &str,
        stdin: Option<&str>,
        ready_pattern: Option<regex::Regex>,
        cwd: &Path,
        config: &ShellConfig,
    ) -> std::io::Result<Arc<background::Task>> {
        let n = self.next_task.fetch_add(1, Ordering::SeqCst);
        let dir = self.dir.join(format!("task-{n}"));
        let launch = self.state.launch(cwd);
        let bash = bash_path(config)?;
        match background::Task::start(background::TaskSpec {
            id: format!("bg-{n}"),
            command,
            prelude: &launch.prelude,
            cwd: &launch.base.cwd.pwd,
            env: &launch.env,
            stdin,
            ready_pattern,
            dir: dir.clone(),
            bash: &bash,
            limits: config.tasks.clone(),
            ordinary: None,
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
        let runs = self.commands.lock().clone();
        for t in tasks {
            t.stop(Duration::from_secs(2)).await;
        }
        for r in runs {
            r.task().stop(Duration::from_secs(2)).await;
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
        for r in self.commands.lock().iter() {
            r.task().kill_now();
        }
        if let Ok(mut slot) = self.pwsh.try_lock()
            && let Some(p) = slot.take()
        {
            p.kill_now();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for ShellSessions {
    fn drop(&mut self) {
        for t in self.tasks.lock().iter() {
            t.kill_now();
        }
        for r in self.commands.lock().iter() {
            r.task().kill_now();
        }
    }
}
