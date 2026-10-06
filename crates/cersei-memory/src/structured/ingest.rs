//! Ingestion: record, extract, resolve, apply, embed — resumable.
//!
//! Episode `extraction_state`:
//!
//! | state | meaning | next |
//! |---|---|---|
//! | `pending` | stored, not extracted | extract |
//! | `extracted` | validated output cached on the episode | apply (no model call) |
//! | `applied` | entities, facts and evidence written | — |
//! | `failed` | last attempt failed (`extraction_error`); retried up to `max_attempts` | extract |
//! | `skipped` | no extractor, or a legacy memory | — |
//!
//! Applying an extraction is one Grafeo transaction that also moves the
//! episode to `applied`: either everything of the episode is written, or
//! nothing and the cached output is applied again next time. Fact and
//! entity ids are derived from content, evidence links are checked before
//! being added: re-processing an episode never duplicates facts.
//!
//! Embedding is separate and idempotent: records whose `embed_model` is not
//! the current model are embedded (again) by [`StructuredMemory::process`].

use super::extract::{bounded, validate, ExtractError, ExtractionRequest, Validated};
use super::index::Item;
use super::model::*;
use super::store::{alias_key, as_i64, as_str, as_strings, q, P, W};
use super::{MemoryError, MemoryResult, StructuredMemory};
use std::collections::{HashMap, HashSet};
use tokio_util::sync::CancellationToken;

/// Result of [`StructuredMemory::record`].
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Recorded {
    pub id: String,
    /// False when the same episode was already stored.
    pub new: bool,
}

/// What a [`StructuredMemory::process`] pass did.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct IngestReport {
    pub extracted: usize,
    /// Applied from a cached extraction, without calling the extractor.
    pub applied_from_cache: usize,
    pub failed: Vec<(String, String)>,
    pub facts_created: usize,
    pub facts_confirmed: usize,
    pub facts_superseded: usize,
    pub facts_contested: usize,
    pub entities_created: usize,
    pub embedded: usize,
    pub embedding_errors: Vec<String>,
    /// Candidates dropped by validation, with the reason.
    pub notes: Vec<String>,
}

/// What [`StructuredMemory::forget_session`] removed.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ForgetReport {
    pub episodes_deleted: usize,
    /// Facts whose only evidence was in the session: now `unsupported`.
    pub facts_unsupported: Vec<String>,
    /// Facts that keep evidence from other sessions.
    pub facts_kept: Vec<String>,
}

const OPEN_STATUSES: [&str; 3] = ["active", "proposed", "contested"];

impl StructuredMemory {
    /// Store an episode durably (no extraction, no embedding yet). The same
    /// episode recorded twice is stored once.
    pub async fn record(&self, input: EpisodeInput) -> MemoryResult<Recorded> {
        let mut r = self.record_all(vec![input]).await?;
        Ok(r.remove(0))
    }

    /// Store several episodes durably in one transaction: all or none.
    /// Grafeo 0.5's transactions cost time proportional to the size of the
    /// graph (begin counts the nodes, commit walks every version chain), so
    /// bulk ingestion records its episodes together.
    pub async fn record_all(&self, inputs: Vec<EpisodeInput>) -> MemoryResult<Vec<Recorded>> {
        for input in &inputs {
            if !valid_space(&input.space) {
                return Err(MemoryError::Config(format!(
                    "invalid space `{}`",
                    input.space
                )));
            }
            if input.content.trim().is_empty() {
                return Err(MemoryError::Config("an episode needs content".into()));
            }
        }
        let _w = self.write.lock().await;
        let mut out = Vec::with_capacity(inputs.len());
        let mut ops = Vec::new();
        let mut batch: HashSet<String> = HashSet::new();
        let now = self.now();
        for input in inputs {
            let id = input.id();
            if batch.contains(&id)
                || !self
                    .store
                    .ids(q::EPISODE_EXISTS, P::new().s("id", &id))?
                    .is_empty()
            {
                out.push(Recorded { id, new: false });
                continue;
            }
            batch.insert(id.clone());
            ops.push((
                W::InsertEpisode,
                P::new()
                    .s("id", &id)
                    .s("space", &input.space)
                    .os("session_id", input.session_id.as_deref())
                    .s("role", &input.role)
                    .os("author", input.author.as_deref())
                    .s("content", &input.content)
                    .oi("occurred_at", input.occurred_at)
                    .i("recorded_at", now)
                    .os("source_ref", input.source_ref.as_deref())
                    .i("ikey", self.store.next_ikey()),
            ));
            if self.extractor.is_none() {
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
            out.push(Recorded { id, new: true });
        }
        if !ops.is_empty() {
            self.store.transaction(ops)?;
        }
        Ok(out)
    }

    /// Record several episodes, then process pending work.
    pub async fn ingest(
        &self,
        inputs: Vec<EpisodeInput>,
        cancel: &CancellationToken,
    ) -> MemoryResult<IngestReport> {
        let mut inputs = inputs.into_iter().peekable();
        while inputs.peek().is_some() {
            self.record_all(inputs.by_ref().take(512).collect()).await?;
        }
        self.process(cancel).await
    }

    /// Extract and apply pending episodes, then embed what has no vector of
    /// the current model. Resumable: call again after a failure or restart.
    ///
    /// One pass runs at a time. A pass holds no lock across a model or
    /// embedding call: recording and recall go on meanwhile, and the
    /// memory's write lock is only taken for each local write.
    pub async fn process(&self, cancel: &CancellationToken) -> MemoryResult<IngestReport> {
        let _pass = self.maintenance.lock().await;
        let mut report = self.extract_pending(cancel).await?;
        if cancel.is_cancelled() {
            return Ok(report);
        }
        let applied = self.apply_extracted().await?;
        merge(&mut report, applied);
        let embedded = self.embed_pending_until(cancel).await;
        merge(&mut report, embedded);
        Ok(report)
    }

    /// Work a pass would still do: episodes waiting for (or allowed to
    /// retry) extraction, cached extractions not applied, records without a
    /// vector of the current model.
    pub fn pending_work(&self) -> MemoryResult<usize> {
        let mut n = 0;
        if self.extractor.is_some() {
            for id in self.store.ids(
                q::PENDING_EPISODES,
                P::new().list("states", &["pending".into(), "failed".into()]),
            )? {
                let state = self.store.query(q::EPISODE_STATE, P::new().s("id", &id))?;
                let retryable = state.first().is_some_and(|row| {
                    as_str(&row[0]).as_deref() != Some("failed")
                        || as_i64(&row[1]).unwrap_or(0) < self.config.extraction.max_attempts as i64
                });
                n += usize::from(retryable);
            }
        }
        n += self
            .store
            .ids(
                q::PENDING_EPISODES,
                P::new().list("states", &["extracted".into()]),
            )?
            .len();
        let model = self.projection.read().model.clone();
        n += self
            .store
            .query(q::UNEMBEDDED, P::new().s("model", &model))?
            .len();
        Ok(n)
    }

    /// Stage 1: run the extractor on pending (and retryable failed)
    /// episodes and cache each validated output on its episode. Nothing
    /// else is written: a stop after this stage costs no model call later.
    pub async fn extract_pending(&self, cancel: &CancellationToken) -> MemoryResult<IngestReport> {
        let mut report = IngestReport::default();
        let Some(extractor) = self.extractor.clone() else {
            return Ok(report);
        };
        let ids = self.store.ids(
            q::PENDING_EPISODES,
            P::new().list("states", &["pending".into(), "failed".into()]),
        )?;
        for id in ids {
            if cancel.is_cancelled() {
                break;
            }
            let state = self.store.query(q::EPISODE_STATE, P::new().s("id", &id))?;
            let Some(row) = state.first() else { continue };
            let st = as_str(&row[0]).unwrap_or_default();
            let attempts = as_i64(&row[1]).unwrap_or(0);
            if st == "failed" && attempts >= self.config.extraction.max_attempts as i64 {
                continue;
            }
            let Some(episode) = self.store.episode(&id)? else {
                continue;
            };
            let (content, truncated) =
                bounded(&episode.content, self.config.extraction.max_input_chars);
            let req = ExtractionRequest {
                episode_id: id.clone(),
                space: episode.space.clone(),
                role: episode.role.clone(),
                content: content.clone(),
                truncated,
                reference_date: episode.occurred_at.map(super::clock::date),
            };
            // The model call runs without any lock held.
            let out = extractor
                .extract(&req, cancel)
                .await
                .and_then(|raw| validate(&raw, &content, &self.config.extraction).map(|_| raw));
            let _w = self.write.lock().await;
            if !matches!(out, Err(ExtractError::Cancelled))
                && self
                    .store
                    .ids(q::EPISODE_EXISTS, P::new().s("id", &id))?
                    .is_empty()
            {
                // Deleted (its session forgotten) while it was extracted.
                continue;
            }
            match out {
                Ok(raw) => {
                    self.store.exec(
                        W::SetExtraction,
                        P::new()
                            .s("id", &id)
                            .s("state", "extracted")
                            .i("attempts", attempts + 1)
                            .os("json", Some(&raw))
                            .os("error", None),
                    )?;
                    report.extracted += 1;
                }
                Err(ExtractError::Cancelled) => break,
                Err(e) => {
                    self.store.exec(
                        W::SetExtraction,
                        P::new()
                            .s("id", &id)
                            .s("state", "failed")
                            .i("attempts", attempts + 1)
                            .os("json", None)
                            .os("error", Some(&e.to_string())),
                    )?;
                    report.failed.push((id.clone(), e.to_string()));
                }
            }
        }
        Ok(report)
    }

    /// Stage 2: apply every cached extraction (no model call).
    pub async fn apply_extracted(&self) -> MemoryResult<IngestReport> {
        let _w = self.write.lock().await;
        let mut report = IngestReport::default();
        let ids = self.store.ids(
            q::PENDING_EPISODES,
            P::new().list("states", &["extracted".into()]),
        )?;
        for id in ids {
            let state = self.store.query(q::EPISODE_STATE, P::new().s("id", &id))?;
            let Some(raw) = state.first().and_then(|r| as_str(&r[2])) else {
                continue;
            };
            let Some(episode) = self.store.episode(&id)? else {
                continue;
            };
            let (content, _) = bounded(&episode.content, self.config.extraction.max_input_chars);
            match validate(&raw, &content, &self.config.extraction) {
                Ok(v) => {
                    self.apply(&episode, v, &mut report)?;
                    report.applied_from_cache += 1;
                }
                Err(e) => report.failed.push((id.clone(), e.to_string())),
            }
        }
        Ok(report)
    }

    /// Resolve an entity in the episode's space: by normalised name or
    /// alias. Returns `(id, created, aliases to add)`.
    fn resolve_entity(
        &self,
        space: &str,
        name: &str,
        aliases: &[String],
        local: &HashMap<String, String>,
        notes: &mut Vec<String>,
    ) -> MemoryResult<(String, bool, Vec<String>)> {
        let mut keys: Vec<String> = vec![normalize_name(name)];
        for a in aliases {
            let n = normalize_name(a);
            if !keys.contains(&n) {
                keys.push(n);
            }
        }
        if let Some(id) = keys.iter().find_map(|k| local.get(k)) {
            return Ok((id.clone(), false, Vec::new()));
        }
        let mut found: Vec<(String, String)> = Vec::new(); // (id, name)
        for k in &keys {
            for id in self
                .store
                .ids(q::ALIAS, P::new().s("key", &alias_key(space, k)))?
            {
                if found.iter().any(|(i, _)| *i == id) {
                    continue;
                }
                let name = self
                    .store
                    .query(q::ENTITY, P::new().s("id", &id))?
                    .first()
                    .and_then(|r| as_str(&r[0]))
                    .unwrap_or_default();
                found.push((id, name));
            }
        }
        found.sort();
        match found.len() {
            0 => Ok((stable_id("ent", &[space, &keys[0]]), true, Vec::new())),
            n => {
                // Prefer the entity whose name matches exactly.
                let pick = found
                    .iter()
                    .find(|(_, name)| normalize_name(name) == keys[0])
                    .unwrap_or(&found[0])
                    .0
                    .clone();
                if n > 1 {
                    notes.push(format!(
                        "`{name}` matches {n} entities in {space}; linked to {pick}"
                    ));
                }
                let mut add = Vec::new();
                for k in &keys {
                    if !self
                        .store
                        .ids(q::ALIAS, P::new().s("key", &alias_key(space, k)))?
                        .contains(&pick)
                    {
                        add.push(k.clone());
                    }
                }
                Ok((pick, false, add))
            }
        }
    }

    fn apply(
        &self,
        episode: &Episode,
        v: Validated,
        report: &mut IngestReport,
    ) -> MemoryResult<()> {
        let space = episode.space.as_str();
        let now = self.now();
        let origin = Origin::from_role(&episode.role);
        let asserted_at = episode.occurred_at.unwrap_or(episode.recorded_at);
        report
            .notes
            .extend(v.notes.iter().map(|n| format!("{}: {n}", episode.id)));
        let mut ops: Vec<(W, P)> = Vec::new();
        // Entities: candidate reference → id.
        let mut ent: HashMap<String, (String, String)> = HashMap::new(); // ref → (id, name)
        let mut local: HashMap<String, String> = HashMap::new(); // norm → id (this episode)
        let mut created_entities = 0;
        for e in &v.entities {
            let (id, created, add) =
                self.resolve_entity(space, &e.name, &e.aliases, &local, &mut report.notes)?;
            if created {
                let norm = normalize_name(&e.name);
                let mut aliases: Vec<String> = e
                    .aliases
                    .iter()
                    .map(|a| normalize_name(a))
                    .filter(|a| *a != norm)
                    .collect();
                aliases.sort();
                aliases.dedup();
                ops.push((
                    W::InsertEntity,
                    P::new()
                        .s("id", &id)
                        .s("space", space)
                        .s("type", &e.kind)
                        .s("name", &e.name)
                        .s("norm", &norm)
                        .list("aliases", &aliases),
                ));
                for k in std::iter::once(&norm).chain(aliases.iter()) {
                    ops.push((
                        W::InsertAlias,
                        P::new().s("key", &alias_key(space, k)).s("entity_id", &id),
                    ));
                }
                created_entities += 1;
            } else if !add.is_empty() {
                let mut all = self
                    .store
                    .query(q::ENTITY, P::new().s("id", &id))?
                    .first()
                    .map(|r| as_strings(&r[2]))
                    .unwrap_or_default();
                for k in &add {
                    ops.push((
                        W::InsertAlias,
                        P::new().s("key", &alias_key(space, k)).s("entity_id", &id),
                    ));
                    if !all.contains(k) {
                        all.push(k.clone());
                    }
                }
                ops.push((
                    W::SetEntityAliases,
                    P::new().s("id", &id).list("aliases", &all),
                ));
            }
            local.insert(normalize_name(&e.name), id.clone());
            for a in &e.aliases {
                local.insert(normalize_name(a), id.clone());
            }
            if !ent.values().any(|(i, _)| *i == id) {
                ops.push((
                    W::LinkEpisodeEntity,
                    P::new().s("episode", &episode.id).s("entity", &id),
                ));
            }
            ent.insert(e.reference.clone(), (id, e.name.clone()));
        }
        // Facts, with the change rules. `pending` holds facts created in
        // this episode so later facts of the same episode see them.
        let mut pending: Vec<Fact> = Vec::new();
        let mut status_changes: HashMap<String, FactStatus> = HashMap::new();
        for f in &v.facts {
            let Some((subject_id, subject_name)) = ent.get(&f.subject.reference).cloned() else {
                continue;
            };
            let object_id = f
                .object
                .as_ref()
                .and_then(|o| ent.get(&o.reference))
                .map(|(i, _)| i.clone());
            let value_norm = normalize_name(&f.value);
            let mut existing: Vec<Fact> = Vec::new();
            for id in self
                .store
                .facts_by_key(space, &subject_id, &f.predicate, &OPEN_STATUSES)?
            {
                if let Some(mut x) = self.store.fact(&id)? {
                    if let Some(s) = status_changes.get(&x.id) {
                        x.status = *s;
                    }
                    existing.push(x);
                }
            }
            existing.extend(
                pending
                    .iter()
                    .filter(|p| p.subject_id == subject_id && p.predicate == f.predicate)
                    .cloned(),
            );
            existing.retain(|x| {
                let s = status_changes.get(&x.id).copied().unwrap_or(x.status);
                OPEN_STATUSES.contains(&s.as_str())
            });
            let new_validity = Validity {
                from: f.valid_from,
                until: f.valid_until,
                asserted_at,
            };
            // Same assertion: a confirmation adds evidence.
            if let Some(same) = existing.iter().find(|x| {
                x.value_norm == value_norm
                    && x.negated == f.negated
                    && overlaps(&x.validity, &new_validity)
            }) {
                let has = self.store.has_evidence(&same.id, &episode.id);
                let in_ops = pending.iter().any(|p| p.id == same.id);
                if !has && !in_ops {
                    ops.push((
                        W::LinkFactEvidence,
                        P::new()
                            .s("fact", &same.id)
                            .s("episode", &episode.id)
                            .s("quote", &f.quote),
                    ));
                    report.facts_confirmed += 1;
                }
                let current = status_changes.get(&same.id).copied().unwrap_or(same.status);
                if current == FactStatus::Proposed && origin.is_authoritative() {
                    ops.push((
                        W::SetFactStatus,
                        P::new().s("id", &same.id).s("status", "active"),
                    ));
                    status_changes.insert(same.id.clone(), FactStatus::Active);
                }
                continue;
            }
            let id = stable_id(
                "fact",
                &[
                    space,
                    &subject_id,
                    &f.predicate,
                    &value_norm,
                    if f.negated { "not" } else { "" },
                    &episode.id,
                ],
            );
            let exclusive = self.config.is_exclusive(&f.predicate);
            let conflicts: Vec<&Fact> = existing
                .iter()
                .filter(|x| overlaps(&x.validity, &new_validity))
                .filter(|x| {
                    if exclusive {
                        x.value_norm != value_norm || x.negated != f.negated
                    } else {
                        x.value_norm == value_norm && x.negated != f.negated
                    }
                })
                .collect();
            let mut status = if origin.is_authoritative() {
                FactStatus::Active
            } else {
                FactStatus::Proposed
            };
            let mut links: Vec<(W, P)> = Vec::new();
            if origin.is_authoritative() {
                for c in &conflicts {
                    let c_status = status_changes.get(&c.id).copied().unwrap_or(c.status);
                    if c_status == FactStatus::Proposed {
                        // A proposal never outranks a statement; it stays a proposal.
                        continue;
                    }
                    if f.explicit_change {
                        links.push((
                            W::SupersedeFact,
                            P::new()
                                .s("id", &c.id)
                                .i("at", now)
                                .oi("until", f.valid_from)
                                .oi("ended_by", f.valid_from.is_none().then_some(asserted_at)),
                        ));
                        links.push((
                            W::LinkSupersedes,
                            P::new().s("new", &id).s("old", &c.id).i("at", now),
                        ));
                        status_changes.insert(c.id.clone(), FactStatus::Superseded);
                        report.facts_superseded += 1;
                    } else {
                        links.push((
                            W::SetFactStatus,
                            P::new().s("id", &c.id).s("status", "contested"),
                        ));
                        links.push((W::LinkContradicts, P::new().s("a", &id).s("b", &c.id)));
                        status_changes.insert(c.id.clone(), FactStatus::Contested);
                        status = FactStatus::Contested;
                        report.facts_contested += 1;
                    }
                }
            }
            let statement = format!(
                "{subject_name} · {}{}: {}",
                if f.negated { "not " } else { "" },
                f.predicate.replace('_', " "),
                f.value
            );
            ops.push((
                W::InsertFact,
                P::new()
                    .s("id", &id)
                    .s("space", space)
                    .s("subject_id", &subject_id)
                    .s("subject_name", &subject_name)
                    .s("predicate", &f.predicate)
                    .s("value", &f.value)
                    .s("value_norm", &value_norm)
                    .os("object_id", object_id.as_deref())
                    .b("negated", f.negated)
                    .s("origin", origin.as_str())
                    .s("status", status.as_str())
                    .of("confidence", f.confidence)
                    .oi("valid_from", f.valid_from)
                    .oi("valid_until", f.valid_until)
                    .i("asserted_at", asserted_at)
                    .i("recorded_at", now)
                    .s("statement", &statement)
                    .i("ikey", self.store.next_ikey()),
            ));
            ops.push((
                W::LinkFactEvidence,
                P::new()
                    .s("fact", &id)
                    .s("episode", &episode.id)
                    .s("quote", &f.quote),
            ));
            ops.push((
                W::LinkFactAbout,
                P::new().s("fact", &id).s("entity", &subject_id),
            ));
            if let Some(o) = &object_id {
                ops.push((W::LinkFactMentions, P::new().s("fact", &id).s("entity", o)));
            }
            ops.extend(links);
            report.facts_created += 1;
            pending.push(Fact {
                id: id.clone(),
                space: space.to_string(),
                subject_id: subject_id.clone(),
                subject_name: subject_name.clone(),
                predicate: f.predicate.clone(),
                value: f.value.clone(),
                value_norm: value_norm.clone(),
                object_id,
                negated: f.negated,
                origin,
                status,
                confidence: f.confidence,
                validity: new_validity,
                supersession: Supersession::default(),
                knowledge: Knowledge {
                    recorded_at: now,
                    superseded_at: None,
                    retracted_at: None,
                },
                statement,
            });
        }
        ops.push((
            W::SetExtraction,
            P::new()
                .s("id", &episode.id)
                .s("state", "applied")
                .i("attempts", 0)
                .os("json", None)
                .os("error", None),
        ));
        self.store.transaction(ops)?;
        report.entities_created += created_entities;
        // Keep the projection's catalogue in step with status changes.
        let mut p = self.projection.write();
        for id in status_changes.keys() {
            if let Some(f) = self.store.fact(id)? {
                p.refresh(Item::Fact(Box::new(f)));
            }
        }
        Ok(())
    }

    /// Stage 3: embed every record without a vector of the current model
    /// (new records, failed embeddings, or all records after a model change).
    pub async fn embed_pending(&self) -> IngestReport {
        self.embed_pending_until(&CancellationToken::new()).await
    }

    /// [`Self::embed_pending`], stopping between batches (and abandoning an
    /// embedding call in flight) when `cancel` fires: what is not embedded
    /// stays without a vector for the next pass.
    pub async fn embed_pending_until(&self, cancel: &CancellationToken) -> IngestReport {
        let mut report = IngestReport::default();
        let report = &mut report;
        let model = self.projection.read().model.clone();
        let rows = match self.store.query(q::UNEMBEDDED, P::new().s("model", &model)) {
            Ok(r) => r,
            Err(e) => {
                report.embedding_errors.push(e);
                return report.clone();
            }
        };
        let mut items: Vec<(String, Item, String)> = Vec::new(); // id, item, text
        for row in rows {
            let (Some(id), labels) = (as_str(&row[0]), as_strings(&row[1])) else {
                continue;
            };
            if labels.iter().any(|l| l == "Fact") {
                if let Ok(Some(f)) = self.store.fact(&id) {
                    let text = f.statement.clone();
                    items.push((id, Item::Fact(Box::new(f)), text));
                }
            } else if let Ok(Some(e)) = self.store.episode(&id) {
                let text = e.content.clone();
                items.push((id, Item::Episode(Box::new(e)), text));
            }
        }
        for chunk in items.chunks(64) {
            if cancel.is_cancelled() {
                break;
            }
            let texts: Vec<String> = chunk.iter().map(|(_, _, t)| t.clone()).collect();
            let call = tokio::select! {
                r = self.embedder.embed_batch(&texts) => r,
                _ = cancel.cancelled() => break,
            };
            let vectors = match call {
                Ok(v) if v.len() == texts.len() => v,
                Ok(v) => {
                    report.embedding_errors.push(format!(
                        "the embedder returned {} vectors for {} texts",
                        v.len(),
                        texts.len()
                    ));
                    continue;
                }
                Err(e) => {
                    report.embedding_errors.push(e.to_string());
                    continue;
                }
            };
            for ((id, item, _), vector) in chunk.iter().zip(vectors) {
                let query = match item {
                    Item::Fact(_) => W::SetEmbedding,
                    Item::Episode(_) => W::SetEmbedding,
                };
                if let Err(e) = self.store.exec(
                    query,
                    P::new()
                        .s("id", id)
                        .vector("embedding", &vector)
                        .s("model", &model),
                ) {
                    report.embedding_errors.push(e);
                    continue;
                }
                let key = self
                    .store
                    .query(
                        match item {
                            Item::Fact(_) => q::IKEY,
                            Item::Episode(_) => q::IKEY,
                        },
                        P::new().s("id", id),
                    )
                    .ok()
                    .and_then(|r| r.first().and_then(|row| as_i64(&row[0])));
                if let Some(key) = key {
                    match self
                        .projection
                        .write()
                        .upsert(key as u64, item.clone(), &vector)
                    {
                        Ok(()) => report.embedded += 1,
                        Err(e) => report.embedding_errors.push(e),
                    }
                }
            }
        }
        report.clone()
    }

    /// Delete a session's episodes. Facts keep the evidence they have from
    /// other episodes; a fact left without any evidence becomes
    /// `unsupported` (kept for history, never recalled).
    pub async fn forget_session(&self, session_id: &str) -> MemoryResult<ForgetReport> {
        let _w = self.write.lock().await;
        let mut report = ForgetReport::default();
        let facts = self
            .store
            .ids(q::FACTS_OF_SESSION, P::new().s("session_id", session_id))?;
        let episodes = self
            .store
            .ids(q::EPISODES_OF_SESSION, P::new().s("session_id", session_id))?;
        let ops: Vec<(W, P)> = episodes
            .iter()
            .map(|id| (W::DeleteNode, P::new().s("id", id)))
            .collect();
        self.store.transaction(ops)?;
        report.episodes_deleted = episodes.len();
        let mut p = self.projection.write();
        for id in &episodes {
            p.remove_id(id);
        }
        for id in facts {
            let left = self
                .store
                .query(q::EVIDENCE_COUNT, P::new().s("fact", &id))?
                .first()
                .and_then(|r| as_i64(&r[0]))
                .unwrap_or(0);
            if left == 0 {
                self.store.exec(
                    W::SetFactStatus,
                    P::new().s("id", &id).s("status", "unsupported"),
                )?;
                report.facts_unsupported.push(id.clone());
            } else {
                report.facts_kept.push(id.clone());
            }
            if let Some(f) = self.store.fact(&id)? {
                p.refresh(Item::Fact(Box::new(f)));
            }
        }
        Ok(report)
    }

    /// Withdraw a fact explicitly (kept for history).
    pub async fn retract(&self, fact_id: &str) -> MemoryResult<()> {
        let _w = self.write.lock().await;
        self.store.exec(
            W::RetractFact,
            P::new().s("id", fact_id).i("at", self.now()),
        )?;
        if let Some(f) = self.store.fact(fact_id)? {
            self.projection.write().refresh(Item::Fact(Box::new(f)));
        }
        Ok(())
    }
}

fn merge(a: &mut IngestReport, b: IngestReport) {
    a.extracted += b.extracted;
    a.applied_from_cache += b.applied_from_cache;
    a.failed.extend(b.failed);
    a.facts_created += b.facts_created;
    a.facts_confirmed += b.facts_confirmed;
    a.facts_superseded += b.facts_superseded;
    a.facts_contested += b.facts_contested;
    a.entities_created += b.entities_created;
    a.embedded += b.embedded;
    a.embedding_errors.extend(b.embedding_errors);
    a.notes.extend(b.notes);
}

/// Do two validity intervals overlap? Unknown bounds extend to infinity.
fn overlaps(a: &Validity, b: &Validity) -> bool {
    let a_start = a.from.unwrap_or(i64::MIN);
    let b_start = b.from.unwrap_or(i64::MIN);
    let a_end = a.until.unwrap_or(i64::MAX);
    let b_end = b.until.unwrap_or(i64::MAX);
    a_start < b_end && b_start < a_end
}
