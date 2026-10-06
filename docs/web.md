# Web search and reading

`WebSearch` and `WebFetch` run on `cersei-web`: search → bounded downloads →
Markdown extraction → structural passages → lexical ranking (BM25) →
citable passages. Everything is configured in `bricks.toml` `[web]`
(annotated in [bricks.example.toml](bricks.example.toml)).

What this is, and is not:

* **Selecting passages loses information.** It is not a reading of the
  page. The full extracted document is always kept and readable in order.
* **BM25 is lexical.** It rewards shared terms weighted by rarity; it does
  not understand paraphrases. A zero score means "no shared term", not "no
  information". Scores order passages within one search only: they are not
  probabilities and are not comparable across searches. They appear in the
  structured data, not in the text.
* **Splitting follows Markdown structure** (headings, paragraphs, code
  blocks, tables). It does not understand how concepts relate.
* **DuckDuckGo's HTML interface is not an API.** It has no service-level
  commitment, its markup can change, and it can refuse automated requests.
  Bricks reports a challenge page or an unknown layout as such. It never
  answers or works around a challenge.
* **No latency target is promised.** Timings are measured per stage (see
  *Measurements*).

## Search providers

| Provider | Key | Endpoint (overridable) | Errors mapped |
|---|---|---|---|
| `duckduckgo` (default) | none | `https://html.duckduckgo.com/html/` | challenge (even HTTP 200), unknown layout, 403/429 refusal, no results |
| `brave` | `X-Subscription-Token` | `https://api.search.brave.com/res/v1/web/search` | 401/403 key, 402 quota, 429 + `X-RateLimit-Reset` |
| `tavily` | `Authorization: Bearer` | `https://api.tavily.com/search` | 401 key, 432/433 plan or pay-as-you-go limit, 429 + `Retry-After` |
| `exa` | `x-api-key` | `https://api.exa.ai/search` | 401/403 key, 402 credits, 429, 503 |

* **Choosing a provider.** `provider` is tried first, then `fallback` in
  order, within `max_attempts` requests and `budget_ms`. A key is read only
  from the variable named by `api_key_env`. A provider is never chosen
  because some variable happens to be set.
* **Validation at load.** Endpoints must be HTTPS (plain HTTP only to a
  loopback address), carry no credentials, and timeouts must be in range.
  A variable name that looks like a key is refused. A configured provider
  whose variable is unset is reported at load, then reported as
  unavailable when it is tried.
* **When a fallback happens.** A timeout, a quota, a rate limit, an
  unavailable service, a refused key, a challenge or an unknown layout moves
  to the next provider. A rate limit is retried once when its stated wait
  fits within `max_retry_wait_ms`. A refused key is reported and never
  retried. An empty result list is an answer, not a failure.
* **Every attempt is shown.** Each attempt appears in the result: provider,
  outcome and duration. A fallback is named (`answered by duckduckgo
  (fallback; preferred: brave)`), and a refused key comes with a
  suggestion. A key never appears in results, logs, errors or `Debug`
  output.
* **Uniform results.** Every hit has a source id (`S1`…), title, direct URL,
  snippet, provider and the provider's own rank. DuckDuckGo's `uddg` link
  is decoded only on its `/l/` redirect path. Duplicate URLs are compared
  ignoring case, fragment and `utm_*` parameters; other parameters
  (identifiers, signatures) are never dropped.

## Downloads

| `[web.fetch]` | default |
|---|---|
| `max_pages` | 5 |
| `concurrency` / `per_host` | 5 / 2 |
| `page_timeout_ms` (connection, redirects and body) | 4000 |
| `max_page_bytes` (decoded, after gzip/brotli/deflate) | 1 500 000 |
| `max_total_bytes` (all pages of a call) | 8 000 000 |
| `max_redirects` | 5 |
| `budget_ms` (all downloads of a call) | 12 000 |

* **Client.** One shared `reqwest` client with a connection pool. HTTP/2 is
  used when the server negotiates it.
* **Bounded reading.** Bodies are read as a stream and cut at the decoded
  limit, whether `Content-Length` is absent, wrong or truthful. A cut page is
  marked *partial* everywhere: the stored raw file is the beginning of the
  page, never the page. A transfer that breaks off keeps what arrived,
  marked incomplete.
* **Ordering and cancellation.** Results come back in the order of the
  sources, whatever finished first, with one error per URL (timeout, HTTP
  status, redirect loop, policy refusal, budget). Dropping the call (a
  cancelled turn) aborts downloads still running and starts no new ones.
* **Network policy.** Only `http`/`https`. Private, loopback, link-local,
  carrier-grade NAT, multicast, documentation and reserved addresses are
  refused, IPv4-mapped and NAT64 forms included. The check runs on the URL
  of every hop, redirects included, and on every DNS answer, in the
  resolver the client connects with. A name that resolves to `127.0.0.1` is
  therefore refused at connection time.
* **Local exceptions.** `allow_private` names explicit exceptions (`host` or
  `host:port`) for local servers and tests. It is separate from the public
  default.
* **Search keys stay with the search client.** Pages are downloaded by
  another client that never carries them, and the search client follows no
  redirects.

## Extraction

* **Decoding.** HTML, Markdown, plain text and JSON are read. The charset
  comes from `Content-Type`, then a BOM, then `<meta charset>`, else UTF-8.
  Undeclared bytes that are not UTF-8 are decoded as windows-1252, and a
  note says so. An unknown charset label is reported as an unsupported
  encoding.
* **HTML, two ways.**
  * Readability (`dom_smoothie`) is good on articles.
  * A conservative pass over `main` / `[role=main]` / `article` / `#content`
    / `body` removes scripts, styles, navigation, footers, sidebars and
    forms, and is better on documentation, whose tables and code Readability
    may drop.
  * The conservative result is kept when Readability failed, is very short,
    has fewer code blocks or tables, or is less than half as long.
  * The strategy and both sizes are reported. An extraction under
    `min_reliable_chars` is flagged as possibly incomplete.
* **What is kept.** Both paths write Markdown with the same DOM library
  (`dom_query`): headings, tables, fenced code and links resolved against
  the final URL. Escapes that only hurt reading (`v2\.1`) are removed
  outside code; meaningful escapes stay.
* **Not shown.** PDF and binary content are reported with their type and the
  bytes received, or the announced length labelled as such; they are never
  displayed. A JavaScript shell (an empty mount point plus scripts) is
  reported as "needs JavaScript", not read as an empty page.
* **Bounded work.** Parsing runs on blocking threads, at most
  `[web.extract] concurrency` at once, on input already bounded by the
  download limit and an element cap (`max_elements`). That bound is what
  guarantees a parse ends: a blocking task that has started is not stopped
  by an async timeout or an abort.

## Passages and ranking

* **Splitting.** Blocks are packed into passages of 500–1 500 characters
  (`[web.passages]`). Each passage keeps its heading path, its exact range
  in the extracted document (characters and bytes) and its verbatim text.
* **Oversized blocks.** Prose is split at sentence ends, code and tables at
  line ends. Each piece says which part it is
  (`part 2/3 of a code block (document lines 40–62)`). A table piece
  carries the table's header.
* **Index.** One BM25 index per search, over the passages of its pages only
  (`bm25` crate, own tokenizer).
* **Tokenizer.** The same for French and English text and for queries:
  lowercase, accents folded, a short stop-word list, plural `s` removed on
  long words. Identifiers are kept whole and also split: `max_retries`,
  `HttpClient`, `rate-limit`. `v2.1` stays whole.
* **Selection.**
  * Passages are chosen within `budget_chars` and `max_passages`, at most
    `per_source` per page on a first pass, so several sources contribute.
  * The preceding passage is added as context when a chosen one starts
    mid-thought.
  * Near-duplicates are dropped, but never two statements that differ by a
    number, version, date or negation.
* **Weak or no match.** No shared term gives an explicit `None` state, with
  the results and pages still listed. Matches covering less than a third of
  the query give `Weak`. No answer is made up in either case.

## Tools

### `WebSearch`

Input: `query`, `num_results` (default 8, max 20), `read_pages` (default
`max_pages`; 0 = results only).

The result contains, in order:

1. The provider line, with every attempt.
2. The results `[S1] title — url` and their snippets.
3. Each page read: `[S1 · D1] final url — strategy, N characters
   (complete | partial…)`, or why it was not read.
4. The passages:

   ```text
   --- P1 [S1 · D1 · Retry policy › Configuration · characters 812–1630] https://… ---
   <verbatim text of the stored document>
   ```

5. What was omitted: other passages, the budget, duplicates.

The structured data carries attempts, results, per-page outcomes, scores,
coverage and timings. No secondary model call is made.

### `WebFetch`

Input: `url` or `doc` (`D1`…), `max_chars` (Unicode characters, default
20 000, max 40 000, separate from the byte limit of the download), `offset`
(characters), `question`, `format` (`markdown` | `raw` | `json_summary`),
`refresh`.

* **No `question`.** The extracted Markdown comes in document order, one
  window at a time, ending on a line break when one is near. The output
  states `Showing characters a–b of N`, the next offset, what came before,
  and how many characters remain. Nothing is reordered or dropped.
* **With `question`.** The passages most related to the question come with
  their ranges, plus how many of the document's passages were omitted
  (lossy, said so).
* **`raw`.** The bytes as received, paged the same way.
* **`json_summary`.** The JSON summary of the compression engine (Sprint 3).
* **Stored copies.** A page is downloaded once per session. Later windows,
  `doc`, and a restored session read the stored copy; `refresh: true`
  downloads it again.
* **Compression.** Output compression leaves the web tools' text unchanged.
  Over the hard cap it is cut once on a line end, never thinned out.

## Storage and provenance

* **Where documents live.** Each document (`D1`, `D2`, … in source order)
  is kept with the session's saved originals
  (`<raw store>/web/D1.raw`, `D1.md`, `index.json`). It survives a restore
  and is deleted with the session.
* **Quota.** When `[web.store] max_session_bytes` is exceeded, the oldest
  documents are removed.
* **Untrusted content.** Page text is external content. It is rendered as
  data: tool descriptions and result headings say so. Nothing in it changes
  permissions, configuration, tools or prompts.

## Measurements

`cargo run --release -p cersei-web --example measure_web` runs on the local
corpus ([tests/fixtures](../crates/cersei-web/tests/fixtures), synthetic,
provenance in its README).

* **Columns.** Sizes and estimated tokens (`cersei_types::tokens`, local
  heuristic) for HTML → Markdown → passages, the median time of
  extraction, splitting and ranking, the allocations of one extraction, and
  whether the reference passage of each query was kept.
* **Run on macOS arm64, release build:**

| page | query | HTML tok | Markdown tok | passages tok | kept vs HTML | extract ms | rank ms | reference kept |
|---|---|---|---|---|---|---|---|---|
| retry.html | max_retries default value | 906 | 376 | 374 | −58.7 % | 0.47 | 0.36 | yes |
| retry.html | POST /payments idempotent | 906 | 376 | 96 | −89.4 % | 0.45 | 0.22 | yes |
| timeouts_fr.html | délai total par défaut | 556 | 295 | 294 | −47.1 % | 0.25 | 0.27 | yes |
| nav_heavy.html | Retry-After 429 | 531 | 72 | 71 | −86.6 % | 0.32 | 0.06 | yes |
| retry.html ×10 | backoff_ms initial delay | 5 745 | 3 765 | 278 | −95.2 % | 4.07 | 4.54 | yes |
| retry.html ×50 | backoff_ms initial delay | 27 249 | 18 803 | 278 | −99.0 % | 18.28 | 24.22 | yes |

* **Reading the numbers.** Small pages fit in the budget, so their passages
  are the whole page. The enlarged pages repeat one section: their passages
  shrink mostly because repeats are removed as near-duplicates. A real long
  page would keep more. The "80–90 %" reduction is a hypothesis to measure
  on real corpora; it is not a guarantee.
* **Loopback download.** 5 pages × 50 ms server delay, 2 per host: 164 ms,
  2 in flight on the host. This shows overlap, not Internet latency.
* **One live run** (DuckDuckGo, query "tokio JoinSet documentation", not a
  benchmark): search 876 ms, download 486 ms, extraction 221 ms, ranking
  260 ms, total 1.84 s.

## Dependencies

| crate | version | why | licence |
|---|---|---|---|
| `dom_smoothie` | 0.18.2 | Readability extraction | MIT |
| `dom_query` | 0.28 | DOM, selectors, Markdown writer (no second converter needed) | MIT |
| `bm25` | 2.3.2, no default features | BM25 scoring (own tokenizer) | MIT |
| `encoding_rs` | 0.8.42 | charsets | Apache-2.0/MIT + BSD-3 |
| `hickory-resolver` | 0.25 | DNS with policy filtering (same resolver as reqwest here) | MIT/Apache-2.0 |

* **Evaluated, not added.** `htmd` (dom_query's writer was enough on the
  fixtures) and `text-splitter` (its Markdown feature brings a large
  Unicode segmentation stack and does not give heading paths or labelled
  pieces of oversized blocks; the splitter is about 300 lines here).
* **Minimum Rust version.** The repository declares no MSRV. The build used
  rustc 1.98.1. No `floor_char_boundary` is used: boundaries are found with
  `is_char_boundary`.
