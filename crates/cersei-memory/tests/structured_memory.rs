//! Structured memory end to end, with a scripted extractor and the
//! deterministic hashing embedder: no network, no paid call.

#![cfg(feature = "structured")]

use async_trait::async_trait;
use cersei_embeddings::{EmbeddingError, EmbeddingProvider, HashingEmbeddings};
use cersei_memory::structured::clock::parse_time;
use cersei_memory::structured::extract::{ExtractError, ExtractionRequest, Extractor};
use cersei_memory::structured::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// ─── Test doubles ────────────────────────────────────────────────────────────

/// Answers with the JSON registered for the first matching substring;
/// counts calls; can fail a number of times first.
#[derive(Default)]
struct Scripted {
    answers: Vec<(&'static str, String)>,
    calls: AtomicUsize,
    fail_first: AtomicUsize,
}

impl Scripted {
    fn new(answers: Vec<(&'static str, String)>) -> Arc<Self> {
        Arc::new(Self {
            answers,
            ..Default::default()
        })
    }
}

#[async_trait]
impl Extractor for Scripted {
    fn id(&self) -> String {
        "scripted/test".into()
    }

    async fn extract(
        &self,
        r: &ExtractionRequest,
        _c: &CancellationToken,
    ) -> Result<String, ExtractError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first.load(Ordering::SeqCst) > 0 {
            self.fail_first.fetch_sub(1, Ordering::SeqCst);
            return Ok("this is not { json".into());
        }
        Ok(self
            .answers
            .iter()
            .find(|(k, _)| r.content.contains(k))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| r#"{"entities":[],"facts":[]}"#.into()))
    }
}

/// An extractor that must not be called.
struct Forbidden;

#[async_trait]
impl Extractor for Forbidden {
    fn id(&self) -> String {
        "forbidden".into()
    }
    async fn extract(
        &self,
        _: &ExtractionRequest,
        _: &CancellationToken,
    ) -> Result<String, ExtractError> {
        panic!("the extractor was called although a cached extraction existed");
    }
}

/// Hashing embeddings that fail while `broken` is set.
struct Flaky {
    inner: HashingEmbeddings,
    broken: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl EmbeddingProvider for Flaky {
    fn name(&self) -> &str {
        "hashing"
    }
    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
    fn model_id(&self) -> String {
        self.inner.model_id()
    }
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(EmbeddingError::Api("embedding service down".into()));
        }
        self.inner.embed_batch(texts).await
    }
}

fn embedder() -> Arc<dyn EmbeddingProvider> {
    Arc::new(HashingEmbeddings::new(256))
}

fn day(s: &str) -> i64 {
    parse_time(s).unwrap()
}

fn ep(space: &str, session: &str, role: &str, at: &str, content: &str) -> EpisodeInput {
    EpisodeInput {
        space: space.into(),
        session_id: Some(session.into()),
        role: role.into(),
        author: None,
        content: content.into(),
        occurred_at: Some(day(at)),
        source_ref: None,
    }
}

fn json(entities: &[(&str, &str, &str, &[&str])], facts: &[serde_json::Value]) -> String {
    let ents: Vec<serde_json::Value> = entities
        .iter()
        .map(|(r, n, t, a)| serde_json::json!({"ref": r, "name": n, "type": t, "aliases": a}))
        .collect();
    serde_json::json!({"entities": ents, "facts": facts}).to_string()
}

fn fact(subject: &str, predicate: &str, value: &str, quote: &str) -> serde_json::Value {
    serde_json::json!({"subject": subject, "predicate": predicate, "value": value, "quote": quote, "confidence": 0.9})
}

fn memory(extractor: Arc<dyn Extractor>, clock: Arc<ManualClock>) -> StructuredMemory {
    StructuredMemory::builder(embedder())
        .config(MemoryConfig::default().with_space("project:bricks"))
        .extractor(extractor)
        .clock(clock)
        .open()
        .unwrap()
}

const P: &str = "project:bricks";

fn axum_ep() -> EpisodeInput {
    ep(
        P,
        "s1",
        "user",
        "2026-01-10",
        "Le projet Bricks utilise Axum pour son serveur HTTP.",
    )
}

fn axum_json() -> String {
    json(
        &[
            ("e1", "Bricks", "project", &["bricks-rs"]),
            ("e2", "Axum", "library", &[]),
        ],
        &[
            serde_json::json!({"subject": "e1", "predicate": "uses_framework", "value": "Axum", "object": "e2",
                             "quote": "Le projet Bricks utilise Axum", "confidence": 0.95}),
        ],
    )
}

fn actix_ep() -> EpisodeInput {
    ep(
        P,
        "s2",
        "user",
        "2026-03-01",
        "Le projet Bricks passe d'Axum à Actix à partir du 2026-03-01.",
    )
}

fn actix_json() -> String {
    json(
        &[
            ("e1", "Bricks", "project", &[]),
            ("e2", "Actix", "library", &[]),
        ],
        &[
            serde_json::json!({"subject": "e1", "predicate": "uses_framework", "value": "Actix", "object": "e2",
                             "valid_from": "2026-03-01", "explicit_change": true,
                             "quote": "Le projet Bricks passe d'Axum à Actix"}),
        ],
    )
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ingestion_keeps_provenance_and_is_idempotent() {
    let x = Scripted::new(vec![("utilise Axum", axum_json())]);
    let clock = Arc::new(ManualClock::new(day("2026-01-11")));
    let m = memory(x.clone(), clock);
    let c = CancellationToken::new();
    let r = m.ingest(vec![axum_ep()], &c).await.unwrap();
    assert_eq!(
        (r.extracted, r.facts_created, r.entities_created),
        (1, 1, 2),
        "{r:?}"
    );
    assert!(r.embedded >= 2, "episode and fact embedded: {r:?}");

    let facts = m.current_facts(P).unwrap();
    assert_eq!(facts.len(), 1);
    let f = &facts[0];
    assert_eq!(
        (
            f.subject_name.as_str(),
            f.predicate.as_str(),
            f.value.as_str()
        ),
        ("Bricks", "uses_framework", "Axum")
    );
    assert_eq!(f.origin, Origin::UserStatement);
    assert_eq!(f.status, FactStatus::Active);
    assert_eq!(
        f.confidence,
        Some(0.95),
        "extraction confidence is kept, separately"
    );
    assert_eq!(
        f.validity.from, None,
        "no stated start: unknown, not the ingestion date"
    );
    assert_eq!(f.validity.asserted_at, day("2026-01-10"));
    let ev = m.evidence(&f.id).unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].quote, "Le projet Bricks utilise Axum");
    assert_eq!(ev[0].session_id.as_deref(), Some("s1"));
    let episode = m.episode(&ev[0].episode_id).unwrap().unwrap();
    assert_eq!(
        episode.content,
        axum_ep().content,
        "the source stays resolvable and unchanged"
    );

    // Same episode again: stored once, extracted once, no new fact.
    let again = m.record(axum_ep()).await.unwrap();
    assert!(!again.new);
    let r = m.process(&c).await.unwrap();
    assert_eq!((r.extracted, r.facts_created), (0, 0));
    assert_eq!(x.calls.load(Ordering::SeqCst), 1);
    assert_eq!(m.stats().facts, 1);

    // A batch: duplicates (stored or within the batch) are stored once.
    let mut other = axum_ep();
    other.content = "Le projet Bricks a une démo vendredi.".into();
    let r = m
        .record_all(vec![axum_ep(), other.clone(), other])
        .await
        .unwrap();
    assert_eq!(
        r.iter().map(|r| r.new).collect::<Vec<_>>(),
        vec![false, true, false]
    );
    assert_eq!(m.stats().episodes, 2);
    // An invalid input refuses the whole batch.
    let mut bad = axum_ep();
    bad.space = "bricks".into();
    let mut third = axum_ep();
    third.content = "Troisième épisode.".into();
    assert!(m.record_all(vec![third, bad]).await.is_err());
    assert_eq!(m.stats().episodes, 2);
}

#[tokio::test]
async fn malformed_extraction_keeps_the_episode_and_is_bounded() {
    let x = Scripted::new(vec![("utilise Axum", axum_json())]);
    x.fail_first.store(1, Ordering::SeqCst);
    let m = memory(x.clone(), Arc::new(ManualClock::new(day("2026-01-11"))));
    let c = CancellationToken::new();
    let r = m.ingest(vec![axum_ep()], &c).await.unwrap();
    assert_eq!(r.failed.len(), 1, "{r:?}");
    assert!(r.failed[0].1.contains("malformed"), "{r:?}");
    assert_eq!(m.stats().episodes, 1, "the episode is not lost");
    assert!(r.embedded >= 1, "and it is still searchable");
    // The next pass retries and succeeds.
    let r = m.process(&c).await.unwrap();
    assert_eq!(r.facts_created, 1);

    // Always malformed: attempts stop at max_attempts (3).
    let bad = Scripted::new(vec![]);
    bad.fail_first.store(100, Ordering::SeqCst);
    let m = memory(bad.clone(), Arc::new(ManualClock::new(0)));
    m.record(axum_ep()).await.unwrap();
    for _ in 0..6 {
        m.process(&c).await.unwrap();
    }
    assert_eq!(bad.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_cached_extraction_is_applied_after_a_restart_without_calling_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.grafeo");
    let x = Scripted::new(vec![("utilise Axum", axum_json())]);
    {
        let m = StructuredMemory::builder(embedder())
            .path(&path)
            .config(MemoryConfig::default().with_space(P))
            .extractor(x.clone())
            .open()
            .unwrap();
        m.record(axum_ep()).await.unwrap();
        let r = m.extract_pending(&CancellationToken::new()).await.unwrap();
        assert_eq!(r.extracted, 1);
        m.close().unwrap();
        // "Crash" here: extracted, not applied.
    }
    let m = StructuredMemory::builder(embedder())
        .path(&path)
        .config(MemoryConfig::default().with_space(P))
        .extractor(Arc::new(Forbidden))
        .open()
        .unwrap();
    let r = m.process(&CancellationToken::new()).await.unwrap();
    assert_eq!((r.applied_from_cache, r.facts_created), (1, 1), "{r:?}");
    assert_eq!(x.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn aliases_resolve_within_a_space_and_projects_stay_apart() {
    let x = Scripted::new(vec![
        ("utilise Axum", axum_json()),
        (
            "bricks-rs est écrit",
            json(
                &[("e1", "bricks-rs", "project", &[])],
                &[fact(
                    "e1",
                    "language",
                    "Rust",
                    "bricks-rs est écrit en Rust",
                )],
            ),
        ),
        (
            "module auth de Bricks",
            json(
                &[("e1", "auth", "module", &[])],
                &[fact(
                    "e1",
                    "storage",
                    "PostgreSQL",
                    "module auth de Bricks stocke dans PostgreSQL",
                )],
            ),
        ),
        (
            "module auth de Zephyr",
            json(
                &[("e1", "auth", "module", &[])],
                &[fact(
                    "e1",
                    "storage",
                    "Redis",
                    "module auth de Zephyr stocke dans Redis",
                )],
            ),
        ),
    ]);
    let m = memory(x, Arc::new(ManualClock::new(day("2026-05-01"))));
    let c = CancellationToken::new();
    m.ingest(
        vec![
            axum_ep(),
            ep(
                P,
                "s2",
                "user",
                "2026-01-12",
                "bricks-rs est écrit en Rust.",
            ),
            ep(
                P,
                "s3",
                "user",
                "2026-01-12",
                "Le module auth de Bricks stocke dans PostgreSQL.",
            ),
            ep(
                "project:zephyr",
                "z1",
                "user",
                "2026-01-12",
                "Le module auth de Zephyr stocke dans Redis.",
            ),
        ],
        &c,
    )
    .await
    .unwrap();
    let facts = m.current_facts(P).unwrap();
    let bricks: Vec<&Fact> = facts
        .iter()
        .filter(|f| f.subject_name == "Bricks" || f.subject_name == "bricks-rs")
        .collect();
    assert_eq!(bricks.len(), 2);
    assert_eq!(
        bricks[0].subject_id, bricks[1].subject_id,
        "the alias resolved to the same entity"
    );
    let auth_bricks = facts.iter().find(|f| f.predicate == "storage").unwrap();
    let zephyr = m.current_facts("project:zephyr").unwrap();
    assert_eq!(zephyr.len(), 1);
    assert_ne!(
        auth_bricks.subject_id, zephyr[0].subject_id,
        "same name, different projects"
    );
    // Recall in Bricks never returns Zephyr's facts.
    let r = m
        .recall(&RecallQuery::new("module auth storage"))
        .await
        .unwrap();
    assert!(
        r.items
            .iter()
            .all(|i| i.fact.as_ref().is_none_or(|f| f.space == P)),
        "{}",
        r.rendered
    );
    assert!(
        r.rendered.contains("PostgreSQL") && !r.rendered.contains("Redis"),
        "{}",
        r.rendered
    );
}

fn chain_extractor() -> Arc<Scripted> {
    Scripted::new(vec![
        (
            "Alice travaille",
            json(
                &[
                    ("e1", "Alice", "person", &[]),
                    ("e2", "Acme", "organization", &[]),
                ],
                &[
                    serde_json::json!({"subject":"e1","predicate":"works_at","value":"Acme","object":"e2","quote":"Alice travaille chez Acme"}),
                ],
            ),
        ),
        (
            "Acme est installée",
            json(
                &[
                    ("e1", "Acme", "organization", &[]),
                    ("e2", "Lyon", "place", &[]),
                ],
                &[
                    serde_json::json!({"subject":"e1","predicate":"located_in","value":"Lyon","object":"e2","quote":"Acme est installée à Lyon"}),
                ],
            ),
        ),
        (
            "Lyon compte",
            json(
                &[("e1", "Lyon", "place", &[])],
                &[fact(
                    "e1",
                    "population",
                    "520000",
                    "Lyon compte 520000 habitants",
                )],
            ),
        ),
    ])
}

#[tokio::test]
async fn relational_expansion_reaches_two_hops_and_stops_at_its_limits() {
    let c = CancellationToken::new();
    let eps = vec![
        ep(
            P,
            "s1",
            "user",
            "2026-01-01",
            "Alice travaille chez Acme depuis longtemps.",
        ),
        ep(P, "s2", "user", "2026-01-02", "Acme est installée à Lyon."),
        ep(
            P,
            "s3",
            "user",
            "2026-01-03",
            "Lyon compte 520000 habitants.",
        ),
    ];
    let mut cfg = MemoryConfig::default().with_space(P);
    cfg.recall.seeds = 1;
    cfg.recall.candidates = 1;
    cfg.recall.weight_lexical = 0.0;
    // Generous: the time budget is exercised separately below (debug builds
    // and parallel tests are slow).
    cfg.recall.expansion_budget = std::time::Duration::from_secs(30);
    let open = |cfg: MemoryConfig| {
        StructuredMemory::builder(embedder())
            .config(cfg)
            .extractor(chain_extractor())
            .clock(Arc::new(ManualClock::new(day("2026-02-01"))))
            .open()
            .unwrap()
    };
    let m = open(cfg.clone());
    m.ingest(eps.clone(), &c).await.unwrap();
    assert_eq!(
        m.stats().entities,
        3,
        "Alice, Acme, Lyon, each resolved once: {:?}",
        m.stats()
    );
    let q = RecallQuery::new("Alice works at");
    let r = m.recall(&q).await.unwrap();
    let values: Vec<&str> = r
        .items
        .iter()
        .filter_map(|i| i.fact.as_ref())
        .map(|f| f.value.as_str())
        .collect();
    assert!(values.contains(&"Lyon"), "1 hop: {values:?}");
    assert!(
        values.contains(&"520000"),
        "2 hops: {values:?}\n{}",
        r.rendered
    );
    let lyon = r
        .items
        .iter()
        .find(|i| i.fact.as_ref().is_some_and(|f| f.value == "Lyon"))
        .unwrap();
    assert!(
        lyon.relational_rank.is_some() && lyon.vector_rank.is_none(),
        "reached by the graph only"
    );

    let mut one = cfg.clone();
    one.recall.hops = 1;
    let m1 = open(one);
    m1.ingest(eps.clone(), &c).await.unwrap();
    let values: Vec<String> = m1
        .recall(&q)
        .await
        .unwrap()
        .items
        .iter()
        .filter_map(|i| i.fact.as_ref())
        .map(|f| f.value.clone())
        .collect();
    assert!(
        values.contains(&"Lyon".to_string()) && !values.contains(&"520000".to_string()),
        "{values:?}"
    );

    let mut no_time = cfg.clone();
    no_time.recall.expansion_budget = std::time::Duration::ZERO;
    let m3 = open(no_time);
    m3.ingest(eps.clone(), &c).await.unwrap();
    let r = m3.recall(&q).await.unwrap();
    assert_eq!(
        r.expansion.stopped.as_deref(),
        Some("time budget"),
        "{:?}",
        r.expansion
    );
    assert!(!r.items.is_empty(), "the other lanes still answer");

    let mut tight = cfg;
    tight.recall.max_visited = 2;
    let m2 = open(tight);
    m2.ingest(eps, &c).await.unwrap();
    let r = m2.recall(&q).await.unwrap();
    assert_eq!(
        r.expansion.stopped.as_deref(),
        Some("max_visited"),
        "{:?}",
        r.expansion
    );
    // Same query, same memory: same order.
    let again = m.recall(&q).await.unwrap();
    let ids = |r: &Recall| r.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&m.recall(&q).await.unwrap()), ids(&again));
}

#[tokio::test]
async fn an_explicit_change_replaces_the_value_and_history_stays_queryable() {
    let x = Scripted::new(vec![
        ("utilise Axum", axum_json()),
        ("passe d'Axum", actix_json()),
    ]);
    let clock = Arc::new(ManualClock::new(day("2026-01-11")));
    let m = memory(x, clock.clone());
    let c = CancellationToken::new();
    m.ingest(vec![axum_ep()], &c).await.unwrap();
    let before_change = clock.now();
    clock.set(day("2026-03-02"));
    let r = m.ingest(vec![actix_ep()], &c).await.unwrap();
    assert_eq!(r.facts_superseded, 1, "{r:?}");

    let current = m.current_facts(P).unwrap();
    assert_eq!(
        current.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(),
        vec!["Actix"]
    );
    let actix = &current[0];
    let all = m.facts(P, Some(day("2026-02-01")), None, false).unwrap();
    let axum = all
        .iter()
        .find(|f| f.value == "Axum")
        .expect("valid in February");
    assert_eq!(axum.status, FactStatus::Superseded);
    assert_eq!(
        axum.supersession.until,
        Some(day("2026-03-01")),
        "end = the change's stated date"
    );
    assert!(
        all.iter().all(|f| f.value != "Actix"),
        "Actix starts 2026-03-01: not valid in February"
    );
    assert_eq!(
        m.supersedes(&actix.id).unwrap(),
        vec![axum.id.clone()],
        "new SUPERSEDES old"
    );
    // What Bricks knew before the change was recorded.
    let then = m
        .facts(P, Some(before_change), Some(before_change), false)
        .unwrap();
    assert_eq!(
        then.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(),
        vec!["Axum"]
    );
    // Recall shows the replacement.
    let r = m
        .recall(&RecallQuery::new("framework of Bricks"))
        .await
        .unwrap();
    assert!(
        r.rendered.contains("Actix") && r.rendered.contains("replaces: Axum"),
        "{}",
        r.rendered
    );
    assert!(
        !r.rendered.contains("· uses framework: Axum ("),
        "the old value is not current: {}",
        r.rendered
    );
}

#[tokio::test]
async fn other_projects_compatible_facts_ambiguity_and_future_facts() {
    let x = Scripted::new(vec![
        ("utilise Axum", axum_json()),
        (
            "autre projet",
            json(
                &[
                    ("e1", "Kestrel", "project", &[]),
                    ("e2", "Actix", "library", &[]),
                ],
                &[
                    serde_json::json!({"subject":"e1","predicate":"uses_framework","value":"Actix","object":"e2","quote":"J'utilise Actix sur un autre projet"}),
                ],
            ),
        ),
        (
            "aime le thé",
            json(
                &[("e1", "Max", "person", &[])],
                &[fact("e1", "likes", "thé", "Max aime le thé")],
            ),
        ),
        (
            "aime aussi le café",
            json(
                &[("e1", "Max", "person", &[])],
                &[fact("e1", "likes", "café", "Max aime aussi le café")],
            ),
        ),
        (
            "Bricks tourne sur Rocket",
            json(
                &[
                    ("e1", "Bricks", "project", &[]),
                    ("e2", "Rocket", "library", &[]),
                ],
                &[
                    serde_json::json!({"subject":"e1","predicate":"uses_framework","value":"Rocket","object":"e2","quote":"Bricks tourne sur Rocket"}),
                ],
            ),
        ),
        (
            "deadline",
            json(
                &[("e1", "Bricks", "project", &[])],
                &[
                    serde_json::json!({"subject":"e1","predicate":"deadline","value":"2027-06-30","valid_from":"2027-01-01","quote":"À partir du 2027-01-01, la deadline de Bricks est le 2027-06-30"}),
                ],
            ),
        ),
    ]);
    let m = memory(x, Arc::new(ManualClock::new(day("2026-04-01"))));
    let c = CancellationToken::new();
    m.ingest(
        vec![
            axum_ep(),
            ep(
                "project:kestrel",
                "k1",
                "user",
                "2026-02-01",
                "J'utilise Actix sur un autre projet, Kestrel.",
            ),
            ep(P, "s4", "user", "2026-02-02", "Max aime le thé."),
            ep(P, "s5", "user", "2026-02-03", "Max aime aussi le café."),
        ],
        &c,
    )
    .await
    .unwrap();
    // Another project replaces nothing in Bricks; likes are compatible.
    let current = m.current_facts(P).unwrap();
    assert!(current
        .iter()
        .any(|f| f.value == "Axum" && f.status == FactStatus::Active));
    assert_eq!(current.iter().filter(|f| f.predicate == "likes").count(), 2);

    // A silent different value of an exclusive property: both kept, contested.
    let r = m
        .ingest(
            vec![ep(
                P,
                "s6",
                "user",
                "2026-03-01",
                "Bricks tourne sur Rocket maintenant ?",
            )],
            &c,
        )
        .await
        .unwrap();
    assert_eq!(r.facts_contested, 1, "{r:?}");
    let fw: Vec<Fact> = m
        .current_facts(P)
        .unwrap()
        .into_iter()
        .filter(|f| f.predicate == "uses_framework")
        .collect();
    assert_eq!(fw.len(), 2);
    assert!(fw.iter().all(|f| f.status == FactStatus::Contested));
    assert_eq!(m.contradictions(&fw[0].id).unwrap().len(), 1);
    let rec = m
        .recall(&RecallQuery::new("Bricks framework"))
        .await
        .unwrap();
    assert!(rec.rendered.contains("CONTESTED"), "{}", rec.rendered);

    // A future fact is not current, but valid at its date.
    m.ingest(
        vec![ep(
            P,
            "s7",
            "user",
            "2026-03-15",
            "À partir du 2027-01-01, la deadline de Bricks est le 2027-06-30.",
        )],
        &c,
    )
    .await
    .unwrap();
    assert!(m
        .current_facts(P)
        .unwrap()
        .iter()
        .all(|f| f.predicate != "deadline"));
    let later = m.facts(P, Some(day("2027-02-01")), None, false).unwrap();
    assert!(later.iter().any(|f| f.predicate == "deadline"));
    let rec = m
        .recall(&RecallQuery::new("deadline of Bricks"))
        .await
        .unwrap();
    assert!(
        rec.items
            .iter()
            .all(|i| i.fact.as_ref().is_none_or(|f| f.predicate != "deadline")),
        "a future fact is not recalled as a current fact (its source passage may be): {}",
        rec.rendered
    );
    let mut at = RecallQuery::new("deadline of Bricks");
    at.valid_at = Some(day("2027-02-01"));
    let rec = m.recall(&at).await.unwrap();
    assert!(
        rec.items
            .iter()
            .any(|i| i.fact.as_ref().is_some_and(|f| f.predicate == "deadline")),
        "{}",
        rec.rendered
    );
}

#[tokio::test]
async fn assistant_proposals_need_user_confirmation() {
    let prop = json(
        &[("e1", "Bricks", "project", &[])],
        &[fact(
            "e1",
            "database",
            "SQLite",
            "Bricks pourrait utiliser SQLite",
        )],
    );
    let conf = json(
        &[("e1", "Bricks", "project", &[])],
        &[fact("e1", "database", "SQLite", "Bricks utilisera SQLite")],
    );
    let x = Scripted::new(vec![
        ("pourrait utiliser SQLite", prop),
        ("utilisera SQLite", conf),
    ]);
    let m = memory(x, Arc::new(ManualClock::new(day("2026-04-01"))));
    let c = CancellationToken::new();
    m.ingest(
        vec![ep(
            P,
            "s1",
            "assistant",
            "2026-03-01",
            "Je propose : Bricks pourrait utiliser SQLite.",
        )],
        &c,
    )
    .await
    .unwrap();
    assert!(
        m.current_facts(P).unwrap().is_empty(),
        "a proposal is not a confirmed fact"
    );
    let proposed = m.facts(P, None, None, true).unwrap();
    assert_eq!(proposed[0].status, FactStatus::Proposed);
    m.ingest(
        vec![ep(
            P,
            "s1",
            "user",
            "2026-03-02",
            "Oui, Bricks utilisera SQLite.",
        )],
        &c,
    )
    .await
    .unwrap();
    let current = m.current_facts(P).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(
        current[0].status,
        FactStatus::Active,
        "confirmed by the user"
    );
    assert_eq!(
        m.evidence(&current[0].id).unwrap().len(),
        2,
        "the confirmation is a second piece of evidence"
    );
}

#[tokio::test]
async fn special_characters_and_unicode_round_trip() {
    let content = "Le client « O'Brien » dit: '}) DETACH DELETE n // 日本語 ✓ \"quoted\" \\ fin.";
    let x = Scripted::new(vec![(
        "O'Brien",
        json(
            &[("e1", "O'Brien \"Ltd\" 日本", "organization", &["O’Brien"])],
            &[fact(
                "e1",
                "says",
                "'}) DETACH DELETE n",
                "'}) DETACH DELETE n",
            )],
        ),
    )]);
    let m = memory(x, Arc::new(ManualClock::new(day("2026-04-01"))));
    m.ingest(
        vec![ep(P, "s'1\"", "user", "2026-03-01", content)],
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let f = &m.current_facts(P).unwrap()[0];
    assert_eq!(f.subject_name, "O'Brien \"Ltd\" 日本");
    assert_eq!(f.value, "'}) DETACH DELETE n");
    let ev = &m.evidence(&f.id).unwrap()[0];
    assert_eq!(ev.session_id.as_deref(), Some("s'1\""));
    assert_eq!(m.episode(&ev.episode_id).unwrap().unwrap().content, content);
    assert_eq!(m.stats().episodes, 1);
    let r = m
        .recall(&RecallQuery::new("O'Brien 日本語 DETACH"))
        .await
        .unwrap();
    assert!(!r.items.is_empty());
}

#[tokio::test]
async fn close_restore_session_deletion_and_index_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.grafeo");
    let x = Scripted::new(vec![
        ("utilise Axum", axum_json()),
        ("Axum toujours", axum_json()),
    ]);
    let flaky = Arc::new(Flaky {
        inner: HashingEmbeddings::new(256),
        broken: true.into(),
    });
    let c = CancellationToken::new();
    {
        let m = StructuredMemory::builder(flaky.clone())
            .path(&path)
            .config(MemoryConfig::default().with_space(P))
            .extractor(x.clone())
            .open()
            .unwrap();
        let r = m
            .ingest(
                vec![
                    axum_ep(),
                    ep(
                        P,
                        "s9",
                        "user",
                        "2026-01-20",
                        "Bricks utilise Axum toujours. Le projet Bricks utilise Axum",
                    ),
                ],
                &c,
            )
            .await
            .unwrap();
        assert!(!r.embedding_errors.is_empty(), "embedding failed…");
        assert_eq!(m.stats().indexed, 0);
        assert_eq!(m.stats().facts, 1, "…but facts and episodes are stored");
        m.close().unwrap();
    }
    flaky.broken.store(false, Ordering::SeqCst);
    let m = StructuredMemory::builder(flaky.clone())
        .path(&path)
        .config(MemoryConfig::default().with_space(P))
        .extractor(Arc::new(Forbidden))
        .open()
        .unwrap();
    let r = m.process(&c).await.unwrap();
    assert_eq!(
        r.embedded, 3,
        "the interrupted embeddings are completed: {r:?}"
    );
    assert_eq!(m.stats().indexed, 3);
    let rec = m.recall(&RecallQuery::new("framework Axum")).await.unwrap();
    assert!(rec.rendered.contains("Axum"));
    m.close().unwrap();
    drop(m);

    // Restored: the projection is rebuilt from the store.
    let m = StructuredMemory::builder(flaky.clone())
        .path(&path)
        .config(MemoryConfig::default().with_space(P))
        .open()
        .unwrap();
    assert_eq!(m.stats().indexed, 3);
    let fact_id = m.current_facts(P).unwrap()[0].id.clone();
    assert_eq!(m.evidence(&fact_id).unwrap().len(), 2);

    // Deleting a session: the fact keeps its other evidence…
    let rep = m.forget_session("s1").await.unwrap();
    assert_eq!(
        (rep.episodes_deleted, rep.facts_kept.len()),
        (1, 1),
        "{rep:?}"
    );
    assert_eq!(m.evidence(&fact_id).unwrap().len(), 1);
    // …and becomes unsupported when its last source goes.
    let rep = m.forget_session("s9").await.unwrap();
    assert_eq!(rep.facts_unsupported, vec![fact_id.clone()]);
    assert!(m.current_facts(P).unwrap().is_empty());
    assert_eq!(
        m.fact(&fact_id).unwrap().unwrap().status,
        FactStatus::Unsupported
    );
    let rec = m.recall(&RecallQuery::new("framework Axum")).await.unwrap();
    assert!(
        rec.items.is_empty(),
        "nothing without surviving evidence is recalled: {}",
        rec.rendered
    );
    m.close().unwrap();
    drop(m);

    // Another embedding model: refused, unless a rebuild is asked for.
    let other: Arc<dyn EmbeddingProvider> = Arc::new(HashingEmbeddings::new(128));
    let err = StructuredMemory::builder(other.clone())
        .path(&path)
        .open()
        .err()
        .unwrap();
    assert!(
        matches!(err, MemoryError::EmbeddingMismatch { .. }),
        "{err}"
    );
    let m = StructuredMemory::builder(other)
        .path(&path)
        .rebuild_embeddings(true)
        .open()
        .unwrap();
    assert_eq!(m.stats().indexed, 0);
    let r = m.embed_pending().await;
    assert_eq!(
        r.embedded, 1,
        "the surviving fact is re-embedded in the new space: {r:?}"
    );
}

#[tokio::test]
async fn legacy_v2_memories_migrate_to_searchable_episodes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.grafeo");
    {
        let db = grafeo::GrafeoDB::open(&path).unwrap();
        cersei_memory::graph_migrate::run_migrations(&db, 0, 2).unwrap();
        let s = db.session();
        let params = |id: &str, c: &str| {
            let mut p = HashMap::new();
            p.insert("id".to_string(), grafeo::Value::from(id));
            p.insert("c".to_string(), grafeo::Value::from(c));
            p
        };
        s.execute_with_params(
            "INSERT (:Memory {id: $id, content: $c, mem_type: 'Project', created_at: '2025-11-02T10:00:00Z'})",
            params("m1", "The staging server is called gandalf."),
        )
        .unwrap();
        db.close().unwrap();
    }
    let m = StructuredMemory::builder(embedder())
        .path(&path)
        .config(MemoryConfig::default().with_space(P))
        .open()
        .unwrap();
    assert_eq!(m.stats().episodes, 1);
    let r = m.process(&CancellationToken::new()).await.unwrap();
    assert_eq!(r.embedded, 1);
    let mut q = RecallQuery::new("staging server name");
    q.spaces = Some(vec!["space:legacy".into()]);
    let rec = m.recall(&q).await.unwrap();
    assert!(rec.rendered.contains("gandalf"), "{}", rec.rendered);
    assert!(
        rec.rendered.contains("2025-11-02"),
        "the legacy creation time is the source time"
    );
    m.close().unwrap();
    // The old API still reads its nodes.
    let g = cersei_memory::graph::GraphMemory::open(&path).unwrap();
    assert_eq!(g.stats().memory_count, 1);
}

#[tokio::test]
async fn recall_respects_its_token_and_result_budget() {
    let x = Scripted::new(vec![]);
    let m = memory(x, Arc::new(ManualClock::new(day("2026-04-01"))));
    let eps: Vec<EpisodeInput> = (0..30)
        .map(|i| {
            ep(
                P,
                "s1",
                "user",
                "2026-01-01",
                &format!(
                    "Note {i} about the Bricks build pipeline and its caching layer number {i}."
                ),
            )
        })
        .collect();
    m.ingest(eps, &CancellationToken::new()).await.unwrap();
    let mut q = RecallQuery::new("Bricks build pipeline caching");
    q.max_tokens = Some(150);
    let r = m.recall(&q).await.unwrap();
    assert!(r.tokens <= 150, "{} tokens", r.tokens);
    assert!(r.omitted > 0 && !r.items.is_empty());
    assert!(r.rendered.contains("left out by the memory budget"));
    assert!(r.timings.total_us > 0);
}
