//! TodoWrite tool: the plan of a session.
//!
//! The plan belongs to a [`PlanService`], found in the tool context's
//! extensions ([`PlanHandle`]): one per session (or per agent instance),
//! never shared with a sub-agent. A service opened on a file
//! ([`PlanService::open`]) keeps the plan in that file (`plan.json` of the
//! session's folder) and survives restarts; an in-memory one
//! ([`PlanService::in_memory`]) lasts as long as its instance.
//!
//! An update is validated, written (temporary file, then rename), adopted,
//! then published to the observer, in that order and one at a time: a
//! failed write changes nothing and publishes nothing. The revision grows
//! with every update, clearing included (an empty plan is written, never a
//! deleted file), so an older state can always be told from a newer one.

use super::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Weak;

/// The format of `plan.json`.
pub const PLAN_FORMAT_VERSION: u32 = 1;
/// The plan's file in a session's folder.
pub const PLAN_FILE: &str = "plan.json";

/// Services of contexts that carry no [`PlanHandle`] (a tool run on its own),
/// keyed by session id, and the services seen through a handle (for
/// [`get_todos`]). Neither holds a copy of a plan.
static UNATTACHED: once_cell::sync::Lazy<dashmap::DashMap<String, Arc<PlanService>>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);
static SEEN: once_cell::sync::Lazy<dashmap::DashMap<String, Weak<PlanService>>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    #[serde(rename = "activeForm")]
    pub active_form: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// A plan as stored and published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanState {
    /// [`PLAN_FORMAT_VERSION`].
    pub version: u32,
    /// 0 before the first update, then one more with each update.
    pub revision: u64,
    pub todos: Vec<TodoItem>,
    /// What the last update said about itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// Unix milliseconds of the last update (0: never updated).
    pub updated_at: i64,
}

impl Default for PlanState {
    fn default() -> Self {
        Self {
            version: PLAN_FORMAT_VERSION,
            revision: 0,
            todos: Vec::new(),
            explanation: None,
            updated_at: 0,
        }
    }
}

impl PlanState {
    pub fn completed(&self) -> usize {
        self.todos
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count()
    }
}

/// Told of every adopted plan, in revision order, before the update returns;
/// `root_run_id`: the run whose tool call made the update, when known.
pub trait PlanObserver: Send + Sync {
    fn plan_updated(&self, plan: &PlanState, root_run_id: Option<&str>);
}

struct PlanInner {
    state: PlanState,
    path: Option<PathBuf>,
    /// Why the stored plan could not be read: updates are refused until it
    /// is resolved, the file is left as it is.
    problem: Option<String>,
    /// The session was deleted: nothing is written any more.
    closed: bool,
}

/// The plan of one session or agent instance.
pub struct PlanService {
    inner: parking_lot::Mutex<PlanInner>,
    observer: parking_lot::RwLock<Option<Arc<dyn PlanObserver>>>,
}

/// The extension giving TodoWrite its plan.
#[derive(Clone)]
pub struct PlanHandle(pub Arc<PlanService>);

impl PlanService {
    fn with(inner: PlanInner) -> Arc<Self> {
        Arc::new(Self {
            inner: parking_lot::Mutex::new(inner),
            observer: parking_lot::RwLock::new(None),
        })
    }

    /// A plan kept in memory, for this instance only.
    pub fn in_memory() -> Arc<Self> {
        Self::with(PlanInner {
            state: PlanState::default(),
            path: None,
            problem: None,
            closed: false,
        })
    }

    /// The plan stored at `path`. No file: no plan yet. An unreadable file,
    /// invalid content or an unknown version is reported by
    /// [`Self::problem`] and refuses updates; the file is not touched.
    pub fn open(path: PathBuf) -> Arc<Self> {
        let (state, problem) = match read_plan(&path) {
            Ok(state) => (state.unwrap_or_default(), None),
            Err(e) => (PlanState::default(), Some(e)),
        };
        Self::with(PlanInner {
            state,
            path: Some(path),
            problem,
            closed: false,
        })
    }

    /// [`Self::open`] on `plan.json` of a session's folder.
    pub fn in_dir(files_dir: &Path) -> Arc<Self> {
        Self::open(files_dir.join(PLAN_FILE))
    }

    pub fn current(&self) -> PlanState {
        self.inner.lock().state.clone()
    }

    pub fn problem(&self) -> Option<String> {
        self.inner.lock().problem.clone()
    }

    pub fn is_durable(&self) -> bool {
        self.inner.lock().path.is_some()
    }

    pub fn set_observer(&self, observer: Option<Arc<dyn PlanObserver>>) {
        *self.observer.write() = observer;
    }

    /// Stop writing: the session's folder is being deleted. Later updates
    /// are refused rather than recreating the file.
    pub fn close(&self) {
        self.inner.lock().closed = true;
    }

    /// Replace the plan with `todos` (the complete list; empty clears it).
    /// Returns the adopted state once it is written and published.
    pub fn update(
        &self,
        todos: Vec<TodoItem>,
        explanation: Option<String>,
    ) -> std::result::Result<PlanState, String> {
        self.update_by(todos, explanation, None)
    }

    /// [`Self::update`], made by a tool call of the run `root_run_id`.
    pub fn update_by(
        &self,
        todos: Vec<TodoItem>,
        explanation: Option<String>,
        root_run_id: Option<&str>,
    ) -> std::result::Result<PlanState, String> {
        validate(&todos)?;
        let explanation = explanation
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err("this conversation was deleted; its plan is no longer kept".into());
        }
        if let Some(p) = &inner.problem {
            return Err(format!(
                "the stored plan cannot be read ({p}); it was left unchanged and no update is \
                 accepted until the file is repaired or removed"
            ));
        }
        let next = PlanState {
            version: PLAN_FORMAT_VERSION,
            revision: inner.state.revision + 1,
            todos,
            explanation,
            updated_at: chrono::Utc::now().timestamp_millis(),
        };
        if let Some(path) = &inner.path {
            write_plan(path, &next).map_err(|e| format!("the plan could not be saved: {e}"))?;
        }
        inner.state = next.clone();
        // Published under the lock: observers see the revisions in order.
        if let Some(o) = self.observer.read().clone() {
            o.plan_updated(&next, root_run_id);
        }
        Ok(next)
    }
}

/// At most one step in progress; every step says what it is.
fn validate(todos: &[TodoItem]) -> std::result::Result<(), String> {
    for (i, t) in todos.iter().enumerate() {
        if t.content.trim().is_empty() {
            return Err(format!(
                "step {} has an empty `content`; give each step a short imperative description",
                i + 1
            ));
        }
        if t.active_form.trim().is_empty() {
            return Err(format!(
                "step {} has an empty `activeForm`; give its present continuous form (e.g. \
                 \"Running tests\")",
                i + 1
            ));
        }
    }
    let active: Vec<usize> = todos
        .iter()
        .enumerate()
        .filter(|(_, t)| t.status == TodoStatus::InProgress)
        .map(|(i, _)| i + 1)
        .collect();
    if active.len() > 1 {
        return Err(format!(
            "steps {} are all `in_progress`; keep at most one step in progress and mark the \
             others `pending` or `completed`",
            active
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}

fn read_plan(path: &Path) -> std::result::Result<Option<PlanState>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("{}: invalid JSON ({e})", path.display()))?;
    match value.get("version").and_then(Value::as_u64) {
        Some(v) if v == u64::from(PLAN_FORMAT_VERSION) => {}
        Some(v) => return Err(format!("{}: unknown plan version {v}", path.display())),
        None => return Err(format!("{}: no plan version", path.display())),
    }
    let state: PlanState = serde_json::from_value(value)
        .map_err(|e| format!("{}: invalid plan ({e})", path.display()))?;
    validate(&state.todos).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(state))
}

fn write_plan(path: &Path, state: &PlanState) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{PLAN_FILE}."))
        .suffix(".tmp")
        .tempfile_in(dir)?;
    let text = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
    tmp.write_all(&text)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// The service a context writes to: its handle, else one kept for its
/// session id.
fn service_of(ctx: &ToolContext) -> Arc<PlanService> {
    if let Some(h) = ctx.extensions.get::<PlanHandle>() {
        SEEN.insert(ctx.session_id.clone(), Arc::downgrade(&h.0));
        return Arc::clone(&h.0);
    }
    UNATTACHED
        .entry(ctx.session_id.clone())
        .or_insert_with(PlanService::in_memory)
        .clone()
}

fn known_service(session_id: &str) -> Option<Arc<PlanService>> {
    UNATTACHED
        .get(session_id)
        .map(|s| Arc::clone(s.value()))
        .or_else(|| SEEN.get(session_id).and_then(|w| w.upgrade()))
}

/// The current steps of a session's plan (compatibility: read from the
/// service TodoWrite last used for that session).
pub fn get_todos(session_id: &str) -> Vec<TodoItem> {
    known_service(session_id)
        .map(|s| s.current().todos)
        .unwrap_or_default()
}

/// Clear a session's plan (compatibility: an empty update of its service).
pub fn clear_todos(session_id: &str) {
    if let Some(s) = known_service(session_id)
        && !s.current().todos.is_empty()
    {
        let _ = s.update(Vec::new(), None);
    }
}

/// The plan as shown to the model: one line per step, then the explanation.
pub fn render_plan(plan: &PlanState) -> String {
    let mut lines: Vec<String> = plan
        .todos
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mark = match t.status {
                TodoStatus::Completed => "[x]",
                TodoStatus::InProgress => "[>]",
                TodoStatus::Pending => "[ ]",
            };
            format!("{}. {mark} {}", i + 1, t.content)
        })
        .collect();
    if let Some(e) = &plan.explanation {
        lines.push(format!("Last update: {e}"));
    }
    lines.join("\n")
}

pub struct TodoWriteTool;

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "TodoWrite"
    }
    fn description(&self) -> &str {
        "Create and update the plan of the conversation: the complete list of its steps, each \
         pending, in_progress or completed. Send the whole updated list each time; at most one \
         step is in_progress; an empty list clears the plan. The plan is kept with the \
         conversation and shown to the user."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete updated list of steps (an empty list clears the plan)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string", "description": "Task description (imperative form)" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                            "activeForm": { "type": "string", "description": "Present continuous form (e.g., 'Running tests')" }
                        },
                        "required": ["content", "status", "activeForm"]
                    }
                },
                "explanation": {
                    "type": "string",
                    "description": "Optional: why the plan changed (a step added, dropped or blocked)"
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            todos: Vec<TodoItem>,
            #[serde(default)]
            explanation: Option<String>,
        }

        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };

        let run = ctx
            .extensions
            .get::<crate::jobs::JobOwner>()
            .map(|o| o.root_run_id.clone());
        match service_of(ctx).update_by(input.todos, input.explanation, run.as_deref()) {
            Ok(plan) if plan.todos.is_empty() => {
                ToolResult::success(format!("Plan cleared (revision {}).", plan.revision))
            }
            Ok(plan) => ToolResult::success(format!(
                "Plan updated (revision {}, {}/{} completed):\n{}",
                plan.revision,
                plan.completed(),
                plan.todos.len(),
                render_plan(&plan)
            )),
            Err(e) => ToolResult::error(format!("TodoWrite: {e}. The previous plan is unchanged.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use std::sync::Mutex;

    fn test_ctx() -> ToolContext {
        ToolContext {
            working_dir: std::env::temp_dir(),
            session_id: "todo-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    fn ctx_with(service: &Arc<PlanService>, session: &str) -> ToolContext {
        let ctx = ToolContext {
            session_id: session.into(),
            ..test_ctx()
        };
        ctx.extensions.insert(PlanHandle(Arc::clone(service)));
        ctx
    }

    fn step(content: &str, status: &str) -> Value {
        serde_json::json!({"content": content, "status": status, "activeForm": format!("Doing {content}")})
    }

    struct Seen(Mutex<Vec<u64>>);
    impl PlanObserver for Seen {
        fn plan_updated(&self, plan: &PlanState, _: Option<&str>) {
            self.0.lock().unwrap().push(plan.revision);
        }
    }

    #[tokio::test]
    async fn test_todo_create_and_read() {
        clear_todos("todo-test");
        let tool = TodoWriteTool;
        let result = tool
            .execute(
                serde_json::json!({
                    "todos": [
                        {"content": "Build feature", "status": "in_progress", "activeForm": "Building feature"},
                        {"content": "Write tests", "status": "pending", "activeForm": "Writing tests"}
                    ]
                }),
                &test_ctx(),
            )
            .await;
        assert!(!result.is_error);
        assert!(
            result.content.contains("0/2 completed"),
            "{}",
            result.content
        );

        let todos = get_todos("todo-test");
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].status, TodoStatus::InProgress);
        assert_eq!(todos[1].status, TodoStatus::Pending);
    }

    #[tokio::test]
    async fn test_todo_update() {
        clear_todos("todo-test2");
        let tool = TodoWriteTool;
        let ctx = ToolContext {
            session_id: "todo-test2".into(),
            ..test_ctx()
        };

        // Create
        tool.execute(
            serde_json::json!({
                "todos": [{"content": "Task A", "status": "pending", "activeForm": "Doing A"}]
            }),
            &ctx,
        )
        .await;

        // Update to completed
        tool.execute(
            serde_json::json!({
                "todos": [{"content": "Task A", "status": "completed", "activeForm": "Doing A"}]
            }),
            &ctx,
        )
        .await;

        let todos = get_todos("todo-test2");
        assert_eq!(todos[0].status, TodoStatus::Completed);
    }

    #[tokio::test]
    async fn two_steps_in_progress_are_refused_and_the_plan_is_kept() {
        let svc = PlanService::in_memory();
        let ctx = ctx_with(&svc, "todo-two-active");
        let ok = TodoWriteTool
            .execute(
                serde_json::json!({"todos": [step("A", "in_progress")]}),
                &ctx,
            )
            .await;
        assert!(!ok.is_error);
        let bad = TodoWriteTool
            .execute(
                serde_json::json!({"todos": [step("A", "in_progress"), step("B", "in_progress")]}),
                &ctx,
            )
            .await;
        assert!(bad.is_error);
        assert!(bad.content.contains("at most one step"), "{}", bad.content);
        assert!(bad.content.contains("unchanged"));
        let plan = svc.current();
        assert_eq!(plan.revision, 1);
        assert_eq!(plan.todos.len(), 1);
    }

    #[tokio::test]
    async fn invalid_steps_change_nothing() {
        let svc = PlanService::in_memory();
        let ctx = ctx_with(&svc, "todo-invalid");
        for input in [
            serde_json::json!({"todos": [step("", "pending")]}),
            serde_json::json!({"todos": [{"content": "A", "status": "pending", "activeForm": " "}]}),
            serde_json::json!({"todos": [{"content": "A", "status": "done", "activeForm": "a"}]}),
            serde_json::json!({"todos": [{"content": 3, "status": "pending", "activeForm": "a"}]}),
            serde_json::json!({"todos": "A"}),
        ] {
            let r = TodoWriteTool.execute(input.clone(), &ctx).await;
            assert!(r.is_error, "{input}");
        }
        assert_eq!(svc.current(), PlanState::default());
        // Zero steps in progress is valid.
        let r = TodoWriteTool
            .execute(
                serde_json::json!({"todos": [step("A", "pending"), step("B", "completed")]}),
                &ctx,
            )
            .await;
        assert!(!r.is_error);
    }

    #[tokio::test]
    async fn clearing_keeps_the_revision_and_the_explanation_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let svc = PlanService::in_dir(dir.path());
        let ctx = ctx_with(&svc, "todo-clear");
        TodoWriteTool
            .execute(
                serde_json::json!({"todos": [step("A", "pending")], "explanation": "first plan"}),
                &ctx,
            )
            .await;
        assert_eq!(svc.current().explanation.as_deref(), Some("first plan"));
        let r = TodoWriteTool
            .execute(serde_json::json!({"todos": []}), &ctx)
            .await;
        assert!(r.content.contains("cleared"), "{}", r.content);
        let stored = read_plan(&dir.path().join(PLAN_FILE)).unwrap().unwrap();
        assert_eq!(stored.revision, 2);
        assert!(stored.todos.is_empty());
        assert_eq!(stored.explanation, None);
        // Reopened: same plan, same revision; the next update follows it.
        let again = PlanService::in_dir(dir.path());
        assert_eq!(again.current(), stored);
        assert_eq!(again.update(vec![], None).unwrap().revision, 3);
    }

    #[test]
    fn a_failed_write_keeps_the_previous_state_and_publishes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // The plan's path is a directory: the rename fails.
        let path = dir.path().join("plan.json");
        let svc = PlanService::open(path.clone());
        let seen = Arc::new(Seen(Mutex::new(Vec::new())));
        svc.set_observer(Some(seen.clone()));
        let first = svc
            .update(
                vec![TodoItem {
                    content: "A".into(),
                    status: TodoStatus::Pending,
                    active_form: "Doing A".into(),
                }],
                None,
            )
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), "x").unwrap();
        let err = svc.update(vec![], None).unwrap_err();
        assert!(err.contains("could not be saved"), "{err}");
        assert_eq!(svc.current(), first);
        assert_eq!(*seen.0.lock().unwrap(), vec![1]);
    }

    #[test]
    fn a_corrupt_or_unknown_file_is_reported_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PLAN_FILE);
        for text in [
            "{ not json",
            r#"{"version": 9, "revision": 1, "todos": [], "updated_at": 0}"#,
            r#"{"revision": 1, "todos": [], "updated_at": 0}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            let svc = PlanService::open(path.clone());
            assert!(svc.problem().is_some(), "{text}");
            assert_eq!(svc.current(), PlanState::default());
            let err = svc.update(vec![], None).unwrap_err();
            assert!(err.contains("cannot be read"), "{err}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
        // No file: no plan, and no problem.
        std::fs::remove_file(&path).unwrap();
        let svc = PlanService::open(path);
        assert!(svc.problem().is_none());
        assert_eq!(svc.current().revision, 0);
    }

    #[test]
    fn concurrent_updates_publish_increasing_revisions_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let svc = PlanService::in_dir(dir.path());
        let seen = Arc::new(Seen(Mutex::new(Vec::new())));
        svc.set_observer(Some(seen.clone()));
        std::thread::scope(|s| {
            for i in 0..8 {
                let svc = Arc::clone(&svc);
                s.spawn(move || {
                    for j in 0..5 {
                        svc.update(
                            vec![TodoItem {
                                content: format!("{i}-{j}"),
                                status: TodoStatus::Pending,
                                active_form: "Doing".into(),
                            }],
                            None,
                        )
                        .unwrap();
                    }
                });
            }
        });
        let seen = seen.0.lock().unwrap().clone();
        assert_eq!(seen, (1..=40).collect::<Vec<u64>>());
        let stored = read_plan(&dir.path().join(PLAN_FILE)).unwrap().unwrap();
        assert_eq!(stored, svc.current());
        assert_eq!(stored.revision, 40);
    }

    #[test]
    fn a_closed_service_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let files = dir.path().join("s.files");
        let svc = PlanService::in_dir(&files);
        svc.close();
        assert!(svc.update(vec![], None).is_err());
        assert!(!files.exists());
    }

    #[tokio::test]
    async fn two_sessions_keep_two_plans() {
        let a = PlanService::in_memory();
        let b = PlanService::in_memory();
        TodoWriteTool
            .execute(
                serde_json::json!({"todos": [step("A", "pending")]}),
                &ctx_with(&a, "todo-sa"),
            )
            .await;
        assert_eq!(a.current().todos.len(), 1);
        assert!(b.current().todos.is_empty());
        assert_eq!(get_todos("todo-sa").len(), 1);
    }
}
