//! Terminal sessions: a shell on a PTY, its output pumped to a sink, its
//! input queued, resize, kill. No Tauri types here: the sink is a closure,
//! which keeps the logic testable on its own.
//!
//! Threads per session: `pty-reader` (PTY → chunks), `pty-output` (chunks →
//! base64 events, batched), `pty-input` (queued input → PTY), `pty-exit`
//! (waits for the shell, then reports `Exit`).

use std::collections::HashMap;
use std::fmt::Display;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use parking_lot::Mutex;
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use super::shell::ShellSpec;
use crate::contract::{IpcError, IpcErrorCode, IpcResult, PtyEvent, PtyInfo};

/// Receives output and exit events. Returns `false` once the consumer is
/// gone: the terminal is then killed, since nobody can read it.
pub type EventSink = Arc<dyn Fn(PtyEvent) -> bool + Send + Sync>;

/// Bytes per PTY read.
const READ_BUF: usize = 8 * 1024;
/// Output is batched until this many bytes or [`COALESCE_WINDOW`].
const COALESCE_BYTES: usize = 16 * 1024;
const COALESCE_WINDOW: Duration = Duration::from_millis(8);
/// Chunks between reader and pump. When full the reader blocks, so a slow
/// UI makes the shell wait on a full PTY instead of growing memory here.
const CHUNK_QUEUE: usize = 64;
/// Input chunks waiting for the writer thread.
const INPUT_QUEUE: usize = 256;
/// Largest single input accepted.
const MAX_INPUT: usize = 1024 * 1024;
const MAX_DIM: u16 = 1000;
/// After the shell exits, how long to wait for its last output to be sent.
const DRAIN_TIMEOUT: Duration = Duration::from_millis(500);
/// After SIGHUP, a shell that is still alive gets SIGKILL to its group.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_secs(1);

/// Registry of open terminals. Cheap to clone; all clones share state.
#[derive(Clone, Default)]
pub struct PtyManager {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    next_seq: AtomicU64,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

pub struct OpenParams {
    pub project_id: String,
    pub session_id: Option<String>,
    /// Working directory of the shell (the project folder).
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

struct Session {
    info: PtyInfo,
    /// Open order, for stable listings.
    seq: u64,
    control: Arc<Control>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: SyncSender<Vec<u8>>,
}

/// Process control, shared by the session and its threads.
struct Control {
    /// The shell's pid. On Unix it is also its process group id: the shell
    /// is a session leader (portable-pty calls `setsid`).
    pid: Option<u32>,
    /// Set by the exit thread once the shell has been reaped. Signals are
    /// only sent while this is false, so a reused pid is never hit.
    exited: AtomicBool,
    kill_requested: AtomicBool,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

/// Set once `Exit` has been sent: no output may follow it.
type Gate = Arc<Mutex<bool>>;

impl PtyManager {
    /// Start `shell` in `params.cwd` on a new PTY and register it.
    pub fn open(&self, params: OpenParams, shell: &ShellSpec, sink: EventSink) -> IpcResult<PtyInfo> {
        if !params.cwd.is_dir() {
            return Err(IpcError {
                code: IpcErrorCode::FolderMissing,
                message: "Le dossier du projet n'existe plus.".into(),
            });
        }
        let pair = native_pty_system()
            .openpty(pty_size(params.cols, params.rows))
            .map_err(|e| io_error("Impossible d'ouvrir un terminal", e))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io_error("Impossible de lire le terminal", e))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| io_error("Impossible d'écrire dans le terminal", e))?;

        let mut command = CommandBuilder::new(&shell.program);
        command.args(&shell.args);
        command.cwd(&params.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| io_error(&format!("Impossible de lancer {}", shell.program), e))?;
        // Our copy of the slave must close, or the reader never sees the end.
        drop(pair.slave);

        let control = Arc::new(Control {
            pid: child.process_id(),
            exited: AtomicBool::new(false),
            kill_requested: AtomicBool::new(false),
            killer: Mutex::new(child.clone_killer()),
        });
        let seq = self.inner.next_seq.fetch_add(1, Ordering::Relaxed);
        let info = PtyInfo {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: params.project_id,
            session_id: params.session_id,
            shell: shell.program.clone(),
            cwd: params.cwd.to_string_lossy().into_owned(),
        };
        let (chunks_tx, chunks_rx) = mpsc::sync_channel::<Vec<u8>>(CHUNK_QUEUE);
        let (input_tx, input_rx) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
        let (drained_tx, drained_rx) = mpsc::channel::<()>();
        let gate: Gate = Arc::new(Mutex::new(false));

        // Registered before any thread can report an exit.
        self.inner.sessions.lock().insert(
            info.id.clone(),
            Arc::new(Session {
                info: info.clone(),
                seq,
                control: Arc::clone(&control),
                master: Mutex::new(pair.master),
                input: input_tx,
            }),
        );

        let inner = Arc::clone(&self.inner);
        let id = info.id.clone();
        let exit_control = Arc::clone(&control);
        let exit_gate = Arc::clone(&gate);
        let exit_sink = Arc::clone(&sink);
        let output_gate = Arc::clone(&gate);
        let output_control = Arc::clone(&control);
        let launched = (move || -> std::io::Result<()> {
            spawn_named("pty-reader", move || read_loop(reader, chunks_tx))?;
            spawn_named("pty-output", move || {
                pump_output(chunks_rx, output_gate, sink, output_control, drained_tx)
            })?;
            spawn_named("pty-input", move || write_loop(writer, input_rx))?;
            spawn_named("pty-exit", move || {
                wait_for_exit(child, exit_control, drained_rx, exit_gate, exit_sink, move || {
                    inner.remove_exited(&id, seq)
                })
            })
        })();

        if let Err(err) = launched {
            control.kill();
            self.inner.remove_exited(&info.id, seq);
            return Err(io_error("Impossible de démarrer le terminal", err));
        }
        Ok(info)
    }

    /// Queue keyboard input. Returns once queued, never waits on the shell.
    pub fn write(&self, id: &str, data: &str) -> IpcResult<()> {
        if data.len() > MAX_INPUT {
            return Err(IpcError {
                code: IpcErrorCode::Invalid,
                message: "Saisie trop longue pour le terminal.".into(),
            });
        }
        let session = self.session(id)?;
        match session.input.try_send(data.as_bytes().to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(IpcError {
                code: IpcErrorCode::Rejected,
                message: "Le terminal ne suit pas la saisie : réessayez.".into(),
            }),
            Err(TrySendError::Disconnected(_)) => Err(not_found()),
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> IpcResult<()> {
        let session = self.session(id)?;
        let size = pty_size(cols, rows);
        session
            .master
            .lock()
            .resize(size)
            .map_err(|e| io_error("Redimensionnement du terminal impossible", e))
    }

    /// Stop a terminal. It leaves the registry at once; `Exit` follows when
    /// the shell is gone. Unknown ids are ignored.
    pub fn kill(&self, id: &str) {
        let removed = self.inner.sessions.lock().remove(id);
        if let Some(session) = removed {
            session.control.kill();
        }
    }

    pub fn kill_all(&self) {
        let sessions: Vec<Arc<Session>> = self.inner.sessions.lock().drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.control.kill();
        }
    }

    /// Open terminals, oldest first, optionally for one project.
    pub fn list(&self, project_id: Option<&str>) -> Vec<PtyInfo> {
        let sessions = self.inner.sessions.lock();
        let mut found: Vec<(u64, PtyInfo)> = sessions
            .values()
            .filter(|s| project_id.is_none_or(|p| s.info.project_id == p))
            .map(|s| (s.seq, s.info.clone()))
            .collect();
        found.sort_by_key(|(seq, _)| *seq);
        found.into_iter().map(|(_, info)| info).collect()
    }

    fn session(&self, id: &str) -> IpcResult<Arc<Session>> {
        self.inner.sessions.lock().get(id).cloned().ok_or_else(not_found)
    }
}

impl Inner {
    /// Forget a terminal whose shell exited. Checks `seq` so a newer session
    /// under the same id (never happens with uuids, but cheap) is kept.
    fn remove_exited(&self, id: &str, seq: u64) {
        let mut sessions = self.sessions.lock();
        if sessions.get(id).is_some_and(|s| s.seq == seq) {
            sessions.remove(id);
        }
    }
}

impl Control {
    /// Ask the shell to exit: SIGHUP, as a closed terminal would. On Unix the
    /// whole process group gets it, and a shell still alive after a grace
    /// period is SIGKILLed. Calling it again has no effect.
    fn kill(self: &Arc<Self>) {
        if self.exited.load(Ordering::SeqCst) || self.kill_requested.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.killer.lock().kill();
        #[cfg(unix)]
        {
            if let Some(pid) = self.pid {
                sys::signal_group(pid, sys::SIGHUP);
                let control = Arc::clone(self);
                let _ = thread::Builder::new()
                    .name("pty-kill".into())
                    .spawn(move || {
                        thread::sleep(KILL_GRACE);
                        if !control.exited.load(Ordering::SeqCst) {
                            sys::signal_group(pid, sys::SIGKILL);
                        }
                    });
            }
        }
    }
}

/// PTY → chunks. Ends only at end of stream (the shell and its jobs closed
/// the terminal, which is also what lets the shell finish exiting). When the
/// pump is gone the chunks are discarded, never the reading itself.
fn read_loop(mut reader: Box<dyn Read + Send>, chunks: SyncSender<Vec<u8>>) {
    let mut buf = vec![0u8; READ_BUF];
    // Once the pump is gone the output has no consumer, but the reader must
    // keep draining until end of stream: a shell cannot finish exiting while
    // its terminal still holds unread output.
    let mut consumer_gone = false;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if !consumer_gone && chunks.send(buf[..n].to_vec()).is_err() {
                    consumer_gone = true;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            // EIO once every holder of the slave is gone.
            Err(_) => break,
        }
    }
}

/// Chunks → `Data` events, batched. If the sink reports the consumer is gone,
/// the terminal is killed (the reader keeps draining, see `read_loop`).
/// `drained` is signalled once no more output can be forwarded.
fn pump_output(
    chunks: Receiver<Vec<u8>>,
    gate: Gate,
    sink: EventSink,
    control: Arc<Control>,
    drained: mpsc::Sender<()>,
) {
    while let Ok(mut batch) = chunks.recv() {
        let deadline = Instant::now() + COALESCE_WINDOW;
        while batch.len() < COALESCE_BYTES {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match chunks.recv_timeout(left) {
                Ok(more) => batch.extend_from_slice(&more),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        let closed = gate.lock();
        if *closed {
            break;
        }
        let delivered = sink(PtyEvent::Data { data: STANDARD.encode(&batch) });
        drop(closed);
        if !delivered {
            control.kill();
            break;
        }
    }
    let _ = drained.send(());
}

/// Queued input → PTY, in order. Ends when the session is dropped or the
/// PTY stops accepting data.
fn write_loop(mut writer: Box<dyn Write + Send>, input: Receiver<Vec<u8>>) {
    while let Ok(bytes) = input.recv() {
        if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
            break;
        }
    }
}

/// Wait for the shell, let the last output drain, forget the session, then
/// report `Exit`. The code is `None` when the shell died from a signal.
fn wait_for_exit(
    mut child: Box<dyn Child + Send + Sync>,
    control: Arc<Control>,
    drained: Receiver<()>,
    gate: Gate,
    sink: EventSink,
    forget: impl FnOnce(),
) {
    let status = child.wait();
    control.exited.store(true, Ordering::SeqCst);
    let _ = drained.recv_timeout(DRAIN_TIMEOUT);
    forget();
    let code = match status {
        Ok(status) if status.signal().is_none() => i32::try_from(status.exit_code()).ok(),
        _ => None,
    };
    let mut closed = gate.lock();
    *closed = true;
    let _ = sink(PtyEvent::Exit { code });
}

fn spawn_named(name: &str, body: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    thread::Builder::new()
        .name(name.to_string())
        .spawn(body)
        .map(|_| ())
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(1, MAX_DIM),
        cols: cols.clamp(1, MAX_DIM),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn io_error(context: &str, err: impl Display) -> IpcError {
    IpcError {
        code: IpcErrorCode::Io,
        message: format!("{context} ({err})"),
    }
}

fn not_found() -> IpcError {
    IpcError {
        code: IpcErrorCode::NotFound,
        message: "Ce terminal n'existe plus.".into(),
    }
}

#[cfg(unix)]
mod sys {
    pub const SIGHUP: i32 = 1;
    pub const SIGKILL: i32 = 9;

    unsafe extern "C" {
        fn killpg(pgrp: i32, sig: i32) -> i32;
    }

    /// Signal every process of group `pgid`. A group that is already gone is
    /// not an error.
    pub fn signal_group(pgid: u32, sig: i32) {
        // SAFETY: killpg only delivers a signal to a process group.
        unsafe {
            killpg(pgid as i32, sig);
        }
    }
}
