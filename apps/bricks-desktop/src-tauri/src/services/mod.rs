//! Application services: projects, conversations, preferences, change
//! journal, attachments (owner: Haiku 1). Each service owns one file kind
//! under `<bricks_home>/desktop/` and writes it atomically.

pub mod attachment_runs;
pub mod attachments;
pub mod changes;
pub mod computer_use;
pub mod conversations;
pub mod keep_awake;
pub mod paths;
pub mod prefs;
pub mod projects;
pub mod providers;
pub mod providers_toml;
pub mod run_labels;
pub mod run_records;
pub mod skills;

use crate::contract::{IpcError, IpcErrorCode};
use std::fmt::Display;

pub(crate) fn error(code: IpcErrorCode, message: impl Into<String>) -> IpcError {
    IpcError {
        code,
        message: message.into(),
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> IpcError {
    error(IpcErrorCode::Invalid, message)
}

pub(crate) fn not_found(message: impl Into<String>) -> IpcError {
    error(IpcErrorCode::NotFound, message)
}

/// A failed read or write of an application file, with what was attempted.
pub(crate) fn io_error(context: &str, e: impl Display) -> IpcError {
    error(IpcErrorCode::Io, format!("{context} : {e}"))
}

/// A blocking task that failed to finish (panicked or was cancelled).
pub(crate) fn join_error(e: tokio::task::JoinError) -> IpcError {
    error(IpcErrorCode::Internal, e.to_string())
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
pub(crate) mod export;
