//! CodeScout: the agents' access to the workspace's shared code
//! understanding engine (`bricks-semantic`).
//!
//! The model states an intent; the engine chooses text search, syntax or a
//! language server, and every answer is sourced (path, exact range,
//! revision, how it was established) and bounded (limits, context budget).
//! Read, Grep, Edit, Write and Bash stay available alongside.

use super::*;
use bricks_semantic::{
    compiler, render, CodeQuery, ContextPolicy, Detail, Intent, MatchMode, Requester,
    SemanticConfig, SemanticEngine, SemanticRegistry, Target,
};
use serde::Deserialize;

/// The engine an agent uses, set by whoever builds the agent (with the
/// workspace's `[semantic]` configuration). Without it, the engine already
/// covering the working directory is used, else one with defaults.
#[derive(Clone)]
pub struct SemanticHandle(pub Arc<SemanticEngine>);

/// Cancellation of the run a tool call belongs to (set by the runner).
#[derive(Clone)]
pub struct RunCancel(pub bricks_semantic::CancellationToken);

/// The engine for a tool context.
pub fn engine_for(ctx: &ToolContext) -> Arc<SemanticEngine> {
    if let Some(h) = ctx.extensions.get::<SemanticHandle>() {
        return Arc::clone(&h.0);
    }
    let reg = SemanticRegistry::global();
    reg.existing_for(&ctx.working_dir)
        .unwrap_or_else(|| reg.engine_for(&ctx.working_dir, &SemanticConfig::default()))
}

/// Files may have changed (a write, an edit, a shell command): drop the
/// cached answers of the engines covering this working directory.
pub fn notify_workspace_changed(ctx: &ToolContext) {
    if let Some(h) = ctx.extensions.get::<SemanticHandle>() {
        h.0.notify_any_change();
    }
    for e in SemanticRegistry::global().all_for(&ctx.working_dir) {
        e.notify_any_change();
    }
}

pub struct CodeScoutTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    #[serde(default)]
    query: String,
    #[serde(default)]
    intent: Option<Intent>,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    case_insensitive: bool,
    file: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    target_id: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    detail: Option<Detail>,
    context: Option<String>,
    max_results: Option<usize>,
    budget_tokens: Option<u64>,
    compiler_output: Option<String>,
}

/// At most this many compiler locations are explained per call.
const MAX_LOCATIONS: usize = 3;

#[async_trait]
impl Tool for CodeScoutTool {
    fn name(&self) -> &str {
        "CodeScout"
    }

    fn description(&self) -> &str {
        "Understand code: find a symbol's definitions, its references, what is at a place, \
         a file's diagnostics, or search text — in one call, with exact sourced locations and \
         bounded context. State the intent; the engine picks text search, syntax or a language \
         server, and says how each result was established (confirmed / syntactic / textual) \
         and what is missing. Homonyms are listed, never chosen silently: pass `target_id` (an \
         `id` from a previous result) or file+line+column to be precise. Read, Grep and the \
         other tools remain available."
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
                "query": { "type": "string", "description": "A symbol name (`parse`, `Config::load`), text, or a regex (with regex=true). Optional when a target is given." },
                "intent": { "type": "string", "enum": ["auto", "text_search", "find_symbol", "definition", "references", "understand", "diagnostics"], "description": "What you want. `auto` (default): identifiers → definitions, other text → text search, a target → understand." },
                "regex": { "type": "boolean", "default": false },
                "case_insensitive": { "type": "boolean", "default": false },
                "file": { "type": "string", "description": "Target file (relative to the working directory)." },
                "line": { "type": "integer", "description": "Target line, 1-based." },
                "column": { "type": "integer", "description": "Target column, 1-based, in characters." },
                "target_id": { "type": "string", "description": "An `id` from a previous CodeScout result." },
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Limit to these files/folders." },
                "extensions": { "type": "array", "items": { "type": "string" }, "description": "Limit to these extensions (`rs`, `ts`)." },
                "exclude": { "type": "array", "items": { "type": "string" }, "description": "Globs to exclude." },
                "detail": { "type": "string", "enum": ["compact", "normal", "deep"], "description": "Amount of context (default normal)." },
                "context": { "type": "string", "enum": ["auto", "none", "lines", "block", "function", "type"] },
                "max_results": { "type": "integer" },
                "budget_tokens": { "type": "integer", "description": "Context budget (capped by configuration)." },
                "compiler_output": { "type": "string", "description": "Compiler output (cargo JSON preferred): explain the first locations it names." }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let engine = engine_for(ctx);
        let mut requester = Requester::new("agent", &ctx.working_dir);
        if let Some(c) = ctx.extensions.get::<RunCancel>() {
            requester = requester.with_cancel(c.0.clone());
        }
        let detail = input.detail.unwrap_or_default();
        let context = match input.context.as_deref() {
            None | Some("auto") => ContextPolicy::Auto,
            Some("none") => ContextPolicy::None,
            Some("lines") => ContextPolicy::Lines { n: 3 },
            Some("block") => ContextPolicy::Block,
            Some("function") => ContextPolicy::Function,
            Some("type") => ContextPolicy::Type,
            Some(other) => {
                return ToolResult::error(format!(
                    "unknown context `{other}`: use auto, none, lines, block, function or type"
                ))
            }
        };
        let base = |text: String, intent: Intent, target: Option<Target>| {
            let mut q = CodeQuery::text(text)
                .intent(intent)
                .detail(detail)
                .context(context);
            q.target = target;
            q.mode = if input.regex {
                MatchMode::Regex
            } else {
                MatchMode::Literal
            };
            q.case_insensitive = input.case_insensitive;
            q.scope.paths = input.paths.clone();
            q.scope.extensions = input.extensions.clone();
            q.scope.exclude = input.exclude.clone();
            q.limits.max_results = input.max_results;
            q.limits.budget_tokens = input.budget_tokens;
            q
        };

        if let Some(output) = &input.compiler_output {
            let locs = compiler::locations(output);
            if locs.is_empty() {
                return ToolResult::error(
                    "no `path:line:col` location found in the compiler output",
                );
            }
            let shown = locs.len().min(MAX_LOCATIONS);
            let mut out = String::new();
            for (i, l) in locs.iter().take(MAX_LOCATIONS).enumerate() {
                let how = match l.source {
                    compiler::LocationSource::Structured => "structured diagnostic",
                    compiler::LocationSource::TextHeuristic => {
                        "matched in text (heuristic: may be wrong)"
                    }
                };
                out.push_str(&format!(
                    "## location {} of {} — {}:{}:{} ({how}){}\n",
                    i + 1,
                    locs.len(),
                    l.path,
                    l.line,
                    l.column,
                    l.message
                        .as_deref()
                        .map(|m| format!(": {m}"))
                        .unwrap_or_default()
                ));
                let mut q = base(
                    String::new(),
                    Intent::Understand,
                    Some(Target::Position {
                        path: l.path.clone(),
                        line: l.line,
                        column: l.column,
                    }),
                );
                q.limits.budget_tokens = Some(
                    input
                        .budget_tokens
                        .unwrap_or(engine.config().default_budget_tokens)
                        / shown as u64,
                );
                let r = engine.query(q, &requester).await;
                out.push_str(&render::render_response(&r));
            }
            if locs.len() > shown {
                out.push_str(&format!(
                    "({} more location(s) not explained; pass them one at a time)\n",
                    locs.len() - shown
                ));
            }
            return ToolResult::success(out);
        }

        let target = match (&input.target_id, &input.file) {
            (Some(id), _) => Some(Target::Item { id: id.clone() }),
            (None, Some(f)) => Some(match input.line {
                Some(line) => Target::Position {
                    path: f.clone(),
                    line,
                    column: input.column.unwrap_or(1),
                },
                None => Target::File { path: f.clone() },
            }),
            (None, None) => None,
        };
        let q = base(
            input.query.clone(),
            input.intent.unwrap_or_default(),
            target,
        );
        let r = engine.query(q, &requester).await;
        let text = render::render_response(&r);
        match r.status {
            bricks_semantic::ResultStatus::Error { .. } => ToolResult::error(text),
            _ => ToolResult::success(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "t".into(),
            permissions: Arc::new(crate::permissions::AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    #[tokio::test]
    async fn definitions_text_and_errors() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("a.rs"),
            "pub fn alpha() -> u32 { 1 }\nfn main() { alpha(); }\n",
        )
        .unwrap();
        let c = ctx(d.path());
        let mut cfg = SemanticConfig::default();
        cfg.lsp.enabled = false;
        c.extensions
            .insert(SemanticHandle(SemanticEngine::new(d.path(), cfg)));
        let r = CodeScoutTool
            .execute(serde_json::json!({"query": "alpha"}), &c)
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(
            r.content.contains("a.rs:1:8 · definition · syntactic"),
            "{}",
            r.content
        );
        let r = CodeScoutTool
            .execute(
                serde_json::json!({"query": "alpha();", "intent": "text_search"}),
                &c,
            )
            .await;
        assert!(
            r.content.contains("a.rs:2:13 · text_mention"),
            "{}",
            r.content
        );
        let r = CodeScoutTool
            .execute(serde_json::json!({"query": ""}), &c)
            .await;
        assert!(r.is_error);
        let r = CodeScoutTool
            .execute(
                serde_json::json!({"compiler_output": "error: x\n --> a.rs:2:13\n"}),
                &c,
            )
            .await;
        assert!(r.content.contains("heuristic"), "{}", r.content);
        assert!(r.content.contains("intent: understand"), "{}", r.content);
    }
}
