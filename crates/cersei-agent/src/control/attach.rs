//! Prompt blocks → the engine's input, at submission.
//!
//! * A file is read **now**: its text goes into the message with its path,
//!   size and SHA-256, so the session keeps exactly what was sent — a later
//!   edit of the file never changes the history.
//! * A folder becomes a bounded listing (respecting `.gitignore`), never its
//!   recursive contents: the model reads what it needs with its tools.
//! * An image is read and sent as an image block. Whether the model accepts
//!   images is decided by the provider before sending, from the model's
//!   declared capabilities.
//!
//! Paths are relative to the working directory. A path that no longer
//! exists is an error: nothing is sent and the submission is refused.

use super::protocol::{AttachmentInfo, Prompt, PromptBlock};
use crate::UserInput;
use base64::Engine as _;
use cersei_types::{ContentBlock, ImageSource};
use std::path::{Path, PathBuf};

/// Bounds of what one submission attaches.
#[derive(Debug, Clone, Copy)]
pub struct AttachLimits {
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_image_bytes: u64,
    pub max_folder_entries: usize,
    pub max_folder_depth: usize,
}

impl Default for AttachLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 256 * 1024,
            max_total_bytes: 1024 * 1024,
            max_image_bytes: 5 * 1024 * 1024,
            max_folder_entries: 300,
            max_folder_depth: 4,
        }
    }
}

#[derive(Debug)]
pub struct Converted {
    pub input: UserInput,
    pub attachments: Vec<AttachmentInfo>,
}

fn resolve(working_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        working_dir.join(p)
    }
}

fn image_media_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Convert a prompt. Errors name the block that could not be attached.
pub fn convert(
    prompt: &Prompt,
    working_dir: &Path,
    limits: AttachLimits,
) -> Result<Converted, String> {
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut infos = Vec::new();
    let mut total: u64 = 0;
    for block in &prompt.blocks {
        match block {
            PromptBlock::Text { text: t } => {
                if !text.is_empty() && !t.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            PromptBlock::File { path } => {
                let abs = resolve(working_dir, path);
                let bytes = std::fs::read(&abs).map_err(|e| {
                    format!(
                        "cannot attach `{path}`: {e} (resolved to {})",
                        abs.display()
                    )
                })?;
                if std::fs::metadata(&abs).map(|m| m.is_dir()).unwrap_or(false) {
                    return Err(format!("`{path}` is a folder: attach it as a folder"));
                }
                let sha = cersei_tools::preview::sha256_hex(&bytes);
                let size = bytes.len() as u64;
                let content = String::from_utf8(bytes).map_err(|_| {
                    format!("`{path}` is not UTF-8 text; attach images as images, or let the agent read it")
                })?;
                let mut note = None;
                let mut shown = content.as_str();
                let budget = limits
                    .max_file_bytes
                    .min(limits.max_total_bytes.saturating_sub(total));
                if size > budget {
                    let mut cut = budget as usize;
                    while cut > 0 && !content.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    shown = &content[..cut];
                    note = Some(format!(
                        "first {cut} of {size} bytes attached (limit); the agent can read the rest"
                    ));
                }
                total += shown.len() as u64;
                blocks.push(ContentBlock::Text {
                    text: format!(
                        "<attached_file path=\"{path}\" bytes=\"{size}\" sha256=\"{sha}\"{}>\n{shown}\n</attached_file>",
                        if note.is_some() { " truncated=\"true\"" } else { "" }
                    ),
                });
                infos.push(AttachmentInfo {
                    kind: "file".into(),
                    path: path.clone(),
                    bytes: size,
                    sha256: Some(sha),
                    note,
                });
            }
            PromptBlock::Folder { path } => {
                let abs = resolve(working_dir, path);
                if !abs.is_dir() {
                    return Err(format!(
                        "cannot attach folder `{path}`: not a folder ({})",
                        abs.display()
                    ));
                }
                let mut entries = Vec::new();
                let mut more = 0usize;
                let walker = ignore::WalkBuilder::new(&abs)
                    .max_depth(Some(limits.max_folder_depth))
                    .sort_by_file_path(|a, b| a.cmp(b))
                    .build();
                for e in walker.flatten() {
                    if e.depth() == 0 {
                        continue;
                    }
                    if entries.len() >= limits.max_folder_entries {
                        more += 1;
                        continue;
                    }
                    let rel = e.path().strip_prefix(&abs).unwrap_or(e.path());
                    let dir = e.file_type().is_some_and(|t| t.is_dir());
                    entries.push(format!("{}{}", rel.display(), if dir { "/" } else { "" }));
                }
                let note = (more > 0).then(|| format!("{more} more entries not listed (limit)"));
                let listing = entries.join("\n");
                total += listing.len() as u64;
                blocks.push(ContentBlock::Text {
                    text: format!(
                        "<attached_folder path=\"{path}\" entries=\"{}\"{}>\n{listing}\n</attached_folder>",
                        entries.len(),
                        note.as_ref().map(|n| format!(" note=\"{n}\"")).unwrap_or_default()
                    ),
                });
                infos.push(AttachmentInfo {
                    kind: "folder".into(),
                    path: path.clone(),
                    bytes: listing.len() as u64,
                    sha256: None,
                    note,
                });
            }
            PromptBlock::Image { path } => {
                let abs = resolve(working_dir, path);
                let media = image_media_type(&abs).ok_or_else(|| {
                    format!("`{path}`: unsupported image type (png, jpeg, gif or webp)")
                })?;
                let meta = std::fs::metadata(&abs)
                    .map_err(|e| format!("cannot attach image `{path}`: {e}"))?;
                if meta.len() > limits.max_image_bytes {
                    return Err(format!(
                        "image `{path}` is {} bytes; the limit is {}",
                        meta.len(),
                        limits.max_image_bytes
                    ));
                }
                let bytes = std::fs::read(&abs)
                    .map_err(|e| format!("cannot attach image `{path}`: {e}"))?;
                let sha = cersei_tools::preview::sha256_hex(&bytes);
                blocks.push(ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".into(),
                        media_type: Some(media.into()),
                        data: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
                        url: None,
                        file_id: None,
                    },
                });
                infos.push(AttachmentInfo {
                    kind: "image".into(),
                    path: path.clone(),
                    bytes: bytes.len() as u64,
                    sha256: Some(sha),
                    note: None,
                });
            }
        }
    }
    if text.trim().is_empty() && blocks.is_empty() {
        return Err("the prompt is empty".into());
    }
    Ok(Converted {
        input: UserInput {
            text,
            attachments: blocks,
        },
        attachments: infos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_captured_at_submission_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a b.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("big.txt"), "é".repeat(1000)).unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(dir.path().join("src/deep/x.rs"), "").unwrap();
        let prompt = Prompt {
            blocks: vec![
                PromptBlock::Text {
                    text: "Regarde".into(),
                },
                PromptBlock::File {
                    path: "a b.rs".into(),
                },
                PromptBlock::File {
                    path: "big.txt".into(),
                },
                PromptBlock::Folder { path: "src".into() },
            ],
        };
        let limits = AttachLimits {
            max_file_bytes: 101,
            ..Default::default()
        };
        let c = convert(&prompt, dir.path(), limits).unwrap();
        assert_eq!(c.input.text, "Regarde");
        assert_eq!(c.input.attachments.len(), 3);
        let ContentBlock::Text { text } = &c.input.attachments[0] else {
            panic!()
        };
        assert!(
            text.contains("path=\"a b.rs\"") && text.contains("fn main()"),
            "{text}"
        );
        // Captured: changing the file afterwards does not change what was sent.
        std::fs::write(dir.path().join("a b.rs"), "changed").unwrap();
        assert!(text.contains("fn main()"));
        assert!(
            c.attachments[1]
                .note
                .as_deref()
                .unwrap()
                .contains("first 100"),
            "cut on a char boundary"
        );
        let ContentBlock::Text { text } = &c.input.attachments[2] else {
            panic!()
        };
        assert!(
            text.contains("deep/") && text.contains("deep/x.rs"),
            "{text}"
        );

        let gone = Prompt {
            blocks: vec![PromptBlock::File {
                path: "missing.rs".into(),
            }],
        };
        assert!(convert(&gone, dir.path(), limits)
            .unwrap_err()
            .contains("missing.rs"));
        assert!(convert(&Prompt::text("  "), dir.path(), limits).is_err());
        // A real PNG (1×1) becomes an image block, with its digest.
        let png: [u8; 67] = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(dir.path().join("capture écran.png"), png).unwrap();
        let shot = |limits| {
            convert(
                &Prompt {
                    blocks: vec![
                        PromptBlock::Text {
                            text: "vois".into(),
                        },
                        PromptBlock::Image {
                            path: "capture écran.png".into(),
                        },
                    ],
                },
                dir.path(),
                limits,
            )
        };
        let c = shot(limits).unwrap();
        let ContentBlock::Image { source } = &c.input.attachments[0] else {
            panic!()
        };
        assert_eq!(source.media_type.as_deref(), Some("image/png"));
        assert_eq!(c.attachments[0].bytes, 67);
        assert!(c.attachments[0].sha256.is_some());
        let too_big = AttachLimits {
            max_image_bytes: 10,
            ..limits
        };
        assert!(shot(too_big).unwrap_err().contains("limit"));
        let image = Prompt {
            blocks: vec![PromptBlock::Image {
                path: "a b.rs".into(),
            }],
        };
        assert!(convert(&image, dir.path(), limits)
            .unwrap_err()
            .contains("unsupported image"));
    }
}
