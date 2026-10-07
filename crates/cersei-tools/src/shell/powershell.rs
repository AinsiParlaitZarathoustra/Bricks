//! A persistent PowerShell (`pwsh`, or Windows PowerShell) driven over a
//! loopback control channel.
//!
//! One process for the life of the session; each request is dot-sourced in
//! the driver's scope (`driver.ps1`), so `$env:…`, `Set-Location`, variables,
//! functions and aliases persist. Outputs, error records and the final status
//! come back as JSON lines on an authenticated loopback TCP connection,
//! never through the process's stdin/stdout.
//!
//! The status distinguishes PowerShell's own success (`$?`), cmdlet errors,
//! and the exit code of a native program — reported only when one ran in
//! that request (`$LASTEXITCODE` is reset before each request).
//!
//! A timeout, `exit`, a crash or a lost connection ends the session: the
//! whole process tree is terminated and the next command starts a new one,
//! which is reported. Unlike bash, a foreground command is not stopped on
//! its own: the session is replaced.

use super::capture::{Captured, StreamCapture};
use super::{procs, Progress, ShellConfig};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub(crate) const DRIVER: &str = include_str!("driver.ps1");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PwshStatus {
    /// The request completed. `ok` is PowerShell's `$?`; `native_exit` is the
    /// exit code of the last native program it ran, if any.
    Completed {
        ok: bool,
        cmdlet_errors: u64,
        native_exit: Option<i32>,
    },
    /// Timed out; the session was destroyed.
    TimedOut,
    /// The process ended during the request (`exit`, crash, lost channel).
    SessionEnded { code: Option<i32> },
}

#[derive(Debug, Clone)]
pub struct PwshOutcome {
    pub status: PwshStatus,
    pub stdout: Captured,
    pub stderr: Captured,
    pub duration: Duration,
    pub cwd: Option<PathBuf>,
    pub notes: Vec<String>,
}

struct Live {
    child: tokio::process::Child,
    lines: tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

pub struct PwshSession {
    pid: u32,
    dir: PathBuf,
    config: ShellConfig,
    live: tokio::sync::Mutex<Option<Live>>,
    dead: AtomicBool,
    next_id: AtomicU64,
}

fn find_pwsh(config: &ShellConfig) -> Option<PathBuf> {
    config
        .pwsh_path
        .clone()
        .or_else(|| which::which("pwsh").ok())
        .or_else(|| {
            if cfg!(windows) {
                which::which("powershell").ok()
            } else {
                None
            }
        })
}

impl PwshSession {
    pub async fn start(
        cwd: &Path,
        config: ShellConfig,
        dir: PathBuf,
    ) -> std::io::Result<Arc<Self>> {
        let exe = find_pwsh(&config).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "PowerShell was not found (pwsh, or powershell on Windows)",
            )
        })?;
        std::fs::create_dir_all(&dir)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let token = uuid::Uuid::new_v4().to_string();
        let encoded = {
            use base64::Engine;
            let utf16: Vec<u8> = DRIVER
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect();
            base64::engine::general_purpose::STANDARD.encode(utf16)
        };
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded,
        ])
        .current_dir(cwd)
        .env("BRICKS_PS_PORT", port.to_string())
        .env("BRICKS_PS_TOKEN", &token)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
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
        let accepted = tokio::time::timeout(Duration::from_secs(30), listener.accept()).await;
        let stream = match accepted {
            Ok(Ok((s, _))) => s,
            _ => {
                let _ = child.start_kill();
                return Err(std::io::Error::other(
                    "PowerShell did not connect its driver",
                ));
            }
        };
        let (read, writer) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let ready = tokio::time::timeout(Duration::from_secs(30), lines.next_line()).await;
        let ok = matches!(&ready, Ok(Ok(Some(l))) if serde_json::from_str::<serde_json::Value>(l)
            .ok()
            .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(|t| t == token))
            .unwrap_or(false));
        if !ok {
            let _ = child.start_kill();
            return Err(std::io::Error::other("PowerShell driver handshake failed"));
        }
        Ok(Arc::new(Self {
            pid,
            dir,
            config,
            live: tokio::sync::Mutex::new(Some(Live {
                child,
                lines,
                writer,
            })),
            dead: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        }))
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    pub async fn run(
        &self,
        command: &str,
        timeout: Duration,
        progress: Option<Progress>,
    ) -> std::io::Result<PwshOutcome> {
        let mut guard = self.live.lock().await;
        let Some(live) = guard.as_mut() else {
            return Err(std::io::Error::other("the PowerShell session has ended"));
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let script = {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(command.as_bytes())
        };
        let request = serde_json::json!({ "id": id, "script": script }).to_string();
        let started = Instant::now();
        live.writer
            .write_all(format!("{request}\n").as_bytes())
            .await?;

        let mut out = StreamCapture::new(
            self.config.capture.clone(),
            self.dir.join(format!("raw-{id}.stdout")),
        );
        let mut err = StreamCapture::new(
            self.config.capture.clone(),
            self.dir.join(format!("raw-{id}.stderr")),
        );
        let deadline = tokio::time::Instant::now() + timeout;
        let mut ticker = tokio::time::interval(self.config.progress_every);
        ticker.tick().await;
        let mut notes = Vec::new();
        let (status, cwd) = loop {
            tokio::select! {
                line = live.lines.next_line() => match line {
                    Ok(Some(l)) => {
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(&l) else { continue };
                        if v.get("id").and_then(|x| x.as_u64()) != Some(id) {
                            continue;
                        }
                        let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        match v.get("kind").and_then(|k| k.as_str()) {
                            Some("out") => out.push(text.as_bytes()),
                            Some("err") => err.push(text.as_bytes()),
                            Some("end") => {
                                let native_exit = v.get("native_exit").and_then(|n| n.as_i64()).map(|n| n as i32);
                                break (
                                    PwshStatus::Completed {
                                        ok: v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false),
                                        cmdlet_errors: v.get("cmdlet_errors").and_then(|n| n.as_u64()).unwrap_or(0),
                                        native_exit,
                                    },
                                    v.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from),
                                );
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        self.dead.store(true, Ordering::SeqCst);
                        let code = tokio::time::timeout(Duration::from_secs(2), live.child.wait())
                            .await
                            .ok()
                            .and_then(|r| r.ok())
                            .and_then(|s| s.code());
                        procs::terminate_tree_now(self.pid);
                        notes.push(
                            "the PowerShell session ended during this command; its state is lost and the \
                             next command starts a new session"
                                .to_string(),
                        );
                        break (PwshStatus::SessionEnded { code }, None);
                    }
                },
                _ = tokio::time::sleep_until(deadline) => {
                    self.dead.store(true, Ordering::SeqCst);
                    procs::terminate_tree_now(self.pid);
                    let _ = tokio::time::timeout(Duration::from_secs(2), live.child.wait()).await;
                    notes.push(
                        "the PowerShell session was destroyed to stop the command; the next command \
                         starts a new session"
                            .to_string(),
                    );
                    break (PwshStatus::TimedOut, None);
                }
                _ = ticker.tick() => {
                    if let Some(p) = &progress {
                        p(format!(
                            "still running after {}",
                            cersei_types::duration::display_ms(started.elapsed())
                        ));
                    }
                }
            }
        };
        if self.is_dead() {
            *guard = None;
        }
        Ok(PwshOutcome {
            status,
            stdout: out.finish(),
            stderr: err.finish(),
            duration: started.elapsed(),
            cwd,
            notes,
        })
    }

    pub async fn close(&self) {
        self.dead.store(true, Ordering::SeqCst);
        procs::terminate_tree_now(self.pid);
        if let Ok(mut guard) = tokio::time::timeout(Duration::from_secs(2), self.live.lock()).await
        {
            if let Some(live) = guard.as_mut() {
                let _ = tokio::time::timeout(Duration::from_secs(2), live.child.wait()).await;
            }
            *guard = None;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    pub fn kill_now(&self) {
        self.dead.store(true, Ordering::SeqCst);
        procs::terminate_tree_now(self.pid);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
