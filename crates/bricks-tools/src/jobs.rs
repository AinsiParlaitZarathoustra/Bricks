//! Commands as jobs: one registry per session over the shell's supervised
//! tasks (`Bash` with `background: true`, or handed over), with an owner, a
//! quota, events, durable logs, and the `Job` tool (`list`, `status`,
//! `output`, `wait`, `stop`).
//!
//! * A job is either a background command (`background: true`, explicitly
//!   durable) or a foreground command handed over after `yield_time_ms`
//!   (not durable: it ends with its run). Both are seen through `Job` by the
//!   agent that started them.
//! * A job belongs to the agent that started it (recorded with its root run
//!   and workspace); only that agent sees it through `Job` (an id guessed
//!   from another agent or session gives nothing). Frontends see the
//!   session's jobs.
//! * It runs from a snapshot of its owner's shell (working directory,
//!   exported variables, functions, aliases); what it changes never reaches
//!   that shell. A sub-agent's jobs are stopped when it ends; the session
//!   agent's jobs live until the session closes. No job outlives Bricks.
//! * Quota: a place is reserved before anything starts ([`JobRegistry::reserve`],
//!   atomic), and kept by the job until it ends; beyond `max_jobs` the start
//!   is refused, with nothing launched.
//! * Stop: TERM to the task's process group and tree, a grace period, then
//!   KILL; idempotent. A process that leaves its group and the task's tree
//!   on purpose can escape (no sandbox).
//! * Output: stdout and stderr apart, paged by line offsets and bounded in
//!   bytes; the pipes are always drained; when a job ends its raw logs (each
//!   capped) are copied to the session's files.
//! * `ready_pattern` makes a job "ready"; ready is never proof that the
//!   service works, and running is not ready.

use super::*;
use crate::shell::background::{Readiness, Stream, Task, TaskState};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Who started a job (set by the runner in each agent's tool context).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobOwner {
    pub agent_id: String,
    pub root_run_id: String,
    pub workspace: PathBuf,
}

/// Limits of the session's jobs (`[background]` in `bricks.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JobSettings {
    /// Jobs running at once in the session.
    pub max_jobs: usize,
    /// Bytes kept in memory per stream of a job.
    pub output_buffer_bytes: usize,
    /// Longest line kept whole.
    pub max_line_bytes: usize,
    /// Size of each raw log (and of its durable copy).
    pub raw_log_bytes: u64,
    /// Grace between TERM and KILL on stop.
    pub stop_grace_ms: u64,
    /// Largest page `output` returns, in bytes.
    pub max_page_bytes: usize,
}

impl Default for JobSettings {
    fn default() -> Self {
        Self {
            max_jobs: 16,
            output_buffer_bytes: 1024 * 1024,
            max_line_bytes: 64 * 1024,
            raw_log_bytes: 50 * 1024 * 1024,
            stop_grace_ms: 2_000,
            max_page_bytes: 64 * 1024,
        }
    }
}

/// Job lifecycle, for event streams.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobEvent {
    Started {
        job_id: String,
        agent_id: String,
        root_run_id: String,
        command: String,
        cwd: String,
        pid: u32,
    },
    /// Throttled: how much output so far (never the output itself).
    Output {
        job_id: String,
        stdout_bytes: u64,
        stderr_bytes: u64,
    },
    Finished {
        job_id: String,
        /// `completed`, `failed`, `stopped`.
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        duration_ms: u64,
        /// Durable copies of the raw logs.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        logs: Vec<String>,
    },
}

pub type JobSink = Arc<dyn Fn(JobEvent) + Send + Sync>;

struct Entry {
    id: String,
    owner: JobOwner,
    task: Arc<Task>,
    cwd: PathBuf,
    logs: Vec<String>,
    /// Explicitly started in the background (not a handed-over command):
    /// survives the run that started it.
    durable: bool,
}

#[derive(Default)]
struct Table {
    entries: Vec<Entry>,
    /// Places reserved and not yet taken by a job.
    reserved: usize,
}

/// The session's jobs.
pub struct JobRegistry {
    pub settings: JobSettings,
    table: Mutex<Table>,
    sink: Mutex<Option<JobSink>>,
    /// Where finished jobs' logs are copied.
    durable_dir: Option<PathBuf>,
}

/// A place reserved for a job: taken by [`JobRegistry::register`], given
/// back when dropped unused (the start failed).
pub struct JobSlot {
    registry: Option<Arc<JobRegistry>>,
}

impl Drop for JobSlot {
    fn drop(&mut self) {
        if let Some(reg) = self.registry.take() {
            reg.table.lock().reserved -= 1;
        }
    }
}

/// The session's job registry, in a tool context.
#[derive(Clone)]
pub struct JobsHandle(pub Arc<JobRegistry>);

impl JobRegistry {
    pub fn new(settings: JobSettings, durable_dir: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            settings,
            table: Mutex::new(Table::default()),
            sink: Mutex::new(None),
            durable_dir,
        })
    }

    pub fn set_sink(&self, sink: JobSink) {
        *self.sink.lock() = Some(sink);
    }

    fn emit(&self, ev: JobEvent) {
        if let Some(s) = self.sink.lock().clone() {
            s(ev);
        }
    }

    /// Jobs running now.
    pub fn running(&self) -> usize {
        self.table
            .lock()
            .entries
            .iter()
            .filter(|e| !e.task.state().is_finished())
            .count()
    }

    /// Reserves a place for a job, atomically: the places in use are the
    /// running jobs plus the reservations. Refused beyond `max_jobs`, before
    /// anything is started.
    pub fn reserve(self: &Arc<Self>) -> std::result::Result<JobSlot, String> {
        let mut table = self.table.lock();
        let running = table
            .entries
            .iter()
            .filter(|e| !e.task.state().is_finished())
            .count();
        let n = running + table.reserved;
        if n >= self.settings.max_jobs {
            return Err(format!(
                "{n} background job(s) already run or are starting in this session; the limit is {} ([background] max_jobs). Stop one first (Job stop).",
                self.settings.max_jobs
            ));
        }
        table.reserved += 1;
        Ok(JobSlot {
            registry: Some(Arc::clone(self)),
        })
    }

    /// Track a started task as a job, in the place `slot` reserved; watch it
    /// until it ends. `durable`: a background command, which outlives its run.
    pub fn register(
        self: &Arc<Self>,
        mut slot: JobSlot,
        owner: JobOwner,
        task: Arc<Task>,
        cwd: &Path,
        durable: bool,
    ) -> String {
        let id = format!("job_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        self.emit(JobEvent::Started {
            job_id: id.clone(),
            agent_id: owner.agent_id.clone(),
            root_run_id: owner.root_run_id.clone(),
            command: task.command.chars().take(300).collect(),
            cwd: cwd.display().to_string(),
            pid: task.pid,
        });
        {
            // The place reserved becomes the job, in one step.
            let mut table = self.table.lock();
            table.reserved -= 1;
            slot.registry = None;
            table.entries.push(Entry {
                id: id.clone(),
                owner,
                task: Arc::clone(&task),
                cwd: cwd.to_path_buf(),
                logs: Vec::new(),
                durable,
            });
        }
        let me = Arc::clone(self);
        let jid = id.clone();
        tokio::spawn(async move {
            let mut last = (0u64, 0u64);
            loop {
                if task.wait_finished(Duration::from_secs(2)).await {
                    break;
                }
                let st = task.status();
                let now = (st.stdout_bytes, st.stderr_bytes);
                if now != last {
                    last = now;
                    me.emit(JobEvent::Output {
                        job_id: jid.clone(),
                        stdout_bytes: now.0,
                        stderr_bytes: now.1,
                    });
                }
            }
            let logs = me.preserve_logs(&jid, &task);
            if let Some(e) = me.table.lock().entries.iter_mut().find(|e| e.id == jid) {
                e.logs = logs.clone();
            }
            let (state, code, signal) = match task.state() {
                TaskState::Completed { code } => ("completed", Some(code), None),
                TaskState::Failed { code, signal } => ("failed", code, signal),
                TaskState::Stopped => ("stopped", None, None),
                _ => ("failed", None, None),
            };
            me.emit(JobEvent::Finished {
                job_id: jid,
                state: state.into(),
                code,
                signal,
                duration_ms: task.elapsed().as_millis() as u64,
                logs,
            });
        });
        id
    }

    /// Copy a finished job's raw logs (each capped) to the session's files.
    fn preserve_logs(&self, id: &str, task: &Task) -> Vec<String> {
        let Some(dir) = &self.durable_dir else {
            return Vec::new();
        };
        let dir = dir.join(id);
        if std::fs::create_dir_all(&dir).is_err() {
            return Vec::new();
        }
        let (out, err) = task.raw_logs();
        let mut kept = Vec::new();
        for (src, name) in [(out, "stdout.log"), (err, "stderr.log")] {
            let dst = dir.join(name);
            if std::fs::copy(&src, &dst).is_ok() {
                kept.push(dst.display().to_string());
            }
        }
        kept
    }

    /// A job its owner (or a frontend: `viewer` = `None`) may see.
    fn find(
        &self,
        id: &str,
        viewer: Option<&str>,
    ) -> Option<(Arc<Task>, JobOwner, PathBuf, Vec<String>)> {
        self.table
            .lock()
            .entries
            .iter()
            .find(|e| e.id == id && viewer.is_none_or(|v| e.owner.agent_id == v))
            .map(|e| {
                (
                    Arc::clone(&e.task),
                    e.owner.clone(),
                    e.cwd.clone(),
                    e.logs.clone(),
                )
            })
    }

    /// The jobs `viewer` may see, with what `Job` shows of them.
    pub fn views(&self, viewer: Option<&str>) -> Vec<JobView> {
        self.table
            .lock()
            .entries
            .iter()
            .filter(|e| viewer.is_none_or(|v| e.owner.agent_id == v))
            .map(|e| JobView {
                id: e.id.clone(),
                task: Arc::clone(&e.task),
                cwd: e.cwd.clone(),
                kept_logs: e.logs.clone(),
            })
            .collect()
    }

    /// One job `viewer` may see.
    pub fn view(&self, id: &str, viewer: Option<&str>) -> Option<JobView> {
        self.views(viewer).into_iter().find(|v| v.id == id)
    }

    /// `(id, owner, status)` of the jobs `viewer` may see.
    pub fn list(
        &self,
        viewer: Option<&str>,
    ) -> Vec<(String, JobOwner, crate::shell::background::TaskStatus)> {
        self.table
            .lock()
            .entries
            .iter()
            .filter(|e| viewer.is_none_or(|v| e.owner.agent_id == v))
            .map(|e| (e.id.clone(), e.owner.clone(), e.task.status()))
            .collect()
    }

    /// Stop a job (idempotent).
    pub async fn stop(&self, id: &str, viewer: Option<&str>) -> Option<TaskState> {
        let (task, ..) = self.find(id, viewer)?;
        Some(
            task.stop(Duration::from_millis(self.settings.stop_grace_ms))
                .await,
        )
    }

    /// Stop every job of an agent (it ended).
    pub async fn stop_owned_by(&self, agent_id: &str) {
        let tasks: Vec<Arc<Task>> = self
            .table
            .lock()
            .entries
            .iter()
            .filter(|e| e.owner.agent_id == agent_id)
            .map(|e| Arc::clone(&e.task))
            .collect();
        for t in tasks {
            t.stop(Duration::from_millis(self.settings.stop_grace_ms))
                .await;
        }
    }

    /// Stop the commands of an agent that were handed over (not the
    /// background jobs it started explicitly): its run ended.
    pub async fn stop_ordinary_owned_by(&self, agent_id: &str) {
        let tasks: Vec<Arc<Task>> = self
            .table
            .lock()
            .entries
            .iter()
            .filter(|e| e.owner.agent_id == agent_id && !e.durable)
            .map(|e| Arc::clone(&e.task))
            .collect();
        for t in tasks {
            t.stop(Duration::from_millis(self.settings.stop_grace_ms))
                .await;
        }
    }

    /// Stop everything (the session closes).
    pub async fn stop_all(&self) {
        let tasks: Vec<Arc<Task>> = self
            .table
            .lock()
            .entries
            .iter()
            .map(|e| Arc::clone(&e.task))
            .collect();
        for t in tasks {
            t.stop(Duration::from_millis(self.settings.stop_grace_ms))
                .await;
        }
    }
}

/// A job as `Job` shows it.
#[derive(Clone)]
pub struct JobView {
    pub id: String,
    pub task: Arc<Task>,
    pub cwd: PathBuf,
    /// Durable copies of the raw logs, once they exist (the job ended).
    pub kept_logs: Vec<String>,
}

impl JobView {
    /// One line: id, state and exit, pid, duration, readiness, what each
    /// stream received, directory and command.
    fn line(&self) -> String {
        let st = self.task.status();
        let exit = match &st.state {
            TaskState::Completed { code } => format!(" (exit {code})"),
            TaskState::Failed { code, signal } => match (code, signal) {
                (Some(c), _) => format!(" (exit {c})"),
                (None, Some(s)) => format!(" (signal {s})"),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let ready = match &st.readiness {
            Readiness::NotChecked => "not checked".to_string(),
            Readiness::Waiting => "not yet".to_string(),
            Readiness::Ready { after, .. } => {
                format!("yes, after {}", bricks_types::duration::display_ms(*after))
            }
        };
        format!(
            "{} [{}{exit}] pid {} · {} · ready: {ready} · stdout {} lines / {} B · stderr {} lines / {} B · cwd {} · {}",
            self.id,
            st.state.label(),
            st.pid,
            bricks_types::duration::display_ms(st.elapsed),
            st.stdout_lines,
            st.stdout_bytes,
            st.stderr_lines,
            st.stderr_bytes,
            self.cwd.display(),
            st.command
        )
    }

    /// Where the raw logs are now: the durable copies once made, else the
    /// live logs (never a copy announced before it exists).
    fn logs_line(&self) -> String {
        if !self.kept_logs.is_empty() {
            return format!("raw logs (kept): {}", self.kept_logs.join(", "));
        }
        let (out, err) = self.task.raw_logs();
        let present: Vec<String> = [out, err]
            .into_iter()
            .filter(|p| p.exists())
            .map(|p| p.display().to_string())
            .collect();
        if present.is_empty() {
            "raw logs: no longer available".to_string()
        } else {
            format!("raw logs: {}", present.join(", "))
        }
    }
}

/// What a job id the caller cannot see gets: the same answer whether it does
/// not exist or belongs to another agent.
pub const NO_ACCESSIBLE_JOB: &str = "No accessible job with this id. A job is visible only to the \
    agent that started it. Use Job with action list to see your jobs.";

/// Marker of a line cut to fit a page.
const PAGE_CUT: &str = " [truncated to fit the page; the whole line is in the raw log]";

/// `Job`: the calling agent's jobs, on its session's registry.
pub struct JobTool {
    registry: Arc<JobRegistry>,
}

impl JobTool {
    pub fn new(registry: Arc<JobRegistry>) -> Self {
        Self { registry }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JobAction {
    #[default]
    List,
    Status,
    Output,
    Wait,
    Stop,
}

impl JobAction {
    fn name(self) -> &'static str {
        match self {
            JobAction::List => "list",
            JobAction::Status => "status",
            JobAction::Output => "output",
            JobAction::Wait => "wait",
            JobAction::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JobStream {
    #[default]
    Stdout,
    Stderr,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobInput {
    #[serde(default)]
    action: JobAction,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    stream: JobStream,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    tail: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

const DEFAULT_LIMIT: u64 = 200;
const MAX_LIMIT: u64 = 2000;
const DEFAULT_WAIT_MS: u64 = 60_000;
const MAX_JOB_WAIT_MS: u64 = 600_000;

impl JobInput {
    /// The values out of range are refused, never corrected.
    fn check(&self) -> std::result::Result<(), String> {
        if let Some(l) = self.limit
            && !(1..=MAX_LIMIT).contains(&l)
        {
            return Err(format!("`limit` must be between 1 and {MAX_LIMIT} (got {l})."));
        }
        if let Some(t) = self.timeout_ms
            && !(1..=MAX_JOB_WAIT_MS).contains(&t)
        {
            return Err(format!(
                "`timeout_ms` must be between 1 and {MAX_JOB_WAIT_MS} (got {t})."
            ));
        }
        if self.tail && self.offset.is_some() {
            return Err(
                "`tail` returns the last `limit` lines: give `tail` or `offset`, not both.".into(),
            );
        }
        if self.action != JobAction::List
            && self.job_id.as_deref().is_none_or(|id| id.trim().is_empty())
        {
            return Err(format!(
                "`{}` needs a non-empty `job_id`. Use Job with action list to see your jobs.",
                self.action.name()
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for JobTool {
    fn name(&self) -> &str {
        "Job"
    }

    fn description(&self) -> &str {
        "Follow your jobs: the commands you started with Bash `background: true`, and the \
         foreground commands handed over after `yield_time_ms` (both give a job_id). `list` \
         (default): your jobs. `status`: one job, with its raw log paths. `output`: lines of \
         stdout or stderr from `offset` (0-based), or the last `limit` lines with `tail`. `wait`: \
         wait for its end up to `timeout_ms` (a timeout stops nothing). `stop`: TERM then KILL, \
         idempotent. Running is not ready, and ready is not proof that a service works."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn permission_level_for(&self, input: &Value) -> PermissionLevel {
        match input["action"].as_str() {
            Some("stop") => PermissionLevel::Execute,
            _ => PermissionLevel::ReadOnly,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }

    fn job_registry(&self) -> Option<Arc<JobRegistry>> {
        Some(Arc::clone(&self.registry))
    }

    fn on_job_registry(&self, registry: &Arc<JobRegistry>) -> Option<Box<dyn Tool>> {
        Some(Box::new(JobTool::new(Arc::clone(registry))))
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "status", "output", "wait", "stop"],
                    "default": "list"
                },
                "job_id": { "type": "string", "minLength": 1 },
                "stream": { "type": "string", "enum": ["stdout", "stderr"], "default": "stdout" },
                "offset": { "type": "integer", "minimum": 0 },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_LIMIT,
                    "default": DEFAULT_LIMIT
                },
                "tail": {
                    "type": "boolean",
                    "default": false,
                    "description": "Return the last limit lines for output; incompatible with offset."
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_JOB_WAIT_MS,
                    "default": DEFAULT_WAIT_MS
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let i: JobInput = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        if let Err(e) = i.check() {
            return ToolResult::error(e);
        }
        let Some(viewer) = ctx.extensions.get::<JobOwner>().map(|o| o.agent_id.clone()) else {
            return ToolResult::error(
                "this agent has no identity, so no job can be shown (the agent was assembled \
                 without one).",
            );
        };
        let reg = &self.registry;
        if i.action == JobAction::List {
            let jobs = reg.views(Some(&viewer));
            if jobs.is_empty() {
                return ToolResult::success(
                    "You have no jobs. Bash starts one with `background: true`, or when a \
                     command is still running after `yield_time_ms`.",
                );
            }
            return ToolResult::success(
                jobs.iter().map(JobView::line).collect::<Vec<_>>().join("\n"),
            );
        }
        let id = i.job_id.as_deref().unwrap_or_default().trim();
        let Some(job) = reg.view(id, Some(&viewer)) else {
            return ToolResult::error(NO_ACCESSIBLE_JOB);
        };
        match i.action {
            JobAction::List => unreachable!("handled above"),
            JobAction::Status => {
                ToolResult::success(format!("{}\n{}", job.line(), job.logs_line()))
            }
            JobAction::Output => {
                let stream = match i.stream {
                    JobStream::Stdout => Stream::Stdout,
                    JobStream::Stderr => Stream::Stderr,
                };
                let limit = i.limit.unwrap_or(DEFAULT_LIMIT) as usize;
                let position = if i.tail {
                    crate::shell::background::OutputPosition::Tail
                } else {
                    crate::shell::background::OutputPosition::Offset(i.offset.unwrap_or(0))
                };
                let page = job.task.read(stream, position, limit);
                ToolResult::success(render_page(&page, reg.settings.max_page_bytes))
            }
            JobAction::Wait => {
                let limit = Duration::from_millis(i.timeout_ms.unwrap_or(DEFAULT_WAIT_MS));
                let ended = job.task.wait_finished(limit).await;
                let view = reg.view(id, Some(&viewer)).unwrap_or(job);
                if ended {
                    ToolResult::success(view.line())
                } else {
                    ToolResult::success(format!(
                        "{}\nStill running after {}: the wait timed out, and the job was not \
                         stopped (wait again, read its output, or stop it).",
                        view.line(),
                        bricks_types::duration::display_ms(limit)
                    ))
                }
            }
            JobAction::Stop => {
                job.task
                    .stop(Duration::from_millis(reg.settings.stop_grace_ms))
                    .await;
                let view = reg.view(id, Some(&viewer)).unwrap_or(job);
                ToolResult::success(view.line())
            }
        }
    }
}

/// A page of output: a header (range, totals, losses, raw log, next offset),
/// then the lines as they were printed, within `budget` bytes. A page that
/// holds a line always moves the cursor: a first line longer than the
/// budget is shown cut (and marked), and the cursor goes past it.
fn render_page(page: &crate::shell::background::Page, budget: usize) -> String {
    let mut body = String::new();
    let mut shown = 0u64;
    let mut cut = false;
    for (_, line) in &page.lines {
        let room = budget.saturating_sub(body.len());
        if line.len() < room {
            body.push_str(line);
            body.push('\n');
            shown += 1;
            continue;
        }
        if shown == 0 {
            // The first line alone exceeds the page: an excerpt, cut on a
            // character boundary, with room for the marker.
            let mut end = budget.saturating_sub(PAGE_CUT.len() + 1).min(line.len());
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            body.push_str(&line[..end]);
            body.push_str(PAGE_CUT);
            body.push('\n');
            shown = 1;
            cut = true;
        }
        break;
    }
    let start = page.lines.first().map(|(n, _)| n - 1);
    let next = start.map_or(page.next_offset, |s| s + shown);
    let name = page.stream.name();
    let mut head = match start {
        Some(s) if shown > 0 => format!(
            "{name}: lines {}–{} of {} complete ({} bytes received)",
            s + 1,
            s + shown,
            page.total_lines,
            page.received_bytes
        ),
        _ => format!(
            "{name}: no complete line from offset {next} ({} complete, {} bytes received)",
            page.total_lines, page.received_bytes
        ),
    };
    if page.evicted > 0 {
        head.push_str(&format!(
            "; {} earlier line(s) no longer in memory",
            page.evicted
        ));
    }
    if page.dropped_before > 0 {
        head.push_str(&format!(
            " (the {} before this page are among them)",
            page.dropped_before
        ));
    }
    if cut {
        head.push_str("; the line shown was truncated to fit the page");
    }
    head.push_str(&format!("; raw log: {}", page.raw_log.display()));
    if page.raw_log_capped {
        head.push_str(" (capped: it stopped growing)");
    }
    if let Some(e) = &page.raw_log_error {
        head.push_str(&format!(" (write error: {e})"));
    }
    head.push_str(&format!("; next offset {next}"));
    let mut out = format!("{head}\n{body}");
    if let Some(p) = &page.partial
        && next == page.total_lines
    {
        let room = budget.saturating_sub(body.len()).max(256);
        let mut end = room.min(p.len());
        while !p.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&format!(
            "[line in progress, not yet complete; it does not move the offset]\n{}{}\n",
            &p[..end],
            if end < p.len() { PAGE_CUT } else { "" }
        ));
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::bash::BashTool;
    use crate::permissions::AllowAll;
    use serde_json::json;

    fn ctx(dir: &Path, reg: &Arc<JobRegistry>, agent: &str) -> ToolContext {
        let extensions = Extensions::default();
        extensions.insert(JobsHandle(Arc::clone(reg)));
        extensions.insert(JobOwner {
            agent_id: agent.into(),
            root_run_id: "run_t".into(),
            workspace: dir.to_path_buf(),
        });
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: format!("jobs-{}", uuid::Uuid::new_v4()),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions,
        }
    }

    fn job_id(out: &str) -> String {
        let i = out.find("job_").expect(out);
        out[i..i + 16].to_string()
    }

    fn session_registry(c: &ToolContext) -> Arc<JobRegistry> {
        Arc::clone(&c.extensions.get::<JobsHandle>().expect("a session registry").0)
    }

    async fn start(c: &ToolContext, cmd: &str, extra: Value) -> ToolResult {
        let mut input = json!({ "command": cmd, "background": true });
        if let (Some(a), Some(b)) = (input.as_object_mut(), extra.as_object()) {
            a.extend(b.clone());
        }
        BashTool::with_jobs(session_registry(c)).execute(input, c).await
    }

    async fn job(c: &ToolContext, input: Value) -> ToolResult {
        JobTool::new(session_registry(c)).execute(input, c).await
    }

    #[tokio::test]
    async fn a_job_is_followed_to_its_end_by_its_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let reg = JobRegistry::new(JobSettings::default(), Some(dir.path().join("jobs")));
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = Arc::clone(&events);
        reg.set_sink(Arc::new(move |e| ev.lock().push(e)));
        let a = ctx(dir.path(), &reg, "agent_a");
        let b = ctx(dir.path(), &reg, "agent_b");
        let r = start(
            &a,
            "echo one; echo oops >&2; sleep 0.3; echo two",
            json!({}),
        )
        .await;
        assert!(!r.is_error, "{}", r.content);
        let id = job_id(&r.content);

        // Another agent cannot see, read or stop it.
        for action in ["status", "output", "wait", "stop"] {
            let r = job(&b, json!({ "action": action, "job_id": id })).await;
            assert!(r.is_error, "{action}: {}", r.content);
            assert_eq!(r.content, NO_ACCESSIBLE_JOB, "{action}");
        }
        assert!(
            job(&b, json!({ "action": "list" }))
                .await
                .content
                .contains("You have no jobs")
        );

        let w = job(
            &a,
            json!({ "action": "wait", "job_id": id, "timeout_ms": 10_000 }),
        )
        .await;
        assert!(w.content.contains("completed"), "{}", w.content);
        let out = job(&a, json!({ "action": "output", "job_id": id })).await;
        assert!(out.content.contains("one\ntwo\n"), "{}", out.content);
        let err = job(
            &a,
            json!({ "action": "output", "job_id": id, "stream": "stderr" }),
        )
        .await;
        assert!(err.content.contains("oops") && !err.content.contains("one"));
        let page = job(
            &a,
            json!({ "action": "output", "job_id": id, "offset": 1, "limit": 1 }),
        )
        .await;
        assert!(
            page.content.contains("two") && !page.content.contains("one\n"),
            "{}",
            page.content
        );

        // Its end is an event, with durable logs.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let evs = events.lock().clone();
        assert!(matches!(&evs[0], JobEvent::Started { agent_id, .. } if agent_id == "agent_a"));
        let logs = evs
            .iter()
            .find_map(|e| match e {
                JobEvent::Finished { state, logs, .. } if state == "completed" => {
                    Some(logs.clone())
                }
                _ => None,
            })
            .expect("finished");
        assert_eq!(logs.len(), 2);
        assert!(std::fs::read_to_string(&logs[0]).unwrap().contains("two"));
    }

    #[tokio::test]
    async fn the_quota_refuses_before_starting_and_stop_takes_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let settings = JobSettings {
            max_jobs: 1,
            stop_grace_ms: 300,
            ..Default::default()
        };
        let reg = JobRegistry::new(settings, None);
        let a = ctx(dir.path(), &reg, "agent_a");
        let r = start(&a, "sleep 300 & sleep 300 & wait", json!({})).await;
        let id = job_id(&r.content);
        let marker = dir.path().join("second_ran");
        let r2 = start(&a, &format!("touch {}", marker.display()), json!({})).await;
        assert!(
            r2.is_error && r2.content.contains("max_jobs"),
            "{}",
            r2.content
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker.exists(), "refused before anything started");

        let pid = reg.list(None)[0].2.pid;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let kids = crate::shell::procs::descendants(pid);
        assert!(kids.len() >= 2, "{kids:?}");
        let s = job(&a, json!({ "action": "stop", "job_id": id })).await;
        assert!(s.content.contains("stopped"), "{}", s.content);
        // Idempotent.
        assert!(
            job(&a, json!({ "action": "stop", "job_id": id }))
                .await
                .content
                .contains("stopped")
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        for k in kids {
            assert!(!crate::shell::procs::is_running(k), "{k} survived");
        }
        // Room again.
        assert!(reg.reserve().is_ok());
    }

    #[tokio::test]
    async fn running_is_not_ready_and_an_owner_stops_its_own_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let reg = JobRegistry::new(JobSettings::default(), None);
        let a = ctx(dir.path(), &reg, "agent_a");
        let r = start(
            &a,
            "sleep 300",
            json!({ "ready_pattern": "LISTENING", "ready_timeout": 200 }),
        )
        .await;
        let id = job_id(&r.content);
        let st = job(&a, json!({ "action": "status", "job_id": id })).await;
        assert!(
            st.content.contains("running") && st.content.contains("ready: not yet"),
            "{}",
            st.content
        );
        let r = start(
            &a,
            "sleep 0.2; echo LISTENING; sleep 300",
            json!({ "ready_pattern": "LISTENING" }),
        )
        .await;
        let ready = job_id(&r.content);
        let st = job(&a, json!({ "action": "status", "job_id": ready })).await;
        assert!(st.content.contains("ready: yes"), "{}", st.content);
        // The agent ends: its jobs go.
        reg.stop_owned_by("agent_a").await;
        assert_eq!(reg.running(), 0);
    }
}

#[cfg(all(test, unix))]
mod reservation_tests {
    use super::*;

    /// Concurrent reservations never exceed the quota: a slot is taken by
    /// one caller only, and given back when dropped unused.
    #[tokio::test]
    async fn concurrent_reservations_never_exceed_the_quota() {
        let settings = JobSettings {
            max_jobs: 8,
            ..Default::default()
        };
        let reg = JobRegistry::new(settings, None);
        let mut handles = Vec::new();
        for _ in 0..32 {
            let r = Arc::clone(&reg);
            handles.push(tokio::spawn(async move { r.reserve() }));
        }
        let mut slots = Vec::new();
        let mut refused = 0;
        for h in handles {
            match h.await.unwrap() {
                Ok(slot) => slots.push(slot),
                Err(e) => {
                    assert!(e.contains("max_jobs"), "{e}");
                    refused += 1;
                }
            }
        }
        assert_eq!(slots.len(), 8, "exactly the quota is granted");
        assert_eq!(refused, 24);
        assert!(reg.reserve().is_err(), "no place left while the slots live");
        // A dropped slot gives its place back.
        slots.pop();
        assert!(reg.reserve().is_ok());
    }
}
