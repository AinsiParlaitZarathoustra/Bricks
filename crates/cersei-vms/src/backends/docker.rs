//! `DockerRuntime` — real container isolation via the local `docker` CLI.
//!
//! Phase 1 implementation uses `docker` as a subprocess. It works on macOS,
//! Linux, and Windows wherever Docker / Docker Desktop is installed.
//!
//! Lifecycle:
//!   - `create()` → `docker create [--mount ...] [--env ...] <image>` then `docker start`
//!   - `kill()`   → `docker rm -f <id>`
//!   - `snapshot()` → `docker commit <id> cersei-snapshot:<snapid>` + JSON manifest
//!   - `restore(snap)` → `create()` using the snapshot image tag
//!
//! Command execution:
//!   - `run`    → `docker exec -e ... <id> /bin/sh -c <cmd>`
//!   - `stream` → same, piped line-by-line
//!
//! Filesystem:
//!   - `read`/`write` → `docker cp` (host ⇄ container)
//!   - `list`/`stat`/`mkdir`/`remove` → `docker exec ... sh -c '...'`
//!
//! Future phases will replace the CLI with direct HTTP-over-UDS calls to
//! the Docker Engine API; the trait surface stays unchanged.

use crate::commands::{CommandStream, Commands, StreamChunk};
use crate::error::{Result, VmError};
use crate::filesystem::{Filesystem, WatchStream};
use crate::primitives::{KvStore, Mailbox};
use crate::runtime::{Sandbox, SandboxHandle, SandboxRuntime};
use crate::snapshot::{SnapshotManifest, SnapshotRegistry};
use crate::types::{
    FileEntry, FileKind, RunOutput, RunRequest, RuntimeCaps, SandboxId, SandboxInfo, SandboxOpts,
    SandboxStatus, Signal, SnapshotId,
};
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_TIMEOUT: Duration = Duration::from_secs(600);
const SNAPSHOT_IMAGE_PREFIX: &str = "cersei-snapshot";

pub struct DockerRuntime {
    inner: Arc<DockerInner>,
}

struct DockerInner {
    sandboxes: DashMap<SandboxId, Arc<DockerSandbox>>,
    mailbox: Mailbox,
    kv: KvStore,
    snapshots: SnapshotRegistry,
    docker_bin: String,
}

impl DockerRuntime {
    pub fn new() -> Result<Self> {
        Self::with_docker_bin("docker")
    }

    pub fn with_docker_bin(bin: impl Into<String>) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(DockerInner {
                sandboxes: DashMap::new(),
                mailbox: Mailbox::new(),
                kv: KvStore::in_memory(),
                snapshots: SnapshotRegistry::default_user()?,
                docker_bin: bin.into(),
            }),
        })
    }

    pub fn mailbox(&self) -> Mailbox {
        self.inner.mailbox.clone()
    }

    pub fn kv(&self) -> KvStore {
        self.inner.kv.clone()
    }

    pub fn snapshots(&self) -> SnapshotRegistry {
        self.inner.snapshots.clone()
    }

    async fn docker_text(&self, args: &[&str]) -> Result<String> {
        let out = Command::new(&self.inner.docker_bin)
            .args(args)
            .output()
            .await
            .map_err(|e| VmError::Backend {
                backend: "docker".into(),
                message: format!("spawn: {e}"),
            })?;
        if !out.status.success() {
            return Err(VmError::Backend {
                backend: "docker".into(),
                message: format!(
                    "docker {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[async_trait]
impl SandboxRuntime for DockerRuntime {
    fn name(&self) -> &str {
        "docker"
    }

    fn capabilities(&self) -> RuntimeCaps {
        RuntimeCaps {
            snapshots: true,
            pause_resume: false, // Phase 2: docker pause / unpause
            gpu: false,
            network_isolation: true,
            shared_volumes: true,
            remote: false,
        }
    }

    async fn create(&self, opts: SandboxOpts) -> Result<SandboxHandle> {
        let id = SandboxId::new();
        let container_name = format!("cersei-vm-{}", id.as_str());

        let image = match &opts.from_snapshot {
            Some(snap) => {
                // Verify snapshot exists; use its image tag.
                let _ = self.inner.snapshots.get(snap)?;
                format!("{SNAPSHOT_IMAGE_PREFIX}:{}", snap.as_str())
            }
            None => opts.image.clone(),
        };

        let mut args: Vec<String> = vec![
            "create".into(),
            "--name".into(),
            container_name.clone(),
            "-l".into(),
            "cersei.sandbox=true".into(),
            "-l".into(),
            format!("cersei.id={}", id.as_str()),
        ];
        for (k, v) in &opts.labels {
            args.push("-l".into());
            args.push(format!("{k}={v}"));
        }
        for (k, v) in &opts.env {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        if let Some(workdir) = &opts.workdir {
            args.push("-w".into());
            args.push(workdir.display().to_string());
        }
        for v in &opts.volumes {
            args.push("-v".into());
            // host_path:container_path[:ro]
            // We resolve host_path through the host VolumeRegistry; callers
            // are expected to set `volume_id`'s host_path before mount.
            // For now, we accept a pre-resolved host path encoded in mount_path
            // tag — Phase 1 simplification — and assume the user passes a
            // direct host bind in `mount_path` when not using a VolumeRegistry.
            // Most callers will route through VolumeMountSpec helpers.
            let mode = if v.read_only { ":ro" } else { "" };
            args.push(format!(
                "{}:{}{}",
                v.volume_id.as_str(),
                v.mount_path.display(),
                mode
            ));
        }
        if let Some(cpu) = opts.cpu_limit {
            args.push("--cpus".into());
            args.push(format!("{cpu}"));
        }
        if let Some(mem) = opts.mem_limit {
            args.push("--memory".into());
            args.push(format!("{mem}"));
        }
        // Keep the container alive — agents inject their own commands via `exec`.
        args.push(image.clone());
        args.push("/bin/sh".into());
        args.push("-c".into());
        args.push("tail -f /dev/null".into());

        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.docker_text(&args_ref).await?;
        self.docker_text(&["start", &container_name]).await?;

        let sandbox = Arc::new(DockerSandbox {
            id: id.clone(),
            container_name,
            image,
            labels: opts.labels.clone(),
            status: RwLock::new(SandboxStatus::Running),
            created_at: chrono::Utc::now(),
            mailbox: self.inner.mailbox.clone(),
            kv: self.inner.kv.clone(),
            snapshots: self.inner.snapshots.clone(),
            docker_bin: self.inner.docker_bin.clone(),
            opts,
        });
        self.inner.sandboxes.insert(id.clone(), sandbox.clone());
        Ok(sandbox)
    }

    async fn get(&self, id: &SandboxId) -> Result<SandboxHandle> {
        self.inner
            .sandboxes
            .get(id)
            .map(|kv| kv.value().clone() as SandboxHandle)
            .ok_or_else(|| VmError::NotFound(id.to_string()))
    }

    async fn list(&self) -> Result<Vec<SandboxInfo>> {
        Ok(self
            .inner
            .sandboxes
            .iter()
            .map(|kv| kv.value().info())
            .collect())
    }

    async fn restore(&self, snapshot: &SnapshotId) -> Result<SandboxHandle> {
        let manifest = self.inner.snapshots.get(snapshot)?;
        let mut opts = manifest.original_opts.clone();
        opts.from_snapshot = Some(snapshot.clone());
        let handle = self.create(opts).await?;
        self.inner.kv.restore(manifest.kv.clone())?;
        Ok(handle)
    }
}

pub(crate) struct DockerSandbox {
    id: SandboxId,
    container_name: String,
    image: String,
    labels: HashMap<String, String>,
    status: RwLock<SandboxStatus>,
    created_at: chrono::DateTime<chrono::Utc>,
    mailbox: Mailbox,
    kv: KvStore,
    snapshots: SnapshotRegistry,
    docker_bin: String,
    opts: SandboxOpts,
}

#[async_trait]
impl Sandbox for DockerSandbox {
    fn id(&self) -> &SandboxId {
        &self.id
    }

    fn info(&self) -> SandboxInfo {
        SandboxInfo {
            id: self.id.clone(),
            backend: "docker".to_string(),
            image: self.image.clone(),
            status: *self.status.read(),
            created_at: self.created_at,
            labels: self.labels.clone(),
        }
    }

    fn commands(&self) -> Arc<dyn Commands> {
        Arc::new(DockerCommands {
            container_name: self.container_name.clone(),
            docker_bin: self.docker_bin.clone(),
            env: self.opts.env.clone(),
            workdir: self.opts.workdir.clone(),
        })
    }

    fn filesystem(&self) -> Arc<dyn Filesystem> {
        Arc::new(DockerFilesystem {
            container_name: self.container_name.clone(),
            docker_bin: self.docker_bin.clone(),
        })
    }

    async fn snapshot(&self) -> Result<SnapshotId> {
        let id = SnapshotId::new();
        let tag = format!("{SNAPSHOT_IMAGE_PREFIX}:{}", id.as_str());
        let out = Command::new(&self.docker_bin)
            .args(["commit", &self.container_name, &tag])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Snapshot(format!(
                "docker commit failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let manifest = SnapshotManifest {
            id: id.clone(),
            backend: "docker".to_string(),
            fs_pointer: tag,
            original_opts: self.opts.clone(),
            volumes: self.opts.volumes.clone(),
            kv: self.kv.snapshot(),
            mailbox_topics: self.mailbox.topics(),
            created_at: chrono::Utc::now(),
            labels: self.labels.clone(),
        };
        self.snapshots.put(manifest)?;
        Ok(id)
    }

    async fn pause(&self) -> Result<()> {
        let out = Command::new(&self.docker_bin)
            .args(["pause", &self.container_name])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Lifecycle(format!(
                "docker pause: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        *self.status.write() = SandboxStatus::Paused;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        let out = Command::new(&self.docker_bin)
            .args(["unpause", &self.container_name])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Lifecycle(format!(
                "docker unpause: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        *self.status.write() = SandboxStatus::Running;
        Ok(())
    }

    async fn kill(&self) -> Result<()> {
        let _ = Command::new(&self.docker_bin)
            .args(["rm", "-f", &self.container_name])
            .output()
            .await;
        *self.status.write() = SandboxStatus::Killed;
        Ok(())
    }
}

struct DockerCommands {
    container_name: String,
    docker_bin: String,
    env: HashMap<String, String>,
    workdir: Option<PathBuf>,
}

#[async_trait]
impl Commands for DockerCommands {
    async fn run(&self, req: RunRequest) -> Result<RunOutput> {
        let timeout = req.timeout.unwrap_or(DEFAULT_TIMEOUT).min(MAX_TIMEOUT);
        let mut args: Vec<String> = vec!["exec".into()];
        if let Some(w) = req.workdir.as_ref().or(self.workdir.as_ref()) {
            args.push("-w".into());
            args.push(w.display().to_string());
        }
        for (k, v) in self.env.iter().chain(req.env.iter()) {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        if req.background {
            args.push("-d".into());
        }
        args.push(self.container_name.clone());
        args.push("/bin/sh".into());
        args.push("-c".into());
        args.push(req.command.clone());

        let child = Command::new(&self.docker_bin)
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| VmError::Backend {
                backend: "docker".into(),
                message: format!("exec spawn: {e}"),
            })?;

        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => Ok(RunOutput {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                exit_code: out.status.code().unwrap_or(-1),
                timed_out: false,
                pid: None,
            }),
            Ok(Err(e)) => Err(VmError::Io(e)),
            Err(_) => Ok(RunOutput {
                stdout: String::new(),
                stderr: format!("timeout after {timeout:?}"),
                exit_code: -1,
                timed_out: true,
                pid: None,
            }),
        }
    }

    async fn stream(&self, req: RunRequest) -> Result<CommandStream> {
        let mut args: Vec<String> = vec!["exec".into()];
        if let Some(w) = req.workdir.as_ref().or(self.workdir.as_ref()) {
            args.push("-w".into());
            args.push(w.display().to_string());
        }
        for (k, v) in self.env.iter().chain(req.env.iter()) {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        args.push(self.container_name.clone());
        args.push("/bin/sh".into());
        args.push("-c".into());
        args.push(req.command.clone());

        let mut child = Command::new(&self.docker_bin)
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamChunk>(64);
        let _ = tx
            .send(StreamChunk::Started {
                pid: child.id().unwrap_or(0),
            })
            .await;

        let tx_out = tx.clone();
        let stdout_task = tokio::spawn(async move {
            let mut r = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = r.next_line().await {
                if tx_out
                    .send(StreamChunk::Stdout { data: line })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let tx_err = tx.clone();
        let stderr_task = tokio::spawn(async move {
            let mut r = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = r.next_line().await {
                if tx_err
                    .send(StreamChunk::Stderr { data: line })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            let exit = match child.wait().await {
                Ok(s) => s.code().unwrap_or(-1),
                Err(e) => {
                    let _ = tx
                        .send(StreamChunk::Error {
                            message: e.to_string(),
                        })
                        .await;
                    return;
                }
            };
            let _ = tx.send(StreamChunk::Exit { code: exit }).await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn signal(&self, pid: u32, sig: Signal) -> Result<()> {
        // Signals into a container map to `docker kill --signal=...` on the
        // *container PID 1*. For an arbitrary child PID we go through `kill`
        // inside the container.
        let out = Command::new(&self.docker_bin)
            .args([
                "exec",
                &self.container_name,
                "/bin/sh",
                "-c",
                &format!("kill -{} {}", sig.as_i32(), pid),
            ])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Lifecycle(format!(
                "kill -{}: {}",
                sig.as_i32(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }
}

struct DockerFilesystem {
    container_name: String,
    docker_bin: String,
}

impl DockerFilesystem {
    async fn exec_text(&self, script: &str) -> Result<String> {
        let out = Command::new(&self.docker_bin)
            .args(["exec", &self.container_name, "/bin/sh", "-c", script])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Backend {
                backend: "docker".into(),
                message: format!(
                    "exec failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[async_trait]
impl Filesystem for DockerFilesystem {
    async fn read(&self, path: &str) -> Result<Bytes> {
        // `docker cp <container>:<path> -` streams a tar archive; for raw
        // file contents we use `cat` over the exec channel — simpler and
        // avoids tar parsing in Phase 1.
        let out = Command::new(&self.docker_bin)
            .args(["exec", &self.container_name, "cat", path])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Backend {
                backend: "docker".into(),
                message: format!(
                    "read {path}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        Ok(Bytes::from(out.stdout))
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        // Use `docker cp` from a temp file → into the container.
        let tmp = tempfile::NamedTempFile::new()?;
        tokio::fs::write(tmp.path(), data).await?;
        // Ensure the destination directory exists first.
        if let Some(parent) = Path::new(path).parent() {
            self.exec_text(&format!(
                "mkdir -p {}",
                shell_quote(&parent.display().to_string())
            ))
            .await?;
        }
        let out = Command::new(&self.docker_bin)
            .args([
                "cp",
                &tmp.path().display().to_string(),
                &format!("{}:{}", self.container_name, path),
            ])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Backend {
                backend: "docker".into(),
                message: format!("docker cp: {}", String::from_utf8_lossy(&out.stderr).trim()),
            });
        }
        Ok(())
    }

    async fn list(&self, path: &str, depth: u32) -> Result<Vec<FileEntry>> {
        let script = format!(
            "find {} -mindepth 1 -maxdepth {} -printf '%y\\t%s\\t%T@\\t%p\\n' 2>/dev/null || true",
            shell_quote(path),
            depth.max(1)
        );
        let out = self.exec_text(&script).await?;
        let mut entries = Vec::new();
        for line in out.lines() {
            let parts: Vec<&str> = line.splitn(4, '\t').collect();
            if parts.len() != 4 {
                continue;
            }
            let kind = match parts[0] {
                "d" => FileKind::Dir,
                "l" => FileKind::Symlink,
                _ => FileKind::File,
            };
            let size: u64 = parts[1].parse().unwrap_or(0);
            let modified_unix_ms = parts[2]
                .parse::<f64>()
                .map(|f| (f * 1000.0) as i64)
                .unwrap_or(0);
            entries.push(FileEntry {
                path: PathBuf::from(parts[3]),
                kind,
                size,
                modified_unix_ms,
            });
        }
        Ok(entries)
    }

    async fn stat(&self, path: &str) -> Result<FileEntry> {
        let script = format!(
            "stat -c '%F|%s|%Y' {} 2>/dev/null || true",
            shell_quote(path)
        );
        let out = self.exec_text(&script).await?;
        let line = out.trim();
        if line.is_empty() {
            return Err(VmError::NotFound(path.to_string()));
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 3 {
            return Err(VmError::Backend {
                backend: "docker".into(),
                message: format!("bad stat output: {line}"),
            });
        }
        let kind = if parts[0].contains("directory") {
            FileKind::Dir
        } else if parts[0].contains("symbolic") {
            FileKind::Symlink
        } else {
            FileKind::File
        };
        let size: u64 = parts[1].parse().unwrap_or(0);
        let mtime_s: i64 = parts[2].parse().unwrap_or(0);
        Ok(FileEntry {
            path: PathBuf::from(path),
            kind,
            size,
            modified_unix_ms: mtime_s * 1000,
        })
    }

    async fn watch(&self, _path: &str, _recursive: bool) -> Result<WatchStream> {
        Err(VmError::Lifecycle(
            "watch not implemented for docker backend (Phase 1)".into(),
        ))
    }

    async fn mkdir(&self, path: &str, recursive: bool) -> Result<()> {
        let flag = if recursive { "-p " } else { "" };
        self.exec_text(&format!("mkdir {flag}{}", shell_quote(path)))
            .await?;
        Ok(())
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        let flag = if recursive { "-rf" } else { "-f" };
        self.exec_text(&format!("rm {flag} {}", shell_quote(path)))
            .await?;
        Ok(())
    }

    async fn upload(&self, local: &Path, remote: &str) -> Result<()> {
        let out = Command::new(&self.docker_bin)
            .args([
                "cp",
                &local.display().to_string(),
                &format!("{}:{}", self.container_name, remote),
            ])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Backend {
                backend: "docker".into(),
                message: format!(
                    "docker cp upload: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        Ok(())
    }

    async fn download(&self, remote: &str, local: &Path) -> Result<()> {
        let out = Command::new(&self.docker_bin)
            .args([
                "cp",
                &format!("{}:{}", self.container_name, remote),
                &local.display().to_string(),
            ])
            .output()
            .await?;
        if !out.status.success() {
            return Err(VmError::Backend {
                backend: "docker".into(),
                message: format!(
                    "docker cp download: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        Ok(())
    }
}

/// Minimal shell-quoting for paths passed to `sh -c`.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
