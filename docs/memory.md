# Long-term memory

`cersei-memory` with the `structured` feature keeps a durable, sourced,
temporal memory and recalls from it with a hybrid of vector search, lexical
search and a bounded relational expansion. An agent uses it through the
`LongTermMemory` trait: what it recalls is added to the system prompt of
each run (within a token budget), and each finished exchange is recorded.

```rust
use cersei_memory::structured::{LlmExtractor, MemoryConfig, StructuredMemory};

let config = MemoryConfig::from_bricks_toml(&std::fs::read_to_string("bricks.toml")?)?;
let memory = Arc::new(
    StructuredMemory::builder(Arc::new(OpenAiEmbeddings::from_env()?))
        .path(".bricks/memory.grafeo")
        .config(config)
        .extractor(Arc::new(LlmExtractor::new(provider, "provider/model", Default::default())))
        .open()?,
);
let agent = Agent::builder()
    .provider(provider)
    .long_term_memory(memory.clone())
    .memory_recall_tokens(1200)
    .build()?;
```

What it is not:

* **Recall order is not a statement of truth.** The RRF score orders results
  for one query; it is not a probability that a fact is true.
* **Extraction is a model's reading.** Every extracted fact must quote its
  source verbatim, or it is dropped. The quote proves that the text says it,
  not that it is right.
* **Recency and frequency do not validate a fact.** Neither feeds into
  ranking. Reading a fact does not re-validate it; only a new statement adds
  evidence.
* **The relational expansion is not Personalized PageRank.** It is a weighted
  propagation over one or two hops, bounded and deterministic. PPR was not
  added, because no comparison has shown a benefit.

## Records (schema v3)

All records live in one Grafeo database (the authority, Grafeo 0.5.43 as
locked). Nothing written by a user or a model is ever spliced into a query.

* **Reads** are fixed GQL texts (`structured::store::q`) whose values —
  contents, names, quotes, ids, times — are typed parameters.
  * They start from a node found through a **property index**: `id`,
    `session_id`, `subject_id`, `alias_key`, `singleton`. The indexes are
    created when the store opens.
  * In 0.5.43 the planner serves `MATCH (n {id: $id})` from the index in
    constant time, but scans the label for `MATCH (n:Episode {id: $id})`. Ids
    are unique across labels (`ep_`, `fact_`, `ent_` prefixes), so the
    unlabelled form is used.
  * Fixed texts also let Grafeo reuse its cached plans. Whether a plan is
    actually reused is Grafeo's business; this is not claimed to avoid
    parsing.
* **Writes** are typed operations (`store::W`) applied with Grafeo's node and
  edge API inside one transaction (commit or rollback, WAL-logged). There is
  no query text at all.
  * Reason: measured on 0.5.43 (release, in memory), GQL `INSERT`, `SET` and
    `MATCH … INSERT` grow superlinearly with the store. An edge insert took
    about 2.5 ms at 4 000 nodes, and 4 000 indexed `SET`s took 1.3 s.
  * The typed API stayed at a few microseconds per operation up to 10 000
    nodes.
  * Text indexes are not updated by typed writes: they are rebuilt before the
    next lexical search after a write.
  * **Transaction cost.** In 0.5.43 one transaction, even an empty one, costs
    time proportional to the whole graph: `begin_transaction` counts every
    node and edge, and `commit` walks every version chain. Measured (release,
    in memory): about 30 µs at 500 nodes and 465 µs at 16 000. Bulk
    ingestion therefore records its episodes together
    (`record_all`, one transaction per 512 episodes), and applying an
    extraction stays one transaction per episode.
* **Neighbours** used by the relational expansion are read from Grafeo's
  adjacency lists (`Store::adjacent`: the node found by its `id` index, then
  its edges of the wanted types), not through a query: this costs time in
  the node's degree, not in the size of the graph.
  * Edge types and single properties are read from the store's columns. A
    session read builds the whole node, embedding included.
  * These reads do not go through a transaction snapshot. A recall running
    while an ingestion transaction is open may therefore see that
    transaction's links before its commit. A rollback removes them.

| record | key properties |
|---|---|
| `:Episode` | `id` (from space, session, role, time, source, content), `space`, `session_id`, `role` (`user` / `assistant` / `tool` / `summary` / `legacy`), `author`, `content` (immutable), `occurred_at` (source time, may be unknown), `recorded_at`, `source_ref`, `extraction_state`, `embedding` + `embed_model` |
| `:Entity` | `id` (from space + normalised name), `space`, `type`, `name`, `name_norm`, `aliases` |
| `:Alias` | `alias_key` (space ␟ normalised name or alias, indexed), `entity_id` — how names resolve to entities |
| `:Fact` | `id`, `space`, `subject_id`, `predicate`, `value`, `value_norm`, `object_id`, `negated`, `origin`, `status`, `confidence`, validity and knowledge times (below), `statement`, `embedding` + `embed_model` |
| `:MemoryMeta` | embedding model id and dimensions of the store |

Relations:

* `(:Fact)-[:SUPPORTED_BY {quote}]->(:Episode)` — the evidence.
* `(:Fact)-[:ABOUT]->(:Entity)` — the subject.
* `(:Fact)-[:MENTIONS]->(:Entity)` — the object, when it is an entity.
* `(:Episode)-[:MENTIONS]->(:Entity)`
* `(:Fact)-[:SUPERSEDES {at}]->(:Fact)` — **the new version points to the one
  it replaces**.
* `(:Fact)-[:CONTRADICTS]->(:Fact)` — unresolved conflict; both are kept.

**Spaces.** Identity is scoped by a space: `user:<id>`, `project:<name>` or
`space:<name>`. Entities are resolved by normalised name or alias *within the
space*. Two projects that both have a module called `auth` get two entities,
and recall searches only the configured spaces.

**Origins.**

* Facts stated by the user (`user_statement`) or observed in a tool output
  (`tool_output`) are `active`.
* Facts from the assistant (`assistant_proposal`) or from a summary are
  `proposed`. They are not current facts until the user states the same thing,
  which promotes them and adds the user's turn as evidence.
* A proposal never replaces or contests a statement.

**Current facts.** "Current facts" is a view (`current_facts`, `facts`) over
sourced facts: active and contested facts, valid now. It is not a third store
that could drift from the others.

## Time

Two axes:

* **Validity — when the fact is true.**
  * `valid_from` and `valid_until` are set only when stated; otherwise they
    are unknown, never filled in with the time Bricks recorded something.
  * `asserted_at` is the time of the source episode (else of the recording).
    It marks the moment the fact was stated: the fact is certainly valid from
    then on, and *uncertain* before when its start is unknown.
* **Knowledge — when the system knew it.**
  * `recorded_at` is when this version was recorded.
  * `superseded_at` is when it was replaced; `retracted_at` is when it was
    withdrawn.
  * When a fact is replaced, `superseded_until` takes the new fact's stated
    start (a known end). If the change has no date, `ended_by` takes the time
    the change was stated. That is an upper bound — it had changed by then —
    not the date of the change.

Queries:

* `facts(space, valid_at, known_at, include_proposed)` returns facts valid at
  `valid_at` as known at `known_at` (both default to now).
  * Both bounds are checked, so a fact that starts in the future is not
    valid now.
  * A replacement recorded after `known_at` was not known then and does not
    bound the fact: this reconstructs what Bricks knew at a given date.
* `Fact::validity_at(t, k)` returns `Valid`, `Uncertain` (an unknown bound
  matters) or `Invalid`.
* Recall applies the same filter, *during* the search. Uncertain facts are
  marked in the rendered context.

## Changes and contradictions

Predicates listed in `exclusive_predicates` (configurable: framework,
employer, city, deadline…) have one value per subject and space at a time.
When a new authoritative fact arrives on the same subject, predicate and
space, with an overlapping validity:

| situation | result |
|---|---|
| same value, same polarity | confirmation: evidence added (no new fact) |
| exclusive predicate, different value or polarity, **explicit change** stated ("passe d'Axum à Actix", "switched to") | the old fact is `superseded` (end = the change's stated date, else bounded by `ended_by`); `new SUPERSEDES old` |
| exclusive predicate, different value, no explicit change | both are `contested`, linked by `CONTRADICTS`; no side is chosen and the rendered context says `CONTESTED` |
| non-exclusive predicate, different value | both coexist ("likes tea", "likes coffee") |
| non-exclusive predicate, same value, opposite polarity | as for exclusive (explicit change replaces, otherwise contested) |
| different space, or disjoint validity intervals | no interaction |

The example "J'utilise Actix sur un autre projet" is in another space or about
another subject, so it replaces nothing in Bricks.

## Ingestion

1. **Record.** `record(EpisodeInput)` stores the episode durably. The same
   episode recorded twice is stored once.
2. **Extract.** `extract_pending` sends each pending episode to the
   `Extractor`, which goes through the provider abstraction (`LlmExtractor`;
   no provider or model is hard-coded).
   * The call has an input limit (`max_input_chars`; cut inputs are marked
     cut), output limits, a timeout and cancellation.
   * The output is validated strictly: JSON shape, verbatim quote, snake_case
     predicate, parseable dates (otherwise unknown), at most `max_facts`
     facts.
   * The validated output is cached on the episode (`extracted`) before
     anything else is written.
   * A malformed or failed extraction leaves the episode stored and
     searchable, marked `failed` with the error. It is retried up to
     `max_attempts` times.
3. **Apply.** `apply_extracted` writes the cached output: identity resolution,
   change rules and evidence, in **one Grafeo transaction** that also marks
   the episode `applied`.
   * After a restart, a cached extraction is applied without calling the
     model again.
   * Fact and entity ids come from content, and evidence links are checked
     before being added, so re-processing never duplicates facts.
4. **Embed.** `embed_pending` embeds every episode and fact without a vector
   of the current model.
   * The vector is stored on its node; the in-process index is a projection
     of those vectors.
   * A failed embedding leaves the record without a vector and the next pass
     completes it.

`process` runs the three stages; `ingest` records then processes. Without an
extractor, episodes are stored and embedded only (a vector-only memory).

**In the agent.** At the end of each run, the agent records the user's prompt
and its final answer (`LongTermMemory::record_turns`) and processes them
**before `run` returns**. Extraction is therefore one extra model call at the
end of each run, made through the configured extractor. A failure there is
reported as a status event. The episodes stay stored and pending, and the
next run retries them.

**Embedding model.** `:MemoryMeta` records the embedding model id
(`EmbeddingProvider::model_id`, e.g. `openai/text-embedding-3-small:1536`).
Opening a store with another model is refused (`EmbeddingMismatch`) unless
`rebuild_embeddings(true)` is given: all vectors are then marked stale and
re-embedded by the next `embed_pending`. Vectors of different models are never
mixed.

## Vector index

* **Engine.** The projection is a USearch HNSW index (cosine). USearch is a
  **C++ library** (built with `cxx`/`cc`, C++17): this crate needs a C++
  toolchain to build. It is not pure Rust. HNSW search is approximate.
* **Rebuilt, never saved.** The index is rebuilt from the vectors stored in
  Grafeo every time the memory opens, then updated as records are embedded or
  change status. It is never written to a file of its own. The claim that a
  Grafeo transaction covers the index would therefore be false, and is not
  needed: the index is derived data, and records missing from it are simply
  embedded again.
* **Built-in Grafeo index not used.** Grafeo 0.5.43's own vector index was
  evaluated and not used: in this version it is not updated by later inserts,
  and it is not restored when the database is reopened.
* **Text indexes.** Grafeo's BM25 text indexes (lexical lane) are likewise
  created on first use and rebuilt after a reopen.

## Recall

| stage | what it does | limits (`[memory.recall]`) |
|---|---|---|
| vector | nearest facts and passages to the query embedding, filtered by space, status and time **during** the HNSW search | `candidates` (30) |
| lexical | BM25 over fact statements and episode contents (exact identifiers, module names, rare words) | `candidates`, `weight_lexical` (0.8) |
| relational | from the `seeds` (10) best vector candidates: record → entities → facts, `hops` (2) hops, edge weights `ABOUT` 1.0 / `MENTIONS` 0.7, second hop × 0.5; admissibility checked at each visit | `max_neighbors` (8), `max_visited` (200), `expansion_budget_ms` (50); the stop reason is reported |
| fusion | weighted RRF: `score = 1.0/(60 + rank_vector) + 1.2/(60 + rank_relational) + 0.8/(60 + rank_lexical)`, ranks from 1, 0 for an absent list, ties by id | `rrf_k`, `weight_*` |
| dedup | same subject, predicate, value, polarity and validity → one fact; identical passages → one passage; statements differing by value, date, version or negation are separate facts and are never merged | |
| render | facts with their status, validity, replacement, origin and sources (role, date, session, quote); passages with role, date, session | `max_results` (8), `max_tokens` (1200), `passage_chars` (600) |

* **Final objects only.** Every list ranks the same final objects (facts and
  passages). Entities are intermediate: they become facts before fusion.
* **Timings.** The time of each stage is in `Recall::timings` (embedding,
  vector, lexical, expansion, fusion, render).
* **In the agent.** The block is capped by `memory_recall_tokens` *and* by a
  tenth of the model's prompt budget, and is counted by the Context Manager
  like the rest of the system prompt.

## Deleting a session

`forget_session(id)` deletes the session's episodes and their index entries.

* Facts keep the evidence they have from other sessions.
* A fact left with no evidence becomes `unsupported`: kept for history, never
  current, never recalled, and reported in `ForgetReport::facts_unsupported`.
* Deleting a conversation from the session store (`Memory::delete`) does not
  touch the long-term memory. Call `forget_session` for that.
* `retract(fact_id)` withdraws a fact explicitly; it stays in the history.

## Migration

Schema v3 follows v2 (`graph_migrate`):

* Each legacy `:Memory` node gets an `:Episode` (role `legacy`, space
  `space:legacy`, source time = its `created_at`, extraction skipped).
* It is embedded at the next `process` and searchable with
  `spaces = ["space:legacy"]`.
* The `:Memory` nodes are kept, and `GraphMemory` keeps reading them.
* `GraphMemory`'s queries now take parameters too. Before, content was
  escaped by hand and spliced into the query.
* Sessions, originals, compaction snapshots and web documents are separate
  stores and are not changed.

## Measurements

### Starting point: where the 6.6 % came from

`CHANGELOG.md` (2026-04-24) reports 6.6 % for the `graph` configuration on
`longmemeval_s` with `gemini-2.5-flash`. Its artefact, removed from the tree
later and recovered from commit `f5b577d`
(`bench/long-mem/results-gemini/c-graph-substring-*`), shows:

* `overall_accuracy` 0.066, a macro average over question types without
  abstention, 46/500 correct;
* abstention 30/30;
* answerer input of 477–533 tokens on **every** question (median 487);
* 347 answers saying the context does not contain the information.

The answerer therefore received an almost empty context for every question.
`GraphMemory::recall_top_k` pulled its candidates with
`m.content CONTAINS '<the whole question>'`: a turn had to contain the entire
question text to be found. The synthetic check below reproduces this
(`legacy`: 0 evidence found). It accounts for an empty retrieval. It does
not prove that nothing else contributed.

### Evidence recall — harness

```sh
./bench/long-mem/setup.sh oracle        # 15 MB; `s` is 265 MB
cargo run --release -p longmem-bench --bin longmem-recall -- \
    --dataset oracle --modes legacy,vector,vector-lexical --embeddings hashing --sample 60
```

* **Metric.** `recall_any@K` / `recall_all@K`: evidence sessions among the
  first K distinct sessions retrieved, per question type. Abstention
  questions are excluded because they have no evidence.
* **What is ingested.** Only roles, contents, session ids and session dates.
  Never `answer`, `answer_session_ids` or `has_answer`.
* **Modes.** `legacy` (the starting point), `vector`, `vector-lexical`, and
  `hybrid` (extraction by `--extractor-model`; paid).
* **`--sample N`.** A fixed validation sample: the first N questions of each
  type, by question id.

Answer quality uses the existing harness (`longmem-bench`) with the new
configurations `structured-vector` and `structured-hybrid`. It needs an
answerer, a judge and a budget.

### Results

See the Sprint 6 report: the LongMemEval dataset was not downloaded and no
paid model was called during the sprint, so no LongMemEval figure is
published here. The latencies below are from `measure_memory` (synthetic
corpus, local hashing embeddings, release build).

```text
cargo run --release -p cersei-memory --features structured --example measure_memory
```

* **Machine.** Apple A18 Pro, 8 GB, macOS (arm64).
* **Build.** The workspace `release` profile: `opt-level = "z"`, thin LTO.
* **Corpus.** Synthetic: one episode in four states where a person works, one
  in four where a company is based, the rest is filler.
  * The extractor is rule-based, so no model is called.
  * Embeddings come from `HashingEmbeddings` (384 dimensions), so no network
    is used.
  * The store is a Grafeo file in a temporary directory.
* **Queries.** 200 recalls with default `[memory.recall]` settings. Each one
  returned 8 items. The expansion stopped early (on `max_visited`) 0, 1 and
  3 times out of 200.

| episodes | facts | ingest (ms / episode) | reopen (ms) | stage | p50 (µs) | p95 (µs) |
|---|---|---|---|---|---|---|
| 500 | 250 | 1.14 | 126 | embed query | 1 | 2 |
| | | | | vector | 39 | 48 |
| | | | | lexical | 1 640 | 2 109 |
| | | | | expansion | 133 | 178 |
| | | | | fusion | 29 | 38 |
| | | | | render | 462 | 908 |
| | | | | **total** | **2 038** | **2 766** |
| 2 000 | 1 000 | 1.77 | 543 | embed query | 1 | 2 |
| | | | | vector | 54 | 79 |
| | | | | lexical | 2 515 | 3 143 |
| | | | | expansion | 348 | 439 |
| | | | | fusion | 51 | 69 |
| | | | | render | 671 | 990 |
| | | | | **total** | **3 527** | **4 377** |
| 10 000 | 5 000 | 5.20 | 3 156 | embed query | 1 | 2 |
| | | | | vector | 104 | 185 |
| | | | | lexical | 4 871 | 5 835 |
| | | | | expansion | 1 594 | 2 039 |
| | | | | fusion | 59 | 81 |
| | | | | render | 23 | 961 |
| | | | | **total** | **6 840** | **8 184** |

How to read the table:

* **What it excludes.** These figures are the memory's own work. With a
  hosted embedder, the query embedding (one network call) and the
  extraction (one model call per episode) dominate and are not included.
* **Ingestion grows with the store.** Applying an extraction is one Grafeo
  transaction per episode, and a Grafeo 0.5.43 transaction costs time in the
  size of the graph (see *Records*).
* **Reopen** rebuilds the vector projection from the stored vectors.
* **Lexical** is Grafeo's BM25 over two text indexes. It is now the largest
  recall stage.
* **Expansion** cost follows the degree of the entities it visits. In this
  corpus, cities and companies gain mentions as the corpus grows.
* **Earlier versions.** Before reads avoided `WHERE`, full-record loads and
  per-query neighbours, the same 10 000-episode run took 19.6 ms in
  expansion alone. Ingestion was about 16 ms per episode at 2 000 episodes.
* **Platforms.** Only macOS arm64 was measured.
