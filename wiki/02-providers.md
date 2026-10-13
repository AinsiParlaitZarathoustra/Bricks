# Providers

A `Provider` is the engine's interface to an LLM. Bricks has no built-in catalogue and no per-vendor types: there is no `Anthropic` or `OpenAi` type, and no `Auth` enum. Which providers and models exist is decided by one file, `providers.toml`. Each model speaks one of three wire protocols, and a model is selected as `provider_id/model_id`.

The full reference is [`docs/providers.md`](../docs/providers.md). The annotated example is [`docs/providers.example.toml`](../docs/providers.example.toml).

## Configuring a provider

The library reads `~/.bricks/providers.toml`, or an explicit `.toml` or `.json` path. The `bricks` CLI also honours `$BRICKS_HOME/providers.toml`. A missing file is an error: there is no fallback catalogue.

```toml
schema_version = 1                     # required

[[providers]]
id = "custom"                          # stable id, no whitespace, no "/"
name = "My provider"
endpoint = "https://api.example.com/v1"  # base URL; the protocol path is appended
protocol = "chat_completions"          # default protocol of this provider's models
auth = "bearer"                        # bearer | api_key_header | none
api_key_env = "BRICKS_CUSTOM_API_KEY"  # the variable holding the key (recommended)

[[providers.models]]
id = "flash"                           # unique within the provider
name = "Flash"
api_model = "vendor-flash"             # sent to the server exactly as written
streaming = true                       # default false: undeclared means unsupported
tool_calls = true

[providers.models.limits]              # required
max_input_tokens = 96000
max_output_tokens = 32000
context_window_tokens = 128000         # optional: one window shared by input and output
```

A local Ollama server needs no key:

```toml
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

Limits are declarative: they cap `max_tokens` and budget the prompt, but never change what the server enforces. Prices are optional; an absent price means "unknown", never "free" (see `docs/providers.md` §9).

## Protocols

| `protocol` | Path appended to `endpoint` | Default auth | Extra header |
|------------|-----------------------------|--------------|--------------|
| `chat_completions` | `chat/completions` | `bearer` | none |
| `responses` | `responses` | `bearer` | none |
| `anthropic_messages` | `messages` | `api_key_header` (`x-api-key`) | `anthropic-version: 2023-06-01` |

A model can override its provider's protocol (`protocol` on the model). The override brings that protocol's path and required headers. The auth style is the model's own `auth` if it sets one, otherwise the protocol's conventional style (unless the provider is `auth = "none"`).

## Authentication

- `bearer`: `Authorization: Bearer <key>`
- `api_key_header`: `<auth_header>: <key>`, where `auth_header` defaults to `x-api-key`
- `none`: no credentials (local servers)

The key comes from `api_key_env` (the name of an environment variable), or from an inline `api_key`, which must never be committed. The two are mutually exclusive, and neither is allowed with `auth = "none"`. Keys are redacted in `Debug` output (`Secret` and `ConfiguredProvider` print `<redacted>`), and an invalid key is reported without its value.

## Using a provider from Rust

```rust
use std::path::Path;
use cersei::prelude::*;

// ~/.bricks/providers.toml
let provider = provider_from_config(None, "custom/flash")?;

// An explicit file replaces the default one; the two are never merged.
let provider = provider_from_config(Some(Path::new("./providers.toml")), "custom/flash")?;

let agent = Agent::builder()
    .provider(provider)
    .reasoning_profile("approfondi")   // optional: a profile defined for this model
    .build()?;
```

`provider_from_config` is shorthand for loading the registry, resolving the selection and building the provider. To load once and inspect or resolve several models:

```rust
let registry = ProviderRegistry::load(None)?;        // parse and validate the file
let model = registry.resolve("custom/flash")?;       // a ResolvedModel
let provider = model.build_provider()?;              // reads api_key_env now
```

`Agent::builder().model("...")` only sets a label. The provider is the model: there is no default model.

## Reasoning profiles

Profiles are defined per model in the file. Each has an `id`, a `label` and the `parameters` actually sent to the server:

```toml
[providers.models.reasoning]
default = "approfondi"                 # optional

[[providers.models.reasoning.profiles]]
id = "approfondi"
label = "Approfondi"
parameters = { reasoning = { effort = "high" } }
remove = ["/temperature"]              # JSON Pointers removed before the merge
```

The profile id is a local name and is never sent to the server. Select one with `Agent::builder().reasoning_profile(id)`. An unknown id is an error that lists the available ones.

## The Provider trait

The trait as it is in `crates/cersei-provider/src/lib.rs`:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    /// The configured provider id (e.g. `custom`).
    fn name(&self) -> &str;

    /// Tokens available for the prompt (declared `max_input_tokens`, reduced
    /// when the window is shared with the output).
    fn context_window(&self, model: &str) -> u64;

    /// Identity, limits and protocol of the bound model. Default: None.
    fn model_info(&self) -> Option<ModelInfo> { None }

    /// Send a streaming completion request.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionStream>;

    /// Default: `complete` followed by `CompletionStream::collect`.
    async fn complete_blocking(&self, request: CompletionRequest) -> Result<CompletionResponse>;

    /// Pre-flight count with the server's counting endpoint, if configured.
    /// Ok(None) otherwise; callers then use a local estimate.
    async fn count_request_tokens(&self, request: &CompletionRequest) -> Result<Option<u64>>;
}
```

There is no `capabilities()` method and no `count_tokens(messages, model)`. Capabilities are declared in the file, and a request needing something the model or adapter cannot carry fails with `Unsupported` before anything is sent.

## Custom providers

Any type implementing `Provider` can be passed to `.provider(...)`, which is useful for tests or for a backend that does not speak one of the three protocols:

```rust
use cersei::prelude::*;
use tokio::sync::mpsc;

struct EchoProvider;

#[async_trait]
impl Provider for EchoProvider {
    fn name(&self) -> &str { "echo" }

    fn context_window(&self, _model: &str) -> u64 { 8192 }

    async fn complete(&self, _request: CompletionRequest) -> Result<cersei::provider::CompletionStream> {
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::MessageStart { id: "1".into(), model: "echo".into(), usage: None }).await;
            let _ = tx.send(StreamEvent::ContentBlockStart { index: 0, block_type: "text".into(), id: None, name: None }).await;
            let _ = tx.send(StreamEvent::TextDelta { index: 0, text: "Hello!".into() }).await;
            let _ = tx.send(StreamEvent::ContentBlockStop { index: 0 }).await;
            let _ = tx.send(StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                usage: Some(Usage { input_tokens: 10, output_tokens: 5, ..Default::default() }),
            }).await;
            let _ = tx.send(StreamEvent::MessageStop).await;
        });
        Ok(cersei::provider::CompletionStream::new(rx))
    }
}
```

## CompletionRequest

```rust
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: u32,                   // default 16384
    pub temperature: Option<f32>,
    pub stop_sequences: Vec<String>,
    pub options: ProviderOptions,
    pub output_modalities: Vec<Modality>,  // empty means text only
}
```

`ProviderOptions` carries engine-level options only: `tool_choice` (for example `"required"`, to force a tool call) and `reasoning_profile`. Parameters a model needs, such as `top_p` or Ollama's `num_ctx`, belong in the model's `parameters` in `providers.toml`, not in code.

## StreamEvent

Events emitted by a provider's stream (`cersei_types::StreamEvent`):

| Event | Description |
|-------|-------------|
| `MessageStart { id, model, usage }` | Response begins; `usage` is optional |
| `ContentBlockStart { index, block_type, id, name }` | New block (`text`, `tool_use`, `thinking`, ...); `id` and `name` for tool use |
| `TextDelta { index, text }` | Incremental text |
| `InputJsonDelta { index, partial_json }` | Incremental tool input JSON |
| `ThinkingDelta { index, thinking }` | Incremental thinking text |
| `SignatureDelta { index, signature }` | Signature of a thinking block, echoed back verbatim in history |
| `ContentBlockStop { index }` | Block finished |
| `ProtocolItem { index, protocol, item }` | A protocol item echoed back verbatim (e.g. a Responses `reasoning` item) |
| `MessageDelta { stop_reason, usage }` | Stop reason and usage (both optional) |
| `MessageStop` | Response complete |
| `Error { message }` | Error occurred |
| `Ping` | Keep-alive |

## Migrating from the earlier API

| Before | Now |
|--------|-----|
| `Anthropic::from_env()`, `Anthropic::builder()...` | An entry with `protocol = "anthropic_messages"`, loaded with `provider_from_config` |
| `OpenAi::builder().base_url(..).model(..).api_key(..)` | An entry with `protocol = "chat_completions"`; local servers use `auth = "none"` |
| `Auth`, `AuthProvider`, `OAuthToken`, `oauth_login` | Removed. Keys come from `api_key_env` or `api_key` |
| `thinking_budget`, `ProviderCapabilities`, `EffortLevel` | Reasoning profiles, and `parameters` on the model |
| `*_API_KEY` and `*_BASE_URL` read implicitly | Removed. Name the key variable with `api_key_env`; put the URL in `endpoint` |

`Gemini` and `AnthropicVertex` have no native adapter any more. A Gemini-compatible endpoint can be used through one of the three protocols. Cloud-signed access (Bedrock, Vertex) and OAuth flows are not covered. See [`docs/providers.md` §10](../docs/providers.md) for the full table.

## Verification status

The test suite exercises the adapters against scripted servers. It does not call real vendor APIs, so vendor quirks, rate limits and model-specific option rejections are not verified. Run your own configuration against your own account before relying on it (`docs/providers.md` §11).
