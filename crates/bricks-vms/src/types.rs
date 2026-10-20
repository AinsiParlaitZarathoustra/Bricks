//! Core types for the bricks-vms sandbox layer.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

/// Stable identifier for a sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SandboxId(pub String);

impl SandboxId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SandboxId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SandboxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SandboxId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SandboxId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Stable identifier for a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SnapshotId(pub String);

impl SnapshotId {
    pub fn new() -> Self {
        Self(format!("snap-{}", uuid::Uuid::new_v4()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SnapshotId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SnapshotId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Identifier for a shared host-side Volume.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VolumeId(pub String);

impl VolumeId {
    pub fn new() -> Self {
        Self(format!("vol-{}", uuid::Uuid::new_v4()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for VolumeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for VolumeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for VolumeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// What a runtime can do beyond the core trait surface.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RuntimeCaps {
    pub snapshots: bool,
    pub pause_resume: bool,
    pub gpu: bool,
    pub network_isolation: bool,
    pub shared_volumes: bool,
    pub remote: bool,
}

/// How a Volume attaches to a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    pub volume_id: VolumeId,
    /// Path inside the sandbox.
    pub mount_path: PathBuf,
    pub read_only: bool,
}

/// Options for creating a new sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxOpts {
    /// Container/template image (e.g. `"cersei/sandbox-base:latest"`).
    pub image: String,
    /// Initial working directory inside the sandbox.
    pub workdir: Option<PathBuf>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Volumes to bind-mount.
    pub volumes: Vec<VolumeMount>,
    /// Soft CPU limit (cores). `None` = no limit.
    pub cpu_limit: Option<f32>,
    /// Memory limit in bytes. `None` = no limit.
    pub mem_limit: Option<u64>,
    /// Optional human-friendly label / metadata.
    pub labels: HashMap<String, String>,
    /// Resume from this snapshot if set (overrides `image`).
    pub from_snapshot: Option<SnapshotId>,
    /// Topics this sandbox should auto-subscribe to on the mailbox.
    pub mailbox_topics: Vec<String>,
}

impl Default for SandboxOpts {
    fn default() -> Self {
        Self {
            image: "cersei/sandbox-base:latest".to_string(),
            workdir: Some(PathBuf::from("/work")),
            env: HashMap::new(),
            volumes: Vec::new(),
            cpu_limit: None,
            mem_limit: None,
            labels: HashMap::new(),
            from_snapshot: None,
            mailbox_topics: Vec::new(),
        }
    }
}

impl SandboxOpts {
    pub fn image(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            ..Self::default()
        }
    }

    pub fn with_env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }

    pub fn with_workdir(mut self, w: impl Into<PathBuf>) -> Self {
        self.workdir = Some(w.into());
        self
    }

    pub fn with_volume(mut self, mount: VolumeMount) -> Self {
        self.volumes.push(mount);
        self
    }

    pub fn with_label(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.labels.insert(k.into(), v.into());
        self
    }
}

/// Summary of a sandbox's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxInfo {
    pub id: SandboxId,
    pub backend: String,
    pub image: String,
    pub status: SandboxStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxStatus {
    Creating,
    Running,
    Paused,
    Exited,
    Killed,
    Failed,
}

/// A command to run inside a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRequest {
    /// Shell command (executed via `/bin/sh -c`).
    pub command: String,
    pub workdir: Option<PathBuf>,
    pub env: HashMap<String, String>,
    /// `None` = no timeout (caller must handle).
    pub timeout: Option<Duration>,
    /// If true, run in background and return an opaque process handle in `RunOutput.pid`.
    pub background: bool,
}

impl RunRequest {
    pub fn new(cmd: impl Into<String>) -> Self {
        Self {
            command: cmd.into(),
            workdir: None,
            env: HashMap::new(),
            timeout: None,
            background: false,
        }
    }

    pub fn workdir(mut self, w: impl Into<PathBuf>) -> Self {
        self.workdir = Some(w.into());
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }
}

/// Result of a blocking command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    /// Set for background runs.
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub modified_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FileEventKind {
    Created,
    Modified,
    Removed,
    Renamed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEvent {
    pub kind: FileEventKind,
    pub path: PathBuf,
}

/// POSIX-ish signals we expose to callers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Signal {
    Term,
    Kill,
    Int,
    Hup,
    Usr1,
    Usr2,
}

impl Signal {
    pub fn as_i32(self) -> i32 {
        match self {
            Signal::Term => 15,
            Signal::Kill => 9,
            Signal::Int => 2,
            Signal::Hup => 1,
            Signal::Usr1 => 10,
            Signal::Usr2 => 12,
        }
    }
}

/// Envelope for the cross-sandbox Mailbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxEnvelope {
    pub topic: String,
    pub from: SandboxId,
    pub seq: u64,
    pub sent_at_unix_ms: i64,
    pub payload: serde_json::Value,
}
