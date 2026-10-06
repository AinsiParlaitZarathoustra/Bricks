//! Per-configuration memory backends. Each implements [`Config`] so the
//! runner can drive them uniformly.

use crate::dataset::Question;
use anyhow::Result;
use async_trait::async_trait;

pub mod baseline;
pub mod embed;
pub mod graph;
pub mod hybrid;
pub mod structured;

/// A memory configuration under test. One instance is built per question
/// (haystacks are independent across questions), then:
///   1. `ingest_question(q)` loads every turn of every session.
///   2. `retrieve(q)` returns the context string that will be fed into the
///      answerer LLM. For the baseline this is the full haystack; for the
///      retrieval-based configs it's the top-k most relevant snippets.
#[async_trait]
pub trait Config: Send + Sync {
    /// Ingest a question's haystack. Called exactly once per question.
    async fn ingest(&mut self, q: &Question) -> Result<()>;

    /// Retrieve the context that will be appended to the answerer prompt.
    async fn retrieve(&self, q: &Question) -> Result<String>;
}

/// How many top-k snippets to pull from retrieval configs. Matches Mastra's
/// published RAG config (`topK 20`) for direct leaderboard comparability.
pub const DEFAULT_TOP_K: usize = 20;
