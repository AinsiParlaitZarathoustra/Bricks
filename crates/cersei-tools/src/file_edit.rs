//! Edit tool: tolerant string replacement (see [`crate::tool_primitives::replace`]).
//!
//! A call makes one write. It is either one replacement (`old_string`,
//! `new_string`, `replace_all`) or several (`edits`), applied in order in
//! memory, each against the result of the previous one. The file is written
//! only when every replacement applies. The same preparation produces the
//! preview shown for an approval and the text the execution writes, and the
//! execution checks the approval again under the file's lock before writing.

use super::*;
use crate::file_lock;
use crate::preview::{ChangePreview, ensure_still_approved, file_change};
use crate::tool_primitives::fs as pfs;
use crate::tool_primitives::replace::{Stage, describe_failure, plan_edit};
use std::path::PathBuf;

pub struct FileEditTool;

/// One replacement of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditSpec {
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}

/// A parsed call: its file and its replacements, in order. The one-replacement
/// form is a single [`EditSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditCall {
    pub file_path: String,
    pub edits: Vec<EditSpec>,
    /// The `edits` form. Its failures name the edit that failed.
    pub several: bool,
}

/// What one replacement did, in the text it was applied to.
#[derive(Debug, Clone)]
pub struct Step {
    pub replacements: usize,
    /// 1-based inclusive line ranges of the replaced regions.
    pub ranges: Vec<(usize, usize)>,
    pub stage: Stage,
}

/// A call applied in memory: the new text of the file and what each
/// replacement did.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub content: String,
    pub steps: Vec<Step>,
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "Edit"
    }
    fn description(&self) -> &str {
        "Replace text in a file. Give one replacement with old_string and new_string, or several \
         with edits: they are applied in order, each against the result of the previous one, and \
         the file is written only if all of them apply. old_string matches with tolerance for \
         leading/trailing whitespace and indentation differences; it must identify one place \
         unless replace_all is set."
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
                "old_string": {
                    "type": "string",
                    "description": "One replacement: the text to replace. Use with new_string, not with edits."
                },
                "new_string": {
                    "type": "string",
                    "description": "One replacement: the replacement text. Omit it, or give an empty string, to delete old_string."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "One replacement: replace every occurrence of old_string.",
                    "default": false
                },
                "edits": {
                    "type": "array",
                    "minItems": 1,
                    "description": "Several replacements applied in order, each against the result of the previous one. Use instead of old_string, new_string and replace_all, not with them.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string", "description": "The text to replace" },
                            "new_string": { "type": "string", "description": "The replacement text; omit it or give an empty string to delete" },
                            "replace_all": { "type": "boolean", "description": "Replace every occurrence of old_string", "default": false }
                        },
                        "required": ["old_string"]
                    }
                }
            },
            "required": ["file_path"]
        })
    }

    async fn preview(&self, input: &Value, ctx: &ToolContext) -> Option<ChangePreview> {
        let call = match parse_call(input) {
            Ok(call) => call,
            Err(e) => return Some(ChangePreview::refused(e.message())),
        };
        let absolute = absolute_path(ctx, &call.file_path);
        let before = match std::fs::read_to_string(&absolute) {
            Ok(text) => text,
            Err(e) => {
                return Some(ChangePreview::refused(format!(
                    "Failed to read {}: {e}",
                    call.file_path
                )));
            }
        };
        match prepare(&before, &call) {
            Ok(prepared) if prepared.content != before => Some(ChangePreview {
                files: vec![file_change(
                    &call.file_path,
                    &absolute,
                    Some(&before),
                    Some(&prepared.content),
                )],
                refusal: None,
            }),
            Ok(_) => Some(ChangePreview::refused(no_change_message(&call.file_path))),
            Err(reason) => Some(ChangePreview::refused(reason)),
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let call = match parse_call(&input) {
            Ok(call) => call,
            Err(e) => return e.into_result(self, &input),
        };
        let absolute = absolute_path(ctx, &call.file_path);
        // Held from the read to the write, so no other call of this process
        // writes the file in between.
        let _lock = file_lock::lock_paths(std::slice::from_ref(&absolute)).await;
        let bytes = match tokio::fs::read(&absolute).await {
            Ok(bytes) => bytes,
            Err(e) => return ToolResult::error(format!("Failed to read {}: {e}", call.file_path)),
        };
        if let Err(stale) = ensure_still_approved(ctx, &absolute, &call.file_path, Some(&bytes)) {
            return stale;
        }
        let before = match String::from_utf8(bytes.clone()) {
            Ok(text) => text,
            Err(e) => return ToolResult::error(format!("Failed to read {}: {e}", call.file_path)),
        };
        #[cfg(test)]
        file_lock::hooks::run_before_commit(&absolute);
        // A writer outside this process (an editor, a script) that changed the
        // file since it was read is not overwritten.
        if tokio::fs::read(&absolute).await.ok().as_deref() != Some(bytes.as_slice()) {
            return ToolResult::error(format!(
                "{} changed on disk while the edit was being prepared, so nothing was written. \
                 Read it again, then retry the edit against its current text.",
                call.file_path
            ));
        }

        let prepared = match prepare(&before, &call) {
            Ok(prepared) => prepared,
            Err(reason) => return ToolResult::error(format!("{reason}\nNothing was written.")),
        };
        if prepared.content == before {
            return ToolResult::error(format!(
                "{} Nothing was written.",
                no_change_message(&call.file_path)
            ));
        }
        if let Err(e) = pfs::write_atomic(&absolute, prepared.content.as_bytes()).await {
            return ToolResult::error(format!("Failed to edit file {}: {}", call.file_path, e));
        }
        ToolResult::success(describe_success(&call, &before, &prepared))
    }
}

/// Applies a call's replacements to `before`, in memory. An error names the
/// failing edit when the call has several; nothing is changed.
pub fn prepare(before: &str, call: &EditCall) -> std::result::Result<Prepared, String> {
    let total = call.edits.len();
    let mut content = before.to_string();
    let mut steps = Vec::with_capacity(total);
    for (i, edit) in call.edits.iter().enumerate() {
        match plan_edit(
            &content,
            &edit.old_string,
            &edit.new_string,
            edit.replace_all,
            Some(&call.file_path),
        ) {
            Ok(plan) => {
                steps.push(Step {
                    replacements: plan.replacements,
                    ranges: plan.ranges,
                    stage: plan.stage,
                });
                content = plan.content;
            }
            Err(err) => {
                let reason = describe_failure(&err, &call.file_path);
                return Err(if call.several {
                    let order = if i > 0 {
                        " Edits apply in order: this one runs against the file as changed by the earlier edits."
                    } else {
                        ""
                    };
                    format!("edit {} of {total} failed: {reason}{order}", i + 1)
                } else {
                    reason
                });
            }
        }
    }
    Ok(Prepared { content, steps })
}

fn no_change_message(file_path: &str) -> String {
    format!("No changes were produced by the edit: {file_path} is unchanged.")
}

fn absolute_path(ctx: &ToolContext, file_path: &str) -> PathBuf {
    let resolved = ctx.working_dir.join(file_path);
    std::path::absolute(&resolved).unwrap_or(resolved)
}

fn describe_success(call: &EditCall, before: &str, prepared: &Prepared) -> String {
    let header = if call.several {
        let replacements: usize = prepared.steps.iter().map(|s| s.replacements).sum();
        let mut header = format!(
            "The file {} has been updated with {} edit(s), {replacements} replacement(s) in all:",
            call.file_path,
            prepared.steps.len()
        );
        for (i, step) in prepared.steps.iter().enumerate() {
            header.push_str(&format!(
                "\n- edit {}: {} replacement(s) at line(s) {} ({})",
                i + 1,
                step.replacements,
                ranges_text(&step.ranges),
                step.stage.describe()
            ));
        }
        header
    } else {
        let step = &prepared.steps[0];
        format!(
            "The file {} has been updated: {} replacement(s) at line(s) {} ({}).",
            call.file_path,
            step.replacements,
            ranges_text(&step.ranges),
            step.stage.describe()
        )
    };
    let diff = crate::tool_primitives::diff::unified_diff(before, &prepared.content, 2);
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
    format!("{header}\n{diff_preview}")
}

fn ranges_text(ranges: &[(usize, usize)]) -> String {
    ranges
        .iter()
        .map(|(a, b)| {
            if a == b {
                format!("{a}")
            } else {
                format!("{a}–{b}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why a call could not be read.
pub enum CallError {
    /// A problem with the arguments as a whole, as a bare reason: the shared
    /// builder adds the tool name, the echo and the parameter table.
    Shape(String),
    /// A problem already phrased for the model (nested mistakes, mixed forms).
    Phrased(String),
}

impl CallError {
    fn message(&self) -> String {
        match self {
            Self::Shape(s) | Self::Phrased(s) => s.clone(),
        }
    }

    fn into_result<S: Tool + ?Sized>(self, tool: &S, sent: &Value) -> ToolResult {
        match self {
            Self::Shape(reason) => crate::tool_feedback::invalid_input(tool, sent, reason),
            Self::Phrased(text) => ToolResult::error(text),
        }
    }
}

/// The top-level parameters `Edit` declares.
const KNOWN_PARAMS: &[&str] = &[
    "file_path",
    "old_string",
    "new_string",
    "replace_all",
    "edits",
];
/// The parameters of one replacement, in `edits` and at the top level.
const KNOWN_EDIT_PARAMS: &[&str] = &["old_string", "new_string", "replace_all"];

/// Parses a call into its file and replacements, coercing scalar *types* but
/// not parameter *names*: a name that is not declared is refused, never
/// guessed. Both forms are read here; they cannot be mixed.
pub fn parse_call(input: &Value) -> std::result::Result<EditCall, CallError> {
    let obj = input
        .as_object()
        .ok_or_else(|| CallError::Shape("the arguments must be a JSON object".to_string()))?;
    crate::tool_feedback::reject_unknown_keys(input, KNOWN_PARAMS).map_err(CallError::Shape)?;

    let file_path = get_str(obj, "file_path").ok_or_else(|| {
        CallError::Shape("missing 'file_path' (the absolute path of the file to edit)".to_string())
    })?;

    if let Some(edits) = obj.get("edits") {
        let mixed: Vec<&str> = KNOWN_EDIT_PARAMS
            .iter()
            .copied()
            .filter(|k| obj.contains_key(*k))
            .collect();
        if !mixed.is_empty() {
            return Err(CallError::Phrased(format!(
                "Tool 'Edit' rejected your arguments: 'edits' cannot be sent together with {}. \
                 Send either old_string and new_string for one replacement, or edits for several. \
                 No changes were written.",
                quoted(&mixed)
            )));
        }
        let items = edits.as_array().ok_or_else(|| {
            CallError::Shape(
                "'edits' must be a JSON array of {old_string, new_string} objects".to_string(),
            )
        })?;
        if items.is_empty() {
            return Err(CallError::Phrased(
                "Tool 'Edit' rejected your arguments: 'edits' is empty, so there is nothing to \
                 replace. Send at least one replacement. No changes were written."
                    .to_string(),
            ));
        }
        let total = items.len();
        let mut parsed = Vec::with_capacity(total);
        for (i, item) in items.iter().enumerate() {
            parsed.push(parse_entry(i, total, item)?);
        }
        return Ok(EditCall {
            file_path,
            edits: parsed,
            several: true,
        });
    }

    let old_string = get_str(obj, "old_string").ok_or_else(|| {
        CallError::Shape(
            "missing 'old_string' (the exact existing text to replace), or 'edits' for several replacements"
                .to_string(),
        )
    })?;
    // new_string may legitimately be empty (a deletion): an omitted one is empty.
    let new_string = get_str(obj, "new_string").unwrap_or_default();
    Ok(EditCall {
        file_path,
        edits: vec![EditSpec {
            old_string,
            new_string,
            replace_all: get_bool(obj, "replace_all"),
        }],
        several: false,
    })
}

/// One entry of `edits`. A problem names the entry, its key and the parameter
/// it probably meant, and asks to leave the other entries alone.
fn parse_entry(i: usize, total: usize, item: &Value) -> std::result::Result<EditSpec, CallError> {
    let nested = |problem: String| {
        CallError::Phrased(format!(
            "Tool 'Edit' rejected your arguments: edit {} of {total} {problem} Send the call again \
             with that edit corrected, leaving the other edits as they are. No changes were written.",
            i + 1
        ))
    };
    let obj = item.as_object().ok_or_else(|| {
        nested("is not a JSON object. Each entry of 'edits' is an object with old_string, and optionally new_string and replace_all.".to_string())
    })?;
    if let Some(key) = obj
        .keys()
        .find(|k| !KNOWN_EDIT_PARAMS.contains(&k.as_str()))
    {
        let mut problem = format!(
            "uses unknown field `{key}`, expected one of {}.",
            quoted(KNOWN_EDIT_PARAMS)
        );
        if let Some(best) = crate::tool_feedback::closest(key, KNOWN_EDIT_PARAMS) {
            problem.push_str(&format!(" It probably means `{best}`."));
        }
        return Err(nested(problem));
    }
    let old_string = get_str(obj, "old_string").ok_or_else(|| {
        nested("is missing 'old_string' (the exact existing text to replace).".to_string())
    })?;
    Ok(EditSpec {
        old_string,
        new_string: get_str(obj, "new_string").unwrap_or_default(),
        replace_all: get_bool(obj, "replace_all"),
    })
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A string field, coercing numbers and booleans to their string form.
fn get_str(obj: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// A boolean field, accepting the strings and numbers models send for it.
fn get_bool(obj: &serde_json::Map<String, Value>, key: &str) -> bool {
    match obj.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => {
            matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes")
        }
        Some(Value::Number(n)) => n.as_i64().map(|v| v != 0).unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn test_ctx(dir: &Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "edit-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    fn one(path: &Path, old: &str, new: &str) -> Value {
        serde_json::json!({
            "file_path": path.to_str().unwrap(),
            "old_string": old,
            "new_string": new,
        })
    }

    fn several(path: &Path, edits: Value) -> Value {
        serde_json::json!({ "file_path": path.to_str().unwrap(), "edits": edits })
    }

    #[tokio::test]
    async fn tolerant_edit_survives_indentation_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.rs");
        // File is indented; the model supplies old_string without indentation.
        std::fs::write(&path, "fn main() {\n        let x = 1;\n}\n").unwrap();

        let res = FileEditTool
            .execute(
                one(&path, "let x = 1;", "let x = 2;"),
                &test_ctx(tmp.path()),
            )
            .await;

        assert!(!res.is_error, "expected success, got: {:?}", res.content);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "fn main() {\n        let x = 2;\n}\n");
    }

    /// Aliased names are refused rather than silently accepted: `Edit` was the
    /// one tool rewarding `path`/`old`/`new`, and a guessed name then travelled
    /// to tools where it means something else.
    #[tokio::test]
    async fn rejects_aliased_field_names_and_names_the_real_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "hello world").unwrap();

        let res = FileEditTool
            .execute(
                serde_json::json!({
                    "path": path.to_str().unwrap(),
                    "old": "world",
                    "new": "there"
                }),
                &test_ctx(tmp.path()),
            )
            .await;

        assert!(res.is_error, "expected refusal, got: {:?}", res.content);
        assert!(res.content.contains("file_path"), "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn coerces_stringified_replace_all() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "a a a").unwrap();

        let res = FileEditTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "a",
                    "new_string": "b",
                    "replace_all": "true"
                }),
                &test_ctx(tmp.path()),
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

        let res = FileEditTool
            .execute(one(&path, "a", "b"), &test_ctx(tmp.path()))
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
                one(
                    &path,
                    "    let a = 1;\n    let b = 3;\n",
                    "    let a = 10;\n",
                ),
                &test_ctx(tmp.path()),
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
                one(
                    &path,
                    "def f(self):\n    return 1\n",
                    "def f(self):\n    if x:\n        return 2\n    return 1\n",
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert!(res.content.contains("normalised match"), "{}", res.content);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "class A:\r\n    def f(self):\r\n        if x:\r\n            return 2\r\n        return 1\r\n"
        );
    }

    #[tokio::test]
    async fn an_omitted_or_empty_new_string_deletes_the_match() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("d.txt");
        std::fs::write(&path, "keep\ndrop me\nkeep too\n").unwrap();
        let omitted =
            serde_json::json!({ "file_path": path.to_str().unwrap(), "old_string": "drop me\n" });
        let res = FileEditTool.execute(omitted, &test_ctx(tmp.path())).await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep\nkeep too\n");

        std::fs::write(&path, "x\ny\n").unwrap();
        let res = FileEditTool
            .execute(one(&path, "x\n", ""), &test_ctx(tmp.path()))
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "y\n");
    }

    #[tokio::test]
    async fn several_edits_apply_in_order_in_one_write() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.rs");
        std::fs::write(&path, "let a = 1;\nlet b = 2;\nlet c = 3;\n").unwrap();
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([
                        {"old_string": "let a = 1;", "new_string": "let a = 10;"},
                        {"old_string": "let c = 3;", "new_string": "let c = 30;"}
                    ]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert!(res.content.contains("2 edit(s)"), "{}", res.content);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "let a = 10;\nlet b = 2;\nlet c = 30;\n"
        );
    }

    #[tokio::test]
    async fn several_edits_with_replace_all_rename_a_symbol() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.rs");
        std::fs::write(&path, "foo(); foo(); let x = foo;\n").unwrap();
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([{"old_string": "foo", "new_string": "bar", "replace_all": true}]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "bar(); bar(); let x = bar;\n"
        );
    }

    #[tokio::test]
    async fn a_later_edit_sees_the_result_of_the_earlier_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        // The second edit only matches if the first one already ran.
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([
                        {"old_string": "alpha", "new_string": "beta"},
                        {"old_string": "beta", "new_string": "gamma"}
                    ]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "gamma\n");
    }

    #[tokio::test]
    async fn a_failing_edit_writes_nothing_and_names_its_position() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        let original = "keep me\n";
        std::fs::write(&path, original).unwrap();
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([
                        {"old_string": "keep me", "new_string": "changed"},
                        {"old_string": "does-not-exist", "new_string": "x"}
                    ]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(res.is_error);
        assert!(
            res.content.contains("edit 2 of 2 failed"),
            "{}",
            res.content
        );
        assert!(
            res.content.contains("Nothing was written"),
            "{}",
            res.content
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[tokio::test]
    async fn several_edits_tolerate_indentation_drift_each() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.rs");
        std::fs::write(&path, "fn main() {\n        let x = 1;\n}\n").unwrap();
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([{"old_string": "let x = 1;", "new_string": "let x = 2;"}]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn main() {\n        let x = 2;\n}\n"
        );
    }

    /// A typo inside one entry must not come back as a "retry with exactly
    /// this" example that still holds the typo, nor one that keeps the edits
    /// that were just refused.
    #[tokio::test]
    async fn a_nested_typo_names_the_entry_and_is_not_echoed_as_the_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.rs");
        std::fs::write(&path, "let a = 1;\nlet c = 3;\n").unwrap();
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([
                        { "old_string": "let a = 1;", "new_string": "let b = 2;" },
                        { "oldString": "let c = 3;", "new_string": "let d = 4;" }
                    ]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(res.is_error, "{}", res.content);
        assert!(
            res.content.contains("oldString"),
            "must quote the key: {}",
            res.content
        );
        assert!(
            res.content.contains("old_string"),
            "must name the parameter: {}",
            res.content
        );
        assert!(
            res.content.contains("edit 2 of 2"),
            "must say which edit: {}",
            res.content
        );
        assert!(
            !res.content.contains("keep the same value you sent"),
            "{}",
            res.content
        );
        assert!(
            !res.content.contains("sending exactly this"),
            "{}",
            res.content
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "let a = 1;\nlet c = 3;\n"
        );
    }

    #[tokio::test]
    async fn the_two_forms_cannot_be_mixed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "one\n").unwrap();
        let res = FileEditTool
            .execute(
                serde_json::json!({
                    "file_path": path.to_str().unwrap(),
                    "old_string": "one",
                    "new_string": "two",
                    "edits": [{"old_string": "one", "new_string": "three"}]
                }),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(res.is_error);
        assert!(
            res.content.contains("cannot be sent together"),
            "{}",
            res.content
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
    }

    #[tokio::test]
    async fn an_empty_edits_list_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "one\n").unwrap();
        let res = FileEditTool
            .execute(several(&path, serde_json::json!([])), &test_ctx(tmp.path()))
            .await;
        assert!(res.is_error);
        assert!(res.content.contains("'edits' is empty"), "{}", res.content);
    }

    #[tokio::test]
    async fn an_unknown_key_inside_an_entry_is_named_with_its_index() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([
                        {"old_string": "one", "new_string": "1"},
                        {"old_string": "two", "new_str": "2"}
                    ]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(res.is_error);
        assert!(res.content.contains("edit 2 of 2"), "{}", res.content);
        assert!(res.content.contains("`new_str`"), "{}", res.content);
        assert!(res.content.contains("new_string"), "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
    }

    #[tokio::test]
    async fn an_entry_without_old_string_or_not_an_object_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "one\n").unwrap();
        let missing = FileEditTool
            .execute(
                several(&path, serde_json::json!([{"new_string": "1"}])),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(
            missing.is_error && missing.content.contains("missing 'old_string'"),
            "{}",
            missing.content
        );
        let not_object = FileEditTool
            .execute(
                several(&path, serde_json::json!(["one"])),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(
            not_object.is_error && not_object.content.contains("is not a JSON object"),
            "{}",
            not_object.content
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
    }

    #[tokio::test]
    async fn several_edits_keep_crlf_and_unicode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("u.txt");
        std::fs::write(&path, "café\r\nnaïve\r\n").unwrap();
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([
                        {"old_string": "café\n", "new_string": "thé\n"},
                        {"old_string": "naïve\n", "new_string": "naïve ✓\n"}
                    ]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            "thé\r\nnaïve ✓\r\n".as_bytes()
        );
    }

    #[tokio::test]
    async fn a_call_that_changes_nothing_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.txt");
        std::fs::write(&path, "x\n").unwrap();
        // The two replacements undo each other: the text comes back unchanged.
        let res = FileEditTool
            .execute(
                several(
                    &path,
                    serde_json::json!([
                        {"old_string": "x\n", "new_string": "y\n"},
                        {"old_string": "y\n", "new_string": "x\n"}
                    ]),
                ),
                &test_ctx(tmp.path()),
            )
            .await;
        assert!(res.is_error, "{}", res.content);
        assert!(res.content.contains("unchanged"), "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x\n");
    }

    /// A change made by a writer outside the lock, between the read and the
    /// write, is not overwritten.
    #[tokio::test]
    async fn a_change_between_the_read_and_the_write_is_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("race.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let outside = path.clone();
        let fired = Arc::new(AtomicBool::new(false));
        let once = Arc::clone(&fired);
        file_lock::hooks::before_commit(&path, move || {
            if !once.swap(true, Ordering::SeqCst) {
                std::fs::write(&outside, "alpha\nbeta\nwritten by someone else\n").unwrap();
            }
        });
        let res = FileEditTool
            .execute(one(&path, "beta", "BETA"), &test_ctx(tmp.path()))
            .await;
        assert!(res.is_error, "{}", res.content);
        assert!(res.content.contains("changed on disk"), "{}", res.content);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "alpha\nbeta\nwritten by someone else\n"
        );
    }

    /// The preview a person approves is the diff that is written.
    #[tokio::test]
    async fn the_preview_describes_exactly_what_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.txt");
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        let ctx = test_ctx(tmp.path());
        let input = several(
            &path,
            serde_json::json!([
                {"old_string": "b\n", "new_string": "B\nB2\n"},
                {"old_string": "d\n", "new_string": ""}
            ]),
        );
        let preview = FileEditTool.preview(&input, &ctx).await.unwrap();
        assert!(preview.refusal.is_none(), "{:?}", preview.refusal);
        let before = std::fs::read_to_string(&path).unwrap();

        let res = FileEditTool.execute(input, &ctx).await;
        assert!(!res.is_error, "{}", res.content);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "a\nB\nB2\nc\n");

        let written = file_change(path.to_str().unwrap(), &path, Some(&before), Some(&after));
        assert_eq!(written.diff, preview.files[0].diff);
        assert_eq!(
            (written.added, written.removed),
            (preview.files[0].added, preview.files[0].removed)
        );
    }

    /// An approval given for a state the file no longer has is not applied.
    #[tokio::test]
    async fn an_approval_for_an_older_state_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let input = one(&path, "beta", "BETA");
        let ctx = test_ctx(tmp.path());
        let preview = FileEditTool.preview(&input, &ctx).await.unwrap();

        // Someone else changes the file after the approval was given.
        std::fs::write(&path, "alpha\nbeta\nadded meanwhile\n").unwrap();
        let mut approved = ctx.clone();
        approved.extensions = approved
            .extensions
            .with_local(crate::preview::ApprovedState::of(&preview));

        let res = FileEditTool.execute(input, &approved).await;
        assert!(res.is_error, "{}", res.content);
        assert!(crate::preview::is_stale_approval(&res));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "alpha\nbeta\nadded meanwhile\n",
            "nothing may be written under a stale approval"
        );
    }

    #[tokio::test]
    async fn an_approval_for_the_current_state_is_executed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let input = one(&path, "beta", "BETA");
        let ctx = test_ctx(tmp.path());
        let preview = FileEditTool.preview(&input, &ctx).await.unwrap();
        let mut approved = ctx.clone();
        approved.extensions = approved
            .extensions
            .with_local(crate::preview::ApprovedState::of(&preview));
        let res = FileEditTool.execute(input, &approved).await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\nBETA\n");
    }

    /// While another call holds the file, this one waits for it.
    #[tokio::test]
    async fn an_edit_waits_for_the_file_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("l.txt");
        std::fs::write(&path, "one\n").unwrap();
        let ctx = test_ctx(tmp.path());
        let held = file_lock::lock_paths(std::slice::from_ref(&path)).await;
        let input = one(&path, "one", "uno");
        let waited = tokio::time::timeout(
            Duration::from_millis(150),
            FileEditTool.execute(input.clone(), &ctx),
        )
        .await;
        assert!(waited.is_err(), "the edit wrote while the file was locked");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        drop(held);
        let res = FileEditTool.execute(input, &ctx).await;
        assert!(!res.is_error, "{}", res.content);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "uno\n");
    }

    /// Two edits of distinct zones of one file, the first paused inside its
    /// critical section: the second must wait, then build on the first. Without
    /// the lock the second reads the original text and the first write is lost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_edits_of_distinct_zones_of_one_file_both_land() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("zones.txt");
        std::fs::write(&path, "alpha\nbeta\ngamma\ndelta\n").unwrap();
        let inside = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(true));
        {
            let inside = Arc::clone(&inside);
            let pause = Arc::clone(&pause);
            file_lock::hooks::before_commit(&path, move || {
                if pause.swap(false, Ordering::SeqCst) {
                    inside.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(300));
                }
            });
        }
        let ctx = test_ctx(tmp.path());
        let first_ctx = ctx.clone();
        let first_input = one(&path, "alpha", "ALPHA");
        let first =
            tokio::spawn(async move { FileEditTool.execute(first_input, &first_ctx).await });
        while !inside.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let second_ctx = ctx.clone();
        let second_input = one(&path, "gamma", "GAMMA");
        let second =
            tokio::spawn(async move { FileEditTool.execute(second_input, &second_ctx).await });
        let (a, b) = (first.await.unwrap(), second.await.unwrap());
        assert!(!a.is_error && !b.is_error, "{} / {}", a.content, b.content);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "ALPHA\nbeta\nGAMMA\ndelta\n"
        );
    }

    /// Two edits of one file, the one entering its critical section first
    /// paused: the other waits, then builds on the paused one. Returns the
    /// final text. `first` and `second` are `(old_string, new_string)`.
    async fn race_of_two_edits(name: &str, first: (&str, &str), second: (&str, &str)) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(name);
        std::fs::write(&path, "alpha\nbeta\ngamma\ndelta\n").unwrap();
        let inside = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(true));
        {
            let inside = Arc::clone(&inside);
            let pause = Arc::clone(&pause);
            file_lock::hooks::before_commit(&path, move || {
                if pause.swap(false, Ordering::SeqCst) {
                    inside.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(300));
                }
            });
        }
        let ctx = test_ctx(tmp.path());
        let (c1, i1) = (ctx.clone(), one(&path, first.0, first.1));
        let first_task = tokio::spawn(async move { FileEditTool.execute(i1, &c1).await });
        while !inside.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let (c2, i2) = (ctx.clone(), one(&path, second.0, second.1));
        let second_task = tokio::spawn(async move { FileEditTool.execute(i2, &c2).await });
        let (a, b) = (first_task.await.unwrap(), second_task.await.unwrap());
        assert!(!a.is_error && !b.is_error, "{} / {}", a.content, b.content);
        std::fs::read_to_string(&path).unwrap()
    }

    /// Both orders of the same race end with the same file: whichever edit
    /// enters first, the other one applies to its result.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_edits_of_one_file_land_in_either_order() {
        let alpha = ("alpha", "ALPHA");
        let gamma = ("gamma", "GAMMA");
        let alpha_first = race_of_two_edits("order-a.txt", alpha, gamma).await;
        let gamma_first = race_of_two_edits("order-g.txt", gamma, alpha).await;
        assert_eq!(alpha_first, "ALPHA\nbeta\nGAMMA\ndelta\n");
        assert_eq!(gamma_first, "ALPHA\nbeta\nGAMMA\ndelta\n");
    }

    /// Edits of two different files do not wait for each other: the edit of
    /// `a.txt` stays inside its critical section until the edit of `b.txt`
    /// has landed. With one shared lock, `b` would wait out the bounded wait
    /// of `a` and the flag would be false.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn edits_of_different_files_advance_while_one_is_held() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        std::fs::write(&a, "one\n").unwrap();
        std::fs::write(&b, "two\n").unwrap();
        let inside = Arc::new(AtomicBool::new(false));
        let b_landed = Arc::new(AtomicBool::new(false));
        let saw_b_landed = Arc::new(AtomicBool::new(false));
        {
            let inside = Arc::clone(&inside);
            let b_landed = Arc::clone(&b_landed);
            let saw = Arc::clone(&saw_b_landed);
            file_lock::hooks::before_commit(&a, move || {
                inside.store(true, Ordering::SeqCst);
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while !b_landed.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                saw.store(b_landed.load(Ordering::SeqCst), Ordering::SeqCst);
            });
        }
        let ctx = test_ctx(tmp.path());
        let (ca, ia) = (ctx.clone(), one(&a, "one", "uno"));
        let a_task = tokio::spawn(async move { FileEditTool.execute(ia, &ca).await });
        while !inside.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let res_b = FileEditTool.execute(one(&b, "two", "dos"), &ctx).await;
        b_landed.store(!res_b.is_error, Ordering::SeqCst);
        let res_a = a_task.await.unwrap();
        assert!(
            !res_a.is_error && !res_b.is_error,
            "{} / {}",
            res_a.content,
            res_b.content
        );
        assert!(
            saw_b_landed.load(Ordering::SeqCst),
            "b waited for the lock of a"
        );
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "uno\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "dos\n");
    }

    /// A link and its target are one file for the lock: an edit through the
    /// link does not enter its critical section while an edit of the target
    /// is inside. (What the link ends up as is a separate matter, see the
    /// ignored symlink test of `tests/edit_recipe.rs`; only the wait is
    /// asserted here.)
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_edit_through_a_link_waits_for_an_edit_of_its_target() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.txt");
        std::fs::write(&target, "one\ntwo\n").unwrap();
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let inside = Arc::new(AtomicBool::new(false));
        let link_started = Arc::new(AtomicBool::new(false));
        let link_done = Arc::new(AtomicBool::new(false));
        let early = Arc::new(AtomicBool::new(false));
        {
            let inside = Arc::clone(&inside);
            let link_started = Arc::clone(&link_started);
            let link_done = Arc::clone(&link_done);
            let early = Arc::clone(&early);
            file_lock::hooks::before_commit(&target, move || {
                inside.store(true, Ordering::SeqCst);
                // Bounded: the link's call is started while this one is held.
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while !link_started.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                std::thread::sleep(Duration::from_millis(200));
                early.store(link_done.load(Ordering::SeqCst), Ordering::SeqCst);
            });
        }
        let ctx = test_ctx(tmp.path());
        let (c1, i1) = (ctx.clone(), one(&target, "one", "uno"));
        let first = tokio::spawn(async move { FileEditTool.execute(i1, &c1).await });
        while !inside.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let (c2, i2) = (ctx.clone(), one(&link, "two", "dos"));
        let started = Arc::clone(&link_started);
        let done = Arc::clone(&link_done);
        let second = tokio::spawn(async move {
            started.store(true, Ordering::SeqCst);
            let r = FileEditTool.execute(i2, &c2).await;
            done.store(true, Ordering::SeqCst);
            r
        });
        let (a, b) = (first.await.unwrap(), second.await.unwrap());
        assert!(!a.is_error && !b.is_error, "{} / {}", a.content, b.content);
        assert!(
            !early.load(Ordering::SeqCst),
            "the link's edit ran inside the target's critical section"
        );
    }
}
