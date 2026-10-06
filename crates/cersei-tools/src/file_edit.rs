//! File edit tool: tolerant string replacement (see [`crate::tool_primitives::replace`]).

use super::*;
use crate::tool_primitives::fs as pfs;

pub struct FileEditTool;

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "Edit"
    }
    fn description(&self) -> &str {
        "Replace a string in a file. Prefers an exact match of old_string but \
         tolerates leading/trailing whitespace and indentation differences. \
         old_string must uniquely identify the target unless replace_all is set."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::FileSystem
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path to the file" },
                "old_string": { "type": "string", "description": "The text to replace" },
                "new_string": { "type": "string", "description": "The replacement text" },
                "replace_all": { "type": "boolean", "description": "Replace all occurrences", "default": false }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    async fn preview(
        &self,
        input: &Value,
        _ctx: &ToolContext,
    ) -> Option<crate::preview::ChangePreview> {
        let input = match coerce_input(input) {
            Ok(i) => i,
            Err(e) => return Some(crate::preview::ChangePreview::refused(e)),
        };
        let path = std::path::Path::new(&input.file_path);
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let before = match std::fs::read_to_string(&absolute) {
            Ok(c) => c,
            Err(e) => {
                return Some(crate::preview::ChangePreview::refused(format!(
                    "Failed to read {}: {e}",
                    input.file_path
                )))
            }
        };
        match crate::tool_primitives::replace::plan_edit(
            &before,
            &input.old_string,
            &input.new_string,
            input.replace_all,
            Some(&input.file_path),
        ) {
            Ok(plan) => Some(crate::preview::ChangePreview {
                files: vec![crate::preview::file_change(
                    &input.file_path,
                    &absolute,
                    Some(&before),
                    Some(&plan.content),
                )],
                refusal: None,
            }),
            Err(e) => Some(crate::preview::ChangePreview::refused(
                crate::tool_primitives::replace::describe_failure(&e, &input.file_path),
            )),
        }
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let input = match coerce_input(&input) {
            Ok(i) => i,
            // The coercion above is already alias-tolerant, so reaching here
            // means the call is genuinely unusable. Hand the reason to the
            // shared builder so the model gets the tool name, an echo of what
            // it sent, and the parameter list (F-05b/F-A14).
            Err(e) => return crate::tool_feedback::invalid_input(self, &input, e),
        };

        let path = std::path::Path::new(&input.file_path);
        let before_content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to read {}: {e}", input.file_path)),
        };

        match pfs::edit_file(
            path,
            &input.old_string,
            &input.new_string,
            input.replace_all,
        )
        .await
        {
            Ok(result) => {
                let after_content = tokio::fs::read_to_string(path).await.unwrap_or_default();
                let diff =
                    crate::tool_primitives::diff::unified_diff(&before_content, &after_content, 2);
                let diff_preview = if diff.lines().count() > 30 {
                    let truncated: String = diff.lines().take(25).collect::<Vec<_>>().join("\n");
                    format!(
                        "{}\n... ({} more lines)",
                        truncated,
                        diff.lines().count() - 25
                    )
                } else {
                    diff
                };
                let ranges = result
                    .ranges
                    .iter()
                    .map(|(a, b)| {
                        if a == b {
                            format!("{a}")
                        } else {
                            format!("{a}–{b}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                ToolResult::success(format!(
                    "The file {} has been updated: {} replacement(s) at line(s) {} ({}).\n{}",
                    input.file_path,
                    result.replacements_made,
                    ranges,
                    result.stage.describe(),
                    diff_preview
                ))
            }
            Err(pfs::EditError::Refused(e)) => ToolResult::error(format!(
                "{}\nNothing was written.",
                crate::tool_primitives::replace::describe_failure(&e, &input.file_path)
            )),
            Err(pfs::EditError::Changed) => ToolResult::error(format!(
                "{} changed on disk while the edit was being prepared, so nothing was written. \
                 Read it again, then retry the edit against its current text.",
                input.file_path
            )),
            Err(pfs::EditError::Io(e)) => {
                ToolResult::error(format!("Failed to edit file {}: {}", input.file_path, e))
            }
        }
    }
}

/// Parsed and coerced edit request.
struct EditInput {
    file_path: String,
    old_string: String,
    new_string: String,
    replace_all: bool,
}

/// The parameters `Edit` declares. Anything else is refused.
const KNOWN_PARAMS: &[&str] = &["file_path", "old_string", "new_string", "replace_all"];

/// Parse a tool call into a valid [`EditInput`], coercing scalar *types* but not
/// parameter *names*.
///
/// This used to accept `path`, `filePath`, `oldString`, `old`, `search` and
/// friends. That leniency was removed: `Edit` was the only tool that rewarded
/// guessing `path`, and a model that had just been rewarded carried the guess
/// to `Grep`, where `path` is a real parameter meaning something else and the
/// unknown key was dropped without a word — turning a naming mistake into a
/// silent whole-directory search. Being the one lenient tool in a strict
/// runtime taught a schema no other tool honoured.
///
/// Type coercion stays. A model that sends `replace_all: "true"` has the name
/// right and only the JSON type wrong, which is unambiguous to repair and
/// teaches nothing false.
fn coerce_input(input: &Value) -> std::result::Result<EditInput, String> {
    let obj = input
        .as_object()
        .ok_or_else(|| "the arguments must be a JSON object".to_string())?;

    crate::tool_feedback::reject_unknown_keys(input, KNOWN_PARAMS)?;

    // Pull a string field, coercing numbers and bools to their string form.
    let get_str = |key: &str| -> Option<String> {
        match obj.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            Some(Value::Bool(b)) => Some(b.to_string()),
            _ => None,
        }
    };

    let file_path = get_str("file_path")
        .ok_or_else(|| "missing 'file_path' (the absolute path of the file to edit)".to_string())?;

    let old_string = get_str("old_string")
        .ok_or_else(|| "missing 'old_string' (the exact existing text to replace)".to_string())?;

    // new_string may legitimately be an empty string (a deletion); treat a
    // missing field as empty so deletions don't fail on omission.
    let new_string = get_str("new_string").unwrap_or_default();

    let replace_all = match obj.get("replace_all") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => {
            matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes")
        }
        Some(Value::Number(n)) => n.as_i64().map(|v| v != 0).unwrap_or(false),
        _ => false,
    };

    Ok(EditInput {
        file_path,
        old_string,
        new_string,
        replace_all,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use std::sync::Arc;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            session_id: "edit-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    #[tokio::test]
    async fn tolerant_edit_survives_indentation_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.rs");
        // File is indented; the model supplies old_string without indentation.
        std::fs::write(&path, "fn main() {\n        let x = 1;\n}\n").unwrap();

        let tool = FileEditTool;
        let res = tool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "let x = 1;",
                    "new_string": "let x = 2;"
                }),
                &test_ctx(),
            )
            .await;

        assert!(!res.is_error, "expected success, got: {:?}", res.content);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "fn main() {\n        let x = 2;\n}\n");
    }

    #[tokio::test]
    /// Aliased names are refused rather than silently accepted.
    ///
    /// This test previously asserted the opposite. `Edit` was the only tool
    /// that accepted `path`/`old`/`new`, and rewarding the guess here is what
    /// led models to reuse `path` on `Grep` and `Glob`, where the key was
    /// dropped and the search quietly widened to the whole working directory.
    /// The one-tool exception cost more elsewhere than it saved here, and the
    /// refusal is cheap to recover from because it names the real parameter.
    async fn rejects_aliased_field_names_and_names_the_real_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "hello world").unwrap();

        let tool = FileEditTool;
        // Model emitted `path`/`old`/`new` instead of the canonical names.
        let res = tool
            .execute(
                serde_json::json!({
                    "path": path.to_str().unwrap(),
                    "old": "world",
                    "new": "there"
                }),
                &test_ctx(),
            )
            .await;

        assert!(res.is_error, "expected refusal, got: {:?}", res.content);
        assert!(
            res.content.contains("file_path"),
            "must name the real parameter: {}",
            res.content
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "hello world",
            "a refused edit must not have touched the file"
        );
    }

    #[tokio::test]
    async fn coerces_stringified_replace_all() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "a a a").unwrap();

        let tool = FileEditTool;
        let res = tool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "a",
                    "new_string": "b",
                    "replace_all": "true"
                }),
                &test_ctx(),
            )
            .await;

        assert!(!res.is_error, "expected success, got: {:?}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b b b");
    }

    #[tokio::test]
    async fn ambiguous_match_gives_corrective_message() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "a a a").unwrap();

        let tool = FileEditTool;
        let res = tool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "a",
                    "new_string": "b"
                }),
                &test_ctx(),
            )
            .await;

        assert!(res.is_error);
        let msg = res.content;
        assert!(msg.contains("ambiguous"), "{msg}");
        assert!(msg.contains("matches 3 regions"), "{msg}");
        assert!(msg.contains("replace_all"));
        assert!(msg.contains("Nothing was written"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a a a");
    }

    #[tokio::test]
    async fn a_near_miss_is_refused_with_candidates_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("calc.rs");
        let original = "fn calc() {\n    let a = 1;\n    let b = 2;\n    a + b\n}\n";
        std::fs::write(&path, original).unwrap();
        let res = FileEditTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "    let a = 1;\n    let b = 3;\n",
                    "new_string": "    let a = 10;\n",
                }),
                &test_ctx(),
            )
            .await;
        assert!(res.is_error);
        assert!(res.content.contains("lines 2–3"), "{}", res.content);
        assert!(
            res.content.contains("code itself differs"),
            "{}",
            res.content
        );
        assert!(res.content.contains("let b = 2;"), "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[tokio::test]
    async fn crlf_python_edit_keeps_style() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("m.py");
        std::fs::write(
            &path,
            "class A:\r\n    def f(self):\r\n        return 1\r\n",
        )
        .unwrap();
        let res = FileEditTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "def f(self):\n    return 1\n",
                    "new_string": "def f(self):\n    if x:\n        return 2\n    return 1\n",
                }),
                &test_ctx(),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert!(res.content.contains("normalised match"), "{}", res.content);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "class A:\r\n    def f(self):\r\n        if x:\r\n            return 2\r\n        return 1\r\n"
        );
    }
}
