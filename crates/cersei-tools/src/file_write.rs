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
        _ctx: &ToolContext,
    ) -> Option<crate::preview::ChangePreview> {
        let input: Input = match crate::tool_feedback::parse_input(self, input) {
            Ok(i) => i,
            Err(e) => return Some(crate::preview::ChangePreview::refused(e.content)),
        };
        let path = std::path::Path::new(&input.file_path);
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

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };

        let path = std::path::Path::new(&input.file_path);
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
