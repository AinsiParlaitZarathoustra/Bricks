//! Background tasks of a shell session.
//!
//! A task is a separate `bash` started from a snapshot of the persistent
//! shell taken at launch: working directory, shell options, exported
//! variables, functions and aliases. It runs in its own session and process
//! group, with its own pipes, so it never blocks the next commands, its
//! output never mixes with theirs, and what it changes never reaches the
//! persistent shell. Non-exported shell variables are not inherited (a
//! foreground `cmd &` would see them; a task does not).
//!
//! Output is read continuously into a bounded in-memory window of lines per
//! stream (with absolute line numbers, so a reader can page with a cursor and
//! is told what was dropped) and into a raw log file capped in size.

use super::clean::TerminalCleaner;
use super::procs;
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::io::AsyncReadExt;

/// Lifecycle of a task. The exit code exists only after a normal exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Starting,
    Running,
    /// Exited with status 0.
    Completed {
        code: i32,
    },
    /// Exited with a non-zero status, or was killed by a signal it did not
    /// receive from Bricks.
    Failed {
        code: Option<i32>,
        signal: Option<i32>,
    },
    /// Stopped on request.
    Stopped,
}

impl TaskState {
    pub fn label(&self) -> &'static str {
        match self {
            TaskState::Starting => "starting",
            TaskState::Running => "running",
            TaskState::Completed { .. } => "completed",
            TaskState::Failed { .. } => "failed",
            TaskState::Stopped => "stopped",
        }
    }

    pub fn is_finished(&self) -> bool {
        !matches!(self, TaskState::Starting | TaskState::Running)
    }
}

/// Whether the task said it is ready (only with a readiness pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// No readiness pattern was given: running does not mean ready.
    NotChecked,
    Waiting,
    Ready {
        after: Duration,
        line: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    pub fn name(self) -> &'static str {
        match self {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        }
    }
}

/// Limits of a task's logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLimits {
    /// Lines kept in memory per stream.
    pub memory_lines: usize,
    /// Size of each raw log file before writing stops.
    pub raw_log_bytes: u64,
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self {
            memory_lines: 5_000,
            raw_log_bytes: 50 * 1024 * 1024,
        }
    }
}

struct Log {
    lines: VecDeque<String>,
    /// Absolute number (from 0) of `lines[0]`.
    first: u64,
    total: u64,
    partial: String,
    cleaner: TerminalCleaner,
    raw: Option<std::fs::File>,
    raw_path: PathBuf,
    raw_written: u64,
    raw_capped: bool,
    limits: TaskLimits,
}

impl Log {
    fn new(raw_path: PathBuf, limits: TaskLimits) -> Self {
        let raw = std::fs::File::create(&raw_path).ok();
        Self {
            lines: VecDeque::new(),
            first: 0,
            total: 0,
            partial: String::new(),
            cleaner: TerminalCleaner::new(),
            raw,
            raw_path,
            raw_written: 0,
            raw_capped: false,
            limits,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        if let Some(f) = &mut self.raw {
            let room = self.limits.raw_log_bytes.saturating_sub(self.raw_written) as usize;
            let n = room.min(bytes.len());
            if f.write_all(&bytes[..n]).is_err() {
                self.raw = None;
            }
            self.raw_written += n as u64;
            if n < bytes.len() {
                self.raw_capped = true;
                self.raw = None;
            }
        }
        let text = self.cleaner.push(bytes);
        self.add(&text)
    }

    fn finish(&mut self) {
        let rest = self.cleaner.finish();
        self.add(&rest);
        if !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            self.add_line(line);
        }
    }

    fn add(&mut self, text: &str) -> Vec<String> {
        let mut new = Vec::new();
        for piece in text.split_inclusive('\n') {
            if let Some(line) = piece.strip_suffix('\n') {
                self.partial.push_str(line);
                let line = std::mem::take(&mut self.partial);
                new.push(line.clone());
                self.add_line(line);
            } else {
                self.partial.push_str(piece);
            }
        }
        new
    }

    fn add_line(&mut self, line: String) {
        self.lines.push_back(line);
        self.total += 1;
        while self.lines.len() > self.limits.memory_lines {
            self.lines.pop_front();
            self.first += 1;
        }
    }
}

/// A page of a task's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub stream: Stream,
    /// Absolute line numbers, from 1.
    pub lines: Vec<(u64, String)>,
    /// Lines that existed before the requested offset but are no longer in
    /// memory (read the raw log for them).
    pub dropped_before: u64,
    /// Lines seen so far on this stream.
    pub total_lines: u64,
    /// Offset to ask for next (0-based count of lines to skip).
    pub next_offset: u64,
    /// The unfinished last line, if any (not yet terminated by `\n`).
    pub partial: Option<String>,
    pub raw_log: PathBuf,
    pub raw_log_capped: bool,
}

/// What to start: `command`, with the shell state written in `snapshot`.
pub struct TaskSpec<'a> {
    pub id: String,
    pub command: &'a str,
    pub snapshot: &'a Path,
    /// Directory if the snapshot cannot restore one.
    pub fallback_cwd: &'a Path,
    pub stdin: Option<&'a str>,
    pub ready_pattern: Option<regex::Regex>,
    /// The task's own directory (command, logs).
    pub dir: PathBuf,
    pub bash: &'a Path,
    pub limits: TaskLimits,
}

pub struct Task {
    pub id: String,
    pub command: String,
    pub pid: u32,
    pub started_at: SystemTime,
    started: Instant,
    state: parking_lot::Mutex<TaskState>,
    ready: parking_lot::Mutex<Readiness>,
    ready_pattern: Option<regex::Regex>,
    logs: [Arc<parking_lot::Mutex<Log>>; 2],
    finished: tokio::sync::watch::Receiver<bool>,
    stop_requested: std::sync::atomic::AtomicBool,
    dir: PathBuf,
}

/// Snapshot of a task for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    pub id: String,
    pub command: String,
    pub pid: u32,
    pub state: TaskState,
    pub readiness: Readiness,
    pub elapsed: Duration,
    pub stdout_lines: u64,
    pub stderr_lines: u64,
}

impl Task {
    /// Start a task (see [`TaskSpec`]).
    pub async fn start(spec: TaskSpec<'_>) -> std::io::Result<Arc<Task>> {
        let TaskSpec {
            id,
            command,
            snapshot,
            fallback_cwd,
            stdin,
            ready_pattern,
            dir,
            bash,
            limits,
        } = spec;
        std::fs::create_dir_all(&dir)?;
        let script = dir.join("command");
        std::fs::write(&script, command)?;
        let stdin_cfg = match stdin {
            Some(text) => {
                let p = dir.join("stdin");
                std::fs::write(&p, text)?;
                std::process::Stdio::from(std::fs::File::open(&p)?)
            }
            None => std::process::Stdio::null(),
        };
        let mut cmd = tokio::process::Command::new(bash);
        cmd.args([
            "--noprofile",
            "--norc",
            "-c",
            // The snapshot restores the state; its errors (read-only
            // variables re-declared) are not the task's output.
            "builtin . \"$1\" 2>/dev/null; builtin . \"$2\"",
            "bricks-task",
        ])
        .arg(snapshot)
        .arg(&script)
        .current_dir(fallback_cwd)
        .stdin(stdin_cfg)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_remove("BASH_ENV")
        .env_remove("ENV");
        #[cfg(unix)]
        // SAFETY: setsid is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        let logs = [
            Arc::new(parking_lot::Mutex::new(Log::new(
                dir.join("stdout.log"),
                limits.clone(),
            ))),
            Arc::new(parking_lot::Mutex::new(Log::new(
                dir.join("stderr.log"),
                limits,
            ))),
        ];
        let (done_tx, done_rx) = tokio::sync::watch::channel(false);
        let task = Arc::new(Task {
            id,
            command: command.to_string(),
            pid,
            started_at: SystemTime::now(),
            started: Instant::now(),
            state: parking_lot::Mutex::new(TaskState::Running),
            ready: parking_lot::Mutex::new(if ready_pattern.is_some() {
                Readiness::Waiting
            } else {
                Readiness::NotChecked
            }),
            ready_pattern,
            logs,
            finished: done_rx,
            stop_requested: std::sync::atomic::AtomicBool::new(false),
            dir,
        });

        let mut readers = Vec::new();
        let streams: [Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>; 2] = [
            child.stdout.take().map(|s| Box::new(s) as _),
            child.stderr.take().map(|s| Box::new(s) as _),
        ];
        for (i, stream) in streams.into_iter().enumerate() {
            let Some(mut stream) = stream else { continue };
            let t = task.clone();
            readers.push(tokio::spawn(async move {
                let mut buf = vec![0u8; 32 * 1024];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let new = t.logs[i].lock().push(&buf[..n]);
                            t.check_ready(&new);
                        }
                    }
                }
                t.logs[i].lock().finish();
            }));
        }

        let t = task.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            for r in readers {
                // The streams close with the processes; bounded anyway.
                let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
            }
            let state = if t.stop_requested.load(std::sync::atomic::Ordering::SeqCst) {
                TaskState::Stopped
            } else {
                match status {
                    Ok(s) => {
                        #[cfg(unix)]
                        let signal = {
                            use std::os::unix::process::ExitStatusExt;
                            s.signal()
                        };
                        #[cfg(not(unix))]
                        let signal = None;
                        match s.code() {
                            Some(0) => TaskState::Completed { code: 0 },
                            code => TaskState::Failed { code, signal },
                        }
                    }
                    Err(_) => TaskState::Failed {
                        code: None,
                        signal: None,
                    },
                }
            };
            *t.state.lock() = state;
            let _ = done_tx.send(true);
        });
        Ok(task)
    }

    fn check_ready(&self, new_lines: &[String]) {
        let Some(re) = &self.ready_pattern else {
            return;
        };
        let mut ready = self.ready.lock();
        if !matches!(*ready, Readiness::Waiting) {
            return;
        }
        if let Some(line) = new_lines.iter().find(|l| re.is_match(l)) {
            *ready = Readiness::Ready {
                after: self.started.elapsed(),
                line: line.clone(),
            };
        }
    }

    pub fn state(&self) -> TaskState {
        self.state.lock().clone()
    }

    pub fn status(&self) -> TaskStatus {
        TaskStatus {
            id: self.id.clone(),
            command: self.command.clone(),
            pid: self.pid,
            state: self.state(),
            readiness: self.ready.lock().clone(),
            elapsed: self.started.elapsed(),
            stdout_lines: self.logs[0].lock().total,
            stderr_lines: self.logs[1].lock().total,
        }
    }

    /// Wait until the readiness pattern matched, the task ended, or `limit`.
    pub async fn wait_ready(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if !matches!(*self.ready.lock(), Readiness::Waiting) || self.state().is_finished() {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait for the task to end, up to `limit`. True when it ended.
    pub async fn wait_finished(&self, limit: Duration) -> bool {
        let mut rx = self.finished.clone();
        let ended = tokio::time::timeout(limit, async { rx.wait_for(|d| *d).await.is_ok() }).await;
        ended.unwrap_or(false)
    }

    /// Lines from `offset` (0-based), at most `limit`.
    pub fn read(&self, stream: Stream, offset: u64, limit: usize) -> Page {
        let log = self.logs[match stream {
            Stream::Stdout => 0,
            Stream::Stderr => 1,
        }]
        .lock();
        let start = offset.max(log.first);
        let skip = (start - log.first) as usize;
        let lines: Vec<(u64, String)> = log
            .lines
            .iter()
            .skip(skip)
            .take(limit)
            .enumerate()
            .map(|(i, l)| (start + i as u64 + 1, l.clone()))
            .collect();
        let next = start + lines.len() as u64;
        Page {
            stream,
            dropped_before: log.first.saturating_sub(offset),
            total_lines: log.total,
            next_offset: next,
            partial: (!log.partial.is_empty() && next == log.total).then(|| log.partial.clone()),
            raw_log: log.raw_path.clone(),
            raw_log_capped: log.raw_capped,
            lines,
        }
    }

    /// Stop the task: TERM to its group and tree, grace, KILL. Idempotent.
    pub async fn stop(&self, grace: Duration) -> TaskState {
        if self.state().is_finished() {
            return self.state();
        }
        self.stop_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut tree = procs::descendants(self.pid);
        tree.insert(0, self.pid);
        procs::signal_group(self.pid, false);
        let _ = procs::terminate(tree.clone(), grace).await;
        procs::signal_group(self.pid, true);
        if !self.wait_finished(Duration::from_secs(5)).await {
            procs::terminate_tree_now(self.pid);
            let _ = self.wait_finished(Duration::from_secs(2)).await;
        }
        self.state()
    }

    pub fn kill_now(&self) {
        if !self.state().is_finished() {
            self.stop_requested
                .store(true, std::sync::atomic::Ordering::SeqCst);
            procs::terminate_tree_now(self.pid);
            procs::signal_group(self.pid, true);
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}
