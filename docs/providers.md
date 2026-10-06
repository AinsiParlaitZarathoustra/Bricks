# Providers: configuration, protocols, capabilities, migration

Which providers and models Bricks can use is decided by **one file**. Adding a
provider, or a model compatible with one of the three supported wire protocols,
is a configuration change — no code, no recompilation. Environment variables are
used for one thing only: resolving the secrets the file explicitly references.

An annotated, secret-free example lives in
[`providers.example.toml`](providers.example.toml); it is loaded by a unit test,
so it cannot drift from the schema.

```rust
use cersei::provider_from_config;

let provider = provider_from_config(None, "custom/flash")?;                 // ~/.bricks/providers.toml
let provider = provider_from_config(Some("./p.json".as_ref()), "custom/flash")?; // explicit path

let agent = Agent::builder()
    .provider(provider)
    .reasoning_profile("approfondi")   // optional: a profile *you* defined for that model
    .build()?;
```

---

## 1. The file

* **Location.** `~/.bricks/providers.toml`, or any explicit path. An explicit
  path **replaces** the default; the two are never merged. A missing file is an
  error — there is no fallback catalogue.
* **Format.** `.toml` or `.json` (same Serde structures, same defaults, same
  validation — one code path). Any other extension is rejected.
* **`schema_version = 1`** is required.
* **No cap.** Any number of providers, and of models per provider. Quotas, access
  rights and which models exist remain the server's business.
* **Selection is explicit:** `provider_id/model_id`. The string is split at the
  first `/` (model ids may contain `/`). The provider is **never** inferred from a
  model name. Provider ids are unique; model ids are unique within a provider.
* **Errors are located and secret-free:**
  `providers.toml: provider "custom", model "flash", field `limits.max_input_tokens`: must be greater than 0`.
  Type errors never echo the offending value; TOML syntax errors report line and
  column but never quote the line (it could hold a key).

## 2. Provider

| Field | Req. | Meaning |
|---|---|---|
| `id` | yes | Stable identifier. Non-empty, no whitespace, no `/`. |
| `name` | yes | Display name. |
| `endpoint` | yes | Base URL, possibly with a version (`/v1`), a gateway prefix and a query string. `http`/`https`; no embedded credentials. |
| `protocol` | yes | Default protocol of the provider's models: `chat_completions`, `responses`, `anthropic_messages`. |
| `auth` | yes | `bearer`, `api_key_header` or `none` (local servers). |
| `auth_header` | no | Header carrying the key with `api_key_header` (default `x-api-key`). |
| `api_key_env` / `api_key` | one of | Name of the environment variable holding the key (recommended), **or** an inline key. Mutually exclusive; neither is allowed with `auth = "none"`. |
| `path` | no | Replaces the protocol's relative path, for models using the provider's default protocol. |
| `headers` | no | Extra static request headers. Values are treated as secrets (never printed). |
| `compat` | no | Protocol quirks, see below. |
| `models` | no | The provider's models. |

An inline `api_key` stays usable for a purely local setup. It must never be
copied into a fixture or a commit; the shipped examples use `api_key_env` only.
Keys are held in a type whose `Debug` is redacted, marked *sensitive* on the
wire, and scrubbed from every error message Bricks produces (including an API
error body that echoes the key back).

**`compat`** (provider-level, overridden field by field by the model's):

| Field | Default | Meaning |
|---|---|---|
| `max_tokens_field` | `max_tokens` | `chat_completions`: name of the output-limit field (e.g. `max_completion_tokens`). |
| `reasoning_field` | none | `chat_completions`: also echo reasoning back under this field on assistant turns that made tool calls. Reasoning is always *read* from `reasoning_content` or `reasoning`. |
| `stream_usage` | `true` | `chat_completions`: send `stream_options.include_usage`. |
| `prompt_cache_markers` | `true` | `anthropic_messages`: place `cache_control` breakpoints on the stable prefix (tools, system prompt). |

## 3. Model

| Field | Req. | Meaning |
|---|---|---|
| `id`, `name` | yes | Local id (unique in the provider) and display name. |
| `api_model` | yes | Exact model identifier sent to the server. |
| `protocol`, `endpoint`, `path` | no | Overrides for this model. |
| `auth`, `auth_header`, `headers`, `compat` | no | Overrides for this model (see §4). |
| `input_modalities`, `output_modalities` | no | Subsets of `text`, `image`, `audio`, `video`, `document`. Default `["text"]`. |
| `document_mime_types` | with `document` | Accepted MIME types (`type/subtype`, `text/*`, `*/*`). |
| `streaming`, `tool_calls` | no | Capability flags. **Default `false`**: undeclared means unsupported. `streaming = false` sends a blocking request. |
| `limits` | yes | `max_input_tokens`, `max_output_tokens`, optional `context_window_tokens` (a window shared by input and output). |
| `pricing` | no | USD per million tokens, see §9. |
| `reasoning` | no | Profiles, see §6. |
| `parameters` | no | Request parameters sent with every request (JSON-compatible, nesting allowed). |
| `remove_parameters` | no | JSON Pointers removed from the adapter's request *before* `parameters` are merged (e.g. `/temperature` for a server that rejects it). |
| `token_counting` | no | Table enabling pre-flight counting with the server: `anthropic_messages` → `messages/count_tokens`, `responses` → `responses/input_tokens`; optional `path`. Refused for `chat_completions` (no documented route). See `docs/context.md` §3. |

**Limits are declarative.** They never change the server's. Bricks uses them to
cap `max_tokens` at `max_output_tokens`, and to budget the prompt: with a shared
window the input budget is `min(max_input_tokens, window − reserved output)`,
because the output reserve includes reasoning tokens on all three protocols.
Without `context_window_tokens`, no total window is assumed. Token counts
computed locally are **estimates** (a character-class heuristic calibrated on
the model's reported usage); only the usage a server reports, or a configured
counting endpoint, measures tokens. See `docs/context.md`.

## 4. Protocols, endpoints, authentication

A *protocol* defines the request, the response and the streaming format. It has
nothing to do with any structured JSON the model is asked to produce.

| Protocol | Relative path | Conventional auth | Required headers |
|---|---|---|---|
| `chat_completions` | `chat/completions` | `bearer` | — |
| `responses` | `responses` | `bearer` | — |
| `anthropic_messages` | `messages` | `api_key_header` (`x-api-key`) | `anthropic-version: 2023-06-01` |

**Assembly.** `endpoint` is a base; the request URL is base + relative path.
The base's prefix and query string are kept; a version segment at the end of the
base (`v1`) is not repeated if the path starts with it; a `path` override that is
an absolute `http(s)://` URL is used as is. A provider-level `path` applies only
to models speaking the provider's default protocol, so a protocol override never
inherits another protocol's path.

**Overriding the protocol on a model** selects that protocol's conventions: its
path, its authentication style (unless the model states `auth`; `none` stays
`none`) and its required headers. Explicit `headers` (provider, then model)
override the protocol's by name.

## 5. Request pipeline

Every request, whatever the protocol:

1. **Refuse what cannot be carried** (§7) — before anything is sent.
2. Build the protocol request (the adapter).
3. Apply `remove_parameters`, then the model's `parameters`.
4. Apply the selected reasoning profile: its `remove`, then its `parameters`.
5. Send. A non-2xx status is returned as a typed error **from `complete()`**
   (429 → `RateLimit` with `Retry-After`, 5xx/529 retryable, 401/403/404 fatal) so
   the runner's retry loop sees it. Transport errors drop the URL.
6. Decode the response (streamed or not) into events; attach a cost estimate.

## 6. Reasoning profiles

A profile is an **id**, a **label** and **parameters**. Nothing more.

```toml
[providers.models.reasoning]
default = "approfondi"            # optional
[[providers.models.reasoning.profiles]]
id = "approfondi"
label = "Approfondi"
parameters = { reasoning = { effort = "high" } }   # what is actually sent
remove = ["/temperature"]                          # JSON Pointers removed first
```

* `low`, `medium`, `high`, `xhigh`, `max`, `ultra` are *suggestions*. There is no
  Rust enum and no list per model. Remove all profiles, remove some, rename them,
  add as many as you like. `profiles = []` exposes no choice.
* A profile id is **never** sent under its local name. `ultra` can be a local
  alias for an option the server accepts; it does not create a level on the
  server. A profile `off` must define the *real* mechanism that disables
  reasoning on that server — an absent parameter does not necessarily mean "off".
* **Merge order** on top of the adapter's request: model `remove_parameters` →
  model `parameters` → profile `remove` → profile `parameters`. Objects merge
  recursively; scalars and arrays replace.
* **Protected fields** — model, messages/input, system prompt/instructions,
  tools, tool choice, streaming control — cannot be set or removed (checked at
  load and again at request time). `parameters` customise the API's options; they
  never overwrite content the engine built.
* **Selection:** `Agent::builder().reasoning_profile(id)`, a per-request option
  `reasoning_profile`, or `provider().reasoning_profile(id)` at build time; else
  the model's `default`; else none. An unknown id is an error naming the available
  ones — a refused profile is never replaced silently.
* Structure is validated locally; **values are the server's call.** Bricks keeps
  no list of accepted levels.

## 7. Modalities and capabilities

Effective capability = **declared by the model ∩ transportable by the adapter**.
A request needing anything outside it fails with `Unsupported`, naming the model
and the block, before sending. No attachment is dropped or converted to text.
Declaring more than an adapter carries is allowed (it may describe the model),
but it does not make the capability operational.

**Capability matrix** (what the three adapters actually transport; locked by
`protocol_support` and its tests):

| Input | `chat_completions` | `responses` | `anthropic_messages` |
|---|---|---|---|
| text | yes | yes | yes |
| image | data, URL | data, URL, file id | data, URL, file id |
| audio | data (`wav`, `mp3`) | no | no |
| video | no | no | no |
| document | data, file id | data, URL, file id | data (`application/pdf`, `text/plain`), URL, file id |
| media inside a tool result | no | no | image, document |

| Output | all three |
|---|---|
| text | yes |
| image, audio, video, document (model-native) | **no** |

Tool calls are not a modality: they are carried by every adapter (ids, arguments
fragmented across stream chunks, results, stop reasons). A document produced *by
a tool* (a tool result) is distinct from a document produced natively by a model;
only the former is represented, and only where the table says so.

**What additional adapters or endpoints would be needed** (not built here):

* video input — a native Gemini-style adapter, or a compatible endpoint with a
  video part type;
* audio output — Chat Completions `modalities`/`audio` parameters (an extension of
  the existing adapter); speech and music generation use specialised endpoints;
* image output — the Responses image-generation tool or a dedicated image API;
* video output — specialised, usually asynchronous, endpoints;
* media inside tool results on `chat_completions`/`responses` — requires the
  content-array forms of tool outputs;
* cloud-signed access (Bedrock, Vertex) and OAuth flows — not covered; those
  providers need a gateway speaking one of the three protocols.

## 8. Multi-turn tool use

Preserved across turns, per protocol: tool-call ids, arguments (even when split
across chunks and across multi-byte characters), results, stop reasons and usage;
Anthropic **thinking blocks with their signatures** and **redacted thinking**
data; Responses **reasoning items** (including `encrypted_content`), echoed back
verbatim and in order; Chat Completions reasoning text (re-sent only when
`compat.reasoning_field` asks for it). A signature-less thinking block cannot be
valid Anthropic history and is not sent.

## 9. Prices and usage

* Prices are exact decimals, **USD per 1,000,000 tokens**: `input` (uncached),
  `output`, `cache_read`, optional `cache_write`, optional
  `cache_write_variants` (e.g. `"1h"`), and named alternative tariffs under
  `pricing.variants.<name>` (peak/off-peak, volume, …). A variant is complete —
  it inherits nothing.
* An absent price means **unknown, never free**. A cost is estimated only for the
  categories whose counter and price are both known; the estimate carries
  `partial = true` and the list of unpriced categories otherwise. Media billed per
  second, per image or per other unit cannot be expressed with per-token prices
  and are reported as unpriced. With no known price at all there is **no
  estimate** (not `$0`).
* One tariff is selected per estimate (`provider().tariff("offpeak")`); its name
  is reported. The result is an estimate under that tariff, not an invoice.
* Counters are normalized per protocol so nothing is counted twice:
  `input_tokens` is always the **uncached** prompt (cached tokens are moved to
  `cache_read_input_tokens`), `output_tokens` always **includes** reasoning
  tokens (`reasoning_tokens` is an informational subset), and Anthropic cache
  writes are kept per retention so each can be priced separately.
* No catalogue or price list is ever downloaded.

## 10. Migration from the previous provider system

There is no second system running in parallel and no fallback to the old one.

| Before | Now |
|---|---|
| `Anthropic::from_env()`, `Anthropic::builder()…` | a provider entry with `protocol = "anthropic_messages"`; `provider_from_config(None, "id/model")` |
| `OpenAi::builder().base_url(..).model(..).api_key(..)` | a provider entry with `protocol = "chat_completions"` (Ollama/vLLM: `auth = "none"`) |
| `.send_num_ctx(true)` (Ollama) | `parameters = { options = { num_ctx = 32768 } }` on the model |
| `Gemini`, `AnthropicVertex`, `gemini_vision_test` | removed (no native adapter). A Gemini-compatible endpoint can be used through one of the three protocols. |
| `Auth`, `AuthProvider`, `OAuthToken`, `oauth_login` | removed. Keys come from `api_key_env`/`api_key`. |
| `from_model_string("openai/gpt-4o")`, auto-detection from a bare model name | explicit `provider_id/model_id`; nothing is detected |
| `*_API_KEY`, `*_BASE_URL` read implicitly | removed; reference a key with `api_key_env`, put the URL in `endpoint` |
| built-in registry, `ApiFormat`, `ProviderQuirks`, `context_window_for_model` | the configuration file (`limits`, `parameters`, `remove_parameters`) |
| `thinking_budget`, `AnthropicBuilder::thinking`, `EffortLevel`, `--effort` | reasoning profiles; `Agent::builder().reasoning_profile(id)` |
| built-in price table, `CostTracker::add_with_model`, `estimate_cost*` | `pricing` in the file; the provider attaches the estimate; unknown stays unknown |
| default model `claude-sonnet-4-6` in the agent | none: a provider *is* one configured model; `.model()` is only a label |
| `cersei::{Anthropic, OpenAi, Gemini}`, prelude `Auth` | `cersei::{provider_from_config, ProviderRegistry, ConfiguredProvider}` |
| `CompletionRequest` options `thinking_budget`, `num_ctx` | only `tool_choice` and `reasoning_profile` remain; new field `output_modalities` |
| `ContentBlock::media_bytes` put audio/video in `Image` blocks | routed to `Audio`, `Video`, `Document`; new blocks `Audio`, `Video`, `ProtocolItem` |
| `tbench-agent --model` default `vertex/…` | `--model provider_id/model_id` required, `--providers <file>` optional |
| `longmem-bench --provider/--answerer-model gemini-…` | `--answerer-model`, `--judge-model`, `--extractor-model` as `provider_id/model_id`, `--providers`, `--embeddings openai|gemini` (embeddings keep their own key variable) |

Services that are not conversational chat are untouched: `cersei-embeddings`
keeps its own providers and key variables.

**Example.** Before:

```rust
let provider = OpenAi::builder()
    .base_url("http://localhost:11434/v1")
    .model("llama3.1:70b").api_key("ollama").build()?;
```

After — `~/.bricks/providers.toml`:

```toml
schema_version = 1
[[providers]]
id = "ollama"
name = "Ollama"
endpoint = "http://localhost:11434/v1"
protocol = "chat_completions"
auth = "none"
[[providers.models]]
id = "llama"
name = "Llama 3.1 70B"
api_model = "llama3.1:70b"
streaming = true
tool_calls = true
[providers.models.limits]
max_input_tokens = 32000
max_output_tokens = 8000
```

```rust
let provider = cersei::provider_from_config(None, "ollama/llama")?;
```

## 11. What is verified, and what is not

Covered by tests, with **scripted servers and no paid call**: TOML/JSON
equivalence, duplicates and invalid fields, secret resolution and absence of
leaks, endpoint assembly, protocol/endpoint/auth inheritance and overrides, the
three request formats, multi-turn tool exchange on each protocol (fragmented and
Unicode-split streams, thinking signatures, reasoning items), non-streaming
responses, HTTP and in-stream errors, connection loss, arbitrary/renamed/removed/
empty reasoning profiles, merge and protected fields, refused modalities,
limits, and costs with cache and unknown prices.

Not verified: behaviour against the real vendor APIs (their quirks beyond the
documented formats, rate limits, model-specific option rejections). The adapters
follow the documented request/stream shapes; run your own configuration against
your own account before relying on it.

The pages under `docs/content/` and `wiki/` that still describe the previous
`Anthropic`/`OpenAi`/`Gemini` types are historical and have not been rewritten.
