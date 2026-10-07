//! Engine configuration (`[semantic]` in `bricks.toml`). Every limit here is
//! a ceiling: a query can lower it, never lift it.

use cersei_lsp::LspServerConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SemanticConfig {
    /// Files a search may visit.
    pub max_files: usize,
    /// Matches a lexical search may collect.
    pub max_matches: usize,
    /// Items a response may contain.
    pub max_results: usize,
    /// Larger files are skipped (and reported).
    pub max_file_bytes: u64,
    /// Files parsed by one query (candidate resolution, context).
    pub max_parsed_files: usize,
    /// Wall-clock ceiling of a query.
    pub timeout_ms: u64,
    /// Context budget when the query sets none.
    pub default_budget_tokens: u64,
    /// Highest budget a query may ask for.
    pub max_budget_tokens: u64,
    /// Trees kept in memory (LRU, one version per file).
    pub tree_cache_entries: usize,
    /// Responses kept (semantic queries only; see `result_cache_ttl_ms`).
    pub result_cache_entries: usize,
    /// A cached response older than this is recomputed, even without a
    /// known change (edits made outside Bricks are not observed).
    pub result_cache_ttl_ms: u64,
    /// Blocking tasks (walks, parses) running at once for this engine.
    pub max_blocking_tasks: usize,
    pub lsp: LspSettings,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_matches: 2_000,
            max_results: 50,
            max_file_bytes: 2 * 1024 * 1024,
            max_parsed_files: 200,
            timeout_ms: 15_000,
            default_budget_tokens: 4_000,
            max_budget_tokens: 16_000,
            tree_cache_entries: 512,
            result_cache_entries: 128,
            result_cache_ttl_ms: 30_000,
            max_blocking_tasks: 4,
            lsp: LspSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LspSettings {
    /// `false`: syntax and text only, no server is ever started.
    pub enabled: bool,
    /// Per request.
    pub request_timeout_ms: u64,
    /// Start + `initialize`.
    pub startup_timeout_ms: u64,
    /// Wait for the diagnostics of a version just sent.
    pub diagnostics_wait_ms: u64,
    /// Wait for a server that reports it is indexing before asking it
    /// (bounded by the query's time limit). Answers given while it still
    /// indexes are reported as partial.
    pub ready_wait_ms: u64,
    /// After a start, how long to wait for the server's first progress or
    /// status report before taking its silence as "ready".
    pub startup_settle_ms: u64,
    /// Servers unused for this long are shut down.
    pub idle_shutdown_secs: u64,
    /// Restarts after a crash, per server instance.
    pub max_restarts: u32,
    /// Documents kept open per server (LRU, the rest closed).
    pub max_open_documents: usize,
    /// Requests in flight across all servers of the engine.
    pub max_concurrent_requests: usize,
    /// Extra or overriding servers (same shape as `[[lsp.servers]]`).
    pub servers: Vec<LspServerConfig>,
}

impl Default for LspSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            request_timeout_ms: 10_000,
            startup_timeout_ms: 60_000,
            diagnostics_wait_ms: 5_000,
            ready_wait_ms: 10_000,
            startup_settle_ms: 2_000,
            idle_shutdown_secs: 600,
            max_restarts: 2,
            max_open_documents: 64,
            max_concurrent_requests: 8,
            servers: Vec::new(),
        }
    }
}

impl SemanticConfig {
    /// Stable identity of the settings that change results (engines with
    /// different settings are not shared).
    pub fn fingerprint(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}
