# Long-term memory

Bricks can remember things between sessions. The memory is off by default.
When it is on, Bricks adds the most relevant memories to the system prompt of
each run and records what was said at the end of the run.

The code lives in `crates/bricks-memory/src/structured/` (feature
`structured`). This is the only long-term memory in Bricks. Session history
(the saved conversation) is separate and is kept as JSONL files.

## How it works

1. **Record.** Each user prompt and final answer is stored as an *episode*.
   The same episode stored twice is kept once.
2. **Extract.** If an extractor model is set, it reads each episode and returns
   short *facts* (subject, predicate, value). Every fact must quote the
   source text word for word, or it is dropped.
3. **Apply.** Facts are linked to their entities and to the evidence. A new
   fact can replace an old one or conflict with it (see below).
4. **Embed.** Episodes and facts get a vector so they can be searched by
   meaning.

Without an extractor model, episodes are only stored and embedded. Recall
still works, but there are no facts.

Extraction is one extra model call at the end of each run. If it fails, the
episodes stay stored and the next run tries again. A fact is tried at most
`max_attempts` times (default 3).

The `bricks run` flag `--no-wait-memory` skips waiting for this work after the
answer. The work stays pending and resumes next time.

## Notes: `memory_search` and `memory_write`

When memory is on, the agent gets two tools:

* `memory_search` looks things up.
* `memory_write` saves one short note. A note is at most 280 characters
  (`NOTE_MAX_CHARS`). Read-only agents get only `memory_search`.

Both tools take a scope: `project` or `global`. Notes can also be deleted.

## Scopes

Bricks Desktop keeps two kinds of store under the Bricks home folder:

| scope | file |
|---|---|
| global (shared by every project) | `memory/global.grafeo` |
| project | `memory/projects/<project_id>.grafeo` |

* New memories from a run go to the project store only, never to the global
  one. A run with no project records nothing.
* Recall asks both stores, merges equal results, and returns at most 5
  entries within the token budget. Each entry says its scope and source.
* If one store fails, the other still answers.

The Desktop memory view lists the entries of each scope and lets you delete
them.

## Settings

Memory is set in the `[memory]` section of `bricks.toml`. Defaults are shown.

```toml
[memory]
enabled = false
# path = "~/.bricks/memory/memory.grafeo"   # single store used outside Desktop
embeddings = "hashing"                      # hashing[:dims], openai[:model], gemini[:model]
# extractor_model = "provider_id/model_id"  # no model means no facts
space = "space:default"                     # where new records go
recall_spaces = []                          # extra spaces to search
exclusive_predicates = ["uses_framework", "lives_in", "employer"]  # example subset

[memory.recall]
seeds = 10
candidates = 30
hops = 2
max_neighbors = 8
max_visited = 200
expansion_budget_ms = 50
rrf_k = 60
weight_vector = 1.0
weight_relational = 1.2
weight_lexical = 0.8
max_results = 8
max_tokens = 1200
passage_chars = 600

[memory.extraction]
max_input_chars = 12000
max_facts = 40
max_attempts = 3
```

* `embeddings = "hashing"` is local and free but matches words, not meaning.
  `openai` and `gemini` read `OPENAI_API_KEY` and `GEMINI_API_KEY`.
* A space is `user:<id>`, `project:<name>` or `space:<name>`. Other values are
  rejected.
* `exclusive_predicates` must be snake_case. The default list is longer than
  the example above (it also has `framework`, `works_at`, `job_title`,
  `language`, `database`, `deadline`, `status`, `owner` and `version`).
* Recall also has `scoped_max_results`, `project_weight` (1.5) and
  `global_weight` (1.0) for merging the two scopes.
* Values outside their allowed range make `bricks.toml` fail to load with an
  error that names the key.

If you change the embedding model on an existing store, Bricks refuses to open
it unless you ask for a rebuild. All vectors are then re-embedded. Vectors of
different models are never mixed.

## Facts, time and changes

**Origin.** A fact the user states, or one seen in a tool output, is `active`.
A fact from the assistant or from a summary is `proposed`. It does not count
until the user states the same thing.

**Time.** A fact has two kinds of time:

* *Validity*: when it is true. `valid_from` and `valid_until` are set only
  when the text says so. Otherwise they are unknown.
* *Knowledge*: when Bricks learned it (`recorded_at`) and when it was replaced
  (`superseded_at`) or withdrawn (`retracted_at`).

Recall only returns facts that are valid now. A fact with an unknown start
is marked as uncertain.

**Changes.** Some predicates hold one value at a time (`exclusive_predicates`).
When a new fact arrives for the same subject, predicate and space:

| situation | result |
|---|---|
| same value | evidence is added, no new fact |
| exclusive predicate, different value, change stated ("switched to") | old fact is `superseded`, new one replaces it |
| exclusive predicate, different value, no change stated | both are `contested`, and the recall text says `CONTESTED` |
| other predicate, different value | both are kept ("likes tea", "likes coffee") |
| different space | no interaction |

Recall order is not a measure of truth. Reading a fact does not make it more
valid. Only a new statement adds evidence.

## Recall

Recall runs these steps, then merges the lists with a weighted reciprocal rank
fusion (`weight / (rrf_k + rank)`):

1. **Vector search** for the closest facts and passages.
2. **Lexical search** (BM25) for exact names and rare words.
3. **Relational expansion** from the best vector results, through entities
   and facts, up to `hops` steps. It stops at `max_neighbors`, `max_visited`
   or `expansion_budget_ms`.
4. **Dedup and render.** Same facts are merged. Facts with different values,
   dates or negation are never merged. The text shows status, dates, sources
   and quotes.

In the agent, the block is capped by `memory_recall_tokens` and by a tenth of
the model's prompt budget. It counts as part of the system prompt.

## Storage

* Records live in one Grafeo graph database per store (`.grafeo` file with a
  write-ahead log). Writes happen in transactions. Values are always passed as
  parameters, never put into query text.
* The vector index is USearch (HNSW, cosine). USearch is a C++ library, so
  building this crate needs a C++ toolchain. The index is rebuilt from the
  stored vectors each time a store opens. It is never saved on its own.
* Records are: `:Episode`, `:Entity`, `:Alias`, `:Fact` and `:MemoryMeta`.
  Facts link to their evidence (`SUPPORTED_BY`), subject (`ABOUT`), objects
  (`MENTIONS`), and to older versions (`SUPERSEDES`) or conflicts
  (`CONTRADICTS`).
* Grafeo needs Rust 1.99 or newer. The repository toolchain is 1.99.1.
* Opening an older database for writing migrates its file format. Stop other
  Bricks processes, copy the database and its WAL first, and try on the copy.
  Older Grafeo versions cannot open the migrated file.

## Deleting

* Deleting an entry in Desktop (`delete_entry(kind, id)` in the library)
  removes a single note, exchange or fact.
* `forget_session(id)` removes all episodes of a session.
* A fact that loses all its evidence becomes `unsupported`. It is kept for
  history and is never recalled.
* `retract(fact_id)` withdraws a fact but keeps it in the history.
* Deleting a conversation from the session store does not touch long-term
  memory.

## Library use

```rust
use bricks_embeddings::OpenAiEmbeddings;
use bricks_memory::structured::{LlmExtractor, MemoryConfig, StructuredMemory};

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
