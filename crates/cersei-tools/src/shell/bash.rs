//! A persistent bash interpreter driven over a dedicated control channel.
//!
//! The driver (`driver.sh`) runs at the top level of one long-lived `bash`.
//! Bricks sends requests on fd 198 and reads framed replies on fd 199; a
//! command's text travels in a file that the driver sources in the shell's
//! own scope, and its stdout and stderr go to two FIFOs created for that
//! request and read from the start. Nothing is parsed out of the user's
//! output: the status and working directory come only from the control
//! channel, so output that imitates a marker or a control message is just
//! output.
//!
//! The shell runs in its own session (`setsid`), without a controlling
//! terminal: a program that would prompt on `/dev/tty` fails instead of
//! blocking. Foreground commands are serialised. On timeout the command's
//! process tree is terminated (TERM, grace, KILL) and the shell keeps its
//! state; if the shell itself does not come back (a builtin loop, a hung
//! shell), it is destroyed and the next command starts a fresh one, which is
//! reported.

#![cfg(unix)]

use super::capture::{CaptureLimits, Captured, StreamCapture};
use super::procs;
use super::{Progress, ShellConfig};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::pipe;

pub(crate) const DRIVER: &str = include_str!("driver.sh");
const REQUEST_FD: i32 = 198;
const REPLY_FD: i32 = 199;

/// How a foreground command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    /// The command completed; `code` is its exit status.
    Exited { code: i32 },
    /// The timeout expired and the command was terminated.
    TimedOut {
        /// The shell had to be destroyed (its state is lost).
        shell_destroyed: bool,
    },
    /// The command was interrupted because the caller gave up (turn
    /// cancelled).
    Cancelled,
    /// The shell itself ended during the command (`exit`, `exec`, a crash,
    /// a lost control channel). Its state is lost.
    ShellEnded {
        code: Option<i32>,
        signal: Option<i32>,
    },
}

/// Everything known about a foreground command.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub stdout: Captured,
    pub stderr: Captured,
    /// Monotonic duration of the command.
    pub duration: Duration,
    /// Working directory of the shell after the command, when known.
    pub cwd: Option<PathBuf>,
    /// Facts the caller should show: session reset, leftover processes
    /// stopped, termination details.
    pub notes: Vec<String>,
    /// Processes terminated because of a timeout, with those that needed
    /// the forced signal.
    pub terminated: procs::Termination,
}

/// Reply frames of the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Frame {
    Ready,
    Start(u64),
    End { id: u64, status: i32, cwd: PathBuf },
}

/// Reads NUL-separated fields from the reply pipe. Cancel-safe: bytes are
/// only consumed once a read completes.
struct FrameReader {
    rx: pipe::Receiver,
    buf: Vec<u8>,
}

impl FrameReader {
    fn try_parse(&mut self) -> Option<Frame> {
        loop {
            // Positions of the complete (NUL-terminated) fields.
            let mut bounds = Vec::new();
            let mut start = 0;
            for (i, b) in self.buf.iter().enumerate() {
                if *b == 0 {
                    bounds.push((start, i));
                    start = i + 1;
                }
            }
            let &(k0, k1) = bounds.first()?;
            let kind = self.buf[k0..k1].to_vec();
            let arity = match kind.as_slice() {
                b"READY" => 3,
                b"START" => 2,
                b"END" => 4,
                _ => 1,
            };
            if bounds.len() < arity {
                return None;
            }
            let field = |i: usize| self.buf[bounds[i].0..bounds[i].1].to_vec();
            let text = |i: usize| String::from_utf8_lossy(&field(i)).to_string();
            let frame = match kind.as_slice() {
                b"READY" => Some(Frame::Ready),
                b"START" => text(1).parse().ok().map(Frame::Start),
                b"END" => {
                    use std::os::unix::ffi::OsStrExt;
                    Some(Frame::End {
                        id: text(1).parse().unwrap_or(u64::MAX),
                        status: text(2).parse().unwrap_or(-1),
                        cwd: PathBuf::from(std::ffi::OsStr::from_bytes(&field(3))),
                    })
                }
                _ => None,
            };
            let consumed = bounds[arity - 1].1 + 1;
            self.buf.drain(..consumed);
            if frame.is_some() {
                return frame;
            }
        }
    }

    async fn next(&mut self) -> std::io::Result<Option<Frame>> {
        loop {
            if let Some(f) = self.try_parse() {
                return Ok(Some(f));
            }
            let mut tmp = [0u8; 4096];
            let n = self.rx.read(&mut tmp).await?;
            if n == 0 {
                return Ok(None);
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }
}

/// The live parts of a session, used under the foreground lock.
struct Live {
    child: tokio::process::Child,
    requests: pipe::Sender,
    replies: FrameReader,
}

/// One persistent bash.
pub struct BashSession {
    pid: u32,
    dir: PathBuf,
    config: ShellConfig,
    live: tokio::sync::Mutex<Option<Live>>,
    dead: AtomicBool,
    next_id: AtomicU64,
    /// Output the shell wrote outside of any command (driver errors).
    stray: Arc<parking_lot::Mutex<String>>,
}

fn make_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: `pipe` writes two descriptors into the array on success.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    for fd in fds {
        // Close-on-exec: these ends reach the shell only through dup2.
        // SAFETY: fcntl on a descriptor we just created.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: both descriptors are open and owned by nobody else.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn mkfifo(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("path contains a NUL byte"))?;
    // SAFETY: valid C string; mkfifo does not retain the pointer.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Reads one FIFO into a shared capture until EOF.
fn spawn_reader(
    path: &Path,
    capture: Arc<parking_lot::Mutex<StreamCapture>>,
) -> std::io::Result<(pipe::Sender, tokio::task::JoinHandle<()>)> {
    let mut rx = pipe::OpenOptions::new().open_receiver(path)?;
    // Our own writer keeps the FIFO open until the command's end is known,
    // so an early EOF cannot be mistaken for the end of the output.
    let keeper = pipe::OpenOptions::new().open_sender(path)?;
    let task = tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match rx.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => capture.lock().push(&buf[..n]),
            }
        }
    });
    Ok((keeper, task))
}

impl BashSession {
    /// Start a shell in `cwd`, ready to take requests.
    pub async fn start(
        cwd: &Path,
        config: ShellConfig,
        dir: PathBuf,
    ) -> std::io::Result<Arc<Self>> {
        std::fs::create_dir_all(&dir)?;
        let bash = config
            .bash_path
            .clone()
            .or_else(|| which::which("bash").ok())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "bash was not found on PATH")
            })?;

        let (req_read, req_write) = make_pipe()?;
        let (rep_read, rep_write) = make_pipe()?;
        let (child_req, child_rep) = (req_read.as_raw_fd(), rep_write.as_raw_fd());

        let mut cmd = tokio::process::Command::new(&bash);
        cmd.args(["--noprofile", "--norc", "-c", DRIVER, "bricks-shell"])
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .env_remove("PROMPT_COMMAND");
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                for (from, to) in [(child_req, REQUEST_FD), (child_rep, REPLY_FD)] {
                    if from == to {
                        libc::fcntl(to, libc::F_SETFD, 0);
                    } else if libc::dup2(from, to) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        drop(req_read);
        drop(rep_write);
        let pid = child.id().unwrap_or(0);

        let stray = Arc::new(parking_lot::Mutex::new(String::new()));
        for stream in [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let stray = stray.clone();
            tokio::spawn(async move {
                let mut s = stream;
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut st = stray.lock();
                    st.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if st.len() > 8192 {
                        let cut = st.len() - 4096;
                        let cut = (cut..st.len())
                            .find(|&i| st.is_char_boundary(i))
                            .unwrap_or(0);
                        st.drain(..cut);
                    }
                }
            });
        }

        let requests = pipe::Sender::from_owned_fd(req_write)?;
        let mut replies = FrameReader {
            rx: pipe::Receiver::from_owned_fd(rep_read)?,
            buf: Vec::new(),
        };
        match tokio::time::timeout(Duration::from_secs(10), replies.next()).await {
            Ok(Ok(Some(Frame::Ready))) => {}
            other => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(std::io::Error::other(format!(
                    "the shell did not start its driver ({}): {}",
                    match other {
                        Err(_) => "timeout".to_string(),
                        Ok(Err(e)) => e.to_string(),
                        Ok(Ok(f)) => format!("{f:?}"),
                    },
                    stray.lock().trim()
                )));
            }
        }
        Ok(Arc::new(Self {
            pid,
            dir,
            config,
            live: tokio::sync::Mutex::new(Some(Live {
                child,
                requests,
                replies,
            })),
            dead: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            stray,
        }))
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The shell ended or was destroyed; a new one is needed.
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    /// Write the shell's current state (cwd, options, exported variables,
    /// functions, aliases) as a script a new bash can source.
    pub async fn snapshot(self: &Arc<Self>, path: &Path) -> std::io::Result<()> {
        self.control("snapshot", "", path).await.map(|_| ())
    }

    /// Definitions of the aliases and functions among `names` (for
    /// permission checks); empty when none is defined.
    pub async fn describe(self: &Arc<Self>, names: &[String]) -> std::io::Result<String> {
        let safe: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|n| {
                !n.is_empty()
                    && n.chars()
                        .all(|c| c.is_alphanumeric() || "_-.:@+".contains(c))
            })
            .collect();
        if safe.is_empty() {
            return Ok(String::new());
        }
        let out = self
            .dir
            .join(format!("describe-{}", self.next_id.load(Ordering::SeqCst)));
        self.control("describe", &safe.join(" "), &out).await?;
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        let _ = std::fs::remove_file(&out);
        Ok(text)
    }

    async fn control(self: &Arc<Self>, kind: &str, arg: &str, out: &Path) -> std::io::Result<()> {
        let mut guard = self.live.lock().await;
        let live = guard
            .as_mut()
            .ok_or_else(|| std::io::Error::other("the shell has ended"))?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let fields = [
            id.to_string(),
            kind.into(),
            arg.into(),
            out.display().to_string(),
            String::new(),
            String::new(),
        ];
        send_request(&mut live.requests, &fields).await?;
        match wait_end(live, id, Duration::from_secs(10)).await {
            Some(_) => Ok(()),
            None => Err(std::io::Error::other("the shell did not answer")),
        }
    }

    /// Run one foreground command in the shell's scope.
    pub async fn run(
        self: &Arc<Self>,
        command: &str,
        stdin: Option<&str>,
        timeout: Duration,
        progress: Option<Progress>,
    ) -> std::io::Result<RunOutcome> {
        let mut guard = self.live.lock().await;
        if self.is_dead() || guard.is_none() {
            return Err(std::io::Error::other("the shell has ended"));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req_dir = self.dir.join(format!("req-{id}"));
        std::fs::create_dir_all(&req_dir)?;
        let script = req_dir.join("command");
        std::fs::write(&script, command)?;
        let input = match stdin {
            Some(text) => {
                let p = req_dir.join("stdin");
                std::fs::write(&p, text)?;
                p
            }
            None => PathBuf::from("/dev/null"),
        };
        let (out_fifo, err_fifo) = (req_dir.join("stdout"), req_dir.join("stderr"));
        mkfifo(&out_fifo)?;
        mkfifo(&err_fifo)?;
        let limits = self.config.capture.clone();
        let out_cap = Arc::new(parking_lot::Mutex::new(StreamCapture::new(
            limits.clone(),
            self.dir.join(format!("raw-{id}.stdout")),
        )));
        let err_cap = Arc::new(parking_lot::Mutex::new(StreamCapture::new(
            limits,
            self.dir.join(format!("raw-{id}.stderr")),
        )));
        let (out_keep, out_task) = spawn_reader(&out_fifo, out_cap.clone())?;
        let (err_keep, err_task) = spawn_reader(&err_fifo, err_cap.clone())?;

        // If this future is dropped mid-command (turn cancelled), interrupt
        // the command in the background instead of leaving it running.
        let mut cancel = CancelGuard {
            session: Some(self.clone()),
            id,
        };

        let live = guard.as_mut().expect("checked above");
        let fields = [
            id.to_string(),
            "run".into(),
            script.display().to_string(),
            out_fifo.display().to_string(),
            err_fifo.display().to_string(),
            input.display().to_string(),
        ];
        let started = Instant::now();
        send_request(&mut live.requests, &fields).await?;

        let mut notes = Vec::new();
        let mut terminated = procs::Termination::default();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut ticker = tokio::time::interval(self.config.progress_every);
        ticker.tick().await;
        let mut last_bytes = 0u64;
        let mut quiet_since = Instant::now();

        enum Waited {
            End(i32, PathBuf),
            Lost,
            Timeout,
        }
        let waited = loop {
            tokio::select! {
                frame = live.replies.next() => match frame {
                    Ok(Some(Frame::End { id: got, status, cwd })) if got == id => break Waited::End(status, cwd),
                    Ok(Some(_)) => continue,
                    Ok(None) | Err(_) => break Waited::Lost,
                },
                _ = tokio::time::sleep_until(deadline) => break Waited::Timeout,
                _ = ticker.tick() => {
                    let bytes = out_cap.lock().raw_bytes() + err_cap.lock().raw_bytes();
                    if bytes != last_bytes {
                        last_bytes = bytes;
                        quiet_since = Instant::now();
                    }
                    if let Some(p) = &progress {
                        let quiet = quiet_since.elapsed();
                        let mut msg = format!(
                            "still running after {:.0}s ({} bytes of output so far)",
                            started.elapsed().as_secs_f64(),
                            bytes
                        );
                        if quiet >= self.config.progress_every {
                            msg.push_str(&format!(
                                "; no output for {:.0}s — it may be computing, or waiting for input it \
                                 will not get (stdin is {})",
                                quiet.as_secs_f64(),
                                if stdin.is_some() { "the provided input" } else { "empty" }
                            ));
                        }
                        p(msg);
                    }
                }
            }
        };

        let (status, cwd) = match waited {
            Waited::End(code, cwd) => (RunStatus::Exited { code }, Some(cwd)),
            Waited::Lost => {
                let st = tokio::time::timeout(Duration::from_secs(2), live.child.wait()).await;
                self.dead.store(true, Ordering::SeqCst);
                let (code, signal) = match st {
                    Ok(Ok(s)) => {
                        use std::os::unix::process::ExitStatusExt;
                        (s.code(), s.signal())
                    }
                    _ => {
                        procs::signal_group(self.pid, true);
                        (None, None)
                    }
                };
                let stray = self.stray.lock().trim().to_string();
                notes.push(format!(
                    "the shell ended during this command ({}); its state (directory, variables, \
                     functions, aliases) is lost and the next command starts a new shell{}",
                    match (code, signal) {
                        (Some(c), _) => format!("exit status {c}"),
                        (None, Some(s)) => format!("signal {s}"),
                        _ => "no status".into(),
                    },
                    if stray.is_empty() {
                        String::new()
                    } else {
                        format!(": {stray}")
                    }
                ));
                (RunStatus::ShellEnded { code, signal }, None)
            }
            Waited::Timeout => {
                if let Some(p) = &progress {
                    p(format!(
                        "timeout of {:.0}s reached: stopping the command",
                        timeout.as_secs_f64()
                    ));
                }
                let (destroyed, term, end_cwd) = self.interrupt(live, id).await;
                terminated = term;
                if destroyed {
                    notes.push(
                        "the command did not stop with its processes, so the shell was destroyed: its \
                         state is lost and the next command starts a new shell"
                            .into(),
                    );
                }
                (
                    RunStatus::TimedOut {
                        shell_destroyed: destroyed,
                    },
                    end_cwd,
                )
            }
        };
        cancel.session = None;
        let duration = started.elapsed();

        // Collect the output: our keepers go, then the readers see EOF once
        // nothing else holds the FIFOs.
        drop(out_keep);
        drop(err_keep);
        let pending = drain(vec![out_task, err_task], self.config.drain).await;
        if !self.is_dead() {
            // Processes left behind by the command (`cmd &`, a daemon that
            // kept the shell's outputs) must not outlive it unsupervised.
            let leftovers = procs::descendants(self.pid);
            if !leftovers.is_empty() {
                let t = procs::terminate(leftovers, self.config.term_grace).await;
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
        }
        if !pending.is_empty() {
            // Writers outside our supervision still hold the streams: stop
            // reading after one more bounded window; what was read is kept.
            for h in drain(pending, self.config.drain).await {
                h.abort();
            }
        }
        let _ = std::fs::remove_dir_all(&req_dir);
        let stdout = take(out_cap);
        let stderr = take(err_cap);
        if self.is_dead() {
            *guard = None;
        }
        Ok(RunOutcome {
            status,
            stdout,
            stderr,
            duration,
            cwd,
            notes,
            terminated,
        })
    }

    /// Stop the running command `id`: its process tree first (TERM, grace,
    /// KILL); the shell itself if it does not come back. Returns whether the
    /// shell was destroyed, the termination record and the cwd if the
    /// command ended.
    async fn interrupt(
        &self,
        live: &mut Live,
        id: u64,
    ) -> (bool, procs::Termination, Option<PathBuf>) {
        let tree = procs::descendants(self.pid);
        let term = procs::terminate(tree, self.config.term_grace).await;
        if let Some(cwd) = wait_end(live, id, Duration::from_secs(2)).await {
            return (false, term, Some(cwd));
        }
        // The shell itself is busy (a builtin loop) or hung: destroy it.
        self.dead.store(true, Ordering::SeqCst);
        procs::signal_group(self.pid, true);
        let _ = tokio::time::timeout(Duration::from_secs(2), live.child.wait()).await;
        (true, term, None)
    }

    /// Stop the shell and everything it started. Awaitable and bounded.
    pub async fn close(&self) {
        self.dead.store(true, Ordering::SeqCst);
        let tree = procs::descendants(self.pid);
        procs::signal_group(self.pid, false);
        let mut all = tree;
        all.push(self.pid);
        let _ = procs::terminate(all, self.config.term_grace).await;
        if let Ok(mut guard) = tokio::time::timeout(Duration::from_secs(2), self.live.lock()).await
        {
            if let Some(live) = guard.as_mut() {
                let _ = tokio::time::timeout(Duration::from_secs(2), live.child.wait()).await;
            }
            *guard = None;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    /// Synchronous last resort (drop paths): kill the whole tree now.
    pub fn kill_now(&self) {
        self.dead.store(true, Ordering::SeqCst);
        procs::terminate_tree_now(self.pid);
        procs::signal_group(self.pid, true);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for BashSession {
    fn drop(&mut self) {
        if !self.dead.load(Ordering::SeqCst) {
            procs::terminate_tree_now(self.pid);
            procs::signal_group(self.pid, true);
        }
    }
}

async fn send_request(tx: &mut pipe::Sender, fields: &[String]) -> std::io::Result<()> {
    let mut frame = Vec::new();
    for f in fields {
        frame.extend_from_slice(f.as_bytes());
        frame.push(0);
    }
    tx.write_all(&frame).await
}

/// Wait for the readers until `limit`; returns those still running.
async fn drain(
    tasks: Vec<tokio::task::JoinHandle<()>>,
    limit: Duration,
) -> Vec<tokio::task::JoinHandle<()>> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut pending = Vec::new();
    for mut h in tasks {
        if tokio::time::timeout_at(deadline, &mut h).await.is_err() {
            pending.push(h);
        }
    }
    pending
}

/// Wait for the END reply of request `id`, up to `limit`.
async fn wait_end(live: &mut Live, id: u64, limit: Duration) -> Option<PathBuf> {
    let fut = async {
        loop {
            match live.replies.next().await {
                Ok(Some(Frame::End { id: got, cwd, .. })) if got == id => return Some(cwd),
                Ok(Some(_)) => continue,
                _ => return None,
            }
        }
    };
    tokio::time::timeout(limit, fut).await.ok().flatten()
}

fn take(cap: Arc<parking_lot::Mutex<StreamCapture>>) -> Captured {
    let placeholder = StreamCapture::new(CaptureLimits::default(), PathBuf::new());
    let inner = std::mem::replace(&mut *cap.lock(), placeholder);
    inner.finish()
}

/// Interrupts a command whose `run` future was dropped.
struct CancelGuard {
    session: Option<Arc<BashSession>>,
    id: u64,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let id = self.id;
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move {
                    // The dropped `run` released the lock; take it so the next
                    // command waits for this cleanup.
                    let mut guard = session.live.lock().await;
                    if let Some(live) = guard.as_mut() {
                        let (destroyed, _, _) = session.interrupt(live, id).await;
                        if destroyed {
                            *guard = None;
                        }
                    }
                });
            }
            Err(_) => session.kill_now(),
        }
    }
}
