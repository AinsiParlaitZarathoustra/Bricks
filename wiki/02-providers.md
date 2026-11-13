# Providers

This page is a summary. The full reference is [Providers](/docs/providers) on the documentation site.

- Providers and models are declared in `~/.bricks/providers.toml` (or `$BRICKS_HOME/providers.toml` when `BRICKS_HOME` is set), in TOML or JSON. Start from `docs/providers.example.toml`.
- Select a model as `provider_id/model_id`. Nothing is guessed from a model name: an unknown id or a missing file returns an error.
- Load a provider with `bricks::provider_from_config(None, "provider_id/model_id")` and pass it to `Agent::builder().provider(...)`.
- Set the API key with `api_key` in `providers.toml` (or pass one in code when building a provider). Keys are redacted from logs and error messages. The older `api_key_env` field is only accepted so legacy files still load; the runtime does not read environment variables from it.
