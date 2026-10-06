//! envd request handlers.
//!
//! These run *inside* the sandbox process — they touch the real local
//! filesystem and spawn real processes via `tokio::process`. The host
//! reaches them via a Unix socket.

use crate::envd::protocol::{methods, Request, Response};
use crate::types::{FileEntry, FileKind, RunOutput, RunRequest};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub async fn dispatch(req: Request) -> Response {
    let id = req.id.clone();
    let result = match req.method.as_str() {
        methods::PING => Ok(json!({ "ok": true, "ts": chrono::Utc::now().timestamp_millis() })),
        methods::INFO => Ok(json!({
            "envd_version": env!("CARGO_PKG_VERSION"),
            "uname": uname_string(),
        })),
        methods::PROCESS_RUN => process_run(req.params).await,
        methods::FS_READ => fs_read(req.params).await,
        methods::FS_WRITE => fs_write(req.params).await,
        methods::FS_LIST => fs_list(req.params).await,
        methods::FS_STAT => fs_stat(req.params).await,
        methods::FS_MKDIR => fs_mkdir(req.params).await,
        methods::FS_REMOVE => fs_remove(req.params).await,
        other => Err(format!("unknown method: {other}")),
    };
    match result {
        Ok(v) => Response::ok(id, v),
        Err(msg) => Response::error(id, -32000, msg),
    }
}

fn uname_string() -> String {
    #[cfg(unix)]
    {
        match nix::sys::utsname::uname() {
            Ok(u) => format!(
                "{} {} {}",
                u.sysname().to_string_lossy(),
                u.release().to_string_lossy(),
                u.machine().to_string_lossy()
            ),
            Err(_) => "unknown".into(),
        }
    }
    #[cfg(not(unix))]
    {
        "unknown".into()
    }
}

async fn process_run(params: Option<Value>) -> Result<Value, String> {
    let req: RunRequest = parse_params(params)?;
    let timeout = req.timeout.unwrap_or(Duration::from_secs(120));
    let cwd = req
        .workdir
        .clone()
        .unwrap_or_else(|| PathBuf::from("/work"));
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(&req.command).current_dir(&cwd);
    for (k, v) in &req.env {
        cmd.env(k, v);
    }
    cmd.kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    let res = tokio::time::timeout(timeout, child.wait_with_output()).await;
    let out = match res {
        Ok(Ok(o)) => RunOutput {
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            exit_code: o.status.code().unwrap_or(-1),
            timed_out: false,
            pid: None,
        },
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => RunOutput {
            stdout: String::new(),
            stderr: format!("timeout after {timeout:?}"),
            exit_code: -1,
            timed_out: true,
            pid: None,
        },
    };
    serde_json::to_value(out).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

#[derive(Deserialize)]
struct WriteParams {
    path: String,
    /// Base64-encoded contents.
    data_b64: String,
}

#[derive(Deserialize)]
struct ListParams {
    path: String,
    depth: Option<u32>,
}

#[derive(Deserialize)]
struct MkdirParams {
    path: String,
    recursive: Option<bool>,
}

#[derive(Deserialize)]
struct RemoveParams {
    path: String,
    recursive: Option<bool>,
}

fn parse_params<T: serde::de::DeserializeOwned>(p: Option<Value>) -> Result<T, String> {
    let v = p.ok_or_else(|| "missing params".to_string())?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

async fn fs_read(params: Option<Value>) -> Result<Value, String> {
    use base64::Engine;
    let p: PathParams = parse_params(params)?;
    let mut f = tokio::fs::File::open(&p.path)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(json!({ "data_b64": b64 }))
}

async fn fs_write(params: Option<Value>) -> Result<Value, String> {
    use base64::Engine;
    let p: WriteParams = parse_params(params)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&p.data_b64)
        .map_err(|e| e.to_string())?;
    if let Some(parent) = Path::new(&p.path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    tokio::fs::write(&p.path, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "bytes_written": bytes.len() }))
}

async fn fs_list(params: Option<Value>) -> Result<Value, String> {
    let p: ListParams = parse_params(params)?;
    let depth = p.depth.unwrap_or(1);
    let mut out = Vec::new();
    walk(Path::new(&p.path), Path::new(&p.path), depth, &mut out).map_err(|e| e.to_string())?;
    serde_json::to_value(out).map_err(|e| e.to_string())
}

async fn fs_stat(params: Option<Value>) -> Result<Value, String> {
    let p: PathParams = parse_params(params)?;
    let meta = tokio::fs::metadata(&p.path)
        .await
        .map_err(|e| e.to_string())?;
    let entry = to_entry(Path::new(&p.path), &meta);
    serde_json::to_value(entry).map_err(|e| e.to_string())
}

async fn fs_mkdir(params: Option<Value>) -> Result<Value, String> {
    let p: MkdirParams = parse_params(params)?;
    if p.recursive.unwrap_or(true) {
        tokio::fs::create_dir_all(&p.path)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        tokio::fs::create_dir(&p.path)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(json!({ "ok": true }))
}

async fn fs_remove(params: Option<Value>) -> Result<Value, String> {
    let p: RemoveParams = parse_params(params)?;
    let path = PathBuf::from(&p.path);
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| e.to_string())?;
    if meta.is_dir() {
        if p.recursive.unwrap_or(false) {
            tokio::fs::remove_dir_all(&path)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            tokio::fs::remove_dir(&path)
                .await
                .map_err(|e| e.to_string())?;
        }
    } else {
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(json!({ "ok": true }))
}

fn to_entry(path: &Path, meta: &std::fs::Metadata) -> FileEntry {
    let kind = if meta.is_dir() {
        FileKind::Dir
    } else if meta.file_type().is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::File
    };
    let modified_unix_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    FileEntry {
        path: path.to_path_buf(),
        kind,
        size: meta.len(),
        modified_unix_ms,
    }
}

fn walk(base: &Path, current: &Path, depth: u32, out: &mut Vec<FileEntry>) -> std::io::Result<()> {
    if depth == 0 {
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let path = entry.path();
        let _ = base;
        out.push(to_entry(&path, &meta));
        if meta.is_dir() && depth > 1 {
            walk(base, &path, depth - 1, out)?;
        }
    }
    Ok(())
}
