# Providers

This page is a summary. The reference is [Providers](/docs/providers) on the documentation site.

- Providers are declared in `~/.bricks/providers.toml` (TOML or JSON). Start from `docs/providers.example.toml`.
- A model is selected as `provider_id/model_id`. Nothing is inferred from a model name: an unknown id or a missing file is an error.
- Build a provider from a selection with `bricks::provider_from_config(None, "provider_id/model_id")`, and pass it to `Agent::builder().provider(...)`.
- Each provider names the environment variable that holds its key in `api_key_env`. A key is read from there and never written to a log or a result.
