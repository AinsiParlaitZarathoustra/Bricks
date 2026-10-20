//! Error types for bricks-vms.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VmError {
    #[error("sandbox not found: {0}")]
    NotFound(String),

    #[error("snapshot not found: {0}")]
    SnapshotNotFound(String),

    #[error("volume not found: {0}")]
    VolumeNotFound(String),

    #[error("sandbox lifecycle error: {0}")]
    Lifecycle(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("backend error ({backend}): {message}")]
    Backend { backend: String, message: String },

    #[error("operation timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("permission denied: {0}")]
    Permission(String),

    #[error("snapshot error: {0}")]
    Snapshot(String),

    #[error("mailbox error: {0}")]
    Mailbox(String),

    #[error("kv error: {0}")]
    Kv(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, VmError>;
