//! Shared helper: a configured provider pointed at an in-process test server.
//!
//! The key comes through `api_key_env` and an injected lookup, so no key —
//! not even a fake one — is written into a configuration fixture.

#![allow(dead_code)]

use cersei_provider::{ConfiguredProvider, ProviderRegistry};

/// A single-model provider speaking `protocol` (`chat_completions`,
/// `responses` or `anthropic_messages`) against `endpoint`.
/// `max_input_tokens` is the prompt budget the agent compacts against.
pub fn provider(endpoint: &str, protocol: &str, max_input_tokens: u64) -> ConfiguredProvider {
    let toml = format!(
        r#"
schema_version = 1

[[providers]]
id = "test"
name = "Test server"
endpoint = "{endpoint}"
protocol = "{protocol}"
auth = "bearer"
api_key_env = "BRICKS_TEST_API_KEY"

[[providers.models]]
id = "m"
name = "Test model"
api_model = "test-model"
streaming = true
tool_calls = true

[providers.models.limits]
max_input_tokens = {max_input_tokens}
max_output_tokens = 16384
"#
    );
    ProviderRegistry::from_toml_str(&toml, "agent-test.toml")
        .expect("test config is valid")
        .resolve("test/m")
        .expect("test model resolves")
        .provider()
        .env(|_| Some("test-key".to_string()))
        .build()
        .expect("provider builds")
}
