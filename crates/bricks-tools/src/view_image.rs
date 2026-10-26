//! ViewImage: shows the model an image file (PNG, JPEG, GIF, WebP).
//!
//! The file is read and its type found from its bytes, never from its name.
//! The result's text names the image; the image itself travels in
//! [`ToolResult::images`], and the agent carries it as the model's protocol
//! allows (in the tool result, next to it, or not at all with a note).

use super::*;
use base64::Engine as _;
use serde::Deserialize;

/// Largest image read, in bytes: its base64 form stays under the 5 MB most
/// providers accept for one image.
pub const MAX_IMAGE_BYTES: usize = 3_750_000;
/// Longest side accepted, in pixels, when the format says it.
pub const MAX_IMAGE_SIDE: u32 = 8_000;

pub struct ViewImageTool;

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &str {
        "ViewImage"
    }
    fn description(&self) -> &str {
        "Look at an image file (PNG, JPEG, GIF or WebP, up to 3.75 MB): you see the image \
         itself, not a description. Use it for screenshots, diagrams or pictures on disk; \
         use Read for text."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::FileSystem
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to the image file" }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            file_path: String,
        }

        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        // Relative paths are resolved against the agent's working directory.
        let path = ctx.working_dir.join(&input.file_path);
        if path.is_dir() {
            return ToolResult::error(format!(
                "{} is a directory, not an image. Use Glob to find the image files in it.",
                input.file_path
            ));
        }
        let read = path.clone();
        let bytes = match tokio::task::spawn_blocking(move || read_limited(&read)).await {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => return ToolResult::error(format!("{}: {e}", input.file_path)),
            Err(e) => return ToolResult::error(format!("{}: {e}", input.file_path)),
        };
        let Some(media_type) = image_type(&bytes) else {
            return ToolResult::error(format!(
                "{} is not a PNG, JPEG, GIF or WebP image (its bytes say so, whatever its name). \
                 Use Read for text files.",
                input.file_path
            ));
        };
        let size = image_size(&bytes, media_type);
        if let Some((w, h)) = size
            && w.max(h) > MAX_IMAGE_SIDE
        {
            return ToolResult::error(format!(
                "{} is {w}×{h} pixels; images over {MAX_IMAGE_SIDE} pixels on a side are not \
                 accepted. Make a smaller copy first (e.g. `sips -Z 2000 <file> --out <copy>`).",
                input.file_path
            ));
        }
        let shown = path.display().to_string();
        let dims = size.map(|(w, h)| format!(", {w}×{h}")).unwrap_or_default();
        let text = format!("Image {shown}: {media_type}{dims}, {} bytes.", bytes.len());
        let image = ToolImage {
            media_type: media_type.to_string(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            path: Some(shown),
            bytes: bytes.len(),
        };
        ToolResult::success(text)
            .with_metadata(serde_json::json!({ "media_type": media_type, "size": image.bytes }))
            .with_images(vec![image])
    }
}

/// The file's bytes, refused past [`MAX_IMAGE_BYTES`] (only one byte more is read).
fn read_limited(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(std::io::Error::other(format!(
            "larger than {MAX_IMAGE_BYTES} bytes; make a smaller copy first \
             (e.g. `sips -Z 2000 <file> --out <copy>`)"
        )));
    }
    Ok(bytes)
}

/// The image type from the file's first bytes.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Width and height, when the header gives them simply (PNG, GIF, JPEG).
pub fn image_size(bytes: &[u8], media_type: &str) -> Option<(u32, u32)> {
    let be = |b: &[u8]| u32::from(b[0]) << 8 | u32::from(b[1]);
    match media_type {
        "image/png" if bytes.len() >= 24 => {
            let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
            let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
            Some((w, h))
        }
        "image/gif" if bytes.len() >= 10 => Some((
            u32::from(u16::from_le_bytes([bytes[6], bytes[7]])),
            u32::from(u16::from_le_bytes([bytes[8], bytes[9]])),
        )),
        "image/jpeg" => {
            // Walk the segments up to a start-of-frame marker.
            let mut i = 2;
            while i + 9 < bytes.len() {
                if bytes[i] != 0xFF {
                    return None;
                }
                let marker = bytes[i + 1];
                let len = be(&bytes[i + 2..i + 4]) as usize;
                let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
                if sof {
                    return Some((be(&bytes[i + 7..i + 9]), be(&bytes[i + 5..i + 7])));
                }
                i += 2 + len;
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use crate::{CostTracker, Extensions};

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "view-image-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    #[tokio::test]
    async fn an_image_is_named_in_text_and_carried_as_an_image() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shot.png"), png(640, 480)).unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "shot.png" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("image/png, 640×480"), "{}", r.content);
        assert_eq!(r.images.len(), 1);
        assert_eq!(r.images[0].media_type, "image/png");
        assert_eq!(r.images[0].bytes, png(640, 480).len());
    }

    #[tokio::test]
    async fn text_named_like_an_image_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fake.png"), "not an image").unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "fake.png" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(r.is_error);
        assert!(r.images.is_empty());
    }

    #[tokio::test]
    async fn too_large_or_too_wide_images_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("wide.png"), png(9_000, 10)).unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "wide.png" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(r.is_error && r.content.contains("9000×10"), "{}", r.content);
        let mut big = png(10, 10);
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        std::fs::write(dir.path().join("big.png"), big).unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "big.png" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(
            r.is_error && r.content.contains("larger than"),
            "{}",
            r.content
        );
    }

    fn webp() -> Vec<u8> {
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&26u32.to_le_bytes());
        b.extend_from_slice(b"WEBPVP8 ");
        b.extend_from_slice(&[0; 18]);
        b
    }

    #[tokio::test]
    async fn a_relative_path_is_read_from_the_working_dir_and_named_with_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/diagram.png"), png(4, 5)).unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "sub/diagram.png" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("4×5"), "{}", r.content);
        let expected = dir.path().join("sub/diagram.png").display().to_string();
        assert_eq!(r.images[0].path.as_deref(), Some(expected.as_str()));
    }

    #[tokio::test]
    async fn an_absolute_path_is_read_as_given_whatever_the_working_dir() {
        let image_dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let abs = image_dir.path().join("abs.png");
        std::fs::write(&abs, png(2, 3)).unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": abs.display().to_string() }),
                &ctx(other.path()),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.images.len(), 1);
        assert_eq!(
            r.images[0].path.as_deref(),
            Some(abs.display().to_string().as_str())
        );
    }

    #[tokio::test]
    async fn a_directory_is_refused_with_a_hint_to_glob_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("pics")).unwrap();
        let r = ViewImageTool
            .execute(serde_json::json!({ "file_path": "pics" }), &ctx(dir.path()))
            .await;
        assert!(r.is_error);
        assert!(
            r.content.contains("is a directory") && r.content.contains("Glob"),
            "{}",
            r.content
        );
        assert!(r.images.is_empty());
    }

    #[tokio::test]
    async fn a_missing_file_is_an_error_naming_it_and_shows_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "nope.png" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(r.is_error);
        assert!(r.content.starts_with("nope.png:"), "{}", r.content);
        assert!(r.images.is_empty());
    }

    #[tokio::test]
    async fn a_webp_is_carried_with_its_type_and_no_invented_size() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = webp();
        std::fs::write(dir.path().join("pic.webp"), &bytes).unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "pic.webp" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(
            r.content.contains("image/webp") && !r.content.contains('×'),
            "{}",
            r.content
        );
        assert_eq!(r.images.len(), 1);
        assert_eq!(r.images[0].media_type, "image/webp");
        assert_eq!(r.images[0].bytes, bytes.len());
    }

    #[tokio::test]
    async fn the_carried_bytes_are_the_file_bytes_in_standard_base64() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = png(7, 9);
        std::fs::write(dir.path().join("exact.png"), &bytes).unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "exact.png" }),
                &ctx(dir.path()),
            )
            .await;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&r.images[0].data_base64)
            .unwrap();
        assert_eq!(decoded, bytes);
    }

    #[tokio::test]
    async fn a_file_of_exactly_the_size_limit_is_accepted_and_one_byte_more_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut exact = png(10, 10);
        exact.resize(MAX_IMAGE_BYTES, 0);
        std::fs::write(dir.path().join("edge.png"), &exact).unwrap();
        let r = ViewImageTool
            .execute(
                serde_json::json!({ "file_path": "edge.png" }),
                &ctx(dir.path()),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.images[0].bytes, MAX_IMAGE_BYTES);
    }

    #[tokio::test]
    async fn unknown_or_missing_arguments_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shot.png"), png(1, 1)).unwrap();
        for input in [
            serde_json::json!({ "file_path": "shot.png", "width": 3 }),
            serde_json::json!({}),
        ] {
            let r = ViewImageTool.execute(input, &ctx(dir.path())).await;
            assert!(r.is_error, "{}", r.content);
            assert!(r.images.is_empty());
        }
    }

    #[test]
    fn view_image_only_reads_and_belongs_to_the_filesystem_set() {
        assert_eq!(ViewImageTool.permission_level(), PermissionLevel::ReadOnly);
        assert_eq!(ViewImageTool.category(), ToolCategory::FileSystem);
        assert!(crate::filesystem().iter().any(|t| t.name() == "ViewImage"));
    }

    #[test]
    fn types_and_sizes_come_from_the_bytes() {
        assert_eq!(image_type(b"GIF89a\x10\x00\x20\x00"), Some("image/gif"));
        assert_eq!(
            image_size(b"GIF89a\x10\x00\x20\x00", "image/gif"),
            Some((16, 32))
        );
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01,
            0xE0, 0x02, 0x80, 0x03,
        ];
        assert_eq!(image_type(&jpeg), Some("image/jpeg"));
        assert_eq!(image_size(&jpeg, "image/jpeg"), Some((640, 480)));
        assert_eq!(image_type(b"hello"), None);
    }
}
