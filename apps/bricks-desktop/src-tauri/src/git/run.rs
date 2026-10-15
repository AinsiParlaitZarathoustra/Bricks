//! Runs git inside a project folder: bounded time, bounded output, no
//! inherited repository redirection (`GIT_DIR` and friends), no prompts.

use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{ChildStderr, Command};

use crate::contract::{IpcError, IpcErrorCode, IpcResult};

/// Upper bound for one git invocation.
const TIMEOUT: Duration = Duration::from_secs(30);
/// Stderr only feeds error messages: keep its head.
const STDERR_KEEP: u64 = 16 * 1024;

pub(super) struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    /// Stdout reached the cap: the rest was discarded and git was stopped.
    pub truncated: bool,
    pub stderr: String,
}

impl Captured {
    pub fn success(&self) -> bool {
        self.status.success()
    }

    pub fn code(&self) -> Option<i32> {
        self.status.code()
    }
}

/// Run `git <args>` in `dir`, keeping at most `cap` bytes of stdout.
pub(super) async fn capture(dir: &Path, args: &[&str], cap: usize) -> IpcResult<Captured> {
    match tokio::time::timeout(TIMEOUT, run(dir, args, cap)).await {
        Ok(result) => result,
        Err(_) => Err(io_error("Git n'a pas répondu à temps.")),
    }
}

/// Error for a git invocation that failed, carrying git's own message.
pub(super) fn failure(out: &Captured, what: &str) -> IpcError {
    let detail = if out.stderr.is_empty() {
        "aucun détail".to_string()
    } else {
        out.stderr.clone()
    };
    io_error(&format!("{what} ({detail})"))
}

pub(super) fn io_error(message: &str) -> IpcError {
    IpcError { code: IpcErrorCode::Io, message: message.to_string() }
}

async fn run(dir: &Path, args: &[&str], cap: usize) -> IpcResult<Captured> {
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.quotepath=off")
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            io_error("Git est introuvable : installez Git pour utiliser cette vue.")
        }
        _ => io_error(&format!("Impossible de lancer Git ({e}).")),
    })?;

    // Drain stderr concurrently: a full stderr pipe would stall git.
    let stderr_task = tokio::spawn(keep_head(child.stderr.take()));
    let Some(stdout) = child.stdout.take() else {
        return Err(io_error("Sortie de Git indisponible."));
    };

    let mut out = Vec::new();
    stdout
        .take(cap as u64 + 1)
        .read_to_end(&mut out)
        .await
        .map_err(|e| io_error(&format!("Lecture de la sortie Git impossible ({e}).")))?;

    let truncated = out.len() > cap;
    if truncated {
        out.truncate(cap);
        let _ = child.start_kill();
    }
    let status = child
        .wait()
        .await
        .map_err(|e| io_error(&format!("Attente de Git impossible ({e}).")))?;
    let stderr = stderr_task.await.unwrap_or_default();

    Ok(Captured {
        status,
        stdout: out,
        truncated,
        stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
    })
}

async fn keep_head(stderr: Option<ChildStderr>) -> Vec<u8> {
    let Some(mut stream) = stderr else {
        return Vec::new();
    };
    let mut kept = Vec::new();
    let _ = (&mut stream).take(STDERR_KEEP).read_to_end(&mut kept).await;
    let _ = tokio::io::copy(&mut stream, &mut tokio::io::sink()).await;
    kept
}
