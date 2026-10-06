//! Structured long-term memory (feature `structured`). See `docs/memory.md`.
//!
//! [`StructuredMemory`] keeps sourced **episodes**, scoped **entities** and
//! temporal **facts** in a Grafeo database (the authority), projects their
//! embeddings into an in-process vector index, and answers recall with a
//! hybrid of vector search, lexical search and a bounded relational
//! expansion fused by weighted reciprocal rank fusion.
//!
//! Ingestion ([`StructuredMemory::record`] then [`StructuredMemory::process`]):
//!
//! 1. the episode is stored durably (immutable content, stable id);
//! 2. an [`extract::Extractor`] proposes entities and facts; the validated
//!    output is cached on the episode before anything else, so a resumed
//!    ingestion never calls the model again for it;
//! 3. identities are resolved within the episode's space, change rules are
//!    applied, evidence is linked — all in one transaction;
//! 4. episodes and facts are embedded and added to the projection.
//!
//! A failure at 2 or 4 leaves the episode stored and pending; `process`
//! picks it up again (bounded attempts for extraction).

pub mod clock;
pub mod config;
pub mod extract;
pub mod index;
mod ingest;
pub mod model;
pub mod recall;
pub mod store;

pub use clock::{Clock, ManualClock, SystemClock};
pub use config::{ExtractionConfig, MemoryConfig, RecallConfig};
pub use extract::{Extractor, LlmExtractor};
pub use ingest::{ForgetReport, IngestReport, Recorded};
pub use model::*;
pub use recall::{Recall, RecallItem, RecallQuery, RecallTimings};

use cersei_embeddings::EmbeddingProvider;
use index::{Item, Projection};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{as_i64, as_str, as_strings, as_vector, q, Store, P, W};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MemoryError {
    #[error("memory store: {0}")]
    Store(String),
    #[error("memory configuration: {0}")]
    Config(String),
    #[error(
        "the store holds embeddings of `{stored}`; the configured embedder is `{configured}` — \
         vectors of different models cannot be mixed; open with `rebuild_embeddings(true)` to \
         re-embed everything with the new model"
    )]
    EmbeddingMismatch { stored: String, configured: String },
    #[error("embedding failed: {0}")]
    Embedding(String),
}

impl From<String> for MemoryError {
    fn from(e: String) -> Self {
        Self::Store(e)
    }
}

impl From<MemoryError> for cersei_types::CerseiError {
    fn from(e: MemoryError) -> Self {
        cersei_types::CerseiError::Config(e.to_string())
    }
}

pub type MemoryResult<T> = Result<T, MemoryError>;

/// Opens a [`StructuredMemory`].
pub struct MemoryBuilder {
    path: Option<PathBuf>,
    config: MemoryConfig,
    clock: Arc<dyn Clock>,
    embedder: Arc<dyn EmbeddingProvider>,
    extractor: Option<Arc<dyn Extractor>>,
    rebuild_embeddings: bool,
}

impl MemoryBuilder {
    pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            path: None,
            config: MemoryConfig::default(),
            clock: Arc::new(SystemClock),
            embedder,
            extractor: None,
            rebuild_embeddings: false,
        }
    }

    /// A file-backed store (else in memory).
    pub fn path(mut self, path: impl AsRef<Path>) -> Self {
        self.path = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn config(mut self, config: MemoryConfig) -> Self {
        self.config = config;
        self
    }

    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Without an extractor, episodes are stored and embedded but no
    /// entities or facts are created (vector-only memory).
    pub fn extractor(mut self, extractor: Arc<dyn Extractor>) -> Self {
        self.extractor = Some(extractor);
        self
    }

    /// Accept an embedder different from the one the store was built with,
    /// and re-embed every record with it (explicit, never automatic).
    pub fn rebuild_embeddings(mut self, yes: bool) -> Self {
        self.rebuild_embeddings = yes;
        self
    }

    pub fn open(self) -> MemoryResult<StructuredMemory> {
        self.config.validate().map_err(MemoryError::Config)?;
        let store = match &self.path {
            Some(p) => {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| MemoryError::Store(e.to_string()))?;
                }
                Store::open(p)?
            }
            None => Store::in_memory()?,
        };
        let model = self.embedder.model_id();
        let dims = self.embedder.dimensions();
        let meta = store.query(q::META, P::new())?;
        match meta.first() {
            None => store.exec(
                W::InsertMeta,
                P::new().s("model", &model).i("dims", dims as i64),
            )?,
            Some(row) => {
                let stored = as_str(&row[0]).unwrap_or_default();
                if stored != model {
                    if !self.rebuild_embeddings {
                        return Err(MemoryError::EmbeddingMismatch {
                            stored,
                            configured: model,
                        });
                    }
                    store.exec(W::ClearEmbeddings, P::new())?;
                    store.exec(
                        W::SetMeta,
                        P::new().s("model", &model).i("dims", dims as i64),
                    )?;
                }
            }
        }
        let projection = Projection::new(&model, dims).map_err(MemoryError::Store)?;
        let memory = StructuredMemory {
            store,
            config: self.config,
            clock: self.clock,
            embedder: self.embedder,
            extractor: self.extractor,
            projection: parking_lot::RwLock::new(projection),
            write: tokio::sync::Mutex::new(()),
            maintenance: tokio::sync::Mutex::new(()),
        };
        memory.rebuild_projection()?;
        Ok(memory)
    }
}

/// The structured long-term memory.
pub struct StructuredMemory {
    store: Store,
    config: MemoryConfig,
    clock: Arc<dyn Clock>,
    embedder: Arc<dyn EmbeddingProvider>,
    extractor: Option<Arc<dyn Extractor>>,
    projection: parking_lot::RwLock<Projection>,
    /// Ingestion is serialised (identity resolution reads then writes).
    write: tokio::sync::Mutex<()>,
    /// One maintenance pass (extraction, embeddings) at a time. Recording
    /// and recall never wait for it.
    maintenance: tokio::sync::Mutex<()>,
}

/// Counts of what the memory holds.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct MemoryStats {
    pub episodes: usize,
    pub entities: usize,
    pub facts: usize,
    pub indexed: usize,
    pub embedding_model: String,
}

impl StructuredMemory {
    pub fn builder(embedder: Arc<dyn EmbeddingProvider>) -> MemoryBuilder {
        MemoryBuilder::new(embedder)
    }

    pub fn config(&self) -> &MemoryConfig {
        &self.config
    }

    pub fn now(&self) -> Millis {
        self.clock.now()
    }

    pub fn stats(&self) -> MemoryStats {
        let p = self.projection.read();
        MemoryStats {
            episodes: self.store.count(q::COUNT_EPISODES),
            entities: self.store.count(q::COUNT_ENTITIES),
            facts: self.store.count(q::COUNT_FACTS),
            indexed: p.len(),
            embedding_model: p.model.clone(),
        }
    }

    /// Rebuild the vector projection from the store (records embedded with
    /// the current model only).
    pub fn rebuild_projection(&self) -> MemoryResult<()> {
        let model = self.projection.read().model.clone();
        let dims = self.projection.read().dims;
        let facts: std::collections::HashMap<String, Fact> = self
            .store
            .all_facts()?
            .into_iter()
            .map(|f| (f.id.clone(), f))
            .collect();
        let episodes: std::collections::HashMap<String, Episode> = self
            .store
            .all_episodes()?
            .into_iter()
            .map(|e| (e.id.clone(), e))
            .collect();
        let mut fresh = Projection::new(&model, dims).map_err(MemoryError::Store)?;
        for row in self
            .store
            .query(q::INDEXED_ITEMS, P::new().s("model", &model))?
        {
            let (Some(key), Some(id), Some(vector)) =
                (as_i64(&row[0]), as_str(&row[1]), as_vector(&row[3]))
            else {
                continue;
            };
            let labels = as_strings(&row[2]);
            let item = if labels.iter().any(|l| l == "Fact") {
                facts.get(&id).cloned().map(|f| Item::Fact(Box::new(f)))
            } else {
                episodes
                    .get(&id)
                    .cloned()
                    .map(|e| Item::Episode(Box::new(e)))
            };
            if let Some(item) = item {
                fresh
                    .upsert(key as u64, item, &vector)
                    .map_err(MemoryError::Store)?;
            }
        }
        *self.projection.write() = fresh;
        Ok(())
    }

    // ── Reads ──

    pub fn episode(&self, id: &str) -> MemoryResult<Option<Episode>> {
        Ok(self.store.episode(id)?)
    }

    pub fn fact(&self, id: &str) -> MemoryResult<Option<Fact>> {
        Ok(self.store.fact(id)?)
    }

    pub fn evidence(&self, fact_id: &str) -> MemoryResult<Vec<Evidence>> {
        Ok(self.store.evidence(fact_id)?)
    }

    /// Facts that replaced `fact_id` (direction: new `SUPERSEDES` old).
    pub fn superseded_by(&self, fact_id: &str) -> MemoryResult<Vec<String>> {
        Ok(self
            .store
            .ids(q::SUPERSEDED_BY, P::new().s("id", fact_id))?)
    }

    pub fn supersedes(&self, fact_id: &str) -> MemoryResult<Vec<String>> {
        Ok(self.store.ids(q::SUPERSEDES, P::new().s("id", fact_id))?)
    }

    pub fn contradictions(&self, fact_id: &str) -> MemoryResult<Vec<String>> {
        Ok(self
            .store
            .ids(q::CONTRADICTIONS, P::new().s("id", fact_id))?)
    }

    /// Facts of `space` as known at `known_at` and valid at `valid_at`
    /// (both default to now): the "current facts" view when both are now.
    /// Unsupported, retracted and (by default) proposed facts are left out;
    /// contested and uncertain ones are included — check their status and
    /// [`Fact::validity_at`].
    pub fn facts(
        &self,
        space: &str,
        valid_at: Option<Millis>,
        known_at: Option<Millis>,
        include_proposed: bool,
    ) -> MemoryResult<Vec<Fact>> {
        let now = self.now();
        let (t, k) = (valid_at.unwrap_or(now), known_at.unwrap_or(now));
        Ok(self
            .store
            .all_facts()?
            .into_iter()
            .filter(|f| f.space == space)
            .filter(|f| recall::admissible_fact(f, t, k, include_proposed).is_some())
            .collect())
    }

    /// Current facts: active (or contested) facts of `space`, valid now.
    pub fn current_facts(&self, space: &str) -> MemoryResult<Vec<Fact>> {
        self.facts(space, None, None, false)
    }

    pub fn close(&self) -> MemoryResult<()> {
        Ok(self.store.close()?)
    }
}

#[async_trait::async_trait]
impl crate::LongTermMemory for StructuredMemory {
    async fn recall_context(
        &self,
        query: &str,
        max_tokens: usize,
    ) -> cersei_types::Result<Option<crate::RecalledContext>> {
        let mut q = RecallQuery::new(query);
        q.max_tokens = Some(max_tokens.min(self.config.recall.max_tokens));
        let r = self.recall(&q).await?;
        if r.items.is_empty() {
            return Ok(None);
        }
        Ok(Some(crate::RecalledContext {
            text: r.rendered,
            tokens: r.tokens,
            items: r.items.len(),
            omitted: r.omitted,
        }))
    }

    async fn record_turns(
        &self,
        session_id: Option<&str>,
        turns: &[crate::MemoryTurn],
    ) -> cersei_types::Result<()> {
        let inputs: Vec<EpisodeInput> = turns
            .iter()
            .filter(|t| !t.content.trim().is_empty())
            .map(|t| EpisodeInput {
                space: self.config.space.clone(),
                session_id: session_id.map(str::to_string),
                role: t.role.clone(),
                author: None,
                content: t.content.clone(),
                occurred_at: Some(t.at.unwrap_or_else(|| self.now())),
                source_ref: session_id.map(|s| format!("session:{s}")),
            })
            .collect();
        if !inputs.is_empty() {
            self.record_all(inputs).await?;
        }
        Ok(())
    }

    async fn maintain(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> cersei_types::Result<crate::MaintenanceReport> {
        let r = self.process(cancel).await?;
        let mut errors: Vec<String> = r
            .failed
            .iter()
            .map(|(id, e)| format!("extraction of {id}: {e}"))
            .chain(r.embedding_errors.iter().map(|e| format!("embedding: {e}")))
            .collect();
        errors.truncate(5);
        Ok(crate::MaintenanceReport {
            extracted: r.extracted,
            facts_created: r.facts_created,
            facts_updated: r.facts_confirmed + r.facts_superseded + r.facts_contested,
            embedded: r.embedded,
            failed: r.failed.len(),
            embedding_errors: r.embedding_errors.len(),
            pending: self.pending_work()?,
            cancelled: cancel.is_cancelled(),
            errors,
        })
    }
}
