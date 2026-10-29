//! Acceptance: the memory of a run, by scope (project and global).
//!
//! Capture goes to the project only; recall fuses both scopes; a note is
//! indexed before it is returned; a failed indexing keeps the note and its
//! retry never duplicates it; one unavailable scope never stops the other;
//! clearing waits for the leases of the runs; pages, counts and sources
//! never count a note as an exchange. Local stores in temporary folders,
//! hashing embeddings, scripted extraction: no network, no paid call.

#![cfg(feature = "structured")]

use async_trait::async_trait;
use bricks_embeddings::{EmbeddingError, EmbeddingProvider, HashingEmbeddings};
use bricks_memory::structured::extract::{ExtractError, ExtractionRequest, Extractor};
use bricks_memory::structured::{
    EntryFilter, EntryKind, FactStatus, MemoryConfig, MemoryError, OpenMemories, Processing,
    RECALL_MAX, ScopedMemory, SharedStore, global_space, project_space,
};
use bricks_memory::{LongTermMemory, MemoryScope, MemoryTurn};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Hashing embeddings that fail while `down` is set (same identity, so the
/// store accepts them in place of the working embedder).
struct Flaky {
    inner: HashingEmbeddings,
    down: Arc<AtomicBool>,
}

impl Flaky {
    fn new(down: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            inner: HashingEmbeddings::new(64),
            down,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for Flaky {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if self.down.load(Ordering::SeqCst) {
            return Err(EmbeddingError::Api("embedder down".into()));
        }
        self.inner.embed_batch(texts).await
    }
}

/// Stores of one test, in a temporary folder, opened through their own
/// [`OpenMemories`] (never the process-wide one).
struct Stores {
    dir: tempfile::TempDir,
    open: OpenMemories,
}

impl Stores {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            open: OpenMemories::default(),
        }
    }

    fn path(&self, file: &str) -> PathBuf {
        self.dir.path().join(file)
    }

    fn open_with(&self, file: &str, space: &str, down: Arc<AtomicBool>) -> SharedStore {
        self.open
            .open(
                &self.path(file),
                space,
                Flaky::new(down),
                &MemoryConfig::default(),
                None,
            )
            .unwrap()
    }

    fn project(&self, id: &str) -> SharedStore {
        self.open_with(
            &format!("{id}.grafeo"),
            &project_space(id),
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn global(&self) -> SharedStore {
        self.open_with(
            "global.grafeo",
            global_space(),
            Arc::new(AtomicBool::new(false)),
        )
    }
}

fn turn(role: &str, content: &str) -> MemoryTurn {
    MemoryTurn {
        role: role.into(),
        content: content.into(),
        at: None,
    }
}

/// The message of an error, whatever the type of the success.
fn err_text<T, E: std::fmt::Display>(r: Result<T, E>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

/// A run's memory over `project` and `global`, with its leases.
fn run_memory(
    project: Option<(&str, Option<SharedStore>)>,
    global: Option<SharedStore>,
    processing: Processing,
) -> (ScopedMemory, Vec<bricks_memory::structured::Lease>) {
    let project_id = project.as_ref().map(|(id, _)| id.to_string());
    let project_slot = project.map(|(_, store)| match store {
        Some(s) => Ok(s),
        None => Err("no store".to_string()),
    });
    let global_slot = global.map(Ok);
    ScopedMemory::for_run(project_id, project_slot, global_slot, processing)
}

// ─── Capture and scopes ──────────────────────────────────────────────────────

#[tokio::test]
async fn a_run_with_a_project_captures_to_the_project_only() {
    let s = Stores::new();
    let (pa, g) = (s.project("pa"), s.global());
    let (m, _leases) = run_memory(
        Some(("pa", Some(pa.clone()))),
        Some(g.clone()),
        Processing::default(),
    );
    m.record_turns(
        Some("s1"),
        &[turn("user", "alpha uses postgres"), turn("assistant", "noted")],
    )
    .await
    .unwrap();
    assert_eq!(pa.memory.counts().unwrap().exchanges, 2);
    assert_eq!(g.memory.counts().unwrap().exchanges, 0, "never the global");
}

#[tokio::test]
async fn no_project_means_no_capture_no_project_search_and_no_fallback() {
    let s = Stores::new();
    let g = s.global();
    let (m, _leases) = run_memory(None, Some(g.clone()), Processing::default());
    m.record_turns(Some("s1"), &[turn("user", "hello there")])
        .await
        .unwrap();
    assert_eq!(g.memory.counts().unwrap().exchanges, 0, "no capture without a project");

    let e = err_text(m.search(MemoryScope::Project, "hello", 5).await);
    assert!(e.contains("no project"), "{e}");
    let e = err_text(
        m.write_note(MemoryScope::Project, Some("s1"), "a project note")
            .await,
    );
    assert!(e.contains("no project"), "{e}");
    assert_eq!(g.memory.counts().unwrap().notes, 0, "no fallback write");

    // The global memory still answers.
    m.write_note(MemoryScope::Global, Some("s1"), "prefers short answers")
        .await
        .unwrap();
    let found = m
        .search(MemoryScope::Global, "short answers", 5)
        .await
        .unwrap()
        .expect("the global note is found");
    assert_eq!(found.items, 1);
}

#[tokio::test]
async fn projects_are_isolated_and_a_global_note_names_the_project_that_wrote_it() {
    let s = Stores::new();
    let (pa, pb, g) = (s.project("pa"), s.project("pb"), s.global());
    let (ma, _la) = run_memory(Some(("pa", Some(pa))), Some(g.clone()), Processing::default());
    let (mb, _lb) = run_memory(Some(("pb", Some(pb))), Some(g), Processing::default());

    ma.write_note(MemoryScope::Project, Some("s1"), "alpha uses postgres")
        .await
        .unwrap();
    ma.write_note(MemoryScope::Global, Some("s1"), "prefers short answers")
        .await
        .unwrap();

    assert!(
        mb.search(MemoryScope::Project, "postgres", 5)
            .await
            .unwrap()
            .is_none(),
        "the other project does not see it"
    );
    let ctx = mb
        .recall_context("short answers", 2000)
        .await
        .unwrap()
        .expect("the global note is recalled");
    assert!(
        ctx.text.contains("(global memory, from project pa)"),
        "provenance named: {}",
        ctx.text
    );
    assert!(!ctx.text.contains("postgres"), "no project note leaks: {}", ctx.text);
}

#[tokio::test]
async fn a_note_is_found_in_the_same_run_without_an_extractor() {
    let s = Stores::new();
    let pa = s.project("pa");
    let (m, _l) = run_memory(Some(("pa", Some(pa.clone()))), None, Processing::default());
    let id = m
        .write_note(MemoryScope::Project, Some("s1"), "Deploys on Fridays only")
        .await
        .unwrap();
    assert!(!id.is_empty());
    let found = m
        .search(MemoryScope::Project, "deploys fridays", 5)
        .await
        .unwrap()
        .expect("found at once, in the same run");
    assert!(found.text.contains("Deploys on Fridays only"), "{}", found.text);
    assert_eq!(pa.memory.counts().unwrap().notes, 1);
}

// ─── Indexing failure and retry ──────────────────────────────────────────────

#[tokio::test]
async fn a_failed_indexing_keeps_the_note_and_the_retry_does_not_duplicate_it() {
    let s = Stores::new();
    let down = Arc::new(AtomicBool::new(true));
    let pa = s.open_with("pa.grafeo", &project_space("pa"), down.clone());
    let (m, _l) = run_memory(Some(("pa", Some(pa.clone()))), None, Processing::default());

    let err = err_text(
        m.write_note(MemoryScope::Project, Some("s1"), "Deploys on Fridays")
            .await,
    );
    assert!(err.contains("is saved but not indexed yet"), "{err}");
    assert_eq!(pa.memory.counts().unwrap().notes, 1, "the note is kept");
    let first_id = pa
        .memory
        .entries(EntryFilter::Notes, "", 0, 10)
        .unwrap()
        .entries[0]
        .id
        .clone();

    down.store(false, Ordering::SeqCst);
    m.write_note(MemoryScope::Project, Some("s1"), "Deploys on Fridays")
        .await
        .unwrap();
    assert_eq!(pa.memory.counts().unwrap().notes, 1, "no duplicate");
    assert_eq!(
        pa.memory.entries(EntryFilter::Notes, "", 0, 10).unwrap().entries[0].id,
        first_id,
        "the same note"
    );
    assert_eq!(pa.memory.stats().indexed, 1, "indexed by the retry");
    assert!(
        m.search(MemoryScope::Project, "deploys fridays", 5)
            .await
            .unwrap()
            .is_some(),
        "found after the retry"
    );
}

// ─── Recall: fusion, dedup, bounds, budget ───────────────────────────────────

#[tokio::test]
async fn fused_recall_is_deterministic_deduplicated_bounded_and_within_budget() {
    let s = Stores::new();
    let (pa, g) = (s.project("pa"), s.global());
    let (m, _l) = run_memory(
        Some(("pa", Some(pa.clone()))),
        Some(g.clone()),
        Processing::default(),
    );
    // The same content in both scopes.
    m.write_note(MemoryScope::Project, Some("s1"), "Prefers tabs over spaces")
        .await
        .unwrap();
    m.write_note(MemoryScope::Global, Some("s1"), "Prefers tabs over spaces")
        .await
        .unwrap();
    for i in 0..8 {
        m.write_note(
            MemoryScope::Project,
            Some("s1"),
            &format!("tabs note variant {i} about formatting"),
        )
        .await
        .unwrap();
    }

    let ctx = m
        .recall_context("tabs over spaces", 4000)
        .await
        .unwrap()
        .expect("recalled");
    assert_eq!(
        ctx.text.matches("Prefers tabs over spaces").count(),
        1,
        "identical contents are merged: {}",
        ctx.text
    );
    let shared = ctx
        .text
        .lines()
        .find(|l| l.contains("Prefers tabs over spaces"))
        .expect("the shared content is listed");
    assert!(shared.contains("[R1] (project memory)"), "{shared}");
    assert!(
        ctx.text.contains("also in: global memory"),
        "its other source is kept: {}",
        ctx.text
    );
    assert!(ctx.items <= RECALL_MAX, "at most {RECALL_MAX}: {}", ctx.items);

    let again = m
        .recall_context("tabs over spaces", 4000)
        .await
        .unwrap()
        .expect("recalled");
    assert_eq!(ctx.text, again.text, "the same query gives the same context");

    // Many candidates: never more than RECALL_MAX entries.
    let many = m
        .recall_context("tabs note variant formatting", 10_000)
        .await
        .unwrap()
        .expect("recalled");
    assert!(many.items <= RECALL_MAX, "{}", many.items);
    assert_eq!(
        many.text.lines().filter(|l| l.contains("[R")).count(),
        many.items
    );

    // The token budget is respected.
    for budget in [200usize, 400, 800] {
        if let Some(c) = m.recall_context("tabs over spaces", budget).await.unwrap() {
            assert!(
                c.tokens <= budget as u64,
                "{} tokens for a budget of {budget}",
                c.tokens
            );
        }
    }
}

#[tokio::test]
async fn one_unavailable_scope_is_reported_and_the_other_still_answers() {
    let s = Stores::new();
    let g = s.global();
    let (m, _l) = ScopedMemory::for_run(
        Some("pa".into()),
        Some(Err("the store is locked".into())),
        Some(Ok(g.clone())),
        Processing::default(),
    );
    m.write_note(MemoryScope::Global, Some("s1"), "prefers short answers")
        .await
        .unwrap();

    let ctx = m
        .recall_context("short answers", 1000)
        .await
        .unwrap()
        .expect("the global answer is kept");
    assert!(ctx.items >= 1, "{ctx:?}");
    assert!(
        ctx.warnings
            .iter()
            .any(|w| w.contains("project memory") && w.contains("unavailable")),
        "{:?}",
        ctx.warnings
    );

    let e = err_text(m.search(MemoryScope::Project, "short", 5).await);
    assert!(e.contains("unavailable"), "{e}");
    assert!(
        m.search(MemoryScope::Global, "short answers", 5)
            .await
            .unwrap()
            .is_some(),
        "the global search works"
    );
    let e = err_text(m.record_turns(Some("s1"), &[turn("user", "hi")]).await);
    assert!(e.contains("unavailable"), "{e}");
    assert_eq!(g.memory.counts().unwrap().exchanges, 0, "no capture into global");
}

// ─── One handle per file ─────────────────────────────────────────────────────

#[test]
fn concurrent_opens_of_one_file_share_one_handle_and_conflicts_name_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let open = Arc::new(OpenMemories::default());
    let path = dir.path().join("shared.grafeo");
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let open = Arc::clone(&open);
            // Another spelling of the same file for half of them.
            let p = if i % 2 == 0 {
                path.clone()
            } else {
                dir.path().join(".").join("shared.grafeo")
            };
            std::thread::spawn(move || {
                open.open(
                    &p,
                    "project:p",
                    Arc::new(HashingEmbeddings::new(64)),
                    &MemoryConfig::default(),
                    None,
                )
                .unwrap()
            })
        })
        .collect();
    let handles: Vec<SharedStore> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    for h in &handles[1..] {
        assert!(Arc::ptr_eq(&handles[0].memory, &h.memory), "one handle per file");
    }

    let e = err_text(open.open(
        &path,
        "project:other",
        Arc::new(HashingEmbeddings::new(64)),
        &MemoryConfig::default(),
        None,
    ));
    assert!(e.contains("shared.grafeo"), "{e}");
    assert!(e.contains("the space"), "{e}");
    assert!(e.contains("project:p") && e.contains("project:other"), "{e}");

    let e = err_text(open.open(
        &path,
        "project:p",
        Arc::new(HashingEmbeddings::new(32)),
        &MemoryConfig::default(),
        None,
    ));
    assert!(e.contains("embedding"), "{e}");
    let (m64, m32) = (
        HashingEmbeddings::new(64).model_id(),
        HashingEmbeddings::new(32).model_id(),
    );
    assert!(e.contains(&m64) && e.contains(&m32), "{e}");
}

// ─── Leases and deletion ─────────────────────────────────────────────────────

struct Nothing;

#[async_trait]
impl Extractor for Nothing {
    fn id(&self) -> String {
        "nothing/test".into()
    }

    async fn extract(
        &self,
        _r: &ExtractionRequest,
        _c: &CancellationToken,
    ) -> Result<String, ExtractError> {
        Ok(r#"{"entities":[],"facts":[]}"#.into())
    }
}

#[tokio::test]
async fn clearing_is_refused_while_a_run_leases_the_store_then_clears_only_its_scope() {
    let s = Stores::new();
    let (pa, g) = (s.project("pa"), s.global());
    let processing = Processing {
        extractor: Some(Arc::new(Nothing)),
        ..Default::default()
    };
    let (m, leases) = run_memory(
        Some(("pa", Some(pa.clone()))),
        Some(g.clone()),
        processing.clone(),
    );
    m.record_turns(Some("s1"), &[turn("user", "pending exchange")])
        .await
        .unwrap();
    m.write_note(MemoryScope::Global, Some("s1"), "a global preference")
        .await
        .unwrap();
    assert!(
        pa.memory.pending_work_with(&processing).unwrap() > 0,
        "the exchange waits for its extraction"
    );

    assert!(matches!(pa.forget_all().await, Err(MemoryError::InUse(_))));
    assert!(matches!(g.forget_all().await, Err(MemoryError::InUse(_))));
    assert!(pa.in_use() && g.in_use());

    drop(leases);
    assert!(!pa.in_use() && !g.in_use());
    let report = pa.forget_all().await.unwrap();
    assert_eq!(report.episodes, 1, "the exchange");
    assert_eq!(pa.memory.counts().unwrap().exchanges, 0);
    assert_eq!(
        pa.memory.pending_work_with(&processing).unwrap(),
        0,
        "its pending work is gone too"
    );
    assert_eq!(
        g.memory.counts().unwrap().notes,
        1,
        "the other scope is untouched"
    );
}

// ─── Pages, counts, sources ──────────────────────────────────────────────────

#[tokio::test]
async fn pages_counts_and_sources_never_duplicate_and_a_note_is_not_an_exchange() {
    let s = Stores::new();
    let pa = s.project("pa");
    let (m, _l) = run_memory(Some(("pa", Some(pa.clone()))), None, Processing::default());
    for i in 0..7 {
        m.write_note(MemoryScope::Project, Some("s1"), &format!("note number {i}"))
            .await
            .unwrap();
    }
    m.record_turns(
        Some("s1"),
        &[turn("user", "question about tabs"), turn("assistant", "answer about tabs")],
    )
    .await
    .unwrap();

    let c = pa.memory.counts().unwrap();
    assert_eq!((c.notes, c.exchanges, c.facts), (7, 2, 0), "a note is not an exchange");

    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    for offset in [0usize, 3, 6] {
        let page = pa.memory.entries(EntryFilter::All, "", offset, 3).unwrap();
        assert_eq!(page.total, 9);
        for e in &page.entries {
            assert!(seen.insert(e.id.clone()), "no entry twice: {}", e.id);
            total += 1;
        }
    }
    assert_eq!(total, 9, "every entry, once");

    assert_eq!(pa.memory.entries(EntryFilter::Notes, "", 0, 50).unwrap().total, 7);
    assert_eq!(pa.memory.entries(EntryFilter::Exchanges, "", 0, 50).unwrap().total, 2);
    assert_eq!(pa.memory.entries(EntryFilter::Notes, "NUMBER 3", 0, 50).unwrap().total, 1);
    assert_eq!(pa.memory.entries(EntryFilter::All, "tabs", 0, 50).unwrap().total, 2);
    assert_eq!(pa.memory.entries(EntryFilter::Facts, "", 0, 50).unwrap().total, 0);

    let page = pa.memory.entries(EntryFilter::Notes, "", 0, 50).unwrap();
    let note = &page.entries[0];
    assert_eq!(note.kind, EntryKind::Note);
    assert_eq!(note.session_id.as_deref(), Some("s1"));
    assert_eq!(note.source_ref.as_deref(), Some("session:s1"));
    assert!(note.status.is_none() && note.evidence.is_empty());

    assert!(pa.memory.entries(EntryFilter::All, "", 0, 0).is_err());
    assert!(pa.memory.entries(EntryFilter::All, "", 0, 201).is_err());
}

struct Axum;

#[async_trait]
impl Extractor for Axum {
    fn id(&self) -> String {
        "scripted/test".into()
    }

    async fn extract(
        &self,
        r: &ExtractionRequest,
        _c: &CancellationToken,
    ) -> Result<String, ExtractError> {
        if r.role == "user" && r.content.contains("utilise Axum") {
            return Ok(json!({
                "entities": [
                    {"ref": "e1", "name": "Bricks", "type": "project"},
                    {"ref": "e2", "name": "Axum", "type": "library"}
                ],
                "facts": [{
                    "subject": "e1", "predicate": "uses_framework", "value": "Axum",
                    "object": "e2", "quote": "Le projet Bricks utilise Axum"
                }]
            })
            .to_string());
        }
        Ok(r#"{"entities":[],"facts":[]}"#.into())
    }
}

#[tokio::test]
async fn extracted_facts_are_listed_with_their_status_and_source_and_counted_apart() {
    let s = Stores::new();
    let pa = s.project("pa");
    let processing = Processing {
        extractor: Some(Arc::new(Axum)),
        ..Default::default()
    };
    let (m, _l) = run_memory(
        Some(("pa", Some(pa.clone()))),
        None,
        processing.clone(),
    );
    m.record_turns(
        Some("s1"),
        &[
            turn("user", "Le projet Bricks utilise Axum pour son serveur HTTP."),
            turn("assistant", "Noté."),
        ],
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let report = pa.memory.process_with(&cancel, &processing).await.unwrap();
    assert_eq!(report.facts_created, 1);

    let c = pa.memory.counts().unwrap();
    assert_eq!((c.notes, c.exchanges, c.facts), (0, 2, 1));

    let facts = pa.memory.entries(EntryFilter::Facts, "", 0, 50).unwrap();
    assert_eq!(facts.total, 1);
    let fact = &facts.entries[0];
    assert_eq!(fact.kind, EntryKind::Fact);
    assert_eq!(fact.status, Some(FactStatus::Active));
    assert!(
        fact.evidence.iter().any(|ev| ev.quote.contains("utilise Axum")),
        "the source quote is kept: {:?}",
        fact.evidence
    );
    assert_eq!(pa.memory.entries(EntryFilter::Exchanges, "", 0, 50).unwrap().total, 2);
    assert_eq!(pa.memory.entries(EntryFilter::All, "", 0, 50).unwrap().total, 3);
    assert_eq!(
        pa.memory.entries(EntryFilter::FactHistory, "", 0, 50).unwrap().total,
        1
    );
}
