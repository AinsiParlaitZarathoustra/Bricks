//! How an image a tool returns can reach the bound model: decided by the
//! model's declared inputs (image or not) and by what its protocol's adapter
//! carries inside a tool result. No request is sent here.

use bricks_provider::{
    CompletionRequest, CompletionStream, ImageTransport, Provider, ProviderRegistry,
};
use bricks_types::Result;

const TOML: &str = r#"
schema_version = 1

[[providers]]
id = "p"
name = "Provider"
endpoint = "http://127.0.0.1:9/v1"
protocol = "chat_completions"
auth = "none"

[[providers.models]]
id = "chat_vision"
name = "Chat vision"
api_model = "vendor-chat-vision"
input_modalities = ["text", "image"]
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "chat_text"
name = "Chat text"
api_model = "vendor-chat-text"
input_modalities = ["text"]
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "resp_vision"
name = "Responses vision"
api_model = "vendor-resp-vision"
protocol = "responses"
input_modalities = ["text", "image"]
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "resp_text"
name = "Responses text"
api_model = "vendor-resp-text"
protocol = "responses"
input_modalities = ["text"]
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "anth_vision"
name = "Anthropic vision"
api_model = "vendor-anth-vision"
protocol = "anthropic_messages"
input_modalities = ["text", "image"]
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "anth_text"
name = "Anthropic text"
api_model = "vendor-anth-text"
protocol = "anthropic_messages"
input_modalities = ["text"]
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

[[providers.models]]
id = "anth_document"
name = "Anthropic documents only"
api_model = "vendor-anth-doc"
protocol = "anthropic_messages"
input_modalities = ["text", "document"]
document_mime_types = ["application/pdf"]
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
"#;

fn transport(selection: &str) -> ImageTransport {
    let reg = ProviderRegistry::from_toml_str(TOML, "image-transport.toml").unwrap();
    reg.resolve(selection)
        .unwrap()
        .provider()
        .build()
        .unwrap()
        .image_transport()
}

#[test]
fn a_protocol_that_carries_images_in_tool_results_uses_them_for_anthropic() {
    assert_eq!(transport("p/anth_vision"), ImageTransport::ToolResult);
}

#[test]
fn chat_completions_and_responses_put_the_image_in_the_user_turn() {
    assert_eq!(transport("p/chat_vision"), ImageTransport::UserTurn);
    assert_eq!(transport("p/resp_vision"), ImageTransport::UserTurn);
}

#[test]
fn a_model_that_does_not_declare_image_input_gets_none_on_every_protocol() {
    assert_eq!(transport("p/chat_text"), ImageTransport::None);
    assert_eq!(transport("p/resp_text"), ImageTransport::None);
    assert_eq!(transport("p/anth_text"), ImageTransport::None);
}

#[test]
fn declaring_documents_but_not_images_is_still_none() {
    assert_eq!(transport("p/anth_document"), ImageTransport::None);
}

/// A provider that does not declare its inputs keeps the trait default.
struct Undeclared;

#[async_trait::async_trait]
impl Provider for Undeclared {
    fn name(&self) -> &str {
        "undeclared"
    }
    fn context_window(&self, _model: &str) -> u64 {
        1_000
    }
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionStream> {
        Err(bricks_types::BricksError::Provider("not called".into()))
    }
}

#[test]
fn a_provider_without_configuration_says_it_takes_no_image() {
    assert_eq!(Undeclared.image_transport(), ImageTransport::None);
}
