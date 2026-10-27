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
use bricks_types::{ContentBlock, ImageSource};
use std::io::Read;
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

/// Opens `abs` to read its content and returns the handle with the size the
/// opened file has. The type is checked first: a folder is refused with its
/// own message, and so is anything that is not a regular file (a pipe would
/// block the read). The check is repeated on the handle that is read.
fn open_regular_file(abs: &Path, path: &str) -> Result<(std::fs::File, u64), String> {
    let describe = |e: std::io::Error| {
        format!(
            "cannot attach `{path}`: {e} (resolved to {})",
            abs.display()
        )
    };
    let meta = std::fs::metadata(abs).map_err(describe)?;
    if meta.is_dir() {
        return Err(format!("`{path}` is a folder: attach it as a folder"));
    }
    if !meta.is_file() {
        return Err(format!("`{path}` is not a regular file"));
    }
    let file = std::fs::File::open(abs).map_err(describe)?;
    let opened = file.metadata().map_err(describe)?;
    if !opened.is_file() {
        return Err(format!("`{path}` is not a regular file"));
    }
    Ok((file, opened.len()))
}

/// Reads at most `limit` bytes from the opened file, never more.
fn read_at_most(file: std::fs::File, limit: u64, path: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read `{path}`: {e}"))?;
    Ok(bytes)
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
                let (file, size) = open_regular_file(&abs, path)?;
                let budget = limits
                    .max_file_bytes
                    .min(limits.max_total_bytes.saturating_sub(total));
                // One byte past the budget tells that the file is cut: the rest
                // of a large file is never read.
                let read = read_at_most(file, budget + 1, path)?;
                let cut = size > budget || read.len() as u64 > budget;
                let kept = if cut {
                    read.len().min(budget as usize)
                } else {
                    read.len()
                };
                let shown = match std::str::from_utf8(&read[..kept]) {
                    Ok(text) => text,
                    // A cut may split a character: the whole characters are kept.
                    Err(e) if cut && e.error_len().is_none() => {
                        std::str::from_utf8(&read[..e.valid_up_to()]).unwrap_or_default()
                    }
                    Err(_) => {
                        return Err(format!(
                            "`{path}` is not UTF-8 text; attach images as images, or let the agent read it"
                        ));
                    }
                };
                // The digest covers the captured text, as the history keeps it.
                let sha = bricks_tools::preview::sha256_hex(shown.as_bytes());
                let note = cut.then(|| {
                    format!(
                        "first {} of {size} bytes attached (limit); the agent can read the rest",
                        shown.len()
                    )
                });
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
                let (file, size) = open_regular_file(&abs, path)?;
                if size > limits.max_image_bytes {
                    return Err(format!(
                        "image `{path}` is {size} bytes; the limit is {}",
                        limits.max_image_bytes
                    ));
                }
                let bytes = read_at_most(file, limits.max_image_bytes + 1, path)?;
                if bytes.len() as u64 > limits.max_image_bytes {
                    return Err(format!(
                        "image `{path}` grew past the limit of {} bytes while it was read",
                        limits.max_image_bytes
                    ));
                }
                let sha = bricks_tools::preview::sha256_hex(&bytes);
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
            skills: prompt.skills.clone(),
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
            ask_timer: None,
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
            skills: Vec::new(),
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
            ask_timer: None,
            blocks: vec![PromptBlock::File {
                path: "missing.rs".into(),
            }],
            skills: Vec::new(),
        };
        assert!(
            convert(&gone, dir.path(), limits)
                .unwrap_err()
                .contains("missing.rs")
        );
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
                    ask_timer: None,
                    blocks: vec![
                        PromptBlock::Text {
                            text: "vois".into(),
                        },
                        PromptBlock::Image {
                            path: "capture écran.png".into(),
                        },
                    ],
                    skills: Vec::new(),
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
            ask_timer: None,
            blocks: vec![PromptBlock::Image {
                path: "a b.rs".into(),
            }],
            skills: Vec::new(),
        };
        assert!(
            convert(&image, dir.path(), limits)
                .unwrap_err()
                .contains("unsupported image")
        );
    }

    #[test]
    fn a_large_file_is_read_only_up_to_its_budget() {
        let dir = tempfile::tempdir().unwrap();
        // 3 MB of two-byte characters: the budget is read, and the file is not.
        std::fs::write(dir.path().join("huge.txt"), "é".repeat(1_500_000)).unwrap();
        let prompt = Prompt {
            ask_timer: None,
            blocks: vec![PromptBlock::File {
                path: "huge.txt".into(),
            }],
            skills: Vec::new(),
        };
        let c = convert(&prompt, dir.path(), AttachLimits::default()).unwrap();
        let note = c.attachments[0].note.as_deref().expect("cut note");
        assert!(note.starts_with("first 262144 of 3000000 bytes"), "{note}");
        assert_eq!(c.attachments[0].bytes, 3_000_000, "the size is the file's");
        let ContentBlock::Text { text } = &c.input.attachments[0] else {
            panic!()
        };
        assert!(text.contains("truncated=\"true\""));
        assert!(text.len() < 262_400, "only the budget is sent");
    }

    #[test]
    fn a_folder_or_a_missing_file_is_refused_by_its_type_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let as_file = Prompt {
            ask_timer: None,
            blocks: vec![PromptBlock::File { path: "src".into() }],
            skills: Vec::new(),
        };
        let err = convert(&as_file, dir.path(), AttachLimits::default()).unwrap_err();
        assert!(err.contains("is a folder"), "{err}");
        let missing = Prompt {
            ask_timer: None,
            blocks: vec![PromptBlock::File {
                path: "nope.txt".into(),
            }],
            skills: Vec::new(),
        };
        let err = convert(&missing, dir.path(), AttachLimits::default()).unwrap_err();
        assert!(err.contains("nope.txt"), "{err}");
    }
}
