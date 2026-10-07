# Code understanding: `bricks-semantic`

One engine per workspace (`SemanticEngine`), shared through an
`Arc<SemanticEngine>` by every client of the process: the agents'
`CodeScout` tool, their sub-agents, and frontends through the controller
(`search` command, `/search` in the terminal interface). It answers typed
queries with **sourced** results (path, exact range, content revision, how
the relation was established) and **bounded** ones (limits, context budget).
It does not claim to understand the whole program.

## Backends and routing

| backend | library | used for |
|---|---|---|
| lexical | ripgrep's `ignore` (walker) and `grep` (matcher, searcher); no `rg` binary | text and regex search, candidate files |
| syntax | Tree-sitter (Rust, TypeScript, TSX, JavaScript via the TSX grammar, Python, Go) | definitions by name, enclosing function/type/block, signatures |
| LSP | `cersei-lsp` (optional; servers started on demand, shared) | definition, references, hover, diagnostics |

Routing is deterministic (`planner.rs`) and reported in every response
(`plan.intent`, `plan.reason`, `plan.steps`, `plan.fallbacks`):

| intent | route |
|---|---|
| `text_search` | lexical; syntax context only if asked (`context = function/type/block`) |
| `find_symbol` | lexical (whole word, code files) → syntax definitions with that name, confirmed (or completed) by `workspace/symbol` when a server **already runs**; homonyms are all listed with `ambiguity` |
| `definition`, `references` with a target | LSP directly (started if allowed), then syntax context |
| `definition`, `references` with a name only | candidates first; one candidate → its position goes to the LSP; several → `ambiguous`, nothing chosen |
| `understand` | enclosing scope (syntax) + hover, definition and, from `normal`, a few references (LSP); on a whole file (`target = file`), its outline: document symbols (LSP), else the syntax tree's definitions, signatures only |
| `diagnostics` | LSP, with the freshness state |
| `auto` | a target → `understand`; an identifier → `find_symbol` (text search if no definition); anything else → `text_search` |

No LLM, embedding model or vector index is involved. A text search never
starts a language server.

Fallbacks keep what worked and say what is missing: without a server, a
definition becomes "definitions named `x` by syntax; which one the target
uses is not established" (`partial`, or `ambiguous` with several), and
references become **text mentions, not confirmed references** (`partial`).

## The contract

```rust
let engine = SemanticRegistry::global().engine_for(workspace, &config);
let response = engine.query(query, &requester).await;
```

`CodeQuery`: `text`, `mode` (`literal` | `regex`), `case_insensitive`,
`intent`, `target` (`position {path, line, column}` 1-based with the column
in characters, `offset {path, offset}`, `item {id}` from an earlier response,
`file {path}`), `scope` (`paths`, `extensions`, `exclude`), `limits`
(`max_files`, `max_matches`, `max_results`, `max_file_bytes`, `timeout_ms`,
`budget_tokens`), `context` (`none`, `lines {n}`, `block`, `function`,
`type`, `auto`), `detail` (`compact`, `normal`, `deep`).

`Requester`: scope (absolute folders inside the workspace), view (shared by
default), cancellation token, whether it may start a server, the active file
when it really is known. Limits are capped by the configuration: a query can
lower them, never lift them.

`CodeResponse`: `status` (`complete`, `partial`, `ambiguous`, `unavailable`,
`cancelled`, `error`), `plan`, `items`, `ambiguity`, `omissions`, `budget`,
`metrics`, `continuation`, `generation`, `diagnostics`.

Each item carries apart:

* **certainty**: `confirmed` (a language server, for that exact version),
  `syntactic` (the syntax tree shows a definition with that name), `textual`
  (the text matches);
* **freshness**: `current`, or `stale` with the reason (the file changed
  during the query, the server answered for another version);
* **score**: a ranking heuristic for the intent (table in `rank.rs`), not
  a probability.

Plus `id` (`path#start-end@hash`, reusable as a target; refused with "search
again" once the file changed), `relation` (`definition`, `reference`,
`text_mention`, `candidate`, `context`, `diagnostic`, …), `symbol`,
`signature`, `snippet`, `documentation` and `provenance` (backend + method,
several when results were merged).

### Positions

Inside the engine: byte offsets, half-open ranges, 0-based lines with byte
columns; lines end at `\n`, `\r\n` or a lone `\r` (the LSP rule).
`PositionMapper` converts, for one exact text, between offsets, line/column,
Tree-sitter points (rows at `\n` only) and LSP positions in the encoding the
server negotiated (UTF-16 unless it chose UTF-8 or UTF-32). Columns are never
terminal cells or graphemes. 1-based `line:column` in characters is the
display convention, applied only by `render` and the frontends.

### Documents and views

* The **shared view** is the disk. Approved edits are on disk, so the next
  query sees them.
* A **private view** (`engine.open_view()`) overlays buffers a client really
  gave the engine (`set_buffer`). In the CLI today, no editor sends buffers:
  the disk is the base.
* A **preview** (`view.preview(changes)`) overlays pending diffs, isolated:
  only its handle reads it.

A query reads each document at most once (`Snapshot`): every offset, tree and
excerpt refers to that text. Files that changed while the query ran are
flagged `stale` (their positions still refer to the version read). Language
servers only ever see the shared view: a query on a private or preview
document gets syntax and text results, with the reason.

### Context and budget

The budget counts everything an item costs once rendered (path, relation,
provenance, signature, snippet), estimated with the Context Manager's local
heuristic (`cersei_types::tokens`); the selection keeps the estimate's upper
bound under the budget: a margin, not the provider's exact count
(`budget.method` says so). Snippets are verbatim with their ranges; a scope
larger than the detail level allows (12 / 40 / 120 lines) becomes an explicit
excerpt (signature, a window around the match, the closing line, and the
number of lines left out). `detail` changes the amount of context only, not
permissions, time or memory ceilings. The same scope is never sent twice.

## Language servers

* One instance per (server, project root), started on first semantic use;
  concurrent first uses share one start. A monorepo gets one instance per
  project root (Rust: the outermost `Cargo.toml` with `[workspace]`; TS:
  `tsconfig.json`/`package.json`; Go: `go.work`/`go.mod`; Python:
  `pyproject.toml`…).
* Capabilities come from `initialize`: position encoding, synchronization
  kind, providers. An unsupported request is reported and falls back.
* Synchronization: `didOpen`, then versioned full-content `didChange`
  before each request, under a per-document lock held for the request (no
  lock across documents or servers); open documents are re-synchronized after
  a workspace change; at most `max_open_documents` stay open (LRU,
  `didClose`).
* URIs are percent-encoded and normalized (a folder with spaces works);
  `Location`, `Location[]`, `LocationLink[]` and `null` are all accepted.
* Features: document symbols, workspace symbols (only on a running
  server), definition, references, hover, diagnostics. Call hierarchy,
  implementations and inlay hints are not implemented.
* Requests time out (`request_timeout_ms`), are cancelled with the requester
  (`$/cancelRequest`), and fail at once if the server exits. A crashed server
  is restarted up to `max_restarts` times; idle servers stop after
  `idle_shutdown_secs`. Server requests (`workspace/configuration`, …) are
  answered, never mistaken for responses.
* **Diagnostics** never rely on a fixed delay. A report is `analyzed` only if
  it was published for the version sent (or, unversioned, received after it
  was sent) or pulled (`textDocument/diagnostic`); otherwise `outdated`,
  `pending` (no conclusion possible) or `unavailable`. After an edit, the
  report compares with the last analyzed version of the file
  (`introduced`, `preexisting`, `resolved`, matched on severity, code and
  message), so an existing error is not announced as new. No report is ever
  taken as proof that the code compiles.

**Installation is manual**: Bricks never downloads or installs a server.
Built-in configurations: rust-analyzer, typescript-language-server, pyright,
gopls and others (`cersei-lsp/src/config.rs`); `[[semantic.lsp.servers]]`
adds or overrides one. For Rust: `rustup component add rust-analyzer`. A
server that is missing or fails to start is reported with its reason (for the
rustup proxy without the component: "Unknown binary 'rust-analyzer'…"), and
the engine works in degraded mode (syntax and text).

## Caches and invalidation

| cache | key | invalidation |
|---|---|---|
| trees | path + content hash (one version per path, LRU `tree_cache_entries`) | none needed (content-addressed); an older version is reparsed incrementally |
| responses | query, scope, view and its generation, workspace generation, permission to start servers, active file | any change notification; restart of a server; `result_cache_ttl_ms` |
| identical queries in flight | same key | shared until done; a requester that cancels stops waiting, the work stops only when no one waits any more |

Only responses that used a language server are cached (lexical searches
re-read the disk each time). A semantic answer can depend on other files
than its own, so **any** change notification drops all cached responses.
Notifications come from the agent runner after every non-read-only tool
call (Write, Edit, Bash…) and from `engine.notify_changed`. Edits made
outside Bricks are not observed: they are caught by the TTL, by the content
hash of the documents a query reads, and by the server's own file watching.

Memory: the engine bounds its own caches (`tree_cache_entries`,
`result_cache_entries`, `max_open_documents`); it cannot bound a language
server's memory, which is reported separately (pid in `EngineStats`).

## Permissions and scope

Every query, cached or not, is limited to the requester's scope (results
outside are dropped and counted). A sub-folder shares its workspace's engine;
a worktree or another checkout (a `.git` boundary) has its own. Private
buffers and previews are capabilities: only their handle reads them.

## Configuration (`[semantic]` in `bricks.toml`)

See `docs/bricks.example.toml`. Every value is a ceiling.

## Limits

* Syntax resolution is by name: macros, `use` renames, dynamic dispatch,
  generated code and re-exports are invisible to it; that is what the
  `syntactic` certainty says.
* Without a server, references are text mentions.
* No cross-language relations (Rust ↔ TS bindings, FFI, SQL in strings).
* A server that indexes: the engine waits up to `ready_wait_ms` (and the
  query's limit), then reports `partial`; a server that never reports its
  activity is taken as ready after `startup_settle_ms`.
* JavaScript uses the TSX grammar (no separate JavaScript grammar).

## Benchmark

```sh
cargo run --release -p cersei-tools --example semantic_bench -- \
    --root . --runs 7 --out target/semantic-bench.json        # add --lsp to allow servers
```

Corpus: `bench/semantic/corpus.json`, 11 questions on this repository with
annotated answers (path + text of the line, robust to line shifts):
5 definitions (one qualified), 2 with homonyms, 2 text searches, 2
reference searches, plus a file edited between two queries.

Baseline ("existing path"): the real `Grep` tool on the name, then the real
`Read` tool on the first 1–3 matching files, as an agent does without
CodeScout. New path: one `CodeScout` query (the rendered response is what is
counted). Tokens: the Context Manager's estimate for both sides (`tokens_raw`
= everything the baseline tools returned). Precision / recall: returned
locations vs annotated ones. "Useful tokens" would need a reading protocol
and are not reported.

Measured on 2026-10-07, macOS (Apple silicon), release build, 7 runs.

**Without a language server** ("cold" = a fresh engine, "warm" = the
long-lived engine, its cached answers dropped before each query):

| category | calls (baseline → scout) | estimated tokens (baseline → scout) | precision (baseline → scout) | recall | p50 ms baseline / scout cold / scout warm |
|---|---|---|---|---|---|
| definition (5) | 4 → 1 | 12.7k–43.3k → 215–483 (−98 to −99 %) | 0.01–0.17 → 1.00 | 1.00 both | 14–17 / 18–61 / 13–30 |
| homonyms (2) | 4 → 1 | 24.8k–34.9k → 371–1 109 (−97 to −99 %) | 0.12–0.14 → 1.00 | 1.00 both | 15–17 / 23–29 / 16–19 |
| text (2) | 2 → 1 | ~1.96k → ~0.77k (−60 %) | 0.25 → 0.25 | 1.00 both | 13–14 / 16–21 / 15–16 |
| references, text fallback (2) | 4 → 1 | 12.7k–33.5k → 1.2k–2.3k (−82 to −96 %) | 0.25–0.40 → 0.20–0.33 | 1.00 both | 14–16 / 34–48 / 30–35 |

Total: 264 k → 8.2 k estimated tokens. Warm engine: tree cache hit rate
91 % (46 trees, 781 KiB of source held); bench process RSS 6.9 → 45 MB.
Edited file: the re-query found the new line (1 → 4) in 0.6 ms with one
incremental reparse.

**With rust-analyzer 1.98.1** (`--lsp`; started and indexed once, measured
apart; warm queries only):

| | |
|---|---|
| start + indexing of this repository | 14.6 s (first query, waiting for indexing within the 15 s limit); server RSS 1.58 GB while indexing, 343 MB after the runs |
| references (2) | precision 0.25–0.40 (Grep) → **1.00**, recall 1.00, 314–563 tokens; warm p50 16–21 ms, p95 up to 7 s (the first query after a change notification re-synchronizes the open documents and the server re-analyses) |
| definitions (5) | precision 1.00 except `parse_locations`: 0.50 — the server also lists the re-export `pub use client::parse_locations`, reported as a confirmed **reference** (re-export), not as a second definition |
| homonyms (2) | the syntactic candidates are confirmed by `workspace/symbol`; still listed as ambiguous |
| latency | every query was slower while the server ran (it uses CPU in the background): Grep p50 14–35 ms, CodeScout warm p50 16–78 ms |

Reading these numbers:

* The token reduction depends on the baseline protocol: reading 3 whole
  files (up to 2 000 lines each) dominates it. An agent that reads with
  offsets would spend less; the "60–70 %" and "7 calls → 1–2" hypotheses
  are neither confirmed nor refuted in general by this protocol. Text
  searches, where the baseline reads one file, gain 60 %.
* **Regression: latency.** CodeScout is slower than Grep when cold (up to
  ~4× on a common name such as `global`, which makes 20+ files to parse),
  and close to it when warm. A first version was 3–5× slower everywhere;
  the search was then parallelized and files are only decoded and hashed
  when they match.
* **References without a server are no better than Grep** (text mentions
  include definitions, comments and the corpus itself); they are labelled
  as such. With rust-analyzer they are exact. Text-search precision is 0.25
  for both because the benchmark's own files contain the searched strings.
* **A server that is indexing answers incompletely.** The first `--lsp` run
  exposed it: rust-analyzer answered "no reference" before indexing, and
  the engine reported `complete`. The engine now follows `$/progress` and
  `experimental/serverStatus`, waits (bounded by `ready_wait_ms` and the
  query's time limit), and reports `partial` — with syntax/text results when
  the server answered nothing — while it still indexes.

### Tested

| | |
|---|---|
| platform | macOS 27 (Apple silicon) |
| servers, real | rust-analyzer 1.98.1 (rustup component), negotiated UTF-8: `tests/rust_analyzer.rs` (homonyms resolved, references, edit then re-query; ignored by default, run with `--ignored`) and the `--lsp` benchmark on this repository. Other servers not tested |
| servers, scripted | `cersei_lsp::mock` (UTF-8 and UTF-16, versioned sync, all response shapes, pending/old diagnostics, timeout, cancellation, crash) |
