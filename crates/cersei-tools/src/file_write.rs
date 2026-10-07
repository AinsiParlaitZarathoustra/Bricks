//! File write tool.

use super::*;
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
        let resolved = ctx.working_dir.join(&input.file_path);
        let path = resolved.as_path();
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let before = match std::fs::read(&absolute) {
            Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Err(_) => None,
        };
        Some(crate::preview::ChangePreview {
            files: vec![crate::preview::file_change(
                &input.file_path,
                &absolute,
                before.as_deref(),
                Some(&input.content),
            )],
            refusal: None,
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };

        let resolved = ctx.working_dir.join(&input.file_path);
        let path = resolved.as_path();
        match pfs::write_file(path, &input.content).await {
            Ok(()) => {
                ToolResult::success(format!("File created successfully at: {}", input.file_path))
            }
            Err(e) => ToolResult::error(format!("Failed to write file: {}", e)),
        }
    }
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
}
