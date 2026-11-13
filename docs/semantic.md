# Code understanding: `bricks-semantic`

`bricks-semantic` helps Bricks find and understand code. There is one engine
per workspace. Everything in the process shares it: the `CodeScout` tool, the
sub-agents, and the frontends (the `search` command and `/search` in the
terminal interface).

An answer always says where it comes from: path, exact range, and how the
link was found. Answers are bounded by limits and by a token budget. The
engine does not claim to understand the whole program.

No language model, embedding or vector index is used here.

## Three backends

| backend | what it uses | used for |
|---|---|---|
| lexical | ripgrep's libraries (`ignore`, `grep`). No `rg` binary needed. | text and regex search |
| syntax | Tree-sitter (Rust, TypeScript, TSX, JavaScript through the TSX grammar, Python, Go) | definitions by name, the enclosing function or type, signatures |
| LSP | `bricks-lsp`. Optional. Servers start on demand and are shared. | definition, references, hover, diagnostics |

The engine picks the route by fixed rules (`planner.rs`). Every response
reports its plan (`plan.intent`, `plan.reason`, `plan.steps`,
`plan.fallbacks`).

| intent | route |
|---|---|
| `text_search` | lexical. Syntax context only if asked. |
| `find_symbol` | lexical, then syntax definitions with that name. A running language server confirms them. Same names are all listed as `ambiguity`. |
| `definition`, `references` with a position | language server first, then syntax context |
| `definition`, `references` with a name only | find candidates first. One candidate goes to the server. Several candidates give `ambiguous` and nothing is chosen. |
| `understand` | enclosing scope (syntax), plus hover, definition and a few references (LSP). For a whole file, its outline. |
| `diagnostics` | language server, with a freshness state |
| `auto` | a target means `understand`. An identifier means `find_symbol`. Anything else means `text_search`. |

A text search never starts a language server.

When a server is missing, the engine keeps what it can and says what is
missing. A definition becomes "definitions named `x` by syntax". References
become text mentions, marked `partial` and not confirmed.

## Using it

```rust
use bricks_semantic::SemanticRegistry;
let engine = SemanticRegistry::global().engine_for(workspace, &config);
let response = engine.query(query, &requester).await;
```

A query has: `text`, `mode` (`literal` or `regex`), `case_insensitive`,
`intent`, `target` (a position, an offset, an `item` id from an earlier
answer, or a file), `scope` (`paths`, `extensions`, `exclude`), `limits`,
`context` and `detail` (`compact`, `normal`, `deep`). Limits are capped by the
configuration. A query can lower them, never raise them.

A response has a `status`: `complete`, `partial`, `ambiguous`,
`unavailable`, `cancelled` or `error`.

Each result item carries:

* **certainty**: `confirmed` (a language server, for that exact file version),
  `syntactic` (the syntax tree shows a definition with that name), or
  `textual` (the text matches).
* **freshness**: `current`, or `stale` with the reason (the file changed during
  the query, or the server answered for another version).
* **score**: a ranking hint. It is not a probability.
* an `id` (`path#start-end@hash`) that can be reused as a target. It is
  refused once the file changes.
* `relation` (`definition`, `reference`, `text_mention`, `candidate`,
  `context`, `diagnostic`), `symbol`, `signature`, `snippet`, and `provenance`.

Lines and columns shown to users are 1-based, with columns counted in
characters.

### Views

* The **shared view** is the disk. Approved edits are on disk, so the next
  query sees them.
* A **private view** (`engine.open_view()`) adds buffers that a client gave
  the engine. The CLI does not send buffers today.
* A **preview** shows pending diffs. Only its own handle can read it.

A query reads each file at most once, so all offsets refer to one text. A file
that changes mid-query is marked `stale`. Language servers only see the shared
view.

### Budget

The budget counts what each item costs once rendered. It uses the Context
Manager's local estimate, kept under the budget with a margin. Snippets are
exact text with their ranges. A scope that is too large for the detail level
(12, 40 or 120 lines) becomes an excerpt: signature, a window around the
match, the closing line, and how many lines were left out.

## Language servers

* One server instance per (server, project root), started on first use. A
  monorepo gets one per project root.
* Bricks never downloads or installs a server. Install it yourself. For
  Rust: `rustup component add rust-analyzer`.
* Built-in servers include rust-analyzer, typescript-language-server,
  pyright, gopls, clangd, and a few more (`crates/bricks-lsp/src/config.rs`).
* If a server is missing or fails to start, the reason is reported and the
  engine keeps working with syntax and text only.
* Used features: document symbols, workspace symbols (only on a running
  server), definition, references, hover, diagnostics. Call hierarchy,
  implementations and inlay hints are not implemented.
* Requests time out, are cancelled with the requester, and fail at once if
  the server exits. A crashed server restarts up to `max_restarts` times.
  Idle servers stop after `idle_shutdown_secs`.
* Diagnostics never depend on a fixed delay. A report is `analyzed` only if it
  matches the file version sent. Otherwise it is `outdated`, `pending` or
  `unavailable`. After an edit, results are compared with the last analyzed
  version (`introduced`, `preexisting`, `resolved`). A report is never proof
  that the code compiles.

To add or override a server:

```toml
[[semantic.lsp.servers]]
name = "rust-analyzer"
command = "rust-analyzer"
file_patterns = ["*.rs"]
extension_to_language = { ".rs" = "rust" }
```

A server entry can also have `args`, `env` and `initialization_options`.

## Caches

| cache | invalidation |
|---|---|
| syntax trees (up to `tree_cache_entries`) | keyed by file content, so no invalidation needed |
| responses (up to `result_cache_entries`) | any change notification, a server restart, or `result_cache_ttl_ms` (30 000 ms) |

Only responses that used a language server are cached. Any change drops all
cached responses, because an answer can depend on other files. The agent
runner sends the change notification after every tool call that is not
read-only (Write, Edit, Bash and so on). Edits made outside Bricks are not
seen, so the TTL is the only limit for those.

The engine bounds its own caches. It cannot bound a language server's memory.

## Scope

Every query is limited to the requester's scope. Results outside it are
dropped and counted. A sub-folder shares its workspace's engine. A worktree or
another checkout (a `.git` boundary) has its own.

## Settings

Set them in `[semantic]` and `[semantic.lsp]` of `bricks.toml`. See
[bricks.example.toml](bricks.example.toml) for every key and default. Every
value is a ceiling.

```toml
[semantic]
max_results = 50
timeout_ms = 15000
default_budget_tokens = 4000
max_budget_tokens = 16000

[semantic.lsp]
enabled = true
request_timeout_ms = 10000
ready_wait_ms = 10000
```

## Limits

* Syntax resolution works by name. Macros, `use` renames, dynamic dispatch,
  generated code and re-exports are invisible to it.
* Without a server, references are text mentions.
* No links across languages (Rust and TypeScript bindings, FFI, SQL in
  strings).
* A server that is still indexing may answer too little. The engine waits up
  to `ready_wait_ms`, then reports `partial`. A server that never reports its
  activity is taken as ready after `startup_settle_ms`.
* JavaScript uses the TSX grammar.

## Benchmark

```sh
cargo run --release -p bricks-tools --example semantic_bench -- \
    --root . --runs 7 --out target/semantic-bench.json
```

Add `--lsp` to allow language servers. The corpus is
`bench/semantic/corpus.json`: 11 questions on this repository with known
answers. The baseline is the `Grep` tool followed by `Read` on the first
matching files. The other side is one `CodeScout` query.

Results on macOS (Apple silicon), release build, without a language server:

* Estimated tokens fell from about 264k to 8.2k over the 11 questions.
* Definitions and homonyms: precision went from 0.01 to 0.17 up to 1.00.
* Text search: the same precision as `Grep` (0.25).
* References without a server are no better than `Grep`. With rust-analyzer
  they were exact.
* `CodeScout` is slower than `Grep` on a cold start and close to it when warm.
  With a language server, indexing the repository took about 15 s.

The token gain depends on the baseline. Reading three whole files dominates
it. An agent that reads with offsets would spend less.
