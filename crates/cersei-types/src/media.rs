//! High-level constructors for multimodal content blocks.
//!
//! The wire types (`ContentBlock::Image` / `ContentBlock::Document` and their
//! `ImageSource` / `DocumentSource` payloads) have always existed, but building
//! them by hand — reading a file, sniffing its MIME type, base64-encoding the
//! bytes — is tedious and easy to get wrong. This module adds ergonomic
//! constructors so a caller can attach an image, video, audio clip, or document
//! to a message in one line:
//!
//! ```no_run
//! use cersei_types::{ContentBlock, Message};
//!
//! // From a local file, media type auto-detected from contents + extension.
//! let img = ContentBlock::from_path("diagram.png")?;
//! let msg = Message::user_with_files("What's in these?", &["a.png", "clip.mp4"])?;
//!
//! // Or build directly from bytes / base64 / a remote URL.
//! let block = ContentBlock::image_bytes("image/png", &std::fs::read("x.png")?);
//! let remote = ContentBlock::image_url("https://example.com/cat.jpg");
//! # Ok::<(), cersei_types::CerseiError>(())
//! ```
//!
//! What each protocol can carry differs (see the capability matrix in
//! `docs/providers.md`). The blocks are protocol-agnostic; an adapter that cannot
//! transport a block refuses the request with an explicit error before anything
//! is sent — it never drops an attachment silently.

use crate::{
    CerseiError, ContentBlock, DocumentSource, ImageSource, MediaSource, Message, MessageContent,
    Result, Role,
};
use base64::Engine;
use std::path::Path;

/// Broad category of a media file, derived from its MIME type. Used to decide
/// whether a file becomes an image, audio, video or document block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    /// PDFs, text, and anything else carried as a document block.
    Document,
}

impl MediaKind {
    /// Classify a MIME type. Unknown types fall back to [`MediaKind::Document`].
    pub fn from_mime(mime: &str) -> Self {
        if mime.starts_with("image/") {
            MediaKind::Image
        } else if mime.starts_with("video/") {
            MediaKind::Video
        } else if mime.starts_with("audio/") {
            MediaKind::Audio
        } else {
            MediaKind::Document
        }
    }
}

/// Detect a MIME type from the leading bytes of a file (magic numbers), falling
/// back to the file extension when the signature is unrecognized. Returns `None`
/// when neither yields a known type.
pub fn detect_mime(bytes: &[u8], path: Option<&Path>) -> Option<String> {
    if let Some(m) = sniff_magic(bytes) {
        return Some(m.to_string());
    }
    path.and_then(mime_from_extension).map(|s| s.to_string())
}

/// Content-sniffing by magic numbers for the formats the major model providers
/// accept. Deliberately small and dependency-free.
fn sniff_magic(b: &[u8]) -> Option<&'static str> {
    if b.len() >= 12 {
        if &b[0..4] == b"RIFF" {
            match &b[8..12] {
                b"WEBP" => return Some("image/webp"),
                b"WAVE" => return Some("audio/wav"),
                b"AVI " => return Some("video/x-msvideo"),
                _ => {}
            }
        }
        // ISO base-media (MP4/MOV/M4A): "....ftyp<brand>"
        if &b[4..8] == b"ftyp" {
            return Some(match &b[8..12] {
                b"qt  " => "video/quicktime",
                b"M4A " | b"M4B " => "audio/mp4",
                _ => "video/mp4",
            });
        }
    }
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if b.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("video/webm");
    }
    if b.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    if b.starts_with(b"ID3") || b.starts_with(&[0xFF, 0xFB]) || b.starts_with(&[0xFF, 0xF3]) {
        return Some("audio/mpeg");
    }
    None
}

fn mime_from_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mpeg" | "mpg" => "video/mpeg",
        "flv" => "video/x-flv",
        "wmv" => "video/x-ms-wmv",
        "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "txt" | "text" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        _ => return None,
    })
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl ContentBlock {
    /// Image block from already-base64-encoded data and an explicit MIME type.
    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentBlock::Image {
            source: ImageSource {
                source_type: "base64".into(),
                media_type: Some(media_type.into()),
                data: Some(data.into()),
                url: None,
                file_id: None,
            },
        }
    }

    /// Image block from raw bytes; the bytes are base64-encoded for you.
    pub fn image_bytes(media_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self::image_base64(media_type, b64(bytes))
    }

    /// Image block referencing a remote URL (provider must support URL sources).
    pub fn image_url(url: impl Into<String>) -> Self {
        ContentBlock::Image {
            source: ImageSource {
                source_type: "url".into(),
                media_type: None,
                data: None,
                url: Some(url.into()),
                file_id: None,
            },
        }
    }

    /// Image block referencing a file already uploaded to the provider.
    pub fn image_file_id(file_id: impl Into<String>) -> Self {
        ContentBlock::Image {
            source: ImageSource {
                source_type: "file".into(),
                media_type: None,
                data: None,
                url: None,
                file_id: Some(file_id.into()),
            },
        }
    }

    /// Document block (PDF, text, …) from already-base64-encoded data.
    pub fn document_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentBlock::Document {
            source: DocumentSource {
                source_type: "base64".into(),
                media_type: Some(media_type.into()),
                data: Some(data.into()),
                url: None,
                file_id: None,
            },
            title: None,
            context: None,
            citations: None,
        }
    }

    /// Document block from raw bytes; the bytes are base64-encoded for you.
    pub fn document_bytes(media_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self::document_base64(media_type, b64(bytes))
    }

    /// Document block referencing a remote URL.
    pub fn document_url(url: impl Into<String>) -> Self {
        ContentBlock::Document {
            source: DocumentSource {
                source_type: "url".into(),
                media_type: None,
                data: None,
                url: Some(url.into()),
                file_id: None,
            },
            title: None,
            context: None,
            citations: None,
        }
    }

    /// Document block referencing a file already uploaded to the provider.
    pub fn document_file_id(file_id: impl Into<String>) -> Self {
        ContentBlock::Document {
            source: DocumentSource {
                source_type: "file".into(),
                media_type: None,
                data: None,
                url: None,
                file_id: Some(file_id.into()),
            },
            title: None,
            context: None,
            citations: None,
        }
    }

    /// Audio block from already-base64-encoded data and an explicit MIME type.
    pub fn audio_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentBlock::Audio {
            source: MediaSource {
                source_type: "base64".into(),
                media_type: Some(media_type.into()),
                data: Some(data.into()),
                url: None,
                file_id: None,
            },
        }
    }

    /// Audio block from raw bytes; the bytes are base64-encoded for you.
    pub fn audio_bytes(media_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self::audio_base64(media_type, b64(bytes))
    }

    /// Audio block referencing a remote URL.
    pub fn audio_url(url: impl Into<String>) -> Self {
        ContentBlock::Audio {
            source: MediaSource {
                source_type: "url".into(),
                media_type: None,
                data: None,
                url: Some(url.into()),
                file_id: None,
            },
        }
    }

    /// Video block from already-base64-encoded data and an explicit MIME type.
    pub fn video_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        ContentBlock::Video {
            source: MediaSource {
                source_type: "base64".into(),
                media_type: Some(media_type.into()),
                data: Some(data.into()),
                url: None,
                file_id: None,
            },
        }
    }

    /// Video block from raw bytes; the bytes are base64-encoded for you.
    pub fn video_bytes(media_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self::video_base64(media_type, b64(bytes))
    }

    /// Video block referencing a remote URL.
    pub fn video_url(url: impl Into<String>) -> Self {
        ContentBlock::Video {
            source: MediaSource {
                source_type: "url".into(),
                media_type: None,
                data: None,
                url: Some(url.into()),
                file_id: None,
            },
        }
    }

    /// Build a block from raw bytes plus a known MIME type, routed by kind:
    /// images → [`ContentBlock::Image`], audio → [`ContentBlock::Audio`],
    /// video → [`ContentBlock::Video`], everything else →
    /// [`ContentBlock::Document`].
    pub fn media_bytes(media_type: impl Into<String>, bytes: &[u8]) -> Self {
        let mt = media_type.into();
        match MediaKind::from_mime(&mt) {
            MediaKind::Image => Self::image_bytes(mt, bytes),
            MediaKind::Audio => Self::audio_bytes(mt, bytes),
            MediaKind::Video => Self::video_bytes(mt, bytes),
            MediaKind::Document => Self::document_bytes(mt, bytes),
        }
    }

    /// Build a block from a local file, auto-detecting the MIME type from the
    /// file's contents (and extension as a fallback). Each file becomes an
    /// image, audio, video or document block according to its MIME type.
    ///
    /// Returns [`CerseiError::Config`] if the media type can't be determined,
    /// or [`CerseiError::Io`] if the file can't be read.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        let mime = detect_mime(&bytes, Some(path)).ok_or_else(|| {
            CerseiError::Config(format!(
                "could not determine a media type for `{}`; pass one explicitly via ContentBlock::media_bytes",
                path.display()
            ))
        })?;
        Ok(Self::media_bytes(mime, &bytes))
    }
}

impl Message {
    /// A user message combining leading text with one or more media blocks.
    pub fn user_with_media(text: impl Into<String>, media: Vec<ContentBlock>) -> Self {
        let mut blocks = Vec::with_capacity(media.len() + 1);
        let text = text.into();
        if !text.is_empty() {
            blocks.push(ContentBlock::Text { text });
        }
        blocks.extend(media);
        Message {
            role: Role::User,
            content: MessageContent::Blocks(blocks),
            id: None,
            metadata: None,
        }
    }

    /// A user message with leading text plus media loaded from local file paths.
    /// Each path is read and classified via [`ContentBlock::from_path`].
    pub fn user_with_files<P: AsRef<Path>>(text: impl Into<String>, paths: &[P]) -> Result<Self> {
        let mut media = Vec::with_capacity(paths.len());
        for p in paths {
            media.push(ContentBlock::from_path(p)?);
        }
        Ok(Self::user_with_media(text, media))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_common_formats() {
        assert_eq!(
            sniff_magic(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
            Some("image/png")
        );
        assert_eq!(sniff_magic(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(sniff_magic(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        let mut mp4 = vec![0u8; 12];
        mp4[4..8].copy_from_slice(b"ftyp");
        mp4[8..12].copy_from_slice(b"isom");
        assert_eq!(sniff_magic(&mp4), Some("video/mp4"));
    }

    #[test]
    fn extension_fallback() {
        assert_eq!(
            detect_mime(b"not-a-known-signature", Some(Path::new("a.webp"))),
            Some("image/webp".to_string())
        );
        assert_eq!(detect_mime(b"???", Some(Path::new("a.unknownext"))), None);
    }

    #[test]
    fn media_bytes_routes_by_kind() {
        assert!(matches!(
            ContentBlock::media_bytes("image/png", b"x"),
            ContentBlock::Image { .. }
        ));
        assert!(matches!(
            ContentBlock::media_bytes("video/mp4", b"x"),
            ContentBlock::Video { .. }
        ));
        assert!(matches!(
            ContentBlock::media_bytes("audio/wav", b"x"),
            ContentBlock::Audio { .. }
        ));
        assert!(matches!(
            ContentBlock::media_bytes("application/pdf", b"x"),
            ContentBlock::Document { .. }
        ));
    }

    #[test]
    fn image_bytes_base64_roundtrip() {
        let block = ContentBlock::image_bytes("image/png", b"hello");
        if let ContentBlock::Image { source } = block {
            assert_eq!(source.source_type, "base64");
            assert_eq!(source.media_type.as_deref(), Some("image/png"));
            assert_eq!(source.data.as_deref(), Some("aGVsbG8="));
        } else {
            panic!("expected image block");
        }
    }

    #[test]
    fn image_block_serializes_to_anthropic_shape() {
        // Anthropic serializes message content via direct serde, so the block's
        // JSON must already match the provider's `image`/`source` wire format.
        let block = ContentBlock::image_base64("image/png", "QUJD");
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(v["type"], "image");
        assert_eq!(v["source"]["type"], "base64");
        assert_eq!(v["source"]["media_type"], "image/png");
        assert_eq!(v["source"]["data"], "QUJD");

        let doc = ContentBlock::document_base64("application/pdf", "UERG");
        let v = serde_json::to_value(&doc).unwrap();
        assert_eq!(v["type"], "document");
        assert_eq!(v["source"]["type"], "base64");
        assert_eq!(v["source"]["media_type"], "application/pdf");
    }

    #[test]
    fn user_with_media_prepends_text() {
        let m = Message::user_with_media("hi", vec![ContentBlock::image_url("http://x/y.png")]);
        if let MessageContent::Blocks(b) = m.content {
            assert_eq!(b.len(), 2);
            assert!(matches!(b[0], ContentBlock::Text { .. }));
            assert!(matches!(b[1], ContentBlock::Image { .. }));
        } else {
            panic!("expected blocks");
        }
    }
}
