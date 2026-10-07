//! Background commands as jobs: one registry per session over the shell's
//! supervised tasks (`Bash` with `background: true`), with an owner, a
//! quota, events, durable logs, and the `Job` tool (`list`, `status`,
//! `output`, `wait`, `stop`).
//!
//! * A job belongs to the agent that started it (and its root run and
//!   workspace); only that agent sees it through `Job` (an id guessed from
//!   another agent, run or session gives nothing). Frontends see the
//!   session's jobs.
//! * It runs from a snapshot of its owner's shell (working directory,
//!   exported variables, functions, aliases); what it changes never reaches
//!   that shell. A sub-agent's jobs are stopped when it ends; the session
//!   agent's jobs live until the session closes. No job outlives Bricks.
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
}

/// The session's jobs.
pub struct JobRegistry {
    pub settings: JobSettings,
    jobs: Mutex<Vec<Entry>>,
    sink: Mutex<Option<JobSink>>,
    /// Where finished jobs' logs are copied.
    durable_dir: Option<PathBuf>,
}

/// The session's job registry, in a tool context.
#[derive(Clone)]
pub struct JobsHandle(pub Arc<JobRegistry>);

impl JobRegistry {
    pub fn new(settings: JobSettings, durable_dir: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            settings,
            jobs: Mutex::new(Vec::new()),
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
        self.jobs
            .lock()
            .iter()
            .filter(|e| !e.task.state().is_finished())
            .count()
    }

    /// Refuse a new job beyond the quota (before anything starts).
    pub fn check_capacity(&self) -> std::result::Result<(), String> {
        let n = self.running();
        if n >= self.settings.max_jobs {
            return Err(format!(
                "{n} background job(s) already run in this session; the limit is {} ([background] max_jobs). Stop one first (Job stop).",
                self.settings.max_jobs
            ));
        }
        Ok(())
    }

    /// Track a started task as a job; watch it until it ends.
    pub fn register(self: &Arc<Self>, owner: JobOwner, task: Arc<Task>, cwd: &Path) -> String {
        let id = format!("job_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        self.emit(JobEvent::Started {
            job_id: id.clone(),
            agent_id: owner.agent_id.clone(),
            root_run_id: owner.root_run_id.clone(),
            command: task.command.chars().take(300).collect(),
            cwd: cwd.display().to_string(),
            pid: task.pid,
        });
        self.jobs.lock().push(Entry {
            id: id.clone(),
            owner,
            task: Arc::clone(&task),
            cwd: cwd.to_path_buf(),
            logs: Vec::new(),
        });
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
            if let Some(e) = me.jobs.lock().iter_mut().find(|e| e.id == jid) {
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
        self.jobs
            .lock()
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

    /// `(id, owner, status)` of the jobs `viewer` may see.
    pub fn list(
        &self,
        viewer: Option<&str>,
    ) -> Vec<(String, JobOwner, crate::shell::background::TaskStatus)> {
        self.jobs
            .lock()
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
            .jobs
            .lock()
            .iter()
            .filter(|e| e.owner.agent_id == agent_id)
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
            .jobs
            .lock()
            .iter()
            .map(|e| Arc::clone(&e.task))
            .collect();
        for t in tasks {
            t.stop(Duration::from_millis(self.settings.stop_grace_ms))
                .await;
        }
    }
}

fn describe(id: &str, st: &crate::shell::background::TaskStatus, cwd: Option<&Path>) -> String {
    let code = match &st.state {
        TaskState::Completed { code } => format!(" (code {code})"),
        TaskState::Failed { code, signal } => match (code, signal) {
            (Some(c), _) => format!(" (code {c})"),
            (None, Some(s)) => format!(" (signal {s})"),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let ready = match &st.readiness {
        Readiness::NotChecked => "not checked".to_string(),
        Readiness::Waiting => "not yet".to_string(),
        Readiness::Ready { after, .. } => {
            format!("yes, after {}", cersei_types::duration::display_ms(*after))
        }
    };
    format!(
        "{id} [{}{code}] pid {} · {} · ready: {ready} · stdout {} B / stderr {} B{} · {}",
        st.state.label(),
        st.pid,
        cersei_types::duration::display_ms(st.elapsed),
        st.stdout_bytes,
        st.stderr_bytes,
        cwd.map(|c| format!(" · cwd {}", c.display()))
            .unwrap_or_default(),
        st.command
    )
}

/// `Job`: the background commands of the calling agent.
pub struct JobTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobInput {
    action: String,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

const MAX_JOB_WAIT_MS: u64 = 600_000;

#[async_trait]
impl Tool for JobTool {
    fn name(&self) -> &str {
        "Job"
    }

    fn description(&self) -> &str {
        "Your background commands (started with Bash `background: true`): `list`, `status` \
         (job_id), `output` (job_id, stream stdout|stderr, offset in lines, limit), `wait` \
         (job_id, timeout_ms; a timeout stops nothing), `stop` (job_id; graceful then forced, \
         idempotent). Running is not ready, and ready is not proof that a service works."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn permission_level_for(&self, input: &Value) -> PermissionLevel {
        match input["action"].as_str().unwrap_or("") {
            "stop" => PermissionLevel::Execute,
            _ => PermissionLevel::ReadOnly,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "status", "output", "wait", "stop"] },
                "job_id": { "type": "string" },
                "stream": { "type": "string", "enum": ["stdout", "stderr"] },
                "offset": { "type": "integer", "minimum": 0, "description": "Lines to skip (output)." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 2000, "description": "Lines at most (output; default 200)." },
                "timeout_ms": { "type": "integer", "minimum": 1, "maximum": MAX_JOB_WAIT_MS }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let i: JobInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolResult::error(format!("Invalid input: {e}")),
        };
        let Some(reg) = ctx.extensions.get::<JobsHandle>() else {
            return ToolResult::error(
                "no job registry in this context: use BashTaskStatus / BashTaskOutput / BashTaskStop",
            );
        };
        let reg = &reg.0;
        let viewer = ctx.extensions.get::<JobOwner>().map(|o| o.agent_id.clone());
        let Some(viewer) = viewer else {
            return ToolResult::error("this agent has no identity: no job is visible");
        };
        let need = |id: &Option<String>| -> std::result::Result<(Arc<Task>, PathBuf, Vec<String>), ToolResult> {
            let id = id
                .as_deref()
                .ok_or_else(|| ToolResult::error(format!("`{}` needs `job_id`", i.action)))?;
            reg.find(id, Some(&viewer))
                .map(|(t, _, cwd, logs)| (t, cwd, logs))
                .ok_or_else(|| ToolResult::error(format!("no job `{id}` of yours")))
        };
        match i.action.as_str() {
            "list" => {
                let jobs = reg.list(Some(&viewer));
                if jobs.is_empty() {
                    return ToolResult::success("No background job of yours.");
                }
                ToolResult::success(
                    jobs.iter()
                        .map(|(id, _, st)| describe(id, st, None))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }
            "status" => match need(&i.job_id) {
                Err(e) => e,
                Ok((t, cwd, logs)) => {
                    let mut s =
                        describe(i.job_id.as_deref().unwrap_or(""), &t.status(), Some(&cwd));
                    if !logs.is_empty() {
                        s.push_str(&format!("\nlogs kept: {}", logs.join(", ")));
                    }
                    ToolResult::success(s)
                }
            },
            "output" => match need(&i.job_id) {
                Err(e) => e,
                Ok((t, _, _)) => {
                    let stream = match i.stream.as_deref().unwrap_or("stdout") {
                        "stdout" => Stream::Stdout,
                        "stderr" => Stream::Stderr,
                        other => return ToolResult::error(format!("unknown stream `{other}`")),
                    };
                    let page = t.read(
                        stream,
                        i.offset.unwrap_or(0),
                        i.limit.unwrap_or(200).clamp(1, 2000),
                    );
                    let mut out = String::new();
                    let mut bytes = 0usize;
                    let mut shown = 0u64;
                    let mut next = page.next_offset;
                    for (n, l) in &page.lines {
                        if bytes + l.len() > reg.settings.max_page_bytes {
                            next = n - 1;
                            break;
                        }
                        bytes += l.len() + 1;
                        out.push_str(l);
                        out.push('\n');
                        shown += 1;
                    }
                    let mut head = format!(
                        "{} lines {}–{} of {} ({} bytes received)",
                        stream.name(),
                        page.lines.first().map(|x| x.0).unwrap_or(next),
                        page.lines
                            .first()
                            .map(|x| x.0 + shown)
                            .unwrap_or(next)
                            .saturating_sub(1),
                        page.total_lines,
                        page.received_bytes
                    );
                    if page.dropped_before > 0 {
                        head.push_str(&format!(
                            "; {} earlier line(s) no longer in memory (raw log: {})",
                            page.dropped_before,
                            page.raw_log.display()
                        ));
                    }
                    if page.raw_log_capped {
                        head.push_str("; raw log capped");
                    }
                    if let Some(e) = &page.raw_log_error {
                        head.push_str(&format!("; raw log write error: {e}"));
                    }
                    if next < page.total_lines {
                        head.push_str(&format!("; next offset {next}"));
                    }
                    if let Some(p) = page.partial {
                        out.push_str(&format!(
                            "[unfinished line] {}\n",
                            p.chars().take(2000).collect::<String>()
                        ));
                    }
                    ToolResult::success(format!("{head}\n{out}"))
                }
            },
            "wait" => match need(&i.job_id) {
                Err(e) => e,
                Ok((t, cwd, _)) => {
                    let limit = Duration::from_millis(
                        i.timeout_ms.unwrap_or(60_000).clamp(1, MAX_JOB_WAIT_MS),
                    );
                    let ended = t.wait_finished(limit).await;
                    let s = describe(i.job_id.as_deref().unwrap_or(""), &t.status(), Some(&cwd));
                    if ended {
                        ToolResult::success(s)
                    } else {
                        ToolResult::success(format!(
                            "{s}\nstill running after {} (not stopped).",
                            cersei_types::duration::display_ms(limit)
                        ))
                    }
                }
            },
            "stop" => {
                let Some(id) = i.job_id.clone() else {
                    return ToolResult::error("`stop` needs `job_id`");
                };
                match reg.stop(&id, Some(&viewer)).await {
                    Some(state) => ToolResult::success(format!("{id}: {}", state.label())),
                    None => ToolResult::error(format!("no job `{id}` of yours")),
                }
            }
            other => ToolResult::error(format!(
                "unknown action `{other}`: list, status, output, wait, stop"
            )),
        }
    }
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

    async fn start(c: &ToolContext, cmd: &str, extra: Value) -> ToolResult {
        let mut input = json!({ "command": cmd, "background": true });
        if let (Some(a), Some(b)) = (input.as_object_mut(), extra.as_object()) {
            a.extend(b.clone());
        }
        BashTool.execute(input, c).await
    }

    async fn job(c: &ToolContext, input: Value) -> ToolResult {
        JobTool.execute(input, c).await
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
            assert!(
                r.is_error && r.content.contains("no job"),
                "{action}: {}",
                r.content
            );
        }
        assert!(job(&b, json!({ "action": "list" }))
            .await
            .content
            .contains("No background job"));

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
        assert!(job(&a, json!({ "action": "stop", "job_id": id }))
            .await
            .content
            .contains("stopped"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        for k in kids {
            assert!(!crate::shell::procs::is_running(k), "{k} survived");
        }
        // Room again.
        assert!(reg.check_capacity().is_ok());
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
