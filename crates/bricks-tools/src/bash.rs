//! Bash tool: commands run in the session's shell state, and supervised
//! background tasks.
//!
//! Each command is its own `bash` process (see [`crate::shell`]), started from
//! the session's state: working directory, exported variables, functions and
//! aliases persist between calls because a command that ends normally
//! publishes its changes; variables that are not exported do not persist.
//! Commands run side by side. Stdin is empty unless `input` is given. A
//! command still running after `yield_time_ms` is handed over as a job: the
//! call returns `Running` with its `job_id`, and the command goes on, followed
//! with the `Job` tool. `timeout` still ends it. `background: true` starts a
//! job at once. Both need the session's job registry ([`BashTool::with_jobs`]);
//! without one, Bash runs only commands that end in the call, and refuses the
//! others before anything starts.
//!
//! Paths: the shell resolves relative paths against its own current
//! directory, which `cd` changes. Other tools keep resolving against the
//! agent's working directory; a `cd` in the shell does not move them.

use super::*;
use crate::shell::{self, ShellConfig};
use crate::tool_report::{ToolBody, ToolReport, ToolStatus, suggest};
use serde::Deserialize;
use std::time::Duration;

/// Separator between stdout and stderr in rendered output (kept by the
/// output compressor).
pub const STDERR_MARKER: &str = "--- stderr ---";

/// The Bash tool. Built on a job registry ([`BashTool::with_jobs`], what
/// [`crate::shell_with_jobs`] does), its commands can be handed over and
/// started in the background as jobs; without one ([`BashTool::default`]),
/// only commands that end in the call run.
#[derive(Default, Clone)]
pub struct BashTool {
    jobs: Option<Arc<crate::jobs::JobRegistry>>,
}

impl BashTool {
    /// Bash on the session's job registry (the one `Job` reads).
    pub fn with_jobs(registry: Arc<crate::jobs::JobRegistry>) -> Self {
        Self {
            jobs: Some(registry),
        }
    }

    /// The registry its jobs are recorded in, if any.
    pub fn registry(&self) -> Option<&Arc<crate::jobs::JobRegistry>> {
        self.jobs.as_ref()
    }
}

/// Why a command that may become a job cannot run here.
#[cfg(unix)]
const NO_REGISTRY: &str = "this Bash has no job registry, so a command that may be handed over \
    as a job cannot run (nothing was started). Give `yield_time_ms` at least equal to `timeout` \
    so the call waits for the end, or assemble the tools with shell_with_jobs(registry).";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    command: String,
    /// Milliseconds.
    timeout: Option<u64>,
    #[serde(default)]
    background: bool,
    /// Text given to the command's stdin (default: empty stdin).
    input: Option<String>,
    /// Background only: a regex on the task's output lines that means
    /// "ready" (e.g. a server's "listening on").
    ready_pattern: Option<String>,
    /// Background only: milliseconds to wait for `ready_pattern`.
    ready_timeout: Option<u64>,
    /// Foreground only: milliseconds before a command still running is
    /// handed over as a task (default: `[bash] yield_time_ms`). Not a
    /// deadline: `timeout` is.
    yield_time_ms: Option<u64>,
}

pub(crate) fn shell_config(ctx: &ToolContext) -> ShellConfig {
    ctx.extensions
        .get::<ShellConfig>()
        .map(|c| (*c).clone())
        .unwrap_or_default()
}

pub(crate) fn progress_for(ctx: &ToolContext, tool: &'static str) -> Option<shell::Progress> {
    ctx.extensions.get::<shell::ProgressSink>().map(|sink| {
        let sink = sink.0.clone();
        Arc::new(move |m: String| sink(tool, &m)) as shell::Progress
    })
}

/// First word of each simple command (for permission details).
fn invoked_names(command: &str) -> Vec<String> {
    let mut names = Vec::new();
    for segment in command.split(['\n', ';', '|', '&', '(', ')', '{', '}']) {
        let word = segment
            .split_whitespace()
            .find(|w| !w.contains('=') || w.starts_with('='));
        if let Some(w) = word
            && !names.iter().any(|n: &String| n == w)
        {
            names.push(w.to_string());
        }
    }
    names
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        "Run a bash command in this session. Each call is its own process, so calls run side by \
         side; the working directory, exported variables, PATH, functions and aliases persist \
         between calls (plain shell variables do not). stdin is empty unless `input` is given. \
         `timeout` (ms, default 120000, max 600000) ends the command; a timed-out command returns \
         its partial output. `yield_time_ms` (default 10000) only gives control back: a command \
         still running then returns as Running with a `job_id` and keeps going until it ends or \
         its timeout. For servers, watchers and other long-running processes use \
         `background: true` (optionally `ready_pattern`) — do not end a command with `&`. Both \
         kinds are jobs: follow them with the Job tool (list, status, output, wait, stop)."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }

    fn job_registry(&self) -> Option<Arc<crate::jobs::JobRegistry>> {
        self.jobs.clone()
    }

    fn on_job_registry(&self, registry: &Arc<crate::jobs::JobRegistry>) -> Option<Box<dyn Tool>> {
        Some(Box::new(BashTool::with_jobs(Arc::clone(registry))))
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The bash command to execute" },
                "timeout": { "type": "integer", "description": "Timeout in milliseconds (default 120000, max 600000); ends the command, also after a hand-over" },
                "background": { "type": "boolean", "description": "Start as a background job and return its job_id" },
                "input": { "type": "string", "description": "Text to give the command on stdin (default: empty stdin)" },
                "ready_pattern": { "type": "string", "description": "Background only: regex on output lines meaning the task is ready" },
                "ready_timeout": { "type": "integer", "description": "Background only: milliseconds to wait for ready_pattern" },
                "yield_time_ms": { "type": "integer", "description": "Foreground only: milliseconds before a command still running returns as a task (Running) while it goes on (default 10000). Distinct from timeout, which still ends it" }
            },
            "required": ["command"]
        })
    }

    async fn permission_details(&self, input: &Value, ctx: &ToolContext) -> Option<String> {
        let command = input.get("command")?.as_str()?;
        #[cfg(unix)]
        {
            let config = shell_config(ctx);
            let sessions = shell::session(&ctx.session_id, &config);
            let names = invoked_names(command);
            let state = sessions.state();
            // What the approval is about: remembered, so that the command runs
            // as described or not at all.
            sessions.remember(command, state.fingerprint(&names));
            let defs = state.describe(&names);
            if defs.trim().is_empty() {
                return None;
            }
            Some(format!(
                "the command invokes these session-defined aliases/functions:\n{}",
                defs.trim_end()
            ))
        }
        #[cfg(not(unix))]
        {
            let _ = (command, ctx);
            None
        }
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let config = shell_config(ctx);
        let timeout = input
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(config.default_timeout)
            .min(config.max_timeout);

        #[cfg(feature = "vms")]
        if let Some(sandbox) = ctx
            .extensions
            .get::<std::sync::Arc<dyn bricks_vms::Sandbox>>()
        {
            if input.background
                || input.yield_time_ms.is_some()
                || input.ready_pattern.is_some()
                || input.ready_timeout.is_some()
            {
                return ToolResult::error(
                    "`background`, `ready_pattern`, `ready_timeout` and `yield_time_ms` are not \
                     available in a sandbox: each command runs there on its own and nothing \
                     stays supervised. Run it without them.",
                );
            }
            return run_in_sandbox(&sandbox, &input.command, timeout).await;
        }

        #[cfg(unix)]
        {
            // What was described for an approval must still be what runs.
            let sessions = shell::session(&ctx.session_id, &config);
            if let Some(described) = sessions.take_remembered(&input.command) {
                let names = invoked_names(&input.command);
                if sessions.state().fingerprint(&names) != described {
                    return ToolResult::error(
                        "the aliases, functions, PATH or directory of this session changed since \
                         this command was described for approval: nothing was run. Call it again \
                         to see the current definitions.",
                    );
                }
            }
            // A command that may become a job needs the registry and the
            // agent's identity: checked before anything starts.
            let yield_after = input
                .yield_time_ms
                .map(Duration::from_millis)
                .unwrap_or(config.yield_time);
            let may_be_a_job = input.background || yield_after < timeout;
            let jobs = match (&self.jobs, may_be_a_job) {
                (_, false) => None,
                (None, true) => return ToolResult::error(NO_REGISTRY),
                (Some(registry), true) => {
                    let Some(owner) = ctx.extensions.get::<crate::jobs::JobOwner>() else {
                        return ToolResult::error(
                            "this agent has no identity, so its jobs could not be followed: \
                             nothing was started (the agent was assembled without one).",
                        );
                    };
                    Some((Arc::clone(registry), (*owner).clone()))
                }
            };
            if input.background {
                if input.yield_time_ms.is_some() {
                    return ToolResult::error(
                        "`yield_time_ms` only applies to a foreground command: a background \
                         command is a job from the start.",
                    );
                }
                let Some(jobs) = jobs else {
                    return ToolResult::error(NO_REGISTRY);
                };
                return start_background(&input, ctx, &config, jobs).await;
            }
            if input.ready_pattern.is_some() || input.ready_timeout.is_some() {
                return ToolResult::error(
                    "`ready_pattern` and `ready_timeout` only apply with `background: true`.",
                );
            }
            run_foreground(&input, ctx, &config, timeout, yield_after, jobs).await
        }
        #[cfg(not(unix))]
        {
            let _ = (timeout, config);
            ToolResult::error(
                "The Bash tool needs a Unix shell; on this platform use the PowerShell tool.",
            )
        }
    }
}

/// A foreground command: its result when it ends before the hand-over, or a
/// task (`Running`) when it does not.
#[cfg(unix)]
async fn run_foreground(
    input: &Input,
    ctx: &ToolContext,
    config: &ShellConfig,
    timeout: Duration,
    yield_after: Duration,
    jobs: Option<(Arc<crate::jobs::JobRegistry>, crate::jobs::JobOwner)>,
) -> ToolResult {
    let sessions = shell::session(&ctx.session_id, config);
    let started = std::time::Instant::now();
    let mut cmd = match sessions
        .spawn_command(
            &input.command,
            input.input.as_deref(),
            &ctx.working_dir,
            config,
            timeout,
        )
        .await
    {
        Ok(c) => c,
        Err(e) => return launch_failure(e, started),
    };
    let task = Arc::clone(cmd.task());
    let progress = progress_for(ctx, "Bash");
    let yield_timer = tokio::time::sleep(yield_after);
    tokio::pin!(yield_timer);
    // A hand-over at or after the deadline never happens: the timeout ends
    // the command, as a timeout (not as a job still running). Without a
    // registry the command was refused unless it cannot be handed over.
    let mut yield_pending = jobs.is_some() && yield_after < timeout;
    let mut ticker = tokio::time::interval(config.progress_every);
    ticker.tick().await;
    let mut last_bytes = 0u64;
    let mut quiet_since = std::time::Instant::now();
    let mut refused: Option<String> = None;

    // `None`: the command ended; `Some(slot)`: it is handed over, with the
    // place reserved for it as a job.
    let handed_over: Option<crate::jobs::JobSlot> = loop {
        tokio::select! {
            _ = task.wait_end() => break None,
            _ = &mut yield_timer, if yield_pending => {
                yield_pending = false;
                match jobs.as_ref().map(|(registry, _)| registry.reserve()) {
                    Some(Ok(slot)) => break Some(slot),
                    Some(Err(e)) => refused = Some(e),
                    None => {}
                }
            }
            _ = ticker.tick() => {
                let st = task.status();
                let bytes = st.stdout_bytes + st.stderr_bytes;
                if bytes != last_bytes {
                    last_bytes = bytes;
                    quiet_since = std::time::Instant::now();
                }
                if let Some(p) = &progress {
                    let quiet = quiet_since.elapsed();
                    let mut msg = format!(
                        "still running after {} ({} bytes of output so far)",
                        bricks_types::duration::display_ms(started.elapsed()),
                        bytes
                    );
                    if quiet >= config.progress_every {
                        msg.push_str(&format!(
                            "; no output for {} — it may be computing, or waiting for input it \
                             will not get (stdin is {})",
                            bricks_types::duration::display_ms(quiet),
                            if input.input.is_some() { "the provided input" } else { "empty" }
                        ));
                    }
                    p(msg);
                }
            }
        }
    };

    let Some(slot) = handed_over else {
        let outcome = cmd.finish().await;
        let mut notes = Vec::new();
        if let Some(e) = refused {
            notes.push(format!(
                "still running after {}: not handed over as a job ({e}); the call waited for it",
                bricks_types::duration::display_ms(yield_after)
            ));
        }
        return finished_result(timeout, outcome, notes);
    };

    let Some((registry, owner)) = jobs else {
        unreachable!("a command is handed over only with a registry");
    };
    let job_id = registry.register(slot, owner, Arc::clone(&task), &ctx.working_dir, false);
    let partial = task
        .output()
        .unwrap_or_else(|| [Default::default(), Default::default()]);
    sessions.adopt(Arc::clone(&task));
    // The command goes on: its watcher keeps its deadline and publishes its end.
    drop(cmd.detach());
    handed_over_result(&task, job_id, started, &ctx.working_dir, partial)
}

/// The result of a command that ended (in the call or after the hand-over).
#[cfg(unix)]
fn finished_result(
    timeout: Duration,
    outcome: crate::shell::bash::RunOutcome,
    extra_notes: Vec<String>,
) -> ToolResult {
    use crate::shell::bash::RunStatus;
    let (status, exit_code, termination) = match &outcome.status {
        RunStatus::Exited { code } => (
            if *code == 0 {
                ToolStatus::Success
            } else {
                ToolStatus::Failure
            },
            Some(*code),
            None,
        ),
        RunStatus::TimedOut => (
            ToolStatus::TimedOut,
            None,
            Some(format!(
                "no exit code ({} processes stopped{})",
                outcome.terminated.targeted.len(),
                if outcome.terminated.forced.is_empty() {
                    String::new()
                } else {
                    format!(", {} of them forcibly", outcome.terminated.forced.len())
                }
            )),
        ),
        RunStatus::Cancelled => (ToolStatus::Cancelled, None, Some("cancelled".into())),
        RunStatus::Killed { signal } => (
            ToolStatus::Failure,
            None,
            Some(match signal {
                Some(s) => format!("command killed by signal {s}"),
                None => "command ended without a status".into(),
            }),
        ),
    };
    let exit_code_value = exit_code;
    let mut report = ToolReport::new(
        status,
        ToolBody::Streams {
            stdout: outcome.stdout.text.clone(),
            stderr: outcome.stderr.text.clone(),
        },
    );
    report.exit_code = exit_code_value;
    report.termination = termination;
    report.duration = Some(outcome.duration);
    report.timeout = (status == ToolStatus::TimedOut).then_some(timeout);
    report.notes.extend(extra_notes);
    report.notes.extend(outcome.notes.iter().cloned());
    for (name, cap) in [("stdout", &outcome.stdout), ("stderr", &outcome.stderr)] {
        if let Some(p) = &cap.raw_path {
            report.notes.push(format!(
                "{name} full raw output ({} bytes): {}",
                cap.raw_bytes,
                p.display()
            ));
        }
    }
    report.suggestion = suggest(status, exit_code_value, &outcome.stderr.text);
    report.data = Some(serde_json::json!({
        "cwd": outcome.cwd,
        "exit_code": exit_code_value,
        "stdout_bytes": outcome.stdout.raw_bytes,
        "stderr_bytes": outcome.stderr.raw_bytes,
    }));
    ToolResult::from_report(report)
}

/// The result of a command handed over: what it printed so far, its ids and
/// the cursors to read the rest with.
#[cfg(unix)]
fn handed_over_result(
    task: &shell::background::Task,
    job_id: String,
    started: std::time::Instant,
    cwd: &std::path::Path,
    partial: [crate::shell::capture::Captured; 2],
) -> ToolResult {
    let st = task.status();
    let cursor_out = task
        .read(shell::background::Stream::Stdout, shell::background::OutputPosition::Offset(0), 0)
        .total_lines;
    let cursor_err = task
        .read(shell::background::Stream::Stderr, shell::background::OutputPosition::Offset(0), 0)
        .total_lines;
    let notes = vec![
        format!(
            "Still running after {}, handed over as job {job_id} (pid {}) in {}: {}",
            bricks_types::duration::display_ms(started.elapsed()),
            st.pid,
            cwd.display(),
            st.command
        ),
        format!(
            "Follow it with Job: action output, job_id \"{job_id}\", offset {cursor_out} for new \
             stdout lines ({cursor_err} for stderr); wait to wait for its end; stop to end it. \
             It is a job like a background command, and its timeout still ends it."
        ),
    ];
    let mut report = ToolReport::new(
        ToolStatus::Running,
        ToolBody::Streams {
            stdout: partial[0].text.clone(),
            stderr: partial[1].text.clone(),
        },
    );
    report.duration = Some(started.elapsed());
    report.notes = notes;
    report.data = Some(serde_json::json!({
        "job_id": job_id,
        "pid": st.pid,
        "state": st.state.label(),
        "cwd": cwd.display().to_string(),
        "cursors": { "stdout": cursor_out, "stderr": cursor_err },
    }));
    ToolResult::from_report(report)
}

#[cfg(unix)]
async fn start_background(
    input: &Input,
    ctx: &ToolContext,
    config: &ShellConfig,
    (registry, owner): (Arc<crate::jobs::JobRegistry>, crate::jobs::JobOwner),
) -> ToolResult {
    use crate::shell::background::{Readiness, TaskState};
    let started = std::time::Instant::now();
    let pattern = match input
        .ready_pattern
        .as_deref()
        .map(regex::Regex::new)
        .transpose()
    {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("Invalid `ready_pattern`: {e}")),
    };
    // The session's jobs: a place reserved before anything starts, limits
    // applied to the task's logs.
    let mut config = config.clone();
    config.tasks.memory_bytes = registry.settings.output_buffer_bytes;
    config.tasks.max_line_bytes = registry.settings.max_line_bytes;
    config.tasks.raw_log_bytes = registry.settings.raw_log_bytes;
    let slot = match registry.reserve() {
        Ok(slot) => slot,
        Err(e) => return ToolResult::error(e),
    };
    let config = &config;
    let sessions = shell::session(&ctx.session_id, config);
    let task = match sessions
        .start_task(
            &input.command,
            input.input.as_deref(),
            pattern,
            &ctx.working_dir,
            config,
        )
        .await
    {
        Ok(t) => t,
        Err(e) => return launch_failure(e, started),
    };
    let wait = match (input.ready_pattern.is_some(), input.ready_timeout) {
        (true, Some(ms)) => Duration::from_millis(ms).min(config.max_timeout),
        (true, None) => Duration::from_secs(10),
        _ => Duration::ZERO,
    };
    if !wait.is_zero() {
        task.wait_ready(wait).await;
    } else {
        // Give a command that fails at once the chance to say so.
        task.wait_finished(Duration::from_millis(150)).await;
    }
    let st = task.status();
    let job_id = registry.register(slot, owner, Arc::clone(&task), &ctx.working_dir, true);
    let mut lines = vec![format!(
        "Started job {job_id} (pid {}) in {}: {}",
        st.pid,
        ctx.working_dir.display(),
        st.command
    )];
    let status = match &st.state {
        TaskState::Completed { .. } => ToolStatus::Success,
        TaskState::Failed { .. } => ToolStatus::Failure,
        _ => ToolStatus::Running,
    };
    lines.push(format!("state: {}", st.state.label()));
    match &st.readiness {
        Readiness::NotChecked => lines.push(
            "ready: not checked (no ready_pattern); a started process is not necessarily ready"
                .into(),
        ),
        Readiness::Waiting => lines.push(format!(
            "ready: not yet (pattern not seen after {})",
            bricks_types::duration::display_ms(started.elapsed())
        )),
        Readiness::Ready { after, line } => lines.push(format!(
            "ready after {}: {line}",
            bricks_types::duration::display_ms(*after)
        )),
    }
    lines.push(format!(
        "Follow it with Job (job_id \"{job_id}\"): status, output, wait, stop."
    ));
    let mut report = ToolReport::new(status, ToolBody::Text(lines.join("\n")));
    if let TaskState::Completed { code } = st.state {
        report.exit_code = Some(code);
    }
    if let TaskState::Failed { code, signal } = st.state {
        report.exit_code = code;
        report.termination = signal.map(|s| format!("signal {s}"));
    }
    report.duration = Some(started.elapsed());
    report.data = Some(serde_json::json!({
        "job_id": job_id,
        "cwd": ctx.working_dir.display().to_string(),
        "pid": st.pid,
        "state": st.state.label(),
        "ready": matches!(st.readiness, Readiness::Ready { .. }),
    }));
    ToolResult::from_report(report)
}

fn launch_failure(e: std::io::Error, started: std::time::Instant) -> ToolResult {
    let mut report = ToolReport::new(
        ToolStatus::Failure,
        ToolBody::Streams {
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    report.termination = Some(format!("cannot launch: {e}"));
    report.duration = Some(started.elapsed());
    if e.kind() == std::io::ErrorKind::NotFound {
        report.suggestion = Some("Check that bash is installed and available on PATH.".into());
    }
    ToolResult::from_report(report)
}

#[cfg(feature = "vms")]
async fn run_in_sandbox(
    sandbox: &std::sync::Arc<dyn bricks_vms::Sandbox>,
    command: &str,
    timeout: Duration,
) -> ToolResult {
    // A sandbox runs each command on its own: no persistent state.
    let started = std::time::Instant::now();
    let req = bricks_vms::RunRequest::new(command.to_string()).timeout(timeout);
    match sandbox.commands().run(req).await {
        Ok(out) => {
            let status = if out.timed_out {
                ToolStatus::TimedOut
            } else if out.exit_code == 0 {
                ToolStatus::Success
            } else {
                ToolStatus::Failure
            };
            let mut report = ToolReport::new(
                status,
                ToolBody::Streams {
                    stdout: out.stdout,
                    stderr: out.stderr,
                },
            );
            report.exit_code = (!out.timed_out).then_some(out.exit_code);
            report.timeout = out.timed_out.then_some(timeout);
            report.duration = Some(started.elapsed());
            report.notes.push(format!(
                "ran in sandbox {} (no state kept between commands)",
                sandbox.id()
            ));
            ToolResult::from_report(report)
        }
        Err(e) => ToolResult::error(format!("Sandbox exec failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobOwner, JobRegistry, JobTool, JobsHandle};

    #[test]
    fn invoked_names_are_the_first_words() {
        assert_eq!(
            invoked_names("FOO=1 ll -a | grep x && deploy prod; (cd y)"),
            vec!["ll", "grep", "deploy", "cd"]
        );
    }

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: format!("bash-tool-{}", uuid::Uuid::new_v4()),
            permissions: Arc::new(crate::permissions::AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    /// Bash on a session's registry, for an agent with an identity (what
    /// `shell_with_jobs` gives a real agent). Background and handed-over
    /// commands need it.
    fn with_jobs(dir: &std::path::Path) -> (BashTool, Arc<JobRegistry>, ToolContext) {
        let registry = JobRegistry::new(Default::default(), None);
        let c = ctx(dir);
        c.extensions.insert(JobsHandle(Arc::clone(&registry)));
        c.extensions.insert(JobOwner {
            agent_id: "agent-bash-test".into(),
            root_run_id: "run-bash-test".into(),
            workspace: dir.to_path_buf(),
        });
        (BashTool::with_jobs(Arc::clone(&registry)), registry, c)
    }

    /// The `job_id` of a background or handed-over result.
    fn job_id_of(r: &ToolResult) -> String {
        r.metadata.as_ref().expect("metadata")["job_id"]
            .as_str()
            .expect("a job_id")
            .to_string()
    }

    /// A quick command returns quickly, with all its output: the end of the
    /// output is not waited for once nothing can write to it any more.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_quick_command_does_not_wait_for_the_output_window() {
        let dir = tempfile::tempdir().unwrap();
        let (bash, _reg, c) = with_jobs(dir.path());
        let big = "seq 1 20000";
        bash.execute(serde_json::json!({ "command": "true" }), &c)
            .await;
        let start = std::time::Instant::now();
        let r = bash
            .execute(serde_json::json!({ "command": big }), &c)
            .await;
        let took = start.elapsed();
        let Some(ToolBody::Streams { stdout, .. }) = r.report.map(|r| r.body) else {
            panic!("streams")
        };
        assert!(
            stdout.starts_with("1\n") && stdout.trim_end().ends_with("20000"),
            "complete output"
        );
        assert!(took < Duration::from_millis(400), "took {took:?}");
        crate::shell::close_session(&c.session_id).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn results_carry_structured_data() {
        let dir = tempfile::tempdir().unwrap();
        let (bash, _reg, c) = with_jobs(dir.path());
        let r = bash
            .execute(
                serde_json::json!({ "command": "echo out; echo err >&2; sh -c 'exit 3'" }),
                &c,
            )
            .await;
        let rep = r.report.clone().unwrap();
        assert_eq!(rep.status, ToolStatus::Failure);
        assert_eq!(rep.exit_code, Some(3));
        assert_eq!(
            rep.suggestion, None,
            "no suggestion without an identified cause"
        );
        assert_eq!(
            rep.body,
            ToolBody::Streams {
                stdout: "out\n".into(),
                stderr: "err\n".into()
            }
        );
        assert!(rep.duration.is_some());
        let data = r.metadata.unwrap();
        assert_eq!(data["exit_code"], 3);
        assert!(data["cwd"].as_str().is_some());
        assert!(r.is_error);
        assert_eq!(r.content, "--- stdout ---\nout\n--- stderr ---\nerr");
        crate::shell::close_session(&c.session_id).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn background_commands_are_jobs_with_ids() {
        let dir = tempfile::tempdir().unwrap();
        let (bash, reg, c) = with_jobs(dir.path());
        bash.execute(serde_json::json!({ "command": "true" }), &c)
            .await;
        let bg = bash
            .execute(
                serde_json::json!({ "command": "sleep 5", "background": true }),
                &c,
            )
            .await;
        assert_eq!(bg.report.as_ref().unwrap().status, ToolStatus::Running);
        assert!(!bg.is_error);
        assert!(job_id_of(&bg).starts_with("job_"), "{}", bg.content);
        assert_eq!(
            bg.report.as_ref().unwrap().exit_code,
            None,
            "no code while running"
        );
        reg.stop_all().await;
        crate::shell::close_session(&c.session_id).await;
    }

    /// A command still running at the yield returns as a job; its output
    /// and end are read through `Job`, and the deadline still holds.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_yielded_command_is_a_job_read_to_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let (bash, reg, c) = with_jobs(dir.path());
        let r = bash
            .execute(
                serde_json::json!({ "command": "echo first; sleep 0.8; echo last", "yield_time_ms": 150 }),
                &c,
            )
            .await;
        let rep = r.report.clone().unwrap();
        assert_eq!(rep.status, ToolStatus::Running, "{}", r.content);
        assert!(!r.is_error);
        let id = job_id_of(&r);
        assert!(
            matches!(&rep.body, ToolBody::Streams { stdout, .. } if stdout.contains("first")),
            "{:?}",
            rep.body
        );
        let task = reg.view(&id, None).expect("the job").task;
        assert!(task.wait_finished(Duration::from_secs(5)).await);
        let out = JobTool::new(Arc::clone(&reg))
            .execute(serde_json::json!({ "action": "output", "job_id": id }), &c)
            .await;
        assert!(out.content.contains("last"), "{}", out.content);
        crate::shell::close_session(&c.session_id).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_timeout_still_ends_a_yielded_command() {
        let dir = tempfile::tempdir().unwrap();
        let (bash, reg, c) = with_jobs(dir.path());
        let r = bash
            .execute(
                serde_json::json!({ "command": "sleep 30", "yield_time_ms": 100, "timeout": 700 }),
                &c,
            )
            .await;
        assert_eq!(r.report.as_ref().unwrap().status, ToolStatus::Running);
        let id = job_id_of(&r);
        let task = reg.view(&id, None).expect("the job").task;
        let started = std::time::Instant::now();
        assert!(task.wait_finished(Duration::from_secs(5)).await);
        assert!(started.elapsed() < Duration::from_secs(4), "deadline held");
        crate::shell::close_session(&c.session_id).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn progress_is_reported_while_a_command_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (bash, _reg, c) = with_jobs(dir.path());
        let seen = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let s = seen.clone();
        c.extensions.insert(crate::shell::ProgressSink(Arc::new(
            move |_tool: &str, m: &str| s.lock().push(m.to_string()),
        )));
        c.extensions.insert(ShellConfig {
            progress_every: Duration::from_millis(200),
            ..ShellConfig::default()
        });
        let r = bash
            .execute(serde_json::json!({ "command": "sleep 0.9" }), &c)
            .await;
        assert!(!r.is_error, "{}", r.content);
        let seen: Vec<String> = seen.lock().clone();
        assert!(seen.len() >= 2, "{seen:?}");
        assert!(seen.iter().any(|m| m.contains("no output")), "{seen:?}");
        crate::shell::close_session(&c.session_id).await;
    }

    /// A timeout that comes before the hand-over ends the command as a
    /// timeout: it is not returned as still running.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_before_the_hand_over_is_a_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let r = BashTool::default()
            .execute(
                serde_json::json!({ "command": "sleep 30", "timeout": 300, "yield_time_ms": 300 }),
                &c,
            )
            .await;
        let rep = r.report.clone().unwrap();
        assert_eq!(rep.status, ToolStatus::TimedOut, "{}", r.content);
        assert!(r.metadata.is_none() || r.metadata.as_ref().unwrap().get("job_id").is_none());
        crate::shell::close_session(&c.session_id).await;
    }

    #[tokio::test]
    async fn unknown_and_misplaced_options_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (bash, _reg, c) = with_jobs(dir.path());
        let r = bash
            .execute(
                serde_json::json!({ "command": "true", "ready_pattern": "x" }),
                &c,
            )
            .await;
        assert!(
            r.is_error && r.content.contains("background: true"),
            "{}",
            r.content
        );
    }
}
