//! cersei-lsp: Language Server Protocol client for the Cersei SDK.
//!
//! Provides on-demand LSP server management with JSON-RPC 2.0 over stdio.
//! Built-in configs for 13+ language servers. Exposed as a reusable library
//! and as a tool for coding agents.
//!
//! # Usage
//!
//! ```rust,ignore
//! use cersei_lsp::{LspManager, config::builtin_servers};
//! use std::path::Path;
//!
//! let mut mgr = LspManager::new("/path/to/project");
//! mgr.register_builtins();
//!
//! // Hover on a Rust file
//! let hover = mgr.hover(Path::new("src/main.rs"), 10, 5).await?;
//!
//! // Get symbols
//! let symbols = mgr.document_symbols(Path::new("src/lib.rs")).await?;
//! ```

pub mod client;
pub mod config;
pub mod jsonrpc;
pub mod manager;
pub mod types;

pub use client::{
    DiagnosticsEntry, LspClient, LspError, LspResult, RawLocation, ServerCaps, SyncKind,
    normalize_uri, parse_locations, path_to_uri, uri_to_path,
};
pub use config::LspServerConfig;
pub use manager::{DiagnosticsFreshness, LspManager, global_lsp_manager};

#[cfg(any(test, feature = "mock"))]
pub mod mock;
pub use types::*;
