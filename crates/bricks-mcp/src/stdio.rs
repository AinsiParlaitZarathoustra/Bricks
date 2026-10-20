//! stdio transport: the server is a child process; one JSON-RPC message per
//! line on its stdin/stdout.
//!
//! * A line longer than `max_message_bytes` closes the connection (the read
//!   fails with an explicit error) instead of growing a buffer without end.
//! * stderr is the server's log, not a failure signal: its last lines are
//!   kept for diagnostics and traced at debug level.
//! * Shutdown closes stdin, waits, then sends SIGTERM and finally SIGKILL
//!   (Unix); elsewhere the process is killed after the wait.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::config::McpServerConfig;

/// Last lines of a server's stderr.
#[derive(Clone, Default)]
pub struct StderrLog(Arc<parking_lot::Mutex<VecDeque<String>>>);

impl StderrLog {
    const KEEP: usize = 50;

    pub fn lines(&self) -> Vec<String> {
        self.0.lock().iter().cloned().collect()
    }

    fn push(&self, line: String) {
        let mut q = self.0.lock();
        if q.len() == Self::KEEP {
            q.pop_front();
        }
        q.push_back(line.chars().take(500).collect());
    }
}

/// Why a connection was closed by this side, when it was.
#[derive(Clone, Default)]
pub struct CloseReason(Arc<parking_lot::Mutex<Option<String>>>);

impl CloseReason {
    pub fn get(&self) -> Option<String> {
        self.0.lock().clone()
    }

    pub fn set(&self, reason: String) {
        *self.0.lock() = Some(reason);
    }
}

/// stdout with a per-line length limit.
pub struct LineLimited<R> {
    inner: R,
    since_newline: usize,
    limit: usize,
    reason: CloseReason,
}

impl<R> LineLimited<R> {
    pub fn new(inner: R, limit: usize) -> Self {
        Self {
            inner,
            since_newline: 0,
            limit,
            reason: CloseReason::default(),
        }
    }

    pub fn close_reason(&self) -> CloseReason {
        self.reason.clone()
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for LineLimited<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let mut overflow = false;
                for &b in &buf.filled()[before..] {
                    if b == b'\n' {
                        this.since_newline = 0;
                    } else {
                        this.since_newline += 1;
                        if this.since_newline > this.limit {
                            overflow = true;
                            break;
                        }
                    }
                }
                if overflow {
                    // An error read leaves the buffer as it was.
                    buf.set_filled(before);
                    let msg = format!(
                        "the server sent a message longer than {} bytes; connection closed",
                        this.limit
                    );
                    this.reason.set(msg.clone());
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        msg,
                    )));
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// A spawned server.
pub struct StdioProcess {
    pub child: Child,
    pub stderr: StderrLog,
    pub close_reason: CloseReason,
}

pub type StdioPipes = (LineLimited<ChildStdout>, ChildStdin);

pub fn spawn(config: &McpServerConfig) -> std::io::Result<(StdioProcess, StdioPipes)> {
    let command = config.command.as_deref().unwrap_or_default();
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(&config.args)
        .envs(&config.env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = &config.cwd {
        cmd.current_dir(dir);
    }
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let log = StderrLog::default();
    let (l, name) = (log.clone(), config.name.clone());
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(server = %name, "stderr: {line}");
            l.push(line);
        }
    });
    let stdout = LineLimited::new(stdout, config.limits.max_message_bytes);
    Ok((
        StdioProcess {
            child,
            stderr: log,
            close_reason: stdout.close_reason(),
        },
        (stdout, stdin),
    ))
}

impl StdioProcess {
    /// Wait for the process to leave after its stdin was closed; then
    /// terminate it (and its process group on Unix).
    pub async fn shutdown(&mut self, grace: Duration) {
        if tokio::time::timeout(grace, self.child.wait()).await.is_ok() {
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // SAFETY: plain kill(2) on the process group we created.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            if tokio::time::timeout(grace, self.child.wait()).await.is_ok() {
                return;
            }
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(grace, self.child.wait()).await;
    }

    pub fn kill_now(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // SAFETY: plain kill(2) on the process group we created.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn long_lines_are_refused_short_ones_pass() {
        let data = b"{\"a\":1}\n0123456789abcdef\n".to_vec();
        let mut ok = LineLimited::new(&data[..8], 10);
        let mut s = String::new();
        ok.read_to_string(&mut s).await.unwrap();
        assert_eq!(s, "{\"a\":1}\n");
        let mut bad = LineLimited::new(&data[..], 10);
        let mut s = String::new();
        let e = bad.read_to_string(&mut s).await.unwrap_err();
        assert!(e.to_string().contains("longer than 10 bytes"));
    }
}
