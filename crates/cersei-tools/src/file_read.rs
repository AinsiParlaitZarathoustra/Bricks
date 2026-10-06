//! File read tool.

use super::*;
use crate::tool_primitives::fs as pfs;
use serde::Deserialize;

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "Read"
    }
    fn description(&self) -> &str {
        "Read a file from the filesystem. Use offset/limit to read a slice of a large file. Read a file before you edit it."
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
                "file_path": { "type": "string", "description": "Absolute path to the file" },
                "offset": { "type": "integer", "description": "Line number to start reading from" },
                "limit": { "type": "integer", "description": "Number of lines to read" }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            file_path: String,
            offset: Option<usize>,
            limit: Option<usize>,
        }

        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };

        // Relative paths are resolved against the agent's working directory
        // (never against the shell's current directory).
        let resolved = ctx.working_dir.join(&input.file_path);
        let path = resolved.as_path();
        if !path.exists() {
            return not_found_with_siblings(path, &input.file_path);
        }
        if path.is_dir() {
            return ToolResult::error(format!(
                "{} is a directory, not a file. Use Glob or Bash `ls` to list it.",
                input.file_path
            ));
        }

        let offset = input.offset.unwrap_or(0);
        let limit = input.limit.unwrap_or(2000);
        let p = path.to_path_buf();
        let window = match tokio::task::spawn_blocking(move || {
            pfs::read_window(&p, offset, limit, MAX_LINE_CHARS)
        })
        .await
        {
            Ok(Ok(w)) => w,
            Ok(Err(e)) => return ToolResult::error(format!("Failed to read file: {e}")),
            Err(e) => return ToolResult::error(format!("Failed to read file: {e}")),
        };

        match &window.kind {
            pfs::FileKind::Binary { mime } => {
                return ToolResult::success(format!(
                    "{} is a binary file ({mime}, {} bytes); its content is not shown as text.",
                    input.file_path, window.size
                ))
                .with_metadata(serde_json::json!({ "binary": true, "mime": mime, "size": window.size }));
            }
            pfs::FileKind::Utf16 => {
                return ToolResult::error(format!(
                    "{} is text encoded in UTF-16, which Read does not decode (accepted: UTF-8, with or \
                     without a BOM). Convert it, e.g. `iconv -f UTF-16 -t UTF-8`.",
                    input.file_path
                ))
            }
            pfs::FileKind::OtherEncoding => {
                return ToolResult::error(format!(
                    "{} is not valid UTF-8 (probably Latin-1 or Windows-1252); Read accepts UTF-8 \
                     text, with or without a BOM. Convert it, e.g. `iconv -f WINDOWS-1252 -t UTF-8`.",
                    input.file_path
                ))
            }
            pfs::FileKind::Text { .. } => {}
        }

        // F-A13: the window is always stated against the exact total, so a
        // partial read never looks like a complete file.
        let mut out = pfs::number_lines(&window.lines, window.total_lines);
        if let Some(notice) = crate::tool_feedback::window_notice(
            window.offset,
            window.lines.len(),
            window.total_lines,
            "lines",
            &format!(
                "To read the rest, call Read again with offset={} (and the same limit).",
                window.offset + window.lines.len()
            ),
        ) {
            out.push_str(&notice);
            out.push('\n');
        }
        if window.invalid_lines > 0 {
            out.push_str(&format!(
                "[{} line(s) of this page contain bytes that are not valid UTF-8, shown as U+FFFD.]\n",
                window.invalid_lines
            ));
        }
        if window.clipped_lines > 0 {
            out.push_str(&format!(
                "[{} line(s) longer than {MAX_LINE_CHARS} characters were cut; use Bash (e.g. `cut -c`) to see them whole.]\n",
                window.clipped_lines
            ));
        }
        if window.changed_during_read {
            out.push_str(
                "[The file changed while it was being read: read it again before relying on it.]\n",
            );
        }
        ToolResult::success(out.trim_end_matches('\n').to_string()).with_metadata(
            serde_json::json!({
                "total_lines": window.total_lines,
                "offset": window.offset,
                "lines_returned": window.lines.len(),
                "next_offset": (window.offset + window.lines.len() < window.total_lines)
                    .then_some(window.offset + window.lines.len()),
                "crlf": window.crlf,
                "bom": matches!(window.kind, pfs::FileKind::Text { bom: true }),
            }),
        )
    }
}

/// Characters shown per line before it is cut.
const MAX_LINE_CHARS: usize = 4000;

/// "File not found" with the sibling names that do exist (F-A15).
///
/// There is no registry to enumerate here, but the parent directory is
/// derivable from the path the model sent, and a near-miss filename is the
/// single most common cause of this error.
fn not_found_with_siblings(path: &std::path::Path, requested: &str) -> ToolResult {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    let Some(parent) = parent else {
        return ToolResult::error(format!(
            "File not found: {requested}\n\nCheck the path and retry. Use Glob or LS to find the correct absolute path."
        ));
    };
    if !parent.is_dir() {
        return ToolResult::error(format!(
            "File not found: {requested}\n\nIts parent directory {} does not exist either, so the path is wrong above the filename. Use Glob to locate the file, then Read the path Glob returns.",
            parent.display()
        ));
    }

    let mut siblings: Vec<String> = std::fs::read_dir(parent)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    siblings.sort();

    let mut msg = format!("File not found: {requested}");
    let refs: Vec<&str> = siblings.iter().map(String::as_str).collect();
    if let Some(best) = crate::tool_feedback::closest(name, &refs) {
        msg.push_str(&format!(
            "\n\nDid you mean: {}?",
            parent.join(best).display()
        ));
    }
    msg.push_str(&format!(
        "\n\nIts directory {} exists and holds {} entr{}. Use Glob or LS to list it, then Read the exact path.",
        parent.display(),
        siblings.len(),
        if siblings.len() == 1 { "y" } else { "ies" }
    ));
    ToolResult::error(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use std::sync::Arc;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            session_id: "read-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    /// The measured Exp-2 regression: `{"path": …}` instead of `{"file_path": …}`.
    /// Weak models recovered 6/12 from the old message and 12/12 from this one.
    #[tokio::test]
    async fn wrong_param_name_tells_the_model_the_real_name() {
        let r = FileReadTool
            .execute(serde_json::json!({ "path": "/x.rs" }), &test_ctx())
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("'Read'"), "{}", r.content);
        assert!(r.content.contains("file_path"), "{}", r.content);
        assert!(r.content.contains("/x.rs"), "{}", r.content);
        assert!(
            !r.content.contains("struct Input"),
            "must not leak a Rust type name: {}",
            r.content
        );
    }

    /// Phase 1 hands malformed wire JSON through as `__parse_error`/`__raw`.
    #[tokio::test]
    async fn wire_parse_failure_reports_the_raw_text() {
        let r = FileReadTool
            .execute(
                serde_json::json!({
                    "__parse_error": "EOF while parsing a string at line 1 column 22",
                    "__raw": "{'file_path': '/x/y.rs",
                }),
                &test_ctx(),
            )
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("not valid JSON"), "{}", r.content);
        assert!(
            r.content.contains("{'file_path': '/x/y.rs"),
            "{}",
            r.content
        );
        assert!(r.content.contains("double quotes"), "{}", r.content);
    }

    /// F-A13: a 5,000-line file under the 2,000-line default must not look
    /// like a complete file.
    #[tokio::test]
    async fn truncated_read_is_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.txt");
        let body: String = (1..=5000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, body).unwrap();

        let r = FileReadTool
            .execute(
                serde_json::json!({ "file_path": path.to_str().unwrap() }),
                &test_ctx(),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("line 2000"), "window should reach 2000");
        assert!(!r.content.contains("line 2001"), "window must stop at 2000");
        assert!(
            r.content.contains("Showing lines 1-2000 of 5000"),
            "truncation must be visible: {}",
            r.content.lines().last().unwrap_or("")
        );
        assert!(
            r.content.contains("offset=2000"),
            "must say how to continue"
        );
    }

    #[tokio::test]
    async fn complete_read_is_not_annotated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("small.txt");
        std::fs::write(&path, "a\nb\nc\n").unwrap();

        let r = FileReadTool
            .execute(
                serde_json::json!({ "file_path": path.to_str().unwrap() }),
                &test_ctx(),
            )
            .await;
        assert!(!r.is_error);
        assert!(!r.content.contains("Showing lines"), "{}", r.content);
    }

    /// An offset past EOF used to return a blank success.
    #[tokio::test]
    async fn offset_past_eof_is_explained() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("small.txt");
        std::fs::write(&path, "a\nb\nc\n").unwrap();

        let r = FileReadTool
            .execute(
                serde_json::json!({ "file_path": path.to_str().unwrap(), "offset": 900 }),
                &test_ctx(),
            )
            .await;
        assert!(r.content.contains("No lines returned"), "{}", r.content);
        assert!(r.content.contains("only 3 lines"), "{}", r.content);
    }

    #[tokio::test]
    async fn missing_file_suggests_the_sibling_that_exists() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();

        let r = FileReadTool
            .execute(
                serde_json::json!({
                    "file_path": tmp.path().join("mian.rs").to_str().unwrap()
                }),
                &test_ctx(),
            )
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("File not found"), "{}", r.content);
        assert!(r.content.contains("main.rs"), "{}", r.content);
    }

    async fn read(path: &std::path::Path, extra: serde_json::Value) -> ToolResult {
        let mut input = serde_json::json!({ "file_path": path.to_str().unwrap() });
        if let (Some(o), Some(e)) = (input.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        FileReadTool.execute(input, &test_ctx()).await
    }

    #[tokio::test]
    async fn lines_are_numbered_from_one_and_aligned() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("n.rs");
        let body: String = (1..=12).map(|i| format!("let x{i} = {i};\n")).collect();
        std::fs::write(&path, body).unwrap();
        let r = read(&path, serde_json::json!({ "offset": 8, "limit": 2 })).await;
        assert!(
            r.content
                .starts_with(" 9 | let x9 = 9;\n10 | let x10 = 10;"),
            "{}",
            r.content
        );
        assert!(
            r.content.contains("Showing lines 9-10 of 12"),
            "{}",
            r.content
        );
        assert_eq!(r.metadata.unwrap()["next_offset"], 10);
    }

    #[tokio::test]
    async fn binary_utf16_and_latin1_are_reported_not_garbled() {
        let tmp = tempfile::tempdir().unwrap();
        let png = tmp.path().join("i.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        let r = read(&png, serde_json::json!({})).await;
        assert!(!r.is_error);
        assert!(
            r.content.contains("binary file (image/png, 16 bytes)"),
            "{}",
            r.content
        );

        let u16 = tmp.path().join("u.txt");
        let text: Vec<u8> = "hello world"
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        std::fs::write(&u16, [&[0xFF, 0xFE][..], &text].concat()).unwrap();
        assert!(read(&u16, serde_json::json!({}))
            .await
            .content
            .contains("UTF-16"));
        std::fs::write(&u16, &text).unwrap();
        assert!(
            read(&u16, serde_json::json!({}))
                .await
                .content
                .contains("UTF-16"),
            "UTF-16 without BOM"
        );

        let latin = tmp.path().join("l.txt");
        std::fs::write(&latin, b"caf\xe9 cr\xe8me\n").unwrap();
        let r = read(&latin, serde_json::json!({})).await;
        assert!(
            r.is_error && r.content.contains("not valid UTF-8"),
            "{}",
            r.content
        );
    }

    #[tokio::test]
    async fn bom_crlf_and_unicode_text() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("b.txt");
        std::fs::write(&p, "\u{feff}première ligne\r\n日本語\r\n").unwrap();
        let r = read(&p, serde_json::json!({})).await;
        assert_eq!(r.content, "1 | première ligne\n2 | 日本語");
        let meta = r.metadata.unwrap();
        assert_eq!(
            (meta["bom"].clone(), meta["crlf"].clone()),
            (serde_json::json!(true), serde_json::json!(true))
        );
    }

    #[tokio::test]
    async fn empty_files_and_empty_pages() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("e.txt");
        std::fs::write(&p, "").unwrap();
        assert!(read(&p, serde_json::json!({}))
            .await
            .content
            .contains("This file is empty"));
    }

    #[tokio::test]
    async fn a_massive_file_is_paged_with_an_exact_total() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("big.log");
        {
            use std::io::Write;
            let mut f = std::io::BufWriter::new(std::fs::File::create(&p).unwrap());
            for i in 0..300_000 {
                writeln!(f, "entry {i}").unwrap();
            }
        }
        let r = read(&p, serde_json::json!({ "offset": 299_998, "limit": 10 })).await;
        assert!(r.content.contains("299999 | entry 299998"), "{}", r.content);
        assert!(r.content.contains("300000 | entry 299999"));
        assert_eq!(r.metadata.unwrap()["total_lines"], 300_000);
    }

    #[tokio::test]
    async fn relative_paths_use_the_working_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("rel.txt"), "x\n").unwrap();
        let mut ctx = test_ctx();
        ctx.working_dir = tmp.path().to_path_buf();
        let r = FileReadTool
            .execute(serde_json::json!({ "file_path": "rel.txt" }), &ctx)
            .await;
        assert_eq!(r.content, "1 | x");
    }
}
