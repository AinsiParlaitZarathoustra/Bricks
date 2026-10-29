//! Shared helper: a configured provider pointed at an in-process test server.
//!
//! The key comes through `api_key_env` and an injected lookup, so no key —
//! not even a fake one — is written into a configuration fixture.

use bricks_provider::{ConfiguredProvider, ProviderRegistry};

pub mod follow;

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

/// A CUSTOM policy: `edit` sets its class effects and per-tool rules (the
/// classes start at the ASK effects).
#[allow(dead_code)]
pub fn custom(
    edit: impl FnOnce(&mut bricks_agent::permission::CustomRules),
) -> bricks_agent::permission::SessionPolicy {
    use bricks_agent::permission::{PermissionProfile, SessionPolicy};
    let mut policy = SessionPolicy::chosen(PermissionProfile::Custom);
    edit(&mut policy.custom);
    policy
}

/// A CUSTOM policy that allows exactly `tools`; the classes keep the ASK
/// effects (reads run, writes, commands and network are asked).
#[allow(dead_code)]
pub fn allow_tools(tools: &[&str]) -> bricks_agent::permission::SessionPolicy {
    use bricks_agent::permission::PermissionEffect;
    custom(|rules| {
        for tool in tools {
            rules
                .tools
                .insert(tool.to_string(), PermissionEffect::Allow);
        }
    })
}

/// The default policy for new sessions, stored in `dir` (what a person chose
/// in the desktop or with the CLI). Pass it as `EngineConfig::permissions`.
#[allow(dead_code)]
pub fn default_store(
    dir: &std::path::Path,
    policy: &bricks_agent::permission::SessionPolicy,
) -> bricks_agent::permission::PermissionStore {
    let store =
        bricks_agent::permission::PermissionStore::new(dir.join("default-permissions.json"));
    store
        .set_default(policy)
        .expect("the default policy is written");
    store
}
