//! Ordinary commands: one `bash` process per call, supervised as a task.
//!
//! Each command starts from a copy of the session state (see [`super::state`]):
//! its working directory, its exported variables, its functions and aliases.
//! It runs in its own session and process group, with its own pipes. Nothing
//! is serialised: commands of one session run side by side, and a command
//! that waits does not hold anything another one needs.
//!
//! When a command ends normally, its shell records its final state in a
//! file (an EXIT trap; the output is never parsed), and the changes it made,
//! compared with the copy it started from, are published to the session (see
//! [`super::state::ShellState::publish`]). A command that is interrupted,
//! times out, or replaces its shell with `exec` publishes nothing, and the
//! result says so.
//!
//! The timeout is a deadline from the start, enforced by the command's
//! watcher: it still applies after the command was handed over as a task
//! ([`Command::detach`]). The end is published once, by whichever of the
//! caller and the watcher gets there first, before the caller sees it.

#![cfg(unix)]

use super::background::{Ordinary, Task, TaskSpec, TaskState};
use super::capture::Captured;
use super::procs;
use super::state::{ShellState, Snapshot, parse_dump};
use super::{ShellConfig, ShellSessions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How an ordinary command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    /// The command completed; `code` is its exit status.
    Exited { code: i32 },
    /// The timeout expired and the command was terminated.
    TimedOut,
    /// Stopped without a timeout (the call gave up, or the task was stopped).
    Cancelled,
    /// Killed by a signal Bricks did not send for a timeout or a cancel.
    Killed { signal: Option<i32> },
}

/// Everything known about an ordinary command that ended.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub stdout: Captured,
    pub stderr: Captured,
    /// Monotonic duration of the command.
    pub duration: std::time::Duration,
    /// Working directory the session has after the command, when its end was
    /// published (`None`: its changes were not kept).
    pub cwd: Option<PathBuf>,
    /// Facts the caller should show: a missing directory, leftover processes
    /// stopped, environment changes not kept.
    pub notes: Vec<String>,
    /// Processes terminated because of a timeout or a stop.
    pub terminated: procs::Termination,
}

struct Settled {
    /// The directory the session has after the command, when published.
    published_cwd: Option<PathBuf>,
}

/// One ordinary command, from its start to its end.
pub struct Run {
    task: Arc<Task>,
    seq: u64,
    base: Snapshot,
    state: Arc<ShellState>,
    timed_out: AtomicBool,
    stop_report: parking_lot::Mutex<procs::Termination>,
    launch_notes: Vec<String>,
    settled: parking_lot::Mutex<Option<Settled>>,
    /// Set once the watcher has recorded the end (stop report, publication).
    watched: tokio::sync::watch::Receiver<bool>,
}

impl Run {
    /// The task behind the command: its id, its pid, its output.
    pub fn task(&self) -> &Arc<Task> {
        &self.task
    }

    /// Publishes the command's end, once. Called only once the task ended.
    fn settle(&self) {
        let mut settled = self.settled.lock();
        if settled.is_some() {
            return;
        }
        let normal = matches!(
            self.task.state(),
            TaskState::Completed { .. }
                | TaskState::Failed {
                    code: Some(_),
                    signal: None
                }
        );
        let mut published_cwd = None;
        if normal && !self.timed_out.load(Ordering::SeqCst) {
            let end = self
                .task
                .state_file()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|bytes| parse_dump(&bytes));
            if let Some(end) = end {
                self.state.publish(self.seq, &self.base, &end);
                published_cwd = Some(end.cwd.pwd);
            }
        }
        *settled = Some(Settled { published_cwd });
    }

    /// The outcome, once the task ended.
    fn outcome(&self) -> RunOutcome {
        let published_cwd = self
            .settled
            .lock()
            .as_ref()
            .and_then(|s| s.published_cwd.clone());
        let status = if self.timed_out.load(Ordering::SeqCst) {
            RunStatus::TimedOut
        } else {
            match self.task.state() {
                TaskState::Completed { code } => RunStatus::Exited { code },
                TaskState::Failed {
                    code: Some(code),
                    signal: None,
                } => RunStatus::Exited { code },
                TaskState::Failed { signal, .. } => RunStatus::Killed { signal },
                TaskState::Stopped | TaskState::Starting | TaskState::Running => {
                    RunStatus::Cancelled
                }
            }
        };
        let [stdout, stderr] = self
            .task
            .output()
            .unwrap_or_else(|| [Captured::default(), Captured::default()]);
        let mut notes = self.launch_notes.clone();
        if published_cwd.is_none() {
            notes.push(
                "environment changes not available: the command did not end normally \
                 (interrupted, timed out, or it replaced its shell with exec), so its \
                 directory and variables are not kept"
                    .into(),
            );
        }
        if let Some(t) = self.task.leftovers()
            && !t.targeted.is_empty()
        {
            notes.push(format!(
                "{} process(es) left running by the command were stopped ({}); use \
                 `background: true` for long-running processes",
                t.targeted.len(),
                t.targeted
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        RunOutcome {
            status,
            stdout,
            stderr,
            duration: self.task.elapsed(),
            cwd: published_cwd,
            notes,
            terminated: self.stop_report.lock().clone(),
        }
    }
}

/// The caller's handle on an ordinary command. Dropped before its end (the
/// call was cancelled), the command is killed: its group, at once.
pub struct Command {
    run: Option<Arc<Run>>,
}

impl Command {
    /// The task behind the command.
    pub fn task(&self) -> &Arc<Task> {
        &self.run.as_ref().expect("command handed over").task
    }

    /// Waits for the command's end, publishes it and returns its outcome.
    pub async fn finish(&mut self) -> RunOutcome {
        let run = Arc::clone(self.run.as_ref().expect("command handed over"));
        let mut watched = run.watched.clone();
        let _ = watched.wait_for(|done| *done).await;
        run.settle();
        let outcome = run.outcome();
        self.run = None;
        outcome
    }

    /// Hands the command over to the session: it goes on as a task, and
    /// its end is still published. Nothing is killed when this handle goes.
    pub fn detach(mut self) -> Arc<Run> {
        self.run.take().expect("command handed over")
    }
}

impl Drop for Command {
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            run.task.kill_now();
        }
    }
}

impl ShellSessions {
    /// Starts an ordinary command in `default_cwd` (used when the session
    /// has no directory yet, or the recorded one vanished). `timeout` is a
    /// deadline from now: at its end the command's processes are stopped.
    pub async fn spawn_command(
        &self,
        command: &str,
        stdin: Option<&str>,
        default_cwd: &Path,
        config: &ShellConfig,
        timeout: Duration,
    ) -> std::io::Result<Command> {
        let n = self.next_task.fetch_add(1, Ordering::SeqCst);
        let dir = self.dir.join(format!("task-{n}"));
        let launch = self.state.launch(default_cwd);
        let bash = super::bash_path(config)?;
        let ordinary = Ordinary {
            capture: config.capture.clone(),
            spill: [
                self.dir.join(format!("raw-{n}.stdout")),
                self.dir.join(format!("raw-{n}.stderr")),
            ],
            grace: config.term_grace,
        };
        let task = match Task::start(TaskSpec {
            id: format!("bg-{n}"),
            command,
            prelude: &launch.prelude,
            cwd: &launch.base.cwd.pwd,
            env: &launch.env,
            stdin,
            ready_pattern: None,
            dir: dir.clone(),
            bash: &bash,
            limits: config.tasks.clone(),
            ordinary: Some(ordinary),
        })
        .await
        {
            Ok(t) => t,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };
        let (watched_tx, _) = tokio::sync::watch::channel(false);
        let watched_rx = watched_tx.subscribe();
        let run = Arc::new(Run {
            task,
            seq: launch.seq,
            base: launch.base,
            state: Arc::clone(&self.state),
            timed_out: AtomicBool::new(false),
            stop_report: parking_lot::Mutex::new(procs::Termination::default()),
            launch_notes: launch.notes,
            settled: parking_lot::Mutex::new(None),
            watched: watched_rx,
        });
        self.track(Arc::clone(&run));
        let watch = Arc::clone(&run);
        let grace = config.term_grace;
        tokio::spawn(async move {
            tokio::select! {
                _ = watch.task.wait_end() => {}
                _ = tokio::time::sleep(timeout) => {
                    if !watch.task.state().is_finished() {
                        watch.timed_out.store(true, Ordering::SeqCst);
                        let (_, report) = watch.task.stop_report(grace).await;
                        *watch.stop_report.lock() = report;
                    }
                }
            }
            watch.task.wait_end().await;
            watch.settle();
            let _ = watched_tx.send(true);
        });
        Ok(Command { run: Some(run) })
    }

    /// Keeps a command's run among the session's, until it ends.
    fn track(&self, run: Arc<Run>) {
        let mut commands = self.commands.lock();
        commands.retain(|r| !r.task().state().is_finished());
        commands.push(run);
    }

    /// Makes a yielded command's task visible to the task tools.
    pub(crate) fn adopt(&self, task: Arc<Task>) {
        self.tasks.lock().push(task);
    }

    /// Remembers what a command was described as for an approval.
    pub(crate) fn remember(&self, command: &str, fingerprint: String) {
        let mut d = self.described.lock();
        d.retain(|(c, _)| c != command);
        if d.len() >= 32 {
            d.remove(0);
        }
        d.push((command.to_string(), fingerprint));
    }

    /// The fingerprint a command was described with, consumed.
    pub(crate) fn take_remembered(&self, command: &str) -> Option<String> {
        let mut d = self.described.lock();
        let pos = d.iter().position(|(c, _)| c == command)?;
        Some(d.remove(pos).1)
    }
}
