//! `Provider::request_identity`: the model name a request is sent as and the
//! reasoning profile it gets. Checked against what the scripted server
//! actually received, so the identity cannot drift from the wire.

#[allow(dead_code)]
mod common;

use bricks_provider::{
    CompletionRequest, ConfiguredProvider, Provider, ProviderOptions, ProviderRegistry,
    REASONING_PROFILE_OPTION, RequestIdentity,
};
use bricks_types::Message;
use common::*;
use serde_json::{Value, json};

const KEY: &str = "sk-test-key-123456";

/// A reasoning-capable model (default profile `deep`, with `fast`, `off`
/// and an alias `ultra`), and a model without any profile.
const EXTRA: &str = r#"
[[providers.models]]
id = "think"
name = "Think"
api_model = "vendor-think"
streaming = true
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
[providers.models.parameters]
temperature = 0.5
reasoning = { summary = "auto", effort = "model-default" }
[providers.models.reasoning]
default = "deep"
[[providers.models.reasoning.profiles]]
id = "fast"
label = "Rapide"
parameters = { reasoning = { effort = "low" } }
[[providers.models.reasoning.profiles]]
id = "deep"
parameters = { reasoning = { effort = "high" } }
[[providers.models.reasoning.profiles]]
id = "off"
label = "Aucun"
remove = ["/reasoning", "/temperature"]
parameters = { thinking = { type = "disabled" } }
[[providers.models.reasoning.profiles]]
id = "ultra"
parameters = { reasoning = { effort = "high" }, extra_budget = 99999 }

[[providers.models]]
id = "plain"
name = "No profiles"
api_model = "vendor-plain"
streaming = true
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
[providers.models.reasoning]
profiles = []
"#;

fn setup(replies: Vec<Reply>) -> (Mock, ProviderRegistry) {
    let mock = serve(replies);
    let reg = registry_for(&mock.base, EXTRA);
    (mock, reg)
}

fn provider(reg: &ProviderRegistry, sel: &str) -> ConfiguredProvider {
    // The key comes through an injected lookup: no key is written anywhere.
    model(reg, sel)
        .provider()
        .env(|_| Some(KEY.to_string()))
        .build()
        .unwrap()
}

fn options(profile: Option<&str>) -> ProviderOptions {
    let mut o = ProviderOptions::default();
    if let Some(p) = profile {
        o.set(REASONING_PROFILE_OPTION, p);
    }
    o
}

/// One answer as a chat stream: the request is recorded, then answered.
fn answer() -> Reply {
    let events: Vec<(Option<&str>, Value)> = vec![
        (None, json!({"choices":[{"delta":{"content":"ok"}}]})),
        (
            None,
            json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}),
        ),
    ];
    Reply::Sse {
        chunks: fragment(&sse_events(&events), 64),
    }
}

/// Sends one request with `profile` as its option, and returns the identity
/// the provider reports for those options and the body the server received.
async fn send(
    mock: &Mock,
    p: &ConfiguredProvider,
    profile: Option<&str>,
) -> (Option<RequestIdentity>, Value) {
    let identity = p.request_identity(&options(profile));
    let mut r = CompletionRequest::new("ignored-local-id");
    r.messages = vec![Message::user("hi")];
    r.max_tokens = 100;
    r.options = options(profile);
    p.complete(r).await.unwrap().collect().await.unwrap();
    let body = mock.requests().last().expect("one request").body.clone();
    (identity, body)
}

fn identity(api_model: &str, profile: Option<&str>) -> RequestIdentity {
    RequestIdentity {
        api_model: api_model.into(),
        reasoning_profile: profile.map(str::to_string),
    }
}

#[tokio::test]
async fn without_an_option_the_model_default_profile_is_the_one_sent() {
    let (mock, reg) = setup(vec![answer()]);
    let p = provider(&reg, "mock/think");
    let (id, body) = send(&mock, &p, None).await;
    assert_eq!(id, Some(identity("vendor-think", Some("deep"))));
    assert_eq!(body["model"], "vendor-think");
    assert_eq!(
        body["reasoning"]["effort"], "high",
        "the profile `deep` was applied"
    );
}

#[tokio::test]
async fn the_requested_profile_is_the_one_reported_and_applied() {
    let (mock, reg) = setup(vec![answer(), answer(), answer()]);
    let p = provider(&reg, "mock/think");

    let (id, body) = send(&mock, &p, Some("fast")).await;
    assert_eq!(id, Some(identity("vendor-think", Some("fast"))));
    assert_eq!(body["reasoning"]["effort"], "low");

    let (id, body) = send(&mock, &p, Some("off")).await;
    assert_eq!(id, Some(identity("vendor-think", Some("off"))));
    assert!(body.get("reasoning").is_none() && body.get("temperature").is_none());
    assert_eq!(body["thinking"], json!({"type": "disabled"}));

    // A local alias: the identity names it, the wire never does.
    let (id, body) = send(&mock, &p, Some("ultra")).await;
    assert_eq!(id, Some(identity("vendor-think", Some("ultra"))));
    assert_eq!(body["extra_budget"], 99999);
    assert!(!body.to_string().contains("ultra"));
}

#[tokio::test]
async fn an_instance_profile_applies_without_an_option_and_the_option_wins() {
    let (mock, reg) = setup(vec![answer(), answer()]);
    let p = model(&reg, "mock/think")
        .provider()
        .env(|_| Some(KEY.to_string()))
        .reasoning_profile("fast")
        .build()
        .unwrap();
    let (id, body) = send(&mock, &p, None).await;
    assert_eq!(id, Some(identity("vendor-think", Some("fast"))));
    assert_eq!(body["reasoning"]["effort"], "low");

    let (id, body) = send(&mock, &p, Some("off")).await;
    assert_eq!(id, Some(identity("vendor-think", Some("off"))));
    assert_eq!(body["thinking"], json!({"type": "disabled"}));
}

#[tokio::test]
async fn a_model_without_profiles_reports_no_profile() {
    let (mock, reg) = setup(vec![answer()]);
    let p = provider(&reg, "mock/plain");
    let (id, body) = send(&mock, &p, None).await;
    assert_eq!(id, Some(identity("vendor-plain", None)));
    assert_eq!(body["model"], "vendor-plain");
    assert!(body.get("reasoning").is_none());
}

#[tokio::test]
async fn an_unknown_profile_is_reported_without_a_profile_and_nothing_is_sent() {
    let (mock, reg) = setup(vec![]);
    let p = provider(&reg, "mock/think");
    // The request is refused before it is sent: no profile is applied.
    assert_eq!(
        p.request_identity(&options(Some("mythic"))),
        Some(identity("vendor-think", None))
    );
    let mut r = CompletionRequest::new("ignored-local-id");
    r.messages = vec![Message::user("hi")];
    r.options = options(Some("mythic"));
    assert!(p.complete(r).await.is_err());
    assert!(mock.requests().is_empty(), "nothing was sent");
}
