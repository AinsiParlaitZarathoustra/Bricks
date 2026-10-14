//! Glob tool: find files by pattern.

use super::*;
use crate::tool_primitives::search as psearch;
use serde::Deserialize;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }
    fn description(&self) -> &str {
        "Find files matching a glob pattern (e.g. src/**/*.rs), rooted at `path` or the working directory. Matches names/paths only — use Grep to search file contents."
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
                "pattern": { "type": "string", "description": "Glob pattern (e.g. **/*.rs)" },
                "path": { "type": "string", "description": "Directory to search in" },
                "limit": { "type": "integer", "description": "Max results to return (default 200)" }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            pattern: String,
            path: Option<String>,
            limit: Option<usize>,
        }

        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };

        let max_results = input.limit.unwrap_or(200);

        // A relative path is relative to the working directory.
        let base_dir = input
            .path
            .as_ref()
            .map(|p| ctx.working_dir.join(p))
            .unwrap_or_else(|| ctx.working_dir.clone());

        let opts = psearch::GlobOptions {
            limit: max_results,
            ..Default::default()
        };
        // The walk honours the project's ignore rules and stops past the
        // limit: the total number of matches is not known, never invented.
        match psearch::glob_walk(&input.pattern, &base_dir, opts).await {
            Ok(found) => {
                let mut result = if max_results == 0 {
                    "Limit is 0: no file listed.".to_string()
                } else if found.paths.is_empty() {
                    "No files matched the pattern.".to_string()
                } else {
                    found
                        .paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                if found.more {
                    result.push_str(&format!(
                        "\n\n[Showing the first {} matches; more matches exist. Narrow the pattern.]",
                        found.paths.len()
                    ));
                }
                if found.visit_cap_hit {
                    result.push_str(&format!(
                        "\n\n[The search stopped after visiting {} entries; the list may be incomplete. Narrow the path or the pattern.]",
                        found.visited - 1
                    ));
                }
                ToolResult::success(result)
            }
            Err(e) => ToolResult::error(format!("Glob failed: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use crate::{CostTracker, Extensions};

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "glob-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    #[tokio::test]
    async fn the_output_never_claims_a_total_it_did_not_count() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        for i in 0..30 {
            std::fs::write(tmp.path().join(format!("sub/f{i:02}.rs")), "").unwrap();
        }
        let c = ctx(tmp.path());
        let out = GlobTool
            .execute(
                serde_json::json!({"pattern": "*.rs", "path": "sub", "limit": 5}),
                &c,
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 7, "{}", out.content);
        assert!(
            lines[0].ends_with("sub/f00.rs"),
            "relative path joined to the working dir"
        );
        assert!(
            out.content
                .contains("[Showing the first 5 matches; more matches exist. Narrow the pattern.]")
        );
        assert!(!out.content.contains(" of 30"));
        let all = GlobTool
            .execute(serde_json::json!({"pattern": "sub/*.rs"}), &c)
            .await;
        assert_eq!(all.content.lines().count(), 30, "{}", all.content);
        let zero = GlobTool
            .execute(serde_json::json!({"pattern": "*.rs", "limit": 0}), &c)
            .await;
        assert_eq!(zero.content, "Limit is 0: no file listed.");
        let none = GlobTool
            .execute(serde_json::json!({"pattern": "*.zig"}), &c)
            .await;
        assert_eq!(none.content, "No files matched the pattern.");
        let bad = GlobTool
            .execute(serde_json::json!({"pattern": "["}), &c)
            .await;
        assert!(
            bad.is_error && bad.content.contains("invalid pattern"),
            "{}",
            bad.content
        );
    }
}
