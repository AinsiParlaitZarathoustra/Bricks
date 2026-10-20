//! File write tool.

use super::*;
use crate::file_lock;
use crate::preview::ensure_still_approved;
use crate::tool_primitives::fs as pfs;
use serde::Deserialize;

pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "Write"
    }
    fn description(&self) -> &str {
        "Write content to a file, creating it if it doesn't exist and replacing its content if it does. For partial changes to an existing file, prefer Edit."
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
                "content": { "type": "string", "description": "Content to write" }
            },
            "required": ["file_path", "content"]
        })
    }

    async fn preview(
        &self,
        input: &Value,
        ctx: &ToolContext,
    ) -> Option<crate::preview::ChangePreview> {
        let input: Input = match crate::tool_feedback::parse_input(self, input) {
            Ok(i) => i,
            Err(e) => return Some(crate::preview::ChangePreview::refused(e.content)),
        };
        let absolute = absolute_path(ctx, &input.file_path);
        let bytes = std::fs::read(&absolute).ok();
        let before = bytes
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned());
        let mut change = crate::preview::file_change(
            &input.file_path,
            &absolute,
            before.as_deref(),
            Some(&input.content),
        );
        // The approval is for the bytes on disk, which a lossy text may not
        // reproduce (a file that is not UTF-8).
        change.before_sha256 = bytes.as_deref().map(crate::preview::sha256_hex);
        change.binary = bytes.as_deref().is_some_and(crate::preview::is_binary)
            || crate::preview::is_binary(input.content.as_bytes());
        Some(crate::preview::ChangePreview {
            files: vec![change],
            refusal: None,
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };

        let absolute = absolute_path(ctx, &input.file_path);
        // Held from the check of the file to the write (see `file_lock`).
        let _lock = file_lock::lock_paths(std::slice::from_ref(&absolute)).await;
        let current = tokio::fs::read(&absolute).await.ok();
        if let Err(stale) =
            ensure_still_approved(ctx, &absolute, &input.file_path, current.as_deref())
        {
            return stale;
        }
        // A write through a symbolic link lands in the file it points to, in
        // that file's directory.
        let target = match pfs::write_target(&absolute) {
            Ok(target) => target,
            Err(e) => return ToolResult::error(format!("Failed to write file: {}", e)),
        };
        if let Some(parent) = target.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return ToolResult::error(format!("Failed to write file: {}", e));
        }
        match pfs::write_atomic(&target, input.content.as_bytes()).await {
            Ok(()) => {
                ToolResult::success(format!("File created successfully at: {}", input.file_path))
            }
            Err(e) => ToolResult::error(format!("Failed to write file: {}", e)),
        }
    }
}

/// The file's absolute path, spelled as the preview spells it.
fn absolute_path(ctx: &ToolContext, file_path: &str) -> std::path::PathBuf {
    let resolved = ctx.working_dir.join(file_path);
    std::path::absolute(&resolved).unwrap_or(resolved)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    file_path: String,
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;

    /// A relative path lands in the agent's working directory (a
    /// sub-agent's worktree), never in the process's current directory.
    #[tokio::test]
    async fn relative_paths_use_the_working_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            working_dir: tmp.path().to_path_buf(),
            session_id: "write-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        };
        let r = FileWriteTool
            .execute(
                serde_json::json!({ "file_path": "sub/new.txt", "content": "x\n" }),
                &ctx,
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("sub/new.txt")).unwrap(),
            "x\n"
        );
        assert!(!std::path::Path::new("sub/new.txt").exists());
    }

    fn ctx_in(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "write-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    /// Overwriting a file that was never read is a plain write: no reading
    /// is required of the model.
    #[tokio::test]
    async fn overwriting_an_unread_file_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("old.txt"), "old\n").unwrap();
        let r = FileWriteTool
            .execute(
                serde_json::json!({ "file_path": "old.txt", "content": "new\n" }),
                &ctx_in(tmp.path()),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("old.txt")).unwrap(),
            "new\n"
        );
    }

    /// The preview of a non-UTF-8 file is approved against its bytes, so the
    /// approval holds while those bytes are unchanged.
    #[tokio::test]
    async fn a_write_to_a_file_changed_after_approval_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("b.bin");
        std::fs::write(&path, [0xff, 0xfe, b'a']).unwrap();
        let ctx = ctx_in(tmp.path());
        let input = serde_json::json!({ "file_path": "b.bin", "content": "x\n" });
        let preview = FileWriteTool.preview(&input, &ctx).await.unwrap();

        // Unchanged bytes: the approval holds.
        let mut approved = ctx.clone();
        approved.extensions = approved
            .extensions
            .with_local(crate::preview::ApprovedState::of(&preview));
        let r = FileWriteTool.execute(input.clone(), &approved).await;
        assert!(!r.is_error, "{}", r.content);

        // Changed after approval: nothing is written.
        std::fs::write(&path, [0xff, 0xfe, b'z']).unwrap();
        let r = FileWriteTool.execute(input, &approved).await;
        assert!(crate::preview::is_stale_approval(&r), "{}", r.content);
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0xfe, b'z']);
    }
}
