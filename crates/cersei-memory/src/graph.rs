//! Graph-backed memory using Grafeo embedded graph database.
//!
//! Optional feature: enable with `features = ["graph"]` in Cargo.toml.
//!
//! ## Schema (v2)
//! ```text
//! (:Memory {id, content, mem_type, confidence, created_at, updated_at,
//!           last_validated_at, decay_rate, embedding_model_version})
//!   -[:RELATES_TO {relationship, weight}]-> (:Memory)
//!
//! (:Session {session_id, started_at, model, turns})
//!   -[:PRODUCED]-> (:Memory)
//!
//! (:Topic {name})
//!   -[:TAGGED]-> (:Memory)
//!
//! (:SchemaVersion {singleton, version, migrated_at, code_version})
//! ```

#[cfg(feature = "graph")]
use grafeo::GrafeoDB;

use crate::memdir::MemoryType;
use cersei_types::*;
use std::path::Path;

// Re-export migration utilities
pub use crate::graph_migrate::{self, effective_confidence, VersionCheck, CURRENT_SCHEMA_VERSION};

/// Graph-backed memory store.
pub struct GraphMemory {
    #[cfg(feature = "graph")]
    db: GrafeoDB,
    #[cfg(not(feature = "graph"))]
    _phantom: (),
}

/// Stats about the graph memory store.
#[derive(Debug, Clone, Default)]
pub struct GraphStats {
    pub memory_count: usize,
    pub session_count: usize,
    pub topic_count: usize,
    pub relationship_count: usize,
}

// ─── Centralized GQL queries ───────────────────────────────────────────────
//
// Fixed texts; every value is a typed parameter (no user content is ever
// spliced into a query).

#[cfg(feature = "graph")]
mod gql {
    use grafeo::Value;
    use std::collections::HashMap;

    pub fn params(pairs: Vec<(&str, Value)>) -> HashMap<String, Value> {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    pub const INSERT_MEMORY: &str = "INSERT (:Memory {id: $id, content: $content, \
        mem_type: $mem_type, confidence: $confidence, created_at: $now, updated_at: $now, \
        last_validated_at: $now, decay_rate: 0.01, embedding_model_version: ''})";
    pub const LINK_MEMORIES: &str = "MATCH (a:Memory {id: $from}), (b:Memory {id: $to}) \
        INSERT (a)-[:RELATES_TO {relationship: $relationship}]->(b)";
    pub const TAG_MEMORY: &str =
        "MATCH (m:Memory {id: $id}) INSERT (:Topic {name: $topic})-[:TAGGED]->(m)";
    pub const INSERT_SESSION: &str = "INSERT (:Session {session_id: $session_id, \
        started_at: $now, model: $model, turns: $turns})";
    /// Substring match of the whole query text (the former behaviour).
    pub const RECALL: &str =
        "MATCH (m:Memory) WHERE m.content CONTAINS $query RETURN m.content LIMIT $limit";
    pub const BY_TYPE: &str = "MATCH (m:Memory {mem_type: $mem_type}) RETURN m.content";
    pub const BY_TOPIC: &str =
        "MATCH (:Topic {name: $topic})-[:TAGGED]->(m:Memory) RETURN m.content";
    pub const REVALIDATE: &str = "MATCH (m:Memory {id: $id}) RETURN m.id";

    pub const COUNT_MEMORIES: &str = "MATCH (m:Memory) RETURN count(m)";
    pub const COUNT_SESSIONS: &str = "MATCH (s:Session) RETURN count(s)";
    pub const COUNT_TOPICS: &str = "MATCH (t:Topic) RETURN count(t)";
    pub const COUNT_RELATIONSHIPS: &str = "MATCH ()-[r:RELATES_TO]->() RETURN count(r)";
}

impl GraphMemory {
    /// Open a persistent graph database at the given path.
    /// Automatically checks schema version and runs migrations if needed.
    #[cfg(feature = "graph")]
    pub fn open(path: &Path) -> Result<Self> {
        let db = GrafeoDB::open(path)
            .map_err(|e| CerseiError::Config(format!("Failed to open graph DB: {}", e)))?;

        // Version check and auto-migrate
        match graph_migrate::check_version(&db) {
            VersionCheck::UpToDate => {}
            VersionCheck::NeedsMigration { from, to } => {
                graph_migrate::run_migrations(&db, from, to)?;
            }
            VersionCheck::CodeBehind {
                graph_version,
                code_version,
            } => {
                tracing::warn!(
                    "Graph schema v{} is newer than code v{}. Forward-compatible reads will be used.",
                    graph_version, code_version
                );
            }
        }

        Ok(Self { db })
    }

    /// Create an in-memory graph database (no persistence).
    /// Automatically stamps the current schema version.
    #[cfg(feature = "graph")]
    pub fn open_in_memory() -> Result<Self> {
        let db = GrafeoDB::new_in_memory();

        // Fresh in-memory graph always needs version stamp
        match graph_migrate::check_version(&db) {
            VersionCheck::UpToDate => {}
            VersionCheck::NeedsMigration { from, to } => {
                graph_migrate::run_migrations(&db, from, to)?;
            }
            _ => {}
        }

        Ok(Self { db })
    }

    /// Fallback: graph feature not enabled.
    #[cfg(not(feature = "graph"))]
    pub fn open(_path: &Path) -> Result<Self> {
        Err(CerseiError::Config(
            "Graph memory requires the 'graph' feature. Enable it in Cargo.toml.".into(),
        ))
    }

    /// Fallback: graph feature not enabled.
    #[cfg(not(feature = "graph"))]
    pub fn open_in_memory() -> Result<Self> {
        Err(CerseiError::Config(
            "Graph memory requires the 'graph' feature. Enable it in Cargo.toml.".into(),
        ))
    }

    // ─── Write operations ────────────────────────────────────────────────

    /// Store a memory as a graph node (v2 schema: includes decay and embedding fields).
    #[cfg(feature = "graph")]
    pub fn store_memory(
        &self,
        content: &str,
        mem_type: MemoryType,
        confidence: f32,
    ) -> Result<String> {
        let session = self.db.session();
        let mem_type_str = format!("{:?}", mem_type);
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        session
            .execute_with_params(
                gql::INSERT_MEMORY,
                gql::params(vec![
                    ("id", id.as_str().into()),
                    ("content", content.into()),
                    ("mem_type", mem_type_str.as_str().into()),
                    ("confidence", grafeo::Value::Float64(confidence as f64)),
                    ("now", now.as_str().into()),
                ]),
            )
            .map_err(|e| CerseiError::Config(format!("Graph insert failed: {}", e)))?;

        Ok(id)
    }

    /// Link two memories with a named relationship.
    #[cfg(feature = "graph")]
    pub fn link_memories(&self, from_id: &str, to_id: &str, relationship: &str) -> Result<()> {
        let session = self.db.session();
        session
            .execute_with_params(
                gql::LINK_MEMORIES,
                gql::params(vec![
                    ("from", from_id.into()),
                    ("to", to_id.into()),
                    ("relationship", relationship.into()),
                ]),
            )
            .map_err(|e| CerseiError::Config(format!("Graph link failed: {}", e)))?;
        Ok(())
    }

    /// Tag a memory with a topic.
    #[cfg(feature = "graph")]
    pub fn tag_memory(&self, memory_id: &str, topic: &str) -> Result<()> {
        let session = self.db.session();
        session
            .execute_with_params(
                gql::TAG_MEMORY,
                gql::params(vec![("id", memory_id.into()), ("topic", topic.into())]),
            )
            .map_err(|e| CerseiError::Config(format!("Graph tag failed: {}", e)))?;
        Ok(())
    }

    /// Record a session in the graph.
    #[cfg(feature = "graph")]
    pub fn record_session(&self, session_id: &str, model: Option<&str>, turns: u32) -> Result<()> {
        let session = self.db.session();
        let now = chrono::Utc::now().to_rfc3339();
        let model_str = model.unwrap_or("unknown");
        session
            .execute_with_params(
                gql::INSERT_SESSION,
                gql::params(vec![
                    ("session_id", session_id.into()),
                    ("now", now.as_str().into()),
                    ("model", model_str.into()),
                    ("turns", grafeo::Value::Int64(turns as i64)),
                ]),
            )
            .map_err(|e| CerseiError::Config(format!("Graph session record failed: {}", e)))?;
        Ok(())
    }

    /// Revalidate a memory — resets the confidence decay clock.
    /// Returns Ok(true) if the memory was found, Ok(false) if not.
    #[cfg(feature = "graph")]
    pub fn revalidate_memory(&self, memory_id: &str) -> Result<bool> {
        let session = self.db.session();
        match session
            .execute_with_params(gql::REVALIDATE, gql::params(vec![("id", memory_id.into())]))
        {
            Ok(result) => Ok(result.iter().next().is_some()),
            Err(e) => Err(CerseiError::Config(format!(
                "Graph revalidate failed: {}",
                e
            ))),
        }
    }

    // ─── Query operations ────────────────────────────────────────────────

    /// Recall memories matching a text query (substring match).
    #[cfg(feature = "graph")]
    pub fn recall(&self, query_text: &str, limit: usize) -> Vec<String> {
        let session = self.db.session();
        match session.execute_with_params(
            gql::RECALL,
            gql::params(vec![
                ("query", query_text.into()),
                ("limit", grafeo::Value::Int64(limit as i64)),
            ]),
        ) {
            Ok(result) => result
                .iter()
                .filter_map(|row| row.first().map(|v| format!("{}", v)))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Recall memories matching a text query with a relevance score.
    /// Score = fraction of query words found in each memory, in [0, 1].
    /// Results are ranked by score descending; ties preserve insertion order.
    /// Pulls up to 4× `limit` candidates via substring match, then re-ranks.
    #[cfg(feature = "graph")]
    pub fn recall_top_k(&self, query_text: &str, limit: usize) -> Vec<(String, f32)> {
        if limit == 0 || query_text.trim().is_empty() {
            return Vec::new();
        }
        // Pull a generous candidate set so word-overlap re-ranking has room.
        let candidates = self.recall(query_text, limit.saturating_mul(4).max(16));
        let words: Vec<String> = query_text
            .split_whitespace()
            .filter_map(|w| {
                let w = w
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase();
                if w.is_empty() || w.len() < 2 {
                    None
                } else {
                    Some(w)
                }
            })
            .collect();
        if words.is_empty() {
            // Fall back to uniform top-limit.
            return candidates
                .into_iter()
                .take(limit)
                .map(|c| (c, 1.0))
                .collect();
        }
        let mut scored: Vec<(String, f32)> = candidates
            .into_iter()
            .map(|c| {
                let lower = c.to_lowercase();
                let hits = words.iter().filter(|w| lower.contains(w.as_str())).count();
                (c, hits as f32 / words.len() as f32)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }

    /// Get all memories of a specific type.
    #[cfg(feature = "graph")]
    pub fn by_type(&self, mem_type: MemoryType) -> Vec<String> {
        let session = self.db.session();
        let type_str = format!("{:?}", mem_type);
        match session.execute_with_params(
            gql::BY_TYPE,
            gql::params(vec![("mem_type", type_str.as_str().into())]),
        ) {
            Ok(result) => result
                .iter()
                .filter_map(|row| row.first().map(|v| format!("{}", v)))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Get memories tagged with a specific topic.
    #[cfg(feature = "graph")]
    pub fn by_topic(&self, topic: &str) -> Vec<String> {
        let session = self.db.session();
        match session.execute_with_params(gql::BY_TOPIC, gql::params(vec![("topic", topic.into())]))
        {
            Ok(result) => result
                .iter()
                .filter_map(|row| row.first().map(|v| format!("{}", v)))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Get graph statistics.
    #[cfg(feature = "graph")]
    pub fn stats(&self) -> GraphStats {
        let session = self.db.session();
        let count = |query: &str| -> usize {
            session
                .execute(query)
                .ok()
                .and_then(|r| r.scalar::<i64>().ok())
                .map(|v| v as usize)
                .unwrap_or(0)
        };

        GraphStats {
            memory_count: count(gql::COUNT_MEMORIES),
            session_count: count(gql::COUNT_SESSIONS),
            topic_count: count(gql::COUNT_TOPICS),
            relationship_count: count(gql::COUNT_RELATIONSHIPS),
        }
    }

    /// Get the current schema version of the graph.
    #[cfg(feature = "graph")]
    pub fn schema_version(&self) -> VersionCheck {
        graph_migrate::check_version(&self.db)
    }

    // ─── Fallback implementations (no graph feature) ─────────────────────

    #[cfg(not(feature = "graph"))]
    pub fn store_memory(&self, _: &str, _: MemoryType, _: f32) -> Result<String> {
        Err(CerseiError::Config("Graph feature not enabled".into()))
    }

    #[cfg(not(feature = "graph"))]
    pub fn recall_top_k(&self, _: &str, _: usize) -> Vec<(String, f32)> {
        Vec::new()
    }

    #[cfg(not(feature = "graph"))]
    pub fn link_memories(&self, _: &str, _: &str, _: &str) -> Result<()> {
        Err(CerseiError::Config("Graph feature not enabled".into()))
    }

    #[cfg(not(feature = "graph"))]
    pub fn tag_memory(&self, _: &str, _: &str) -> Result<()> {
        Err(CerseiError::Config("Graph feature not enabled".into()))
    }

    #[cfg(not(feature = "graph"))]
    pub fn record_session(&self, _: &str, _: Option<&str>, _: u32) -> Result<()> {
        Err(CerseiError::Config("Graph feature not enabled".into()))
    }

    #[cfg(not(feature = "graph"))]
    pub fn revalidate_memory(&self, _: &str) -> Result<bool> {
        Err(CerseiError::Config("Graph feature not enabled".into()))
    }

    #[cfg(not(feature = "graph"))]
    pub fn recall(&self, _: &str, _: usize) -> Vec<String> {
        Vec::new()
    }

    #[cfg(not(feature = "graph"))]
    pub fn by_type(&self, _: MemoryType) -> Vec<String> {
        Vec::new()
    }

    #[cfg(not(feature = "graph"))]
    pub fn by_topic(&self, _: &str) -> Vec<String> {
        Vec::new()
    }

    #[cfg(not(feature = "graph"))]
    pub fn stats(&self) -> GraphStats {
        GraphStats::default()
    }

    #[cfg(not(feature = "graph"))]
    pub fn schema_version(&self) -> VersionCheck {
        VersionCheck::UpToDate
    }
}

/// Check if graph memory is available (compiled with the feature).
pub fn is_graph_available() -> bool {
    cfg!(feature = "graph")
}
