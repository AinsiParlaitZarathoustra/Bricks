//! Deferred tools: lent by the host ([`crate::ToolLender::deferred_tools`])
//! but announced to the model only once loaded. `LoadTools` names them and
//! loads those asked for; a loaded tool stays announced for the rest of the
//! session, and one already called in the conversation is loaded again at
//! the start of a run.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use bricks_types::{ContentBlock, Message};
use serde_json::{Value, json};

pub const LOAD_TOOLS: &str = "LoadTools";

/// Matches loaded by a search without `max_results`.
const DEFAULT_RESULTS: usize = 5;

/// The names of the deferred tools loaded in a session.
pub(crate) type Loaded = Arc<parking_lot::Mutex<HashSet<String>>>;

/// Loads deferred tools by name (`select:a,b`) or by search.
pub(crate) struct LoadTools {
    tools: Arc<Vec<Arc<dyn Tool>>>,
    loaded: Loaded,
    description: String,
}

impl LoadTools {
    pub(crate) fn new(tools: Arc<Vec<Arc<dyn Tool>>>, loaded: Loaded) -> Self {
        let description = format!(
            "Loads tools that are available but not announced yet: they can be called from \
             the next request on. `select:a,b` loads those tools by name; other words search \
             names and descriptions and load the best matches. Available: {}.",
            names(&tools)
        );
        Self {
            tools,
            loaded,
            description,
        }
    }

    /// The tools `query` names (`select:`, full or short names) or matches.
    fn find(&self, query: &str, max: usize) -> Vec<Arc<dyn Tool>> {
        if let Some(list) = query.trim().strip_prefix("select:") {
            let mut found: Vec<Arc<dyn Tool>> = Vec::new();
            for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let tool = self
                    .tools
                    .iter()
                    .find(|t| t.name() == name || short_name(t.name()) == name);
                if let Some(t) = tool
                    && !found.iter().any(|f| f.name() == t.name())
                {
                    found.push(Arc::clone(t));
                }
            }
            return found;
        }
        let terms: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let mut scored: Vec<(usize, &Arc<dyn Tool>)> = self
            .tools
            .iter()
            .filter_map(|t| {
                let name = t.name().to_lowercase();
                let description = t.description().to_lowercase();
                let score: usize = terms
                    .iter()
                    .map(|w| {
                        3 * usize::from(name.contains(w.as_str()))
                            + usize::from(description.contains(w.as_str()))
                    })
                    .sum();
                (score > 0).then_some((score, t))
            })
            .collect();
        // Stable: equal scores keep the order of the listing.
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored
            .into_iter()
            .take(max)
            .map(|(_, t)| Arc::clone(t))
            .collect()
    }
}

#[async_trait]
impl Tool for LoadTools {
    fn name(&self) -> &str {
        LOAD_TOOLS
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "`select:name1,name2` to load those tools, or words to search for"
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Matches loaded by a search (default 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let Some(query) = input.get("query").and_then(Value::as_str) else {
            return ToolResult::error("`query` is required");
        };
        let max = input
            .get("max_results")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_RESULTS, |n| n.max(1) as usize);
        let found = self.find(query, max);
        if found.is_empty() {
            return ToolResult::error(format!(
                "No tool matches \"{query}\". Available: {}.",
                names(&self.tools)
            ));
        }
        let mut loaded = self.loaded.lock();
        let lines: Vec<String> = found
            .iter()
            .map(|t| {
                loaded.insert(t.name().to_string());
                format!("- {}: {}", t.name(), summary(t.description()))
            })
            .collect();
        ToolResult::success(format!(
            "Loaded, callable from the next request:\n{}",
            lines.join("\n")
        ))
    }
}

/// The deferred tools among `tools` that a call in `messages` names.
pub(crate) fn called(messages: &[Message], tools: &[Arc<dyn Tool>]) -> Vec<String> {
    let mut names = Vec::new();
    for block in messages.iter().flat_map(|m| m.content_blocks()) {
        if let ContentBlock::ToolUse { name, .. } = block
            && tools.iter().any(|t| t.name() == name)
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    names
}

fn names(tools: &[Arc<dyn Tool>]) -> String {
    tools
        .iter()
        .map(|t| t.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `mcp__server__tool` → `tool`.
fn short_name(name: &str) -> &str {
    name.rsplit("__").next().unwrap_or(name)
}

/// The first sentence (or line) of a description, at most 200 characters.
fn summary(description: &str) -> String {
    let line = description.trim().lines().next().unwrap_or_default();
    let end = line.find(". ").map_or(line.len(), |i| i + 1);
    let first = &line[..end];
    match first.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &first[..i]),
        None => first.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named(&'static str, &'static str);

    #[async_trait]
    impl Tool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            self.1
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn execute(&self, _input: Value, _ctx: &ToolContext) -> ToolResult {
            ToolResult::success("ok")
        }
    }

    fn loader() -> LoadTools {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Named("mcp__cu__click", "Click an element. Long details.")),
            Arc::new(Named(
                "mcp__cu__type_text",
                "Type text into the focused field.",
            )),
            Arc::new(Named(
                "mcp__cu__zoom",
                "Zoom into a region of a screenshot.",
            )),
        ];
        LoadTools::new(Arc::new(tools), Loaded::default())
    }

    fn found(l: &LoadTools, q: &str) -> Vec<String> {
        l.find(q, DEFAULT_RESULTS)
            .iter()
            .map(|t| t.name().to_string())
            .collect()
    }

    #[test]
    fn select_takes_full_and_short_names_once_in_order() {
        let l = loader();
        assert_eq!(
            found(&l, "select:zoom, mcp__cu__click,zoom,nope"),
            ["mcp__cu__zoom", "mcp__cu__click"]
        );
    }

    #[test]
    fn a_search_ranks_names_before_descriptions() {
        let l = loader();
        assert_eq!(found(&l, "text"), ["mcp__cu__type_text"]);
        assert_eq!(
            found(&l, "screenshot click"),
            ["mcp__cu__click", "mcp__cu__zoom"]
        );
        assert!(found(&l, "calendar").is_empty());
    }

    #[test]
    fn the_description_lists_the_tools_and_a_summary_is_one_sentence() {
        let l = loader();
        assert!(
            l.description()
                .contains("mcp__cu__click, mcp__cu__type_text, mcp__cu__zoom")
        );
        assert_eq!(
            summary("Click an element. Long details."),
            "Click an element."
        );
        assert_eq!(summary(&"x".repeat(300)).chars().count(), 201);
    }
}
