# Context: counters, budget and compaction

How Bricks knows how full the context is, decides whether a request can be
sent, and compacts the history without losing it. Settings live in the
`[context]` table of `bricks.toml` (see `docs/bricks.example.toml`) or are
passed with `AgentBuilder::context_policy`.

## 1. Three counters

| Value | Rust | Meaning |
| --- | --- | --- |
| `contextUsed` | `ContextStatus::context_used` (`ContextUsed`) | Occupation of the active context — what the next request carries — with its **provenance**. |
| `contextWindow` | `ContextStatus::context_window` (`ContextWindow`) | Limits of the selected model, from its configuration: `max_input_tokens`, `max_output_tokens`, and `total` (`context_window_tokens`) only when configured. |
| `totalTokens` | `ContextStatus::total_tokens` / `totals` (`SessionTotals`) | Everything the session consumed. Never decreases; compaction calls are included, once. |

`Agent::context_status()` returns all three; `AgentEvent::ContextUpdate` is
emitted after each response, and `ModelRequestStart.token_estimate` carries the
occupation of the request being sent.

### Provenance of `contextUsed`

| Provenance | Source |
| --- | --- |
| `measured` | The `usage` the server reported for the request that carried exactly this context. |
| `counted` | A configured counting endpoint, for exactly this context (see §3). |
| `mixed` | A measured or counted base, plus a local estimate of what was added since. `measured_tokens` and `estimated_tokens` give the two parts. |
| `estimated` | Local estimate only (no usage yet, or the measurement was invalidated). Never shown as a measurement, never zero for a non-empty context. |

The local estimate (`cersei_types::tokens`, "local character-class heuristic
(v1)") weighs ASCII letters, punctuation, whitespace and non-ASCII characters
separately and carries an upper bound (+25 %). After a measurement it is
calibrated per model on the request just measured (ratio bounded to 0.5–2).
There is no universal tokenizer: the estimate is a fallback, not a count.

### What a measurement covers

A `usage` measures the request that was **executed**, recorded with the model
identity (`provider_id/model_id`), the version of the context, and the identity
of the instructions and tool definitions. The next occupation is that
measurement plus:

* the response kept in the history — from its own `output_tokens`, minus the
  reasoning the protocol does **not** send back (`chat_completions` without
  `compat.reasoning_field`). Reasoning that is re-sent (`anthropic_messages`
  thinking blocks, `responses` reasoning items, chat with `reasoning_field`)
  stays counted;
* every message appended since (tool results, user messages), estimated.

Prompt occupation is `input + cache_read + cache_write` on every protocol
(adapters normalise `input` to the uncached part). The cache lowers the price,
not the size: it never enlarges the window. `reasoning_tokens` is a part of
`output_tokens`, never added to it.

The measurement is dropped (and the context re-estimated) when the history is
rewritten — compaction, removal of old tool results, session restore — when the
instructions or tool definitions change, and when the model changes.

### Session totals

`SessionTotals` keeps `input_tokens` (uncached), `cache_read_tokens`,
`cache_write_tokens`, `output_tokens`, `reasoning_tokens` (included in output),
`requests`, `requests_without_usage` (not counted, reported apart) and
`compaction_requests`. `total_tokens() = input + cache_read + cache_write +
output`.

## 2. Budget

The prompt budget of a request is `max_input_tokens`, or, with a configured
total window, `min(max_input_tokens, context_window_tokens − reserved output)`.
The reserved output is the request's `max_tokens` bounded by the model's
`max_output_tokens`; it includes reasoning on all three protocols. A margin is
kept free: `max(safety_margin_tokens, safety_margin_ratio × budget)`.

Before every request:

```text
light check (measured base + estimated additions, or full estimate)
  → fits                                 → send
  → close, large injection, rewritten    → pre-flight (§3) → send if it fits
  → manifestly over (central estimate)   → compaction, check again
                                           → still over: not sent,
                                             CerseiError::ContextOverflow + Status
```

A pre-flight is triggered when the upper bound exceeds the budget, when more
than `large_injection_tokens` were added since the last measurement, when the
upper bound reaches `preflight_ratio` of the budget, or after a rewrite once the
context is past half of the budget.

When a model has no `context_window_tokens`, the input and output limits are
applied separately and the status says so; no total is invented. A provider
that is not built from configuration (tests, custom implementations) reports
only its prompt budget, which the status also notes.

If the server still refuses a request as too long (HTTP 400/413/422 or a stream
error with a documented overflow code or message), the history is compacted and
the request resent, at most `max_overflow_recoveries` times per turn. Nothing
ran for that turn yet, so no tool effect repeats.

## 3. Pre-flight counting

A server counting endpoint is used **only** when the model configures it:

```toml
[providers.models.token_counting]   # opt-in
# path = "messages/count_tokens"    # default: the protocol's documented route
```

Accepted for `anthropic_messages` (`messages/count_tokens`) and `responses`
(`responses/input_tokens`), which document a counting request; refused at load
time for `chat_completions`, which has none — no route is guessed for a custom
server. The request sent is the real one (same messages, system, tools,
reasoning parameters) restricted to the fields the route accepts. If counting
fails, the local estimate is used and the event says so. A count is valid for
the exact request counted; it is not an upper bound on future content.

## 4. Compaction

Compaction (summarising old history) is distinct from the compression of tool
outputs (`docs/compression.md`).

**When:** after a turn once the occupation reaches `compact_threshold` of the
budget; before a request that manifestly does not fit; after an overflow
refusal; or on demand (`Agent::compact()`).

**How:**

1. The tail is kept verbatim: at least `keep_recent_messages`, never splitting a
   tool call from its result, shortened down to the last message group if it
   exceeds `max_recent_ratio` of the budget. Kept messages are copied as they
   are — reasoning blocks, signatures and opaque protocol items included.
2. The older part is rendered as a transcript sized for the summary call itself
   (budget minus `summary_max_tokens`, its instructions and a margin): tool
   results and long assistant text are shortened first, user words last; if it
   still does not fit, the beginning and the most recent part are kept and the
   omission is stated. The call can therefore run before saturation.
3. The model summarises under fixed headings: objective, active instructions and
   constraints, decisions, files, actions done, open problems, key facts.
4. The user's own messages are added **verbatim** (long ones shortened, at most a
   quarter of what is summarised), so constraints survive even if the summary
   misses them.
5. The new history is checked: unless it is at least `min_compaction_gain`
   smaller, nothing is replaced (`InsufficientGain`).

**Outcomes** (`CompactionOutcome`, also on `AgentEvent::CompactionResult`):
`Compacted`, `InsufficientGain`, `Failed` (call error, empty or truncated
summary), `Skipped` (nothing old enough, no budget for the call, automatic
compaction disabled, or no change since the last unsuccessful attempt). Only
`Compacted` changes the history. After `max_compaction_failures` unsuccessful
attempts in a row, automatic compaction stops for the session (manual
compaction still runs). There is no silent fallback that drops messages.

**Nothing is lost:**

* `Agent::raw_history()` — every message as it happened, tool results
  unreduced; compaction never shortens it.
* `Agent::compaction_snapshots()` — the active history just before each
  compaction.
* With a session memory (`AgentBuilder::memory` + `session_id`), both are stored
  with the session's own backend: the raw history under `<session>.raw`, each
  snapshot under `<session>.compaction-<n>` (with `JsonlMemory`: files
  `<session>.raw.jsonl`, `<session>.compaction-1.jsonl`, …), written at each
  compaction and at the end of each run. The saved originals of reduced tool
  outputs go to the session's files directory (`JsonlMemory`:
  `<session>.files/`). Restoring a session restores its raw history and its
  snapshots (later compactions continue the numbering), so every reference in
  the history still resolves. `sessions()` does not list these internal files,
  and `delete()` removes them with the session.

## 5. Settings and starting values

| `[context]` key | Default | Role |
| --- | --- | --- |
| `safety_margin_tokens` | 1024 | Minimum margin below the budget. |
| `safety_margin_ratio` | 0.02 | Margin as a fraction of the budget (larger wins). |
| `large_injection_tokens` | 8000 | Additions since the last measurement that trigger a pre-flight. |
| `preflight_ratio` | 0.80 | Occupation (upper bound) that triggers a pre-flight. |
| `compact_threshold` | 0.85 | Occupation that triggers compaction after a turn. |
| `keep_recent_messages` | 10 | Messages kept verbatim (floor). |
| `max_recent_ratio` | 0.30 | Largest share of the budget the kept tail may use. |
| `summary_max_tokens` | 4096 | Output reserved for the summary. |
| `min_compaction_gain` | 0.20 | Required relative gain. |
| `max_compaction_failures` | 3 | Unsuccessful attempts before automatic compaction stops. |
| `max_overflow_recoveries` | 1 | Compactions per turn after an overflow refusal. |
| `image_tokens` | 1600 | Estimate per image (the real cost depends on its size). |
| `document_tokens` | 3000 | Estimate per document/audio/video item. |

These are starting points for models with 100k–200k-token windows and typical
coding sessions, not universal values: small local models want a lower
`compact_threshold` and `keep_recent_messages`; very large windows can afford a
higher threshold. `AgentBuilder::compact_threshold` still overrides the policy's
value.
