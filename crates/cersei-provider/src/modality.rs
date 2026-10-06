//! Modalities, capabilities and pre-send validation.
//!
//! A model *declares* what it accepts and produces; an adapter knows what its
//! protocol can *transport*. The effective capability is the intersection. A
//! request that needs anything outside it is refused with an explicit error
//! before it is sent — an attachment is never dropped or converted silently.
//!
//! Declaring more than an adapter can carry is allowed (the declaration may
//! describe the model while a future adapter or a specialised endpoint carries
//! the media), but it does not make the capability operational: see
//! [`protocol_support`] and the matrix in `docs/providers.md`.

use crate::config::{ModelConfig, Protocol};
use cersei_types::{ContentBlock, Message, MessageContent, Modality, Role, ToolResultContent};
use std::collections::BTreeSet;

/// What an adapter can put on the wire for one protocol.
#[derive(Debug, Clone, Copy)]
pub struct ProtocolSupport {
    /// Input modalities the adapter can encode in a user message.
    pub inputs: &'static [Modality],
    /// Output modalities the adapter can decode from a response.
    pub outputs: &'static [Modality],
    /// Media (beyond text) the adapter can place inside a tool result.
    pub tool_result_media: &'static [Modality],
}

/// The transportable surface of each protocol adapter.
pub fn protocol_support(protocol: Protocol) -> ProtocolSupport {
    use Modality::*;
    match protocol {
        Protocol::ChatCompletions => ProtocolSupport {
            inputs: &[Text, Image, Audio, Document],
            outputs: &[Text],
            tool_result_media: &[],
        },
        Protocol::Responses => ProtocolSupport {
            inputs: &[Text, Image, Document],
            outputs: &[Text],
            tool_result_media: &[],
        },
        Protocol::AnthropicMessages => ProtocolSupport {
            inputs: &[Text, Image, Document],
            outputs: &[Text],
            tool_result_media: &[Image, Document],
        },
    }
}

/// How a media payload is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Inline base64 data.
    Data,
    /// A remote URL the server fetches.
    Url,
    /// A file previously uploaded to the provider.
    FileId,
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SourceKind::Data => "inline data",
            SourceKind::Url => "URL",
            SourceKind::FileId => "file id",
        })
    }
}

/// Whether `protocol` can carry `modality` as `kind`.
pub fn supports_source(protocol: Protocol, modality: Modality, kind: SourceKind) -> bool {
    use Modality::*;
    use SourceKind::*;
    matches!(
        (protocol, modality, kind),
        (_, Text, _)
            | (Protocol::ChatCompletions, Image, Data | Url)
            | (Protocol::ChatCompletions, Audio, Data)
            | (Protocol::ChatCompletions, Document, Data | FileId)
            | (Protocol::Responses, Image | Document, _)
            | (Protocol::AnthropicMessages, Image | Document, _)
    )
}

/// Declared and effective capabilities of a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub input: BTreeSet<Modality>,
    pub output: BTreeSet<Modality>,
    pub document_mime_types: Vec<String>,
    pub streaming: bool,
    pub tool_calls: bool,
}

impl Capabilities {
    /// What the configuration declares.
    pub fn declared(model: &ModelConfig) -> Self {
        Capabilities {
            input: model.input_modalities.iter().copied().collect(),
            output: model.output_modalities.iter().copied().collect(),
            document_mime_types: model.document_mime_types.clone(),
            streaming: model.streaming,
            tool_calls: model.tool_calls,
        }
    }

    /// Declared ∩ transportable by `protocol`'s adapter.
    pub fn effective(&self, protocol: Protocol) -> Self {
        let sup = protocol_support(protocol);
        let keep = |set: &BTreeSet<Modality>, allowed: &[Modality]| {
            set.iter()
                .copied()
                .filter(|m| allowed.contains(m))
                .collect()
        };
        Capabilities {
            input: keep(&self.input, sup.inputs),
            output: keep(&self.output, sup.outputs),
            document_mime_types: self.document_mime_types.clone(),
            streaming: self.streaming,
            tool_calls: self.tool_calls,
        }
    }

    /// Declared modalities no adapter of this protocol can carry yet.
    pub fn declared_but_not_transportable(
        &self,
        protocol: Protocol,
    ) -> (Vec<Modality>, Vec<Modality>) {
        let eff = self.effective(protocol);
        (
            self.input.difference(&eff.input).copied().collect(),
            self.output.difference(&eff.output).copied().collect(),
        )
    }
}

/// The payload fields shared by image, audio, video and document sources.
pub(crate) struct SourceFields<'a> {
    pub media_type: Option<&'a str>,
    pub data: Option<&'a str>,
    pub url: Option<&'a str>,
    pub file_id: Option<&'a str>,
}

impl SourceFields<'_> {
    pub fn kind(&self) -> Result<SourceKind, String> {
        match (
            self.data.is_some(),
            self.url.is_some(),
            self.file_id.is_some(),
        ) {
            (true, false, false) => Ok(SourceKind::Data),
            (false, true, false) => Ok(SourceKind::Url),
            (false, false, true) => Ok(SourceKind::FileId),
            (false, false, false) => Err("has no data, url or file_id".into()),
            _ => Err("has more than one of data, url and file_id".into()),
        }
    }
}

pub(crate) fn source_fields(block: &ContentBlock) -> Option<(Modality, SourceFields<'_>)> {
    match block {
        ContentBlock::Image { source } => Some((
            Modality::Image,
            SourceFields {
                media_type: source.media_type.as_deref(),
                data: source.data.as_deref(),
                url: source.url.as_deref(),
                file_id: source.file_id.as_deref(),
            },
        )),
        ContentBlock::Audio { source } => Some((
            Modality::Audio,
            SourceFields {
                media_type: source.media_type.as_deref(),
                data: source.data.as_deref(),
                url: source.url.as_deref(),
                file_id: source.file_id.as_deref(),
            },
        )),
        ContentBlock::Video { source } => Some((
            Modality::Video,
            SourceFields {
                media_type: source.media_type.as_deref(),
                data: source.data.as_deref(),
                url: source.url.as_deref(),
                file_id: source.file_id.as_deref(),
            },
        )),
        ContentBlock::Document { source, .. } => Some((
            Modality::Document,
            SourceFields {
                media_type: source.media_type.as_deref(),
                data: source.data.as_deref(),
                url: source.url.as_deref(),
                file_id: source.file_id.as_deref(),
            },
        )),
        _ => None,
    }
}

fn mime_allowed(allowed: &[String], mime: &str) -> bool {
    allowed.iter().any(|a| {
        a == "*/*"
            || a.eq_ignore_ascii_case(mime)
            || a.strip_suffix("/*").is_some_and(|prefix| {
                mime.to_ascii_lowercase()
                    .starts_with(&format!("{}/", prefix.to_ascii_lowercase()))
            })
    })
}

fn check_media_block(
    block: &ContentBlock,
    caps: &Capabilities,
    protocol: Protocol,
    place: &str,
) -> Result<(), String> {
    let Some((modality, src)) = source_fields(block) else {
        return Ok(());
    };
    if !caps.input.contains(&modality) {
        let declared = caps
            .input
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "{place}: a {modality} block cannot be sent: this model's declared input modalities \
             are [{declared}]"
        ));
    }
    if !protocol_support(protocol).inputs.contains(&modality) {
        return Err(format!(
            "{place}: a {modality} block cannot be sent: {modality} input is declared, but the \
             `{protocol}` adapter cannot transport it"
        ));
    }
    let kind = src
        .kind()
        .map_err(|e| format!("{place}: the {modality} block {e}"))?;
    if !supports_source(protocol, modality, kind) {
        return Err(format!(
            "{place}: the `{protocol}` protocol cannot carry {modality} as {kind}"
        ));
    }
    if modality == Modality::Document {
        match src.media_type {
            Some(mime) => {
                if !mime_allowed(&caps.document_mime_types, mime) {
                    return Err(format!(
                        "{place}: document type `{mime}` is not in this model's document_mime_types ({})",
                        caps.document_mime_types.join(", ")
                    ));
                }
            }
            None => {
                if !mime_allowed(&caps.document_mime_types, "*/*") {
                    return Err(format!(
                        "{place}: the document has no media_type, so it cannot be checked against \
                         document_mime_types; set media_type (or declare \"*/*\")"
                    ));
                }
            }
        }
    }
    if matches!(modality, Modality::Audio) && kind == SourceKind::Data && src.media_type.is_none() {
        return Err(format!(
            "{place}: inline audio needs a media_type (for example audio/wav)"
        ));
    }
    Ok(())
}

/// Refuse, before sending, anything the model or adapter cannot carry.
pub fn validate_messages(
    messages: &[Message],
    requested_outputs: &[Modality],
    has_tools: bool,
    caps: &Capabilities,
    protocol: Protocol,
) -> Result<(), String> {
    if has_tools && !caps.tool_calls {
        return Err(
            "tools were provided, but this model does not declare `tool_calls = true`".into(),
        );
    }
    let eff = caps.effective(protocol);
    for m in requested_outputs {
        if !eff.output.contains(m) {
            let (_, undeliverable) = caps.declared_but_not_transportable(protocol);
            let why = if undeliverable.contains(m) {
                format!("declared, but the `{protocol}` adapter cannot transport {m} output")
            } else {
                "not declared by this model".to_string()
            };
            return Err(format!("{m} output was requested: {why}"));
        }
    }

    let sup = protocol_support(protocol);
    for (i, msg) in messages.iter().enumerate() {
        let place = format!(
            "message {i} ({})",
            match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
            }
        );
        let MessageContent::Blocks(blocks) = &msg.content else {
            continue;
        };
        for block in blocks {
            match (msg.role, block) {
                (Role::Assistant, b)
                    if source_fields(b).is_some() && b.modality() != Some(Modality::Text) =>
                {
                    return Err(format!(
                        "{place}: model-native media output in history cannot be re-sent \
                         (no adapter transports it); remove the {} block",
                        b.modality().map(|m| m.as_str()).unwrap_or("media")
                    ));
                }
                (_, ContentBlock::ToolResult { content, .. }) => {
                    if let ToolResultContent::Blocks(inner) = content {
                        for b in inner {
                            match b.modality() {
                                None | Some(Modality::Text) => {}
                                Some(m) => {
                                    // A tool-produced image/document is distinct from
                                    // model-native output; it is carried only where the
                                    // protocol has a tool-result media form.
                                    if !sup.tool_result_media.contains(&m) {
                                        return Err(format!(
                                            "{place}: a tool result contains a {m} block, which the \
                                             `{protocol}` protocol cannot carry inside a tool result"
                                        ));
                                    }
                                    if !caps.input.contains(&m) {
                                        return Err(format!(
                                            "{place}: a tool result contains a {m} block, but this \
                                             model does not accept {m} input"
                                        ));
                                    }
                                    check_media_block(
                                        b,
                                        caps,
                                        protocol,
                                        &format!("{place}, tool result"),
                                    )?;
                                }
                            }
                        }
                    }
                }
                (_, b) => check_media_block(b, caps, protocol, &place)?,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cersei_types::Message;

    fn caps(input: &[Modality], output: &[Modality], mimes: &[&str]) -> Capabilities {
        Capabilities {
            input: input.iter().copied().collect(),
            output: output.iter().copied().collect(),
            document_mime_types: mimes.iter().map(|s| s.to_string()).collect(),
            streaming: true,
            tool_calls: true,
        }
    }

    fn user(blocks: Vec<ContentBlock>) -> Vec<Message> {
        vec![Message::user_blocks(blocks)]
    }

    use Modality::*;

    #[test]
    fn effective_is_declared_intersect_transportable() {
        let c = caps(
            &[Text, Image, Audio, Video, Document],
            &[Text, Image, Audio, Video, Document],
            &["application/pdf"],
        );
        let eff = c.effective(Protocol::ChatCompletions);
        assert_eq!(eff.input, BTreeSet::from([Text, Image, Audio, Document]));
        assert_eq!(eff.output, BTreeSet::from([Text]));
        let eff = c.effective(Protocol::Responses);
        assert_eq!(eff.input, BTreeSet::from([Text, Image, Document]));
        let eff = c.effective(Protocol::AnthropicMessages);
        assert_eq!(eff.input, BTreeSet::from([Text, Image, Document]));
        let (inputs, outputs) = c.declared_but_not_transportable(Protocol::AnthropicMessages);
        assert_eq!(inputs, vec![Audio, Video]);
        assert_eq!(outputs, vec![Image, Audio, Video, Document]);
    }

    #[test]
    fn undeclared_media_is_refused_with_an_explicit_error() {
        let c = caps(&[Text], &[Text], &[]);
        let msgs = user(vec![ContentBlock::image_url("https://x/y.png")]);
        let err = validate_messages(&msgs, &[], false, &c, Protocol::ChatCompletions).unwrap_err();
        assert!(
            err.contains("image block cannot be sent") && err.contains("[text]"),
            "{err}"
        );
    }

    #[test]
    fn declared_but_untransportable_media_is_refused() {
        let c = caps(&[Text, Video, Audio], &[Text], &[]);
        let video = user(vec![ContentBlock::video_url("https://x/v.mp4")]);
        for p in [
            Protocol::ChatCompletions,
            Protocol::Responses,
            Protocol::AnthropicMessages,
        ] {
            let err = validate_messages(&video, &[], false, &c, p).unwrap_err();
            assert!(err.contains("video"), "{p}: {err}");
        }
        let audio = user(vec![ContentBlock::audio_bytes("audio/wav", b"RIFF")]);
        assert!(validate_messages(&audio, &[], false, &c, Protocol::ChatCompletions).is_ok());
        assert!(validate_messages(&audio, &[], false, &c, Protocol::Responses).is_err());
        assert!(validate_messages(&audio, &[], false, &c, Protocol::AnthropicMessages).is_err());
    }

    #[test]
    fn source_kinds_are_checked_per_protocol() {
        let c = caps(
            &[Text, Image, Audio, Document],
            &[Text],
            &["application/pdf"],
        );
        let audio_url = user(vec![ContentBlock::audio_url("https://x/a.wav")]);
        assert!(
            validate_messages(&audio_url, &[], false, &c, Protocol::ChatCompletions)
                .unwrap_err()
                .contains("cannot carry audio as URL")
        );
        let img_file = user(vec![ContentBlock::image_file_id("file-1")]);
        assert!(validate_messages(&img_file, &[], false, &c, Protocol::ChatCompletions).is_err());
        assert!(validate_messages(&img_file, &[], false, &c, Protocol::Responses).is_ok());
        assert!(validate_messages(&img_file, &[], false, &c, Protocol::AnthropicMessages).is_ok());
        let doc_url = user(vec![ContentBlock::Document {
            source: cersei_types::DocumentSource {
                source_type: "url".into(),
                media_type: Some("application/pdf".into()),
                data: None,
                url: Some("https://x/d.pdf".into()),
                file_id: None,
            },
            title: None,
            context: None,
            citations: None,
        }]);
        assert!(validate_messages(&doc_url, &[], false, &c, Protocol::ChatCompletions).is_err());
        assert!(validate_messages(&doc_url, &[], false, &c, Protocol::Responses).is_ok());
    }

    #[test]
    fn document_mime_types_are_enforced() {
        let c = caps(&[Text, Document], &[Text], &["application/pdf", "text/*"]);
        let pdf = user(vec![ContentBlock::document_bytes(
            "application/pdf",
            b"%PDF",
        )]);
        let txt = user(vec![ContentBlock::document_bytes("text/markdown", b"# hi")]);
        let zip = user(vec![ContentBlock::document_bytes("application/zip", b"PK")]);
        for p in [
            Protocol::ChatCompletions,
            Protocol::Responses,
            Protocol::AnthropicMessages,
        ] {
            assert!(validate_messages(&pdf, &[], false, &c, p).is_ok());
            assert!(validate_messages(&txt, &[], false, &c, p).is_ok());
            assert!(validate_messages(&zip, &[], false, &c, p)
                .unwrap_err()
                .contains("document_mime_types"));
        }
        // No media_type -> cannot be checked.
        let untyped = user(vec![ContentBlock::document_url("https://x/d")]);
        assert!(
            validate_messages(&untyped, &[], false, &c, Protocol::Responses)
                .unwrap_err()
                .contains("no media_type")
        );
        let any = caps(&[Text, Document], &[Text], &["*/*"]);
        assert!(validate_messages(&untyped, &[], false, &any, Protocol::Responses).is_ok());
    }

    #[test]
    fn requested_outputs_must_be_declared_and_transportable() {
        let c = caps(&[Text], &[Text, Image, Audio], &[]);
        assert!(validate_messages(&[], &[Text], false, &c, Protocol::ChatCompletions).is_ok());
        let err =
            validate_messages(&[], &[Audio], false, &c, Protocol::ChatCompletions).unwrap_err();
        assert!(
            err.contains(
                "declared, but the `chat_completions` adapter cannot transport audio output"
            ),
            "{err}"
        );
        let err =
            validate_messages(&[], &[Video], false, &c, Protocol::ChatCompletions).unwrap_err();
        assert!(err.contains("not declared"), "{err}");
    }

    #[test]
    fn tools_require_the_declaration() {
        let mut c = caps(&[Text], &[Text], &[]);
        c.tool_calls = false;
        assert!(validate_messages(&[], &[], true, &c, Protocol::Responses)
            .unwrap_err()
            .contains("tool_calls"));
        assert!(validate_messages(&[], &[], false, &c, Protocol::Responses).is_ok());
    }

    #[test]
    fn tool_produced_media_vs_model_native_media() {
        let c = caps(&[Text, Image, Document], &[Text], &["application/pdf"]);
        let tool_img = ContentBlock::ToolResult {
            tool_use_id: "t1".into(),
            content: ToolResultContent::Blocks(vec![ContentBlock::image_bytes("image/png", b"x")]),
            is_error: None,
        };
        let msgs = user(vec![tool_img.clone()]);
        // Anthropic carries media in tool results; the other two do not.
        assert!(validate_messages(&msgs, &[], false, &c, Protocol::AnthropicMessages).is_ok());
        assert!(
            validate_messages(&msgs, &[], false, &c, Protocol::ChatCompletions)
                .unwrap_err()
                .contains("inside a tool result")
        );
        assert!(validate_messages(&msgs, &[], false, &c, Protocol::Responses).is_err());
        // Model-native media in an assistant turn is not re-sendable.
        let asst = vec![Message::assistant_blocks(vec![ContentBlock::image_bytes(
            "image/png",
            b"x",
        )])];
        assert!(
            validate_messages(&asst, &[], false, &c, Protocol::AnthropicMessages)
                .unwrap_err()
                .contains("model-native")
        );
    }

    #[test]
    fn ambiguous_sources_are_refused() {
        let c = caps(&[Text, Image], &[Text], &[]);
        let both = user(vec![ContentBlock::Image {
            source: cersei_types::ImageSource {
                source_type: "base64".into(),
                media_type: Some("image/png".into()),
                data: Some("QQ==".into()),
                url: Some("https://x".into()),
                file_id: None,
            },
        }]);
        assert!(
            validate_messages(&both, &[], false, &c, Protocol::ChatCompletions)
                .unwrap_err()
                .contains("more than one")
        );
    }

    #[test]
    fn matrix_matches_the_documented_surface() {
        // Locks the table printed in docs/providers.md.
        assert_eq!(
            protocol_support(Protocol::ChatCompletions).inputs,
            &[Text, Image, Audio, Document]
        );
        assert_eq!(
            protocol_support(Protocol::Responses).inputs,
            &[Text, Image, Document]
        );
        assert_eq!(
            protocol_support(Protocol::AnthropicMessages).inputs,
            &[Text, Image, Document]
        );
        for p in [
            Protocol::ChatCompletions,
            Protocol::Responses,
            Protocol::AnthropicMessages,
        ] {
            assert_eq!(protocol_support(p).outputs, &[Text]);
            assert!(
                !protocol_support(p).inputs.contains(&Video),
                "no protocol here carries video"
            );
        }
        assert_eq!(
            protocol_support(Protocol::AnthropicMessages).tool_result_media,
            &[Image, Document]
        );
    }
}
