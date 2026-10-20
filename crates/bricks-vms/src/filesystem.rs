//! `Filesystem` trait — file ops inside a sandbox.

use crate::error::Result;
use crate::types::{FileEntry, FileEvent};
use async_trait::async_trait;
use bytes::Bytes;
use std::path::Path;
use std::pin::Pin;
use tokio_stream::Stream;

pub type WatchStream = Pin<Box<dyn Stream<Item = FileEvent> + Send>>;

#[async_trait]
pub trait Filesystem: Send + Sync {
    async fn read(&self, path: &str) -> Result<Bytes>;
    async fn write(&self, path: &str, data: &[u8]) -> Result<()>;
    async fn list(&self, path: &str, depth: u32) -> Result<Vec<FileEntry>>;
    async fn stat(&self, path: &str) -> Result<FileEntry>;
    async fn watch(&self, path: &str, recursive: bool) -> Result<WatchStream>;
    async fn mkdir(&self, path: &str, recursive: bool) -> Result<()>;
    async fn remove(&self, path: &str, recursive: bool) -> Result<()>;
    async fn upload(&self, local: &Path, remote: &str) -> Result<()>;
    async fn download(&self, remote: &str, local: &Path) -> Result<()>;
}
