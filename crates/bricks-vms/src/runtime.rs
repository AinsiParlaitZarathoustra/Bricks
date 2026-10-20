//! Core `SandboxRuntime` + `Sandbox` traits.

use crate::commands::Commands;
use crate::error::Result;
use crate::filesystem::Filesystem;
use crate::types::{RuntimeCaps, SandboxId, SandboxInfo, SandboxOpts, SnapshotId};
use async_trait::async_trait;
use std::sync::Arc;

/// Thin opaque handle for a running sandbox.
pub type SandboxHandle = Arc<dyn Sandbox>;

#[async_trait]
pub trait SandboxRuntime: Send + Sync {
    /// Runtime name — `"local"`, `"docker"`, ...
    fn name(&self) -> &str;

    /// Capability flags.
    fn capabilities(&self) -> RuntimeCaps;

    /// Allocate a fresh sandbox.
    async fn create(&self, opts: SandboxOpts) -> Result<SandboxHandle>;

    /// Attach to an existing sandbox by id.
    async fn get(&self, id: &SandboxId) -> Result<SandboxHandle>;

    /// List all known sandboxes managed by this runtime.
    async fn list(&self) -> Result<Vec<SandboxInfo>>;

    /// Restore a sandbox from a snapshot. Implementations that don't support
    /// snapshots should return `VmError::Snapshot` here.
    async fn restore(&self, snapshot: &SnapshotId) -> Result<SandboxHandle> {
        let _ = snapshot;
        Err(crate::error::VmError::Snapshot(format!(
            "runtime {} does not support snapshot restore",
            self.name()
        )))
    }
}

#[async_trait]
pub trait Sandbox: Send + Sync {
    fn id(&self) -> &SandboxId;
    fn info(&self) -> SandboxInfo;
    fn commands(&self) -> Arc<dyn Commands>;
    fn filesystem(&self) -> Arc<dyn Filesystem>;

    /// Take a snapshot. Optional capability.
    async fn snapshot(&self) -> Result<SnapshotId> {
        Err(crate::error::VmError::Snapshot(
            "snapshots not supported for this sandbox".into(),
        ))
    }

    /// Pause execution. Optional capability.
    async fn pause(&self) -> Result<()> {
        Err(crate::error::VmError::Lifecycle(
            "pause not supported for this sandbox".into(),
        ))
    }

    /// Resume execution. Optional capability.
    async fn resume(&self) -> Result<()> {
        Err(crate::error::VmError::Lifecycle(
            "resume not supported for this sandbox".into(),
        ))
    }

    /// Terminate and clean up the sandbox.
    async fn kill(&self) -> Result<()>;
}

/// Trait for components that can allocate a sandbox for a given delegated task.
///
/// Plugged into `bricks-agent` sub-agents so each parallel worker gets its own VM.
#[async_trait]
pub trait SandboxAllocator: Send + Sync {
    async fn allocate(&self, label: &str) -> Result<SandboxHandle>;
}

/// Trivial allocator that wraps a runtime + base opts.
pub struct DefaultAllocator {
    runtime: Arc<dyn SandboxRuntime>,
    base_opts: SandboxOpts,
}

impl DefaultAllocator {
    pub fn new(runtime: Arc<dyn SandboxRuntime>, base_opts: SandboxOpts) -> Self {
        Self { runtime, base_opts }
    }
}

#[async_trait]
impl SandboxAllocator for DefaultAllocator {
    async fn allocate(&self, label: &str) -> Result<SandboxHandle> {
        let mut opts = self.base_opts.clone();
        opts.labels
            .insert("cersei.task".to_string(), label.to_string());
        self.runtime.create(opts).await
    }
}
