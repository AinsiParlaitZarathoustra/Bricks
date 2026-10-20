//! `Commands` trait — process execution surface for a sandbox.

use crate::error::Result;
use crate::types::{RunOutput, RunRequest, Signal};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tokio_stream::Stream;

/// A streamed chunk from a running process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StreamChunk {
    Started { pid: u32 },
    Stdout { data: String },
    Stderr { data: String },
    Exit { code: i32 },
    Error { message: String },
}

/// Bi-directional process stream. Future iteration will add stdin pumping;
/// Phase 1 is one-way (sandbox → host).
pub type CommandStream = Pin<Box<dyn Stream<Item = StreamChunk> + Send>>;

#[async_trait]
pub trait Commands: Send + Sync {
    /// Run a command and wait for it to exit (or time out).
    async fn run(&self, req: RunRequest) -> Result<RunOutput>;

    /// Run a command and stream its stdio chunks.
    async fn stream(&self, req: RunRequest) -> Result<CommandStream>;

    /// Send a signal to a previously-backgrounded process.
    async fn signal(&self, pid: u32, sig: Signal) -> Result<()>;
}
