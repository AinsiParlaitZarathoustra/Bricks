# Web search and reading

`WebSearch` and `WebFetch` run on the `bricks-web` crate. The steps are:
search, download a few pages with strict limits, turn them into Markdown, cut
them into passages, rank the passages with BM25, and return passages you can
cite.

Everything is set in the `[web]` section of `bricks.toml`. Every key is
shown in [bricks.example.toml](bricks.example.toml).

What to know:

* **Choosing passages loses information.** The full extracted page is always
  kept and can be read in order with `WebFetch`.
* **BM25 matches words.** It rewards shared words that are rare. It does not
  understand paraphrases. A zero score means "no shared word", not "no
  information". Scores only order passages within one search.
* **Splitting follows Markdown structure** (headings, paragraphs, code blocks,
  tables).
* **DuckDuckGo's HTML page is not an API.** The markup can change and it can
  refuse automated requests. Bricks reports a challenge page or an unknown
  layout as an error. It never answers or works around a challenge.
* **No speed is promised.** Speed depends on the network and the provider.

## Search providers

| provider | key header | default endpoint |
|---|---|---|
| `duckduckgo` (default, no key) | none | `https://html.duckduckgo.com/html/` |
| `brave` | `X-Subscription-Token` | `https://api.search.brave.com/res/v1/web/search` |
| `tavily` | `Authorization: Bearer` | `https://api.tavily.com/search` |
| `exa` | `x-api-key` | `https://api.exa.ai/search` |

```toml
[web.search]
provider = "duckduckgo"   # duckduckgo | brave | tavily | exa
fallback = []             # for example ["tavily", "duckduckgo"], tried in order
max_attempts = 3          # provider requests per search, retries included
timeout_ms = 5000         # one provider request
budget_ms = 12000         # the whole search, fallbacks included
max_retry_wait_ms = 2000  # longest rate-limit wait honoured once

[web.search.brave]
api_key_env = "BRAVE_SEARCH_API_KEY"
```

* The key is read only from the environment variable named in `api_key_env`.
  A provider is never chosen just because a variable is set. A name that looks
  like a key itself is refused.
* `provider` is tried first, then each `fallback` in order, within
  `max_attempts` and `budget_ms`.
* Endpoints must be HTTPS (plain HTTP only to a loopback address) and must not
  contain credentials. Wrong values are reported when the file loads.
* A timeout, quota, rate limit, unavailable service, refused key, challenge
  or unknown layout moves on to the next provider. A rate limit is retried
  once if its wait fits in `max_retry_wait_ms`. A refused key is never
  retried. An empty result list is an answer, not a failure.
* Every attempt is shown in the result (provider, outcome, time). A fallback is
  named, for example `answered by duckduckgo (fallback; preferred: brave)`.
  A key never appears in results, logs or errors.
* Each hit has a source id (`S1`, `S2`...), title, URL, snippet, provider and
  rank. Duplicate URLs are merged, ignoring case, fragment and `utm_*`
  parameters.

## Downloads

```toml
[web.fetch]
max_pages = 5             # pages read after a search
concurrency = 5           # downloads at once
per_host = 2              # downloads at once to one host
page_timeout_ms = 4000    # connection, redirects and body of one page
max_page_bytes = 1500000  # decoded bytes kept per page
max_total_bytes = 8000000 # decoded bytes for all pages of one call
max_redirects = 5
budget_ms = 12000         # all downloads of one call
allow_private = []        # explicit exceptions such as ["127.0.0.1:8080"]
```

* Bodies are read as a stream and cut at the limit. A cut page is marked
  *partial* everywhere.
* Results come back in source order. Each URL has its own error if it
  failed. A cancelled turn stops running downloads.
* Only `http` and `https` are allowed. Private, loopback, link-local,
  multicast and reserved addresses are refused. This is checked on every
  redirect and on every DNS answer, so a name that resolves to `127.0.0.1` is
  refused at connection time.
* `allow_private` is the only way to allow local servers.
* Search keys stay with the search client. Pages are downloaded by another
  client that never carries them.

## Extraction

* HTML, Markdown, plain text and JSON are read. The charset comes from
  `Content-Type`, then a BOM, then `<meta charset>`, else UTF-8. Bytes that are
  not UTF-8 are decoded as windows-1252 and a note says so.
* HTML is read two ways: Readability (`dom_smoothie`, good on articles) and a
  careful pass over `main`, `article`, `#content` or `body` that removes
  scripts, navigation, footers and forms (better on documentation). The
  careful result is kept when Readability fails, is very short, or has fewer
  code blocks or tables.
* The output is Markdown with headings, tables, fenced code and links
  resolved against the final URL.
* PDF and binary content are reported with their type and size, never shown.
  A page that needs JavaScript is reported as such.
* An extraction shorter than `min_reliable_chars` is flagged as possibly
  incomplete. Pages above `max_elements` are not parsed.

```toml
[web.extract]
max_elements = 60000
concurrency = 2
min_reliable_chars = 400
```

## Passages and ranking

```toml
[web.passages]
min_chars = 500
max_chars = 1500
max_passages = 5
budget_chars = 6000   # characters of passages in one result
per_source = 2        # first pass: at most this many per page
```

* Each passage keeps its heading path, its exact range in the extracted page
  and its exact text.
* Large blocks are split at sentence ends (prose) or line ends (code and
  tables). Each piece says which part it is. A table piece carries the table
  header.
* One BM25 index is built per search, over the passages of its pages only.
* The tokenizer is the same for French and English: lowercase, accents
  removed, a short stop-word list. Identifiers are kept whole and also split
  (`max_retries`, `HttpClient`).
* Passages are chosen within `budget_chars` and `max_passages`, with at most
  `per_source` per page at first, so several sources contribute.
* Near-duplicates are dropped, but never two passages that differ by a number,
  version, date or negation.
* If no word matches, the state is `None`. If matches cover less than a third
  of the query, it is `Weak`. Nothing is made up in either case.

## Tools

### `WebSearch`

Input: `query`, `num_results` (default 8, max 20), `read_pages` (default is
`max_pages`; 0 means results only).

The result contains, in order:

1. The provider line, with every attempt.
2. The results (`[S1] title, url`) and their snippets.
3. Each page read (`[S1 · D1]`), or why it was not read.
4. The passages, each with its source, document, heading path and character
   range.
5. What was left out: other passages, the budget, duplicates.

No second model call is made.

### `WebFetch`

Input: `url` or `doc` (`D1`...), `max_chars` (characters, default 20 000, max
40 000), `offset` (characters), `question`, `format` (`markdown`, `raw` or
`json_summary`), `refresh`.

* **No `question`.** You get the Markdown in order, one window at a time. The
  output says which characters are shown, the next offset and how many remain.
* **With `question`.** You get the passages most related to the question, and
  how many were left out.
* **`raw`.** The text as served, paged the same way.
* **`json_summary`.** A summary of a JSON document.
* A page is downloaded once per session. Later windows, `doc`, and a restored
  session read the stored copy. `refresh: true` downloads it again.
* Output compression does not change the text of the web tools.

## Storage and safety

* Each document (`D1`, `D2`...) is kept with the session's saved originals
  under `web/` (`D1.raw`, `D1.md`, `index.json`). It survives a restore and is
  deleted with the session.
* When `[web.store] max_session_bytes` (default 64 MiB) is exceeded, the
  oldest documents are removed.
* Page text is untrusted content. It is shown as data. Nothing in it changes
  permissions, configuration, tools or prompts.

## Measuring

`cargo run --release -p bricks-web --example measure_web` runs on a local
synthetic corpus (`crates/bricks-web/tests/fixtures`). It prints sizes,
estimated tokens, and timings for extraction and ranking. Run it to get
current numbers.

## Dependencies

| crate | why |
|---|---|
| `dom_smoothie` 0.18.2 | Readability extraction |
| `dom_query` 0.28 | DOM, selectors, Markdown writer |
| `bm25` 2.3.2 | BM25 scoring (own tokenizer) |
| `encoding_rs` 0.8.42 | charsets |
| `hickory-resolver` 0.26.3 | DNS for every HTTP client, so the policy checks see the answers the client uses |
