//! cersei-tools: Tool trait, built-in tool implementations, and permission system.

pub mod apply_patch;
pub mod ask_user;
pub mod bash;
pub mod bash_classifier;
pub mod code_scout;
pub mod code_search;
pub mod config_tool;
pub mod cron;
pub mod file_edit;
pub mod file_history;
pub mod file_read;
pub mod file_snapshot;
pub mod file_watcher;
pub mod file_write;
pub mod git_utils;
pub mod glob_tool;
pub mod grep_tool;
pub mod jobs;
pub mod lsp_tool;
pub mod mcp_tool;
pub mod multi_edit;
pub mod notebook_edit;
pub mod permissions;
pub mod plan_mode;
pub mod powershell;
pub mod preview;
pub mod remote_trigger;
pub mod send_message;
pub mod shell;
pub mod skill_tool;
pub mod skills;
pub mod sleep;
pub mod synthetic_output;
pub mod tasks;
pub mod todo_write;
pub mod tool_feedback;
pub mod tool_primitives;
pub mod tool_report;
pub mod tool_search;
#[cfg(feature = "vms")]
pub mod vm_tools;
pub mod web_fetch;
pub mod web_runtime;
pub mod web_search;
pub mod worktree;

use async_trait::async_trait;
use cersei_mcp::McpManager;
use cersei_types::*;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

// ─── Tool trait ──────────────────────────────────────────────────────────────

#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (used by the model to invoke it).
    fn name(&self) -> &str;

    /// Human-readable description shown to the model.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's input parameters.
    fn input_schema(&self) -> Value;

    /// Permission level required for this tool.
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    /// Permission level of one call, for tools whose actions differ (a
    /// read-only status next to a writing apply). Defaults to
    /// [`Tool::permission_level`].
    fn permission_level_for(&self, _input: &Value) -> PermissionLevel {
        self.permission_level()
    }

    /// Category for grouping in tool listings.
    fn category(&self) -> ToolCategory {
        ToolCategory::Custom
    }

    /// Execute the tool with the given JSON input.
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult;

    /// Facts a permission policy should see beyond the input itself (for a
    /// shell: the working directory the command will run in, and the
    /// definitions of session aliases or functions it invokes). Added to the
    /// permission request's description by the agent.
    async fn permission_details(&self, _input: &Value, _ctx: &ToolContext) -> Option<String> {
        None
    }

    /// The file changes this call would make, computed without writing, for
    /// an approval to show before anything is written. `None` for tools
    /// whose effects are not file edits (a shell command, an MCP call): a
    /// diff could not represent them, so none is pretended.
    async fn preview(&self, _input: &Value, _ctx: &ToolContext) -> Option<preview::ChangePreview> {
        None
    }

    /// Convert to a ToolDefinition for the provider.
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: self.input_schema(),
        }
    }
}

/// Typed tool execution trait — used with `#[derive(Tool)]`.
#[async_trait]
pub trait ToolExecute: Send + Sync {
    type Input: serde::de::DeserializeOwned + schemars::JsonSchema;

    async fn run(&self, input: Self::Input, ctx: &ToolContext) -> ToolResult;
}

// ─── Permission levels ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionLevel {
    None,
    ReadOnly,
    Write,
    Execute,
    Dangerous,
    Forbidden,
}

// ─── Tool categories ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    FileSystem,
    Shell,
    Web,
    Memory,
    Orchestration,
    Mcp,
    Custom,
}

// ─── Tool result ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Text of the result (without the uniform header the agent adds).
    pub content: String,
    pub is_error: bool,
    pub metadata: Option<Value>,
    /// Structured form of the result, when the tool provides one; the agent
    /// renders it uniformly (see [`tool_report`]).
    pub report: Option<Box<tool_report::ToolReport>>,
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            metadata: None,
            report: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            metadata: None,
            report: None,
        }
    }

    pub fn with_metadata(mut self, meta: Value) -> Self {
        self.metadata = Some(meta);
        self
    }

    /// A result described by a report. `content` holds its text without the
    /// header (output, notes, suggestion), for callers that read text.
    pub fn from_report(report: tool_report::ToolReport) -> Self {
        let mut content = report.render_output();
        if !report.notes.is_empty() {
            content.push_str("--- remarques ---\n");
            for n in &report.notes {
                content.push_str(&format!("- {n}\n"));
            }
        }
        if let Some(s) = &report.suggestion {
            content.push_str(&format!("--- suggestion ---\n{s}\n"));
        }
        Self {
            is_error: report.status.is_error(),
            metadata: report.data.clone(),
            content: content.trim_end_matches('\n').to_string(),
            report: Some(Box::new(report)),
        }
    }

    /// The report of this result: the tool's own, or one derived from its
    /// text and error flag (no exit code is invented).
    pub fn to_report(&self) -> tool_report::ToolReport {
        match &self.report {
            Some(r) => (**r).clone(),
            None => tool_report::ToolReport::new(
                if self.is_error {
                    tool_report::ToolStatus::Failure
                } else {
                    tool_report::ToolStatus::Success
                },
                tool_report::ToolBody::Text(self.content.clone()),
            ),
        }
    }
}

// ─── Tool context ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub session_id: String,
    pub permissions: Arc<dyn permissions::PermissionPolicy>,
    pub cost_tracker: Arc<CostTracker>,
    pub mcp_manager: Option<Arc<McpManager>>,
    pub extensions: Extensions,
}

type ExtMap = dashmap::DashMap<std::any::TypeId, Arc<dyn std::any::Any + Send + Sync>>;

/// Type-map for injecting custom data into the tool context.
///
/// Clones share their values. [`Extensions::with_local`] makes a view with
/// values of its own on top (for one tool call: concurrent calls never see
/// each other's), reads fall through to the shared values, writes with
/// `insert` still go to the shared ones.
#[derive(Clone, Default)]
pub struct Extensions {
    data: Arc<ExtMap>,
    local: Option<Arc<ExtMap>>,
}

impl Extensions {
    pub fn insert<T: Send + Sync + 'static>(&self, val: T) {
        self.data.insert(std::any::TypeId::of::<T>(), Arc::new(val));
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let id = std::any::TypeId::of::<T>();
        if let Some(local) = &self.local {
            if let Some(v) = local.get(&id) {
                return Arc::clone(v.value()).downcast::<T>().ok();
            }
        }
        self.data
            .get(&id)
            .and_then(|v| Arc::clone(v.value()).downcast::<T>().ok())
    }

    /// A view sharing these values, with `val` visible only through it.
    pub fn with_local<T: Send + Sync + 'static>(&self, val: T) -> Extensions {
        let local: Arc<ExtMap> = Arc::new(dashmap::DashMap::new());
        if let Some(existing) = &self.local {
            for e in existing.iter() {
                local.insert(*e.key(), Arc::clone(e.value()));
            }
        }
        local.insert(std::any::TypeId::of::<T>(), Arc::new(val));
        Extensions {
            data: Arc::clone(&self.data),
            local: Some(local),
        }
    }
}

/// The id of the tool call a context was made for (set by the runner on a
/// per-call view of the extensions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentToolCall(pub String);

impl ToolContext {
    /// The context of one tool call: same services, plus its call id.
    pub fn for_call(&self, tool_call_id: &str) -> ToolContext {
        let mut c = self.clone();
        c.extensions = self
            .extensions
            .with_local(CurrentToolCall(tool_call_id.to_string()));
        c
    }
}

/// Tracks cumulative token usage and cost.
///
/// Costs are not computed here: the provider attaches a [`CostEstimate`]
/// (from the prices declared in the provider configuration) to each response's
/// usage, and the tracker only accumulates it. A response whose model declares
/// no price carries no estimate, and the running total then reports itself as
/// partial rather than counting that usage as free.
pub struct CostTracker {
    usage: parking_lot::Mutex<Usage>,
}

impl CostTracker {
    pub fn new() -> Self {
        Self {
            usage: parking_lot::Mutex::new(Usage::default()),
        }
    }

    pub fn add(&self, usage: &Usage) {
        self.usage.lock().merge(usage);
    }

    pub fn current(&self) -> Usage {
        self.usage.lock().clone()
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod cost_tests {
    use super::*;
    use cersei_types::CostEstimate;

    fn priced(input: u64, cache_read: u64, usd: f64, partial: bool) -> Usage {
        Usage {
            input_tokens: input,
            cache_read_input_tokens: cache_read,
            cost_usd: Some(usd),
            cost_estimate: Some(CostEstimate {
                amount_usd: usd,
                tariff: "default".into(),
                partial,
                unpriced: vec![],
            }),
            ..Default::default()
        }
    }

    /// The cache counters themselves must accumulate across requests.
    #[test]
    fn tracker_accumulates_tokens_cache_fields_and_cost() {
        let tracker = CostTracker::new();
        tracker.add(&priced(1_000_000, 0, 3.0, false));
        tracker.add(&priced(0, 1_000_000, 0.3, false));
        let cur = tracker.current();
        assert_eq!(cur.input_tokens, 1_000_000);
        assert_eq!(cur.cache_read_input_tokens, 1_000_000);
        let est = cur.cost_estimate.expect("estimate carried");
        assert!((est.amount_usd - 3.3).abs() < 1e-9 && !est.partial);
    }

    /// A request with no known price must not be counted as free: the running
    /// total becomes a partial estimate.
    #[test]
    fn unpriced_usage_makes_the_total_partial_not_free() {
        let tracker = CostTracker::new();
        tracker.add(&priced(1_000, 0, 0.003, false));
        tracker.add(&Usage {
            input_tokens: 5_000,
            ..Default::default()
        });
        let cur = tracker.current();
        assert_eq!(cur.input_tokens, 6_000);
        let est = cur.cost_estimate.unwrap();
        assert!(est.partial, "{est:?}");
        assert!(
            (est.amount_usd - 0.003).abs() < 1e-12,
            "only the priced part is counted"
        );
    }

    /// When nothing was ever priced there is no estimate at all: unknown, not $0.
    #[test]
    fn nothing_priced_means_no_estimate() {
        let tracker = CostTracker::new();
        tracker.add(&Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        });
        assert!(tracker.current().cost_estimate.is_none());
        assert!(tracker.current().cost_usd.is_none());
    }
}

// ─── Built-in tool sets ──────────────────────────────────────────────────────

/// All built-in tools.
pub fn all() -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    tools.extend(filesystem());
    tools.extend(shell());
    tools.extend(web());
    tools.extend(planning());
    tools.extend(scheduling());
    tools.extend(orchestration());
    tools.push(Box::new(ask_user::AskUserQuestionTool));
    tools.push(Box::new(synthetic_output::SyntheticOutputTool));
    tools.push(Box::new(config_tool::ConfigTool));
    // Last, so it indexes every tool registered above.
    let search = tool_search::ToolSearchTool::new(&tools);
    tools.push(Box::new(search));
    tools
}

/// All coding-oriented tools (filesystem + shell + web).
pub fn coding() -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    tools.extend(filesystem());
    tools.extend(shell());
    tools.extend(web());
    tools
}

/// File system tools: Read, Write, Edit, MultiEdit, ApplyPatch, Glob, Grep,
/// CodeSearch, CodeScout, NotebookEdit.
pub fn filesystem() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(file_read::FileReadTool),
        Box::new(file_write::FileWriteTool),
        Box::new(file_edit::FileEditTool),
        Box::new(multi_edit::MultiEditTool),
        Box::new(apply_patch::ApplyPatchTool),
        Box::new(glob_tool::GlobTool),
        Box::new(grep_tool::GrepTool),
        Box::new(code_search::CodeSearchTool::new()),
        Box::new(code_scout::CodeScoutTool),
        Box::new(notebook_edit::NotebookEditTool),
    ]
}

/// Shell tools: Bash (with its background task tools), PowerShell.
pub fn shell() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(bash::BashTool),
        Box::new(bash::BashTaskStatusTool),
        Box::new(bash::BashTaskOutputTool),
        Box::new(bash::BashTaskStopTool),
        Box::new(jobs::JobTool),
        Box::new(powershell::PowerShellTool),
    ]
}

/// Web tools: WebFetch, WebSearch (providers, including Exa, are configured
/// in `bricks.toml` `[web.search]`).
pub fn web() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(web_fetch::WebFetchTool),
        Box::new(web_search::WebSearchTool),
    ]
}

/// Planning tools: EnterPlanMode, ExitPlanMode, TodoWrite.
pub fn planning() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(plan_mode::EnterPlanModeTool),
        Box::new(plan_mode::ExitPlanModeTool),
        Box::new(todo_write::TodoWriteTool),
    ]
}

/// Scheduling tools: Cron (Create/List/Delete), Sleep, RemoteTrigger.
pub fn scheduling() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(cron::CronCreateTool),
        Box::new(cron::CronListTool),
        Box::new(cron::CronDeleteTool),
        Box::new(sleep::SleepTool),
        Box::new(remote_trigger::RemoteTriggerTool),
    ]
}

/// Orchestration tools: SendMessage, Tasks, Worktree.
pub fn orchestration() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(send_message::SendMessageTool),
        Box::new(tasks::TaskCreateTool),
        Box::new(tasks::TaskGetTool),
        Box::new(tasks::TaskUpdateTool),
        Box::new(tasks::TaskListTool),
        Box::new(tasks::TaskStopTool),
        Box::new(tasks::TaskOutputTool),
        Box::new(worktree::EnterWorktreeTool),
        Box::new(worktree::ExitWorktreeTool),
    ]
}

/// No tools (for pure chat agents).
pub fn none() -> Vec<Box<dyn Tool>> {
    vec![]
}

// ─── Unknown-parameter policy (F-10) ─────────────────────────────────────────

/// One policy, applied to every tool: a parameter a tool does not declare is an
/// error, never a silent drop.
///
/// The alternative — accepting near-miss names per tool — was rejected because
/// partial leniency is what caused the bug. `Edit` accepted `path` as an alias
/// for `file_path`, so a model that guessed `path` was *rewarded*, carried the
/// hypothesis to `Grep`, and there `path` means something else entirely: the
/// unknown key was dropped, the search silently widened to the whole working
/// directory, and up to 250 matches from unrelated files came back as though
/// they came from the one file the model asked about. No layer emitted an
/// error, so nothing downstream could recover from it.
///
/// Rejecting is only viable because the rejection is *actionable*:
/// [`tool_feedback`] turns serde's unknown-field error into a message that
/// names the tool, echoes the arguments, points at the parameter the model
/// probably meant, and prints a corrected call.
#[cfg(test)]
mod unknown_parameter_policy {
    use super::*;
    use crate::permissions::AllowAll;
    use std::sync::Arc;

    /// A key no tool declares. Deliberately unmistakable in failure output,
    /// and deliberately *not* `__`-prefixed: that prefix is reserved for the
    /// provider's wire markers and is skipped by the near-miss reporter.
    const UNKNOWN_KEY: &str = "cersei_probe_bogus_param";

    /// The deserializer's wording when it refuses a key it does not know.
    ///
    /// Asserting on this specific phrase is the point of the test. A tool that
    /// merely *echoes* the arguments back inside some other complaint — "missing
    /// field `pattern`" — looks like a rejection but has still silently dropped
    /// the unknown key, which is the bug. Only a deserializer that actually
    /// refuses the key produces this.
    const REJECTION: &str = "unknown field";

    fn ctx_in(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "unknown-param-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    fn required_params(tool: &dyn Tool) -> Vec<String> {
        tool.input_schema()
            .get("required")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every tool must reject a parameter it does not declare.
    ///
    /// The probe sends *only* the unknown key. That is deliberate, and is what
    /// makes running this against the real registry safe: every tool covered
    /// here has at least one required parameter, so the call can never
    /// deserialize into a runnable request and no tool body executes — not
    /// `Bash`, not `Write`, not `CronCreate`. The assertion is only about which
    /// *error* comes back.
    ///
    /// Before `deny_unknown_fields`, the unknown key was discarded during
    /// deserialization and the resulting complaint named the missing required
    /// field, never the key the model actually got wrong.
    #[tokio::test]
    async fn every_tool_rejects_an_unknown_parameter() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_in(tmp.path());

        let mut covered = 0usize;
        let mut failures: Vec<String> = Vec::new();

        for tool in all() {
            // A tool with no required parameter would actually run on this
            // input, so it is not probed this way.
            if required_params(tool.as_ref()).is_empty() {
                continue;
            }
            covered += 1;

            let res = tool
                .execute(serde_json::json!({ UNKNOWN_KEY: "x" }), &ctx)
                .await;

            if !res.is_error {
                failures.push(format!(
                    "{}: accepted an unknown parameter (silent drop)",
                    tool.name()
                ));
            } else if !res.content.contains(REJECTION) {
                failures.push(format!(
                    "{}: failed for some other reason, so the unknown key was still \
                     dropped rather than refused — got: {}",
                    tool.name(),
                    res.content.lines().next().unwrap_or("")
                ));
            } else if !res.content.contains(UNKNOWN_KEY) {
                failures.push(format!(
                    "{}: refused a key without naming it — got: {}",
                    tool.name(),
                    res.content.lines().next().unwrap_or("")
                ));
            }
        }

        assert!(
            covered >= 25,
            "coverage collapsed to {covered} tools; the filter is hiding the registry"
        );
        assert!(
            failures.is_empty(),
            "{} of {} tools mishandled an unknown parameter:\n  {}",
            failures.len(),
            covered,
            failures.join("\n  ")
        );
    }

    /// F-10's exact scenario: the model reads a file with `file_path`, then
    /// searches it with `Grep`, whose parameter is `path`.
    ///
    /// The dangerous outcome is not a failed search — it is a *successful* one.
    /// With `file_path` dropped, `Grep` fell back to the working directory and
    /// returned matches from files the model never asked about, with nothing in
    /// the result to say the scope had changed.
    #[tokio::test]
    async fn grep_does_not_silently_widen_to_the_whole_working_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("target.txt"), "fn login() {}\n").unwrap();
        std::fs::create_dir(tmp.path().join("elsewhere")).unwrap();
        std::fs::write(
            tmp.path().join("elsewhere/decoy.txt"),
            "fn login_unrelated() {}\n",
        )
        .unwrap();

        let res = grep_tool::GrepTool
            .execute(
                serde_json::json!({
                    "pattern": "login",
                    // Wrong name: Grep declares `path`, not `file_path`.
                    "file_path": tmp.path().join("target.txt").to_str().unwrap(),
                }),
                &ctx_in(tmp.path()),
            )
            .await;

        assert!(
            res.is_error,
            "Grep accepted `file_path` and searched somewhere else instead; it returned: {}",
            res.content
        );
        assert!(
            !res.content.contains("decoy"),
            "result leaked matches from outside the requested file: {}",
            res.content
        );
        assert!(
            res.content.contains("file_path"),
            "error must quote the parameter the model sent: {}",
            res.content
        );
        assert!(
            res.content.contains("path"),
            "error must name the real parameter: {}",
            res.content
        );
    }

    /// The other half of F-10: `Edit` accepted `path` as an alias, which is
    /// where the model *learned* the wrong name before carrying it to `Grep`.
    /// One tool rewarding a guess that every other tool punishes is worse than
    /// either policy applied consistently.
    #[tokio::test]
    async fn edit_no_longer_teaches_the_path_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f.rs");
        std::fs::write(&file, "let x = 1;\n").unwrap();

        let res = file_edit::FileEditTool
            .execute(
                serde_json::json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "let x = 1;",
                    "new_string": "let x = 2;",
                }),
                &ctx_in(tmp.path()),
            )
            .await;

        assert!(
            res.is_error,
            "Edit still accepts the `path` alias, so it keeps teaching a name \
             that Grep and Glob silently mis-handle"
        );
        assert!(
            res.content.contains("file_path"),
            "error must name the real parameter: {}",
            res.content
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "let x = 1;\n",
            "a rejected edit must not have touched the file"
        );
    }
}
