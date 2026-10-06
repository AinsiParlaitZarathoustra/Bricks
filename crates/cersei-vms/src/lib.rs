//! # cersei-vms
//!
//! Sandbox & VM isolation for the Cersei coding-agent SDK.
//!
//! Provides a pluggable [`SandboxRuntime`] trait, several backends
//! ([`LocalProcessRuntime`], [`DockerRuntime`]), three host-mediated
//! cross-sandbox primitives ([`Volume`](primitives::Volume),
//! [`Mailbox`](primitives::Mailbox), [`KvStore`](primitives::KvStore)),
//! and a snapshot/restore subsystem.
//!
//! See `crates/cersei-vms/README.md` (and the project plan) for the full
//! architecture.

pub mod backends;
pub mod commands;
pub mod envd;
pub mod error;
pub mod filesystem;
pub mod primitives;
pub mod runtime;
pub mod snapshot;
pub mod types;

#[cfg(feature = "backend-docker")]
pub use backends::DockerRuntime;
pub use backends::LocalProcessRuntime;

pub use commands::{CommandStream, Commands, StreamChunk};
pub use error::{Result, VmError};
pub use filesystem::{Filesystem, WatchStream};
pub use primitives::{KvSnapshot, KvStore, Mailbox, MailboxSubscription, Volume, VolumeRegistry};
pub use runtime::{DefaultAllocator, Sandbox, SandboxAllocator, SandboxHandle, SandboxRuntime};
pub use snapshot::{SnapshotManifest, SnapshotRegistry};
pub use types::{
    FileEntry, FileEvent, FileEventKind, FileKind, MailboxEnvelope, RunOutput, RunRequest,
    RuntimeCaps, SandboxId, SandboxInfo, SandboxOpts, SandboxStatus, Signal, SnapshotId, VolumeId,
    VolumeMount,
};

pub mod prelude {
    //! `use cersei_vms::prelude::*;` for the common surface.
    pub use crate::commands::{Commands, StreamChunk};
    pub use crate::error::{Result, VmError};
    pub use crate::filesystem::Filesystem;
    pub use crate::primitives::{KvStore, Mailbox, MailboxSubscription, Volume, VolumeRegistry};
    pub use crate::runtime::{Sandbox, SandboxAllocator, SandboxHandle, SandboxRuntime};
    pub use crate::types::{
        FileEntry, MailboxEnvelope, RunOutput, RunRequest, SandboxId, SandboxOpts, Signal,
        SnapshotId, VolumeId, VolumeMount,
    };
    #[cfg(feature = "backend-docker")]
    pub use crate::DockerRuntime;
    pub use crate::LocalProcessRuntime;
}
