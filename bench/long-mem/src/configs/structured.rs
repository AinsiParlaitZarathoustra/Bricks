//! Configs E/F — the structured memory (Sprint 6).
//!
//! * `structured-vector`: episodes (one per turn) embedded; vector lane only.
//! * `structured-hybrid`: facts extracted by the extractor model, then
//!   vector + lexical + relational lanes fused by weighted RRF.
//!
//! Only roles, contents, session ids and session dates are ingested — never
//! the answer, the evidence session ids or the turn annotations.

use crate::configs::{Config, DEFAULT_TOP_K};
use crate::dataset::Question;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use cersei_embeddings::EmbeddingProvider;
use cersei_memory::structured::clock::parse_time;
use cersei_memory::structured::{
    EpisodeInput, Extractor, LlmExtractor, MemoryConfig, RecallQuery, StructuredMemory,
};
use cersei_provider::Provider;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct StructuredConfig {
    hybrid: bool,
    embed: Box<dyn Fn() -> Arc<dyn EmbeddingProvider> + Send + Sync>,
    extractor: Option<Arc<dyn Extractor>>,
    top_k: usize,
    mem: Option<StructuredMemory>,
}

impl StructuredConfig {
    pub fn vector(embed: impl Fn() -> Arc<dyn EmbeddingProvider> + Send + Sync + 'static) -> Self {
        Self {
            hybrid: false,
            embed: Box::new(embed),
            extractor: None,
            top_k: DEFAULT_TOP_K,
            mem: None,
        }
    }

    pub fn hybrid(
        embed: impl Fn() -> Arc<dyn EmbeddingProvider> + Send + Sync + 'static,
        provider: Arc<dyn Provider>,
        extractor_model: String,
    ) -> Self {
        Self {
            hybrid: true,
            embed: Box::new(embed),
            extractor: Some(Arc::new(LlmExtractor::new(
                provider,
                extractor_model,
                Default::default(),
            ))),
            top_k: DEFAULT_TOP_K,
            mem: None,
        }
    }

    pub fn with_top_k(mut self, k: usize) -> Self {
        self.top_k = k;
        self
    }
}

#[async_trait]
impl Config for StructuredConfig {
    async fn ingest(&mut self, q: &Question) -> Result<()> {
        let mut cfg = MemoryConfig::default().with_space("user:longmemeval");
        cfg.recall.max_results = self.top_k;
        cfg.recall.max_tokens = 8_000;
        if !self.hybrid {
            cfg.recall.weight_lexical = 0.0;
            cfg.recall.weight_relational = 0.0;
            cfg.recall.hops = 0;
        }
        let mut b = StructuredMemory::builder((self.embed)()).config(cfg);
        if let Some(x) = &self.extractor {
            b = b.extractor(x.clone());
        }
        let mem = b.open().map_err(|e| anyhow!("{e}"))?;
        let mut inputs = Vec::new();
        for (i, session) in q.haystack_sessions.iter().enumerate() {
            let sid = q.haystack_session_ids.get(i).cloned().unwrap_or_default();
            let at = q.haystack_dates.get(i).and_then(|d| parse_time(d));
            for (n, turn) in session.iter().enumerate() {
                inputs.push(EpisodeInput {
                    space: "user:longmemeval".into(),
                    session_id: Some(sid.clone()),
                    role: turn.role.clone(),
                    author: None,
                    content: turn.content.clone(),
                    occurred_at: at.map(|t| t + n as i64),
                    source_ref: Some(format!("{sid}#{n}")),
                });
            }
        }
        mem.ingest(inputs, &CancellationToken::new())
            .await
            .map_err(|e| anyhow!("{e}"))?;
        self.mem = Some(mem);
        Ok(())
    }

    async fn retrieve(&self, q: &Question) -> Result<String> {
        let Some(mem) = &self.mem else {
            return Ok(String::new());
        };
        let mut query = RecallQuery::new(q.question.clone());
        query.valid_at = parse_time(&q.question_date);
        let r = mem.recall(&query).await.map_err(|e| anyhow!("{e}"))?;
        Ok(r.rendered)
    }
}
