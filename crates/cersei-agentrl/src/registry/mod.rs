//! The dynamic tool registry — a local database of reusable, agent-built tools.
//!
//! Before building a new sub-agent, the orchestrator searches here; on a win it
//! registers the solution. MVP persistence is append-only JSONL with keyword
//! search; Phase 2 swaps search for a `cersei-embeddings` vector index.
//!
//! All persisted strings are scrubbed (see [`crate::scrub`]).

pub mod dynamic_tool;

use crate::graph::FailureTrace;
use crate::scrub::redact;
use cersei_types::{CerseiError, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How to replay a registered solution as a fresh sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SolutionSpec {
    pub system_prompt: String,
    pub goal_template: String,
    pub allowed_tools: Vec<String>,
    pub snapshot_id: Option<String>,
}

/// One registered tool: a solved problem class plus how to re-apply it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryEntry {
    pub tool_id: String,
    pub name: String,
    pub description: String,
    pub problem_domain: String,
    pub failure_trace: FailureTrace,
    pub solution: SolutionSpec,
    pub created_at: i64,
    pub success_count: u32,
}

impl RegistryEntry {
    /// Scrub every free-text field before the entry can be persisted.
    fn scrubbed(mut self) -> Self {
        self.name = redact(&self.name);
        self.description = redact(&self.description);
        self.problem_domain = redact(&self.problem_domain);
        self.solution.system_prompt = redact(&self.solution.system_prompt);
        self.solution.goal_template = redact(&self.solution.goal_template);
        // failure_trace excerpts are already scrubbed at construction, but be safe.
        for f in &mut self.failure_trace.failing_nodes {
            f.input_excerpt = redact(&f.input_excerpt);
            f.error_excerpt = redact(&f.error_excerpt);
        }
        self
    }
}

/// In-memory + JSONL-backed registry of [`RegistryEntry`].
pub struct ToolRegistry {
    entries: DashMap<String, RegistryEntry>,
    path: PathBuf,
}

impl ToolRegistry {
    /// Open (or create) a registry rooted at `dir`. Loads `entries.jsonl`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Arc<Self>> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .map_err(|e| CerseiError::Other(anyhow::anyhow!("registry mkdir: {e}")))?;
        let path = dir.join("entries.jsonl");
        let entries = DashMap::new();
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| CerseiError::Other(anyhow::anyhow!("registry read: {e}")))?;
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<RegistryEntry>(line) {
                    entries.insert(entry.tool_id.clone(), entry);
                }
            }
        }
        Ok(Arc::new(Self { entries, path }))
    }

    /// In-memory registry that does not persist (for tests).
    pub fn in_memory() -> Arc<Self> {
        Arc::new(Self {
            entries: DashMap::new(),
            path: PathBuf::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, tool_id: &str) -> Option<RegistryEntry> {
        self.entries.get(tool_id).map(|e| e.clone())
    }

    pub fn all(&self) -> Vec<RegistryEntry> {
        self.entries.iter().map(|e| e.clone()).collect()
    }

    /// Register a new tool. Scrubs all fields, inserts, and appends to JSONL.
    pub fn register(&self, entry: RegistryEntry) -> Result<()> {
        let entry = entry.scrubbed();
        if !self.path.as_os_str().is_empty() {
            let line = serde_json::to_string(&entry)
                .map_err(|e| CerseiError::Other(anyhow::anyhow!("registry serialize: {e}")))?;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .map_err(|e| CerseiError::Other(anyhow::anyhow!("registry open: {e}")))?;
            writeln!(f, "{line}")
                .map_err(|e| CerseiError::Other(anyhow::anyhow!("registry write: {e}")))?;
        }
        self.entries.insert(entry.tool_id.clone(), entry);
        Ok(())
    }

    /// Keyword search: rank entries by how many query terms appear in their
    /// name/description/domain (higher `success_count` breaks ties). MVP stand-in
    /// for the Phase-2 vector index.
    pub fn search(&self, query: &str, k: usize) -> Vec<RegistryEntry> {
        let terms: Vec<String> = tokenize(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u32, RegistryEntry)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let hay =
                    format!("{} {} {}", e.name, e.description, e.problem_domain).to_lowercase();
                let score: u32 = terms.iter().filter(|t| hay.contains(t.as_str())).count() as u32;
                if score > 0 {
                    Some((score, e.clone()))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.success_count.cmp(&a.1.success_count))
        });
        scored.into_iter().take(k).map(|(_, e)| e).collect()
    }

    /// Bump the success counter for a tool (persisted lazily on next register).
    pub fn record_success(&self, tool_id: &str) {
        if let Some(mut e) = self.entries.get_mut(tool_id) {
            e.success_count += 1;
        }
    }
}

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(|t| t.to_string())
        .collect()
}
