//! `LocalProcessRuntime` — fallback backend that gives no real isolation.
//!
//! Commands run as direct `tokio::process` children of the host. Filesystem
//! operations target a host-side per-sandbox directory (`~/.cersei/vms/local/<id>/`)
//! plus any bind-mounted Volume host paths. Useful for tests, dev mode, and
//! `--sandbox local` on the CLI.

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

pub struct LocalProcessRuntime {
    inner: Arc<LocalInner>,
}

struct LocalInner {
    root: PathBuf,
    sandboxes: DashMap<SandboxId, Arc<LocalSandbox>>,
    mailbox: Mailbox,
    kv: KvStore,
    snapshots: SnapshotRegistry,
}

impl LocalProcessRuntime {
    pub fn new() -> Result<Self> {
        let root = dirs::home_dir()
            .ok_or_else(|| VmError::Invalid("no home directory".into()))?
            .join(".cersei")
            .join("vms")
            .join("local");
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            inner: Arc::new(LocalInner {
                root,
                sandboxes: DashMap::new(),
                mailbox: Mailbox::new(),
                kv: KvStore::in_memory(),
                snapshots: SnapshotRegistry::default_user()?,
            }),
        })
    }

    /// Create a runtime rooted under `root` (mostly for tests).
    pub fn with_root(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            inner: Arc::new(LocalInner {
                root: root.clone(),
                sandboxes: DashMap::new(),
                mailbox: Mailbox::new(),
                kv: KvStore::in_memory(),
                snapshots: SnapshotRegistry::open(root.join("snapshots"))?,
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
}

#[async_trait]
impl SandboxRuntime for LocalProcessRuntime {
    fn name(&self) -> &str {
        "local"
    }

    fn capabilities(&self) -> RuntimeCaps {
        RuntimeCaps {
            snapshots: true,
            pause_resume: false,
            gpu: false,
            network_isolation: false,
            shared_volumes: true,
            remote: false,
        }
    }

    async fn create(&self, opts: SandboxOpts) -> Result<SandboxHandle> {
        let id = SandboxId::new();
        let root = self.inner.root.join(id.as_str());
        tokio::fs::create_dir_all(&root).await?;

        let workdir = opts
            .workdir
            .clone()
            .unwrap_or_else(|| PathBuf::from("/work"));
        // Map the in-sandbox workdir to a real host dir under root.
        let host_workdir = root
            .join("rootfs")
            .join(workdir.strip_prefix("/").unwrap_or(&workdir));
        tokio::fs::create_dir_all(&host_workdir).await?;
        let _ = workdir;

        let sandbox = Arc::new(LocalSandbox {
            id: id.clone(),
            root,
            host_workdir: host_workdir.clone(),
            image: opts.image.clone(),
            env: opts.env.clone(),
            labels: opts.labels.clone(),
            status: RwLock::new(SandboxStatus::Running),
            created_at: chrono::Utc::now(),
            mailbox: self.inner.mailbox.clone(),
            kv: self.inner.kv.clone(),
            snapshots: self.inner.snapshots.clone(),
            background: DashMap::new(),
            opts: opts.clone(),
        });
        self.inner.sandboxes.insert(id.clone(), sandbox.clone());

        // Auto-subscribe to declared mailbox topics by pre-creating them.
        for topic in &opts.mailbox_topics {
            let _ = self.inner.mailbox.subscribe(topic);
        }
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
        // Recreate the sandbox using the original opts, then restore FS dump + KV.
        let mut opts = manifest.original_opts.clone();
        opts.from_snapshot = Some(snapshot.clone());
        let handle = self.create(opts).await?;

        // Local backend: fs_pointer is a directory under <root>/_snapshots/<id>.
        let src = self
            .inner
            .root
            .join("_snapshots")
            .join(&manifest.fs_pointer);
        if src.exists() {
            // Find the concrete LocalSandbox to access host_workdir.
            let id = handle.id().clone();
            if let Some(sb) = self.inner.sandboxes.get(&id) {
                copy_dir_all(&src, &sb.host_workdir)?;
            }
        }
        self.inner.kv.restore(manifest.kv.clone())?;
        Ok(handle)
    }
}

pub(crate) struct LocalSandbox {
    id: SandboxId,
    root: PathBuf,
    host_workdir: PathBuf,
    image: String,
    env: HashMap<String, String>,
    labels: HashMap<String, String>,
    status: RwLock<SandboxStatus>,
    created_at: chrono::DateTime<chrono::Utc>,
    mailbox: Mailbox,
    kv: KvStore,
    snapshots: SnapshotRegistry,
    background: DashMap<u32, tokio::task::JoinHandle<()>>,
    opts: SandboxOpts,
}

#[async_trait]
impl Sandbox for LocalSandbox {
    fn id(&self) -> &SandboxId {
        &self.id
    }

    fn info(&self) -> SandboxInfo {
        SandboxInfo {
            id: self.id.clone(),
            backend: "local".to_string(),
            image: self.image.clone(),
            status: *self.status.read(),
            created_at: self.created_at,
            labels: self.labels.clone(),
        }
    }

    fn commands(&self) -> Arc<dyn Commands> {
        Arc::new(LocalCommands {
            sandbox_root: self.root.clone(),
            host_workdir: self.host_workdir.clone(),
            env: self.env.clone(),
            background: Arc::new(self.background_handle()),
        })
    }

    fn filesystem(&self) -> Arc<dyn Filesystem> {
        Arc::new(LocalFilesystem {
            sandbox_root: self.root.clone(),
            host_workdir: self.host_workdir.clone(),
        })
    }

    async fn snapshot(&self) -> Result<SnapshotId> {
        let id = SnapshotId::new();
        let dest_rel = format!("local-{}", id.as_str());
        let dest = self
            .root
            .parent()
            .unwrap_or(&self.root)
            .join("_snapshots")
            .join(&dest_rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        copy_dir_all(&self.host_workdir, &dest)?;
        let manifest = SnapshotManifest {
            id: id.clone(),
            backend: "local".to_string(),
            fs_pointer: dest_rel,
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
        Err(VmError::Lifecycle(
            "local backend does not support pause".into(),
        ))
    }

    async fn resume(&self) -> Result<()> {
        Err(VmError::Lifecycle(
            "local backend does not support resume".into(),
        ))
    }

    async fn kill(&self) -> Result<()> {
        for kv in self.background.iter() {
            kv.value().abort();
        }
        *self.status.write() = SandboxStatus::Killed;
        // Note: we deliberately leave the rootfs on disk for forensics.
        // Callers can wipe via the runtime if desired.
        Ok(())
    }
}

impl LocalSandbox {
    fn background_handle(&self) -> DashMap<u32, tokio::task::JoinHandle<()>> {
        // Each LocalCommands instance gets its own background-bookkeeping map.
        // Phase 1 simplification — we don't share background-PID state across
        // multiple `commands()` snapshots of the same sandbox.
        DashMap::new()
    }
}

struct LocalCommands {
    sandbox_root: PathBuf,
    host_workdir: PathBuf,
    env: HashMap<String, String>,
    background: Arc<DashMap<u32, tokio::task::JoinHandle<()>>>,
}

#[async_trait]
impl Commands for LocalCommands {
    async fn run(&self, req: RunRequest) -> Result<RunOutput> {
        let timeout = req.timeout.unwrap_or(DEFAULT_TIMEOUT).min(MAX_TIMEOUT);
        let cwd = req
            .workdir
            .clone()
            .map(|p| resolve_against(&self.sandbox_root, &self.host_workdir, &p))
            .unwrap_or_else(|| self.host_workdir.clone());

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(&req.command).current_dir(&cwd);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        cmd.kill_on_drop(true);

        if req.background {
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let mut child = cmd.spawn()?;
            let pid = child.id().unwrap_or(0);
            let bg = self.background.clone();
            let handle = tokio::spawn(async move {
                let _ = child.wait().await;
                bg.remove(&pid);
            });
            self.background.insert(pid, handle);
            return Ok(RunOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
                pid: Some(pid),
            });
        }

        let child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
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
        let cwd = req
            .workdir
            .clone()
            .map(|p| resolve_against(&self.sandbox_root, &self.host_workdir, &p))
            .unwrap_or_else(|| self.host_workdir.clone());

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(&req.command).current_dir(&cwd);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamChunk>(64);

        let tx_started = tx.clone();
        let _ = tx_started.send(StreamChunk::Started { pid }).await;

        let tx_stdout = tx.clone();
        let stdout_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx_stdout
                    .send(StreamChunk::Stdout { data: line })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let tx_stderr = tx.clone();
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx_stderr
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

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Box::pin(stream))
    }

    async fn signal(&self, pid: u32, sig: Signal) -> Result<()> {
        #[cfg(unix)]
        {
            use nix::sys::signal::{self, Signal as NixSig};
            use nix::unistd::Pid;
            let s = match sig {
                Signal::Term => NixSig::SIGTERM,
                Signal::Kill => NixSig::SIGKILL,
                Signal::Int => NixSig::SIGINT,
                Signal::Hup => NixSig::SIGHUP,
                Signal::Usr1 => NixSig::SIGUSR1,
                Signal::Usr2 => NixSig::SIGUSR2,
            };
            signal::kill(Pid::from_raw(pid as i32), s)
                .map_err(|e| VmError::Lifecycle(format!("signal: {e}")))?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (pid, sig);
            Err(VmError::Lifecycle(
                "signal not supported on this platform".into(),
            ))
        }
    }
}

struct LocalFilesystem {
    sandbox_root: PathBuf,
    host_workdir: PathBuf,
}

impl LocalFilesystem {
    fn resolve(&self, path: &str) -> PathBuf {
        resolve_against(&self.sandbox_root, &self.host_workdir, Path::new(path))
    }
}

#[async_trait]
impl Filesystem for LocalFilesystem {
    async fn read(&self, path: &str) -> Result<Bytes> {
        let p = self.resolve(path);
        let bytes = tokio::fs::read(&p).await?;
        Ok(Bytes::from(bytes))
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let p = self.resolve(path);
        if let Some(parent) = p.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&p, data).await?;
        Ok(())
    }

    async fn list(&self, path: &str, depth: u32) -> Result<Vec<FileEntry>> {
        let root = self.resolve(path);
        let mut out = Vec::new();
        walk(&root, &root, depth, &mut out)?;
        Ok(out)
    }

    async fn stat(&self, path: &str) -> Result<FileEntry> {
        let p = self.resolve(path);
        let meta = tokio::fs::metadata(&p).await?;
        Ok(to_entry(&p, Path::new(path), &meta))
    }

    async fn watch(&self, _path: &str, _recursive: bool) -> Result<WatchStream> {
        Err(VmError::Lifecycle(
            "watch not implemented for local backend (Phase 1)".into(),
        ))
    }

    async fn mkdir(&self, path: &str, recursive: bool) -> Result<()> {
        let p = self.resolve(path);
        if recursive {
            tokio::fs::create_dir_all(&p).await?;
        } else {
            tokio::fs::create_dir(&p).await?;
        }
        Ok(())
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        let p = self.resolve(path);
        let meta = tokio::fs::metadata(&p).await?;
        if meta.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(&p).await?;
            } else {
                tokio::fs::remove_dir(&p).await?;
            }
        } else {
            tokio::fs::remove_file(&p).await?;
        }
        Ok(())
    }

    async fn upload(&self, local: &Path, remote: &str) -> Result<()> {
        let dest = self.resolve(remote);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(local, &dest).await?;
        Ok(())
    }

    async fn download(&self, remote: &str, local: &Path) -> Result<()> {
        let src = self.resolve(remote);
        if let Some(parent) = local.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&src, local).await?;
        Ok(())
    }
}

fn resolve_against(sandbox_root: &Path, host_workdir: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        let stripped = p.strip_prefix("/").unwrap_or(p);
        sandbox_root.join("rootfs").join(stripped)
    } else {
        host_workdir.join(p)
    }
}

fn to_entry(host_path: &Path, virtual_path: &Path, meta: &std::fs::Metadata) -> FileEntry {
    let kind = if meta.is_dir() {
        FileKind::Dir
    } else if meta.is_symlink() {
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
    let _ = host_path;
    FileEntry {
        path: virtual_path.to_path_buf(),
        kind,
        size: meta.len(),
        modified_unix_ms,
    }
}

fn walk(base: &Path, current: &Path, depth: u32, out: &mut Vec<FileEntry>) -> Result<()> {
    if depth == 0 {
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let host_path = entry.path();
        let rel = host_path.strip_prefix(base).unwrap_or(&host_path);
        out.push(to_entry(&host_path, rel, &meta));
        if meta.is_dir() && depth > 1 {
            walk(base, &host_path, depth - 1, out)?;
        }
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}
