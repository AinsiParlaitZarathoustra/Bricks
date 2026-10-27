//! Explicit notes, the reading of a store page by page, and the clearing of
//! a whole store.
//!
//! A **note** is an episode of role [`ROLE_NOTE`], written by the model
//! (`author: "model"`): it is indexed before [`StructuredMemory::write_note`]
//! returns, so it can be found at once, in the same run. Its id comes from
//! an operation key (space, project, session, text): writing it again finds
//! it, and only finishes its indexing when that had failed.

use super::index::{Item, Projection};
use super::model::*;
use super::store::{P, W, as_str, q};
use super::{MemoryError, MemoryResult, StructuredMemory};

/// Characters a note may hold.
pub const NOTE_MAX_CHARS: usize = 4_000;

/// A note to write.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteInput {
    pub space: String,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub text: String,
}

/// A note written and indexed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NoteWritten {
    pub id: String,
    /// False when the same note was already stored.
    pub new: bool,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NoteError {
    #[error("{0}")]
    Invalid(String),
    #[error(
        "the note {id} is saved but not indexed yet: {error}. Writing the same note again \
         resumes its indexing without saving it twice"
    )]
    NotIndexed { id: String, error: String },
    #[error(transparent)]
    Memory(#[from] MemoryError),
}

impl NoteInput {
    /// The note's id: its operation key, so a retry never stores it twice.
    pub fn id(&self) -> String {
        stable_id(
            "ep",
            &[
                ROLE_NOTE,
                &self.space,
                self.project_id.as_deref().unwrap_or(""),
                self.session_id.as_deref().unwrap_or(""),
                self.text.trim(),
            ],
        )
    }

    /// Where it comes from: its session, else its project.
    fn source_ref(&self) -> Option<String> {
        match (&self.session_id, &self.project_id) {
            (Some(s), _) => Some(format!("session:{s}")),
            (None, Some(p)) => Some(format!("project:{p}")),
            (None, None) => None,
        }
    }
}

/// What a store holds, by kind (a note is never counted as an exchange).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct EntryCounts {
    pub notes: usize,
    pub exchanges: usize,
    /// Current facts.
    pub facts: usize,
}

/// The kind of a stored entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Note,
    Exchange,
    Fact,
}

/// Which entries a page lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryFilter {
    /// Notes, exchanges and current facts.
    #[default]
    All,
    Notes,
    Exchanges,
    /// Current facts.
    Facts,
    /// Every version of every fact, replaced and withdrawn ones included.
    FactHistory,
}

/// One entry of a store, as listed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StoredEntry {
    pub id: String,
    pub kind: EntryKind,
    pub text: String,
    /// When it happened (an episode), or was recorded (a fact).
    pub at: Millis,
    /// `user`, `assistant`, `tool`, `note` (episodes).
    pub role: Option<String>,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub source_ref: Option<String>,
    /// Facts only.
    pub status: Option<FactStatus>,
    /// Facts only: the episodes and quotes supporting it.
    pub evidence: Vec<Evidence>,
}

/// A page of entries, newest first, and how many match in all.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct EntryPage {
    pub entries: Vec<StoredEntry>,
    pub total: usize,
}

/// Entries one page may hold.
pub const PAGE_MAX: usize = 200;

/// What [`StructuredMemory::forget_all`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ClearReport {
    pub episodes: usize,
    pub facts: usize,
    pub entities: usize,
}

impl StructuredMemory {
    /// Store a note and index it before returning: it is listed and found
    /// by recall at once. `extracted`: whether the processing that will
    /// maintain this store has an extractor (else the note is never
    /// extracted). A failure to index leaves the note stored, and a new
    /// call with the same note only indexes it.
    pub async fn write_note(
        &self,
        input: NoteInput,
        extracted: bool,
    ) -> Result<NoteWritten, NoteError> {
        if !valid_space(&input.space) {
            return Err(NoteError::Invalid(format!(
                "invalid space `{}`",
                input.space
            )));
        }
        let text = input.text.trim();
        if text.is_empty() {
            return Err(NoteError::Invalid("a note needs text".into()));
        }
        if text.chars().count() > NOTE_MAX_CHARS {
            return Err(NoteError::Invalid(format!(
                "a note holds at most {NOTE_MAX_CHARS} characters"
            )));
        }
        let id = input.id();
        let new = {
            let _w = self.write.lock().await;
            let exists = !self
                .store
                .ids(q::EPISODE_EXISTS, P::new().s("id", &id))
                .map_err(MemoryError::Store)?
                .is_empty();
            if !exists {
                let now = self.now();
                let mut ops = vec![(
                    W::InsertEpisode,
                    P::new()
                        .s("id", &id)
                        .s("space", &input.space)
                        .os("project_id", input.project_id.as_deref())
                        .os("session_id", input.session_id.as_deref())
                        .s("role", ROLE_NOTE)
                        .s("author", "model")
                        .s("content", text)
                        .i("occurred_at", now)
                        .i("recorded_at", now)
                        .os("source_ref", input.source_ref().as_deref())
                        .i("ikey", self.store.next_ikey()),
                )];
                if !extracted {
                    ops.push((
                        W::SetExtraction,
                        P::new()
                            .s("id", &id)
                            .s("state", "skipped")
                            .i("attempts", 0)
                            .os("json", None)
                            .os("error", None),
                    ));
                }
                self.store.transaction(ops).map_err(MemoryError::Store)?;
            }
            !exists
        };
        self.index_episode(&id)
            .await
            .map_err(|error| NoteError::NotIndexed {
                id: id.clone(),
                error,
            })?;
        Ok(NoteWritten { id, new })
    }

    /// Embed one episode with the current model, unless it already is, and
    /// add it to the projection.
    async fn index_episode(&self, id: &str) -> Result<(), String> {
        let model = self.projection.read().model.clone();
        let current = self
            .store
            .query(q::EMBED_MODEL, P::new().s("id", id))?
            .first()
            .and_then(|r| as_str(&r[0]));
        if current.as_deref() == Some(model.as_str()) && self.projection.read().key_of(id).is_some()
        {
            return Ok(());
        }
        let episode = self
            .store
            .episode(id)?
            .ok_or_else(|| format!("no memory record `{id}`"))?;
        #[cfg(test)]
        if let Some(e) = tests::FAIL_INDEX.with(|f| f.borrow_mut().take()) {
            return Err(e);
        }
        let vector = self
            .embedder
            .embed(&episode.content)
            .await
            .map_err(|e| e.to_string())?;
        self.store.exec(
            W::SetEmbedding,
            P::new()
                .s("id", id)
                .vector("embedding", &vector)
                .s("model", &model),
        )?;
        let key = self
            .store
            .query(q::IKEY, P::new().s("id", id))?
            .first()
            .and_then(|r| super::store::as_i64(&r[0]))
            .ok_or_else(|| format!("no index key for `{id}`"))?;
        self.projection
            .write()
            .upsert(key as u64, Item::Episode(Box::new(episode)), &vector)
    }

    /// How many notes, exchanges and current facts the store holds.
    pub fn counts(&self) -> MemoryResult<EntryCounts> {
        let mut c = EntryCounts::default();
        for e in self.store.all_episodes()? {
            if e.role == ROLE_NOTE {
                c.notes += 1;
            } else {
                c.exchanges += 1;
            }
        }
        c.facts = self.current_facts_all()?.len();
        Ok(c)
    }

    /// Current facts of every space of this store.
    fn current_facts_all(&self) -> MemoryResult<Vec<Fact>> {
        let now = self.now();
        Ok(self
            .store
            .all_facts()?
            .into_iter()
            .filter(|f| super::recall::admissible_fact(f, now, now, true).is_some())
            .collect())
    }

    /// A page of entries matching `filter` and `query` (case and accents
    /// ignored; empty: all), newest first, ties by id: the order is stable
    /// from one page to the next.
    pub fn entries(
        &self,
        filter: EntryFilter,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> MemoryResult<EntryPage> {
        if limit == 0 || limit > PAGE_MAX {
            return Err(MemoryError::Config(format!(
                "a page holds 1 to {PAGE_MAX} entries (got {limit})"
            )));
        }
        let needle = normalize_name(query);
        let matches = |text: &str| needle.is_empty() || normalize_name(text).contains(&needle);
        let mut all: Vec<StoredEntry> = Vec::new();
        let episodes = matches!(
            filter,
            EntryFilter::All | EntryFilter::Notes | EntryFilter::Exchanges
        );
        if episodes {
            for e in self.store.all_episodes()? {
                let kind = if e.role == ROLE_NOTE {
                    EntryKind::Note
                } else {
                    EntryKind::Exchange
                };
                let wanted = match filter {
                    EntryFilter::Notes => kind == EntryKind::Note,
                    EntryFilter::Exchanges => kind == EntryKind::Exchange,
                    _ => true,
                };
                if !wanted || !matches(&e.content) {
                    continue;
                }
                all.push(StoredEntry {
                    at: e.occurred_at.unwrap_or(e.recorded_at),
                    id: e.id,
                    kind,
                    text: e.content,
                    role: Some(e.role),
                    project_id: e.project_id,
                    session_id: e.session_id,
                    source_ref: e.source_ref,
                    status: None,
                    evidence: Vec::new(),
                });
            }
        }
        let facts = match filter {
            EntryFilter::All | EntryFilter::Facts => self.current_facts_all()?,
            EntryFilter::FactHistory => self.store.all_facts()?,
            _ => Vec::new(),
        };
        for f in facts {
            if !matches(&f.statement) {
                continue;
            }
            all.push(StoredEntry {
                at: f.knowledge.recorded_at,
                id: f.id,
                kind: EntryKind::Fact,
                text: f.statement,
                role: None,
                project_id: None,
                session_id: None,
                source_ref: None,
                status: Some(f.status),
                evidence: Vec::new(),
            });
        }
        all.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.id.cmp(&b.id)));
        let total = all.len();
        let mut entries: Vec<StoredEntry> = all.into_iter().skip(offset).take(limit).collect();
        for e in &mut entries {
            if e.kind == EntryKind::Fact {
                e.evidence = self.store.evidence(&e.id)?;
            }
        }
        Ok(EntryPage { entries, total })
    }

    /// Delete everything the store remembers — exchanges, notes, facts,
    /// their evidence and relations, entities and aliases — with the vector
    /// projection and the pending processing of those records. The store's
    /// technical metadata (schema, embedding model) stays; the file is not
    /// compacted. Waits for a maintenance pass in progress.
    pub async fn forget_all(&self) -> MemoryResult<ClearReport> {
        let _pass = self.maintenance.lock().await;
        let _w = self.write.lock().await;
        let report = ClearReport {
            episodes: self.store.count(q::COUNT_EPISODES),
            facts: self.store.count(q::COUNT_FACTS),
            entities: self.store.count(q::COUNT_ENTITIES),
        };
        self.store
            .exec(W::ClearRecords, P::new())
            .map_err(MemoryError::Store)?;
        let (model, dims) = {
            let p = self.projection.read();
            (p.model.clone(), p.dims)
        };
        *self.projection.write() = Projection::new(&model, dims).map_err(MemoryError::Store)?;
        Ok(report)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::structured::StructuredMemory;
    use bricks_embeddings::HashingEmbeddings;
    use std::sync::Arc;

    thread_local! {
        /// The next indexing of a note fails with this error (same thread).
        pub(crate) static FAIL_INDEX: std::cell::RefCell<Option<String>> =
            const { std::cell::RefCell::new(None) };
    }

    fn memory() -> StructuredMemory {
        StructuredMemory::builder(Arc::new(HashingEmbeddings::new(64)))
            .open()
            .unwrap()
    }

    fn note(text: &str) -> NoteInput {
        NoteInput {
            space: GLOBAL_SPACE.into(),
            project_id: Some("p1".into()),
            session_id: Some("s1".into()),
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn a_note_is_found_at_once_and_never_stored_twice() {
        let m = memory();
        let w = m
            .write_note(note("Prefers tabs over spaces"), false)
            .await
            .unwrap();
        assert!(w.new);
        let again = m
            .write_note(note("Prefers tabs over spaces"), false)
            .await
            .unwrap();
        assert_eq!((again.id.as_str(), again.new), (w.id.as_str(), false));
        let mut q = crate::structured::RecallQuery::new("tabs spaces");
        q.spaces = Some(vec![GLOBAL_SPACE.into()]);
        let r = m.recall(&q).await.unwrap();
        assert_eq!(r.items.len(), 1);
        let e = m.episode(&w.id).unwrap().unwrap();
        assert_eq!(
            (e.role.as_str(), e.author.as_deref()),
            (ROLE_NOTE, Some("model"))
        );
        assert_eq!(e.project_id.as_deref(), Some("p1"));
        assert_eq!(e.source_ref.as_deref(), Some("session:s1"));
        assert_eq!(
            m.counts().unwrap(),
            EntryCounts {
                notes: 1,
                exchanges: 0,
                facts: 0
            }
        );
    }

    #[tokio::test]
    async fn a_failed_indexing_keeps_the_note_and_resumes() {
        let m = memory();
        FAIL_INDEX.with(|f| *f.borrow_mut() = Some("embedder down".into()));
        let e = m
            .write_note(note("Deploys on Fridays"), false)
            .await
            .unwrap_err();
        let NoteError::NotIndexed { id, error } = &e else {
            panic!("{e:?}")
        };
        assert_eq!(error, "embedder down");
        assert!(e.to_string().contains("is saved but not indexed yet"));
        assert!(m.episode(id).unwrap().is_some(), "kept");
        let w = m
            .write_note(note("Deploys on Fridays"), false)
            .await
            .unwrap();
        assert_eq!((&w.id, w.new), (id, false));
        assert_eq!(m.counts().unwrap().notes, 1, "no duplicate");
        assert_eq!(m.stats().indexed, 1);
    }

    #[tokio::test]
    async fn pages_are_stable_and_clearing_empties_the_store() {
        let m = memory();
        for i in 0..5 {
            m.write_note(note(&format!("note number {i}")), false)
                .await
                .unwrap();
        }
        let first = m.entries(EntryFilter::All, "", 0, 2).unwrap();
        let second = m.entries(EntryFilter::All, "", 2, 2).unwrap();
        assert_eq!((first.total, first.entries.len()), (5, 2));
        assert!(
            first
                .entries
                .iter()
                .all(|a| second.entries.iter().all(|b| a.id != b.id))
        );
        assert_eq!(
            m.entries(EntryFilter::Notes, "NUMBER 3", 0, 50)
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            m.entries(EntryFilter::Exchanges, "", 0, 50).unwrap().total,
            0
        );
        assert!(m.entries(EntryFilter::All, "", 0, 0).is_err());
        let r = m.forget_all().await.unwrap();
        assert_eq!(r.episodes, 5);
        assert_eq!(m.entries(EntryFilter::All, "", 0, 50).unwrap().total, 0);
        let mut q = crate::structured::RecallQuery::new("note number");
        q.spaces = Some(vec![GLOBAL_SPACE.into()]);
        assert!(m.recall(&q).await.unwrap().items.is_empty());
        // Still usable afterwards.
        m.write_note(note("after"), false).await.unwrap();
        assert_eq!(m.counts().unwrap().notes, 1);
    }
}
