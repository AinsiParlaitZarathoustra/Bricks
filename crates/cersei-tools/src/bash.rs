//! Bash tool: commands run in the session's shell state, and supervised
//! background tasks.
//!
//! Each command is its own `bash` process (see [`crate::shell`]), started from
//! the session's state: working directory, exported variables, functions and
//! aliases persist between calls because a command that ends normally
//! publishes its changes; variables that are not exported do not persist.
//! Commands run side by side. Stdin is empty unless `input` is given. A
//! command still running after `yield_time_ms` is handed over as a task: the
//! call returns `Running` with its ids, and the command goes on, readable and
//! stoppable with `BashTaskStatus` / `BashTaskOutput` / `BashTaskStop` and
//! `Job`. `timeout` still ends it. `background: true` starts a task at once.
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

pub struct BashTool;

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
         `timeout` is in milliseconds (default 120000, max 600000) and a timed-out command returns \
         its partial output. A command still running after `yield_time_ms` (default 10000) returns \
         as Running with a task id and keeps going: read it with BashTaskOutput / BashTaskStatus, \
         stop it with BashTaskStop. For servers, watchers and other long-running processes use \
         `background: true` (optionally `ready_pattern`) — do not end a command with `&`."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The bash command to execute" },
                "timeout": { "type": "integer", "description": "Timeout in milliseconds (default 120000, max 600000); ends the command, also after a hand-over" },
                "background": { "type": "boolean", "description": "Start as a supervised background task and return its task id" },
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
            .get::<std::sync::Arc<dyn cersei_vms::Sandbox>>()
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
            if input.background {
                if input.yield_time_ms.is_some() {
                    return ToolResult::error(
                        "`yield_time_ms` only applies to a foreground command: a background \
                         command is a task from the start.",
                    );
                }
                return start_background(&input, ctx, &config).await;
            }
            if input.ready_pattern.is_some() || input.ready_timeout.is_some() {
                return ToolResult::error(
                    "`ready_pattern` and `ready_timeout` only apply with `background: true`.",
                );
            }
            run_foreground(&input, ctx, &config, timeout).await
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
    let jobs = ctx.extensions.get::<crate::jobs::JobsHandle>();
    let owner = ctx.extensions.get::<crate::jobs::JobOwner>();
    let wants_job = jobs.is_some() && owner.is_some();
    let yield_after = input
        .yield_time_ms
        .map(Duration::from_millis)
        .unwrap_or(config.yield_time);
    let progress = progress_for(ctx, "Bash");
    let yield_timer = tokio::time::sleep(yield_after);
    tokio::pin!(yield_timer);
    // A hand-over at or after the deadline never happens: the timeout ends
    // the command, as a timeout (not as a task still running).
    let mut yield_pending = yield_after < timeout;
    let mut ticker = tokio::time::interval(config.progress_every);
    ticker.tick().await;
    let mut last_bytes = 0u64;
    let mut quiet_since = std::time::Instant::now();
    let mut refused: Option<String> = None;

    // `None`: the command ended; `Some(slot)`: it is handed over, with the
    // place reserved for it as a job (if any).
    let handed_over: Option<Option<crate::jobs::JobSlot>> = loop {
        tokio::select! {
            _ = task.wait_end() => break None,
            _ = &mut yield_timer, if yield_pending => {
                yield_pending = false;
                if !wants_job {
                    break Some(None);
                }
                match jobs.as_ref().map(|j| j.0.reserve()) {
                    Some(Ok(slot)) => break Some(Some(slot)),
                    Some(Err(e)) => refused = Some(e),
                    None => break Some(None),
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
                        cersei_types::duration::display_ms(started.elapsed()),
                        bytes
                    );
                    if quiet >= config.progress_every {
                        msg.push_str(&format!(
                            "; no output for {} — it may be computing, or waiting for input it \
                             will not get (stdin is {})",
                            cersei_types::duration::display_ms(quiet),
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
                "still running after {}: not handed over as a task ({e}); the call waited for it",
                cersei_types::duration::display_ms(yield_after)
            ));
        }
        return finished_result(timeout, outcome, notes);
    };

    let job_id = match (slot, owner.as_ref(), jobs.as_ref()) {
        (Some(slot), Some(owner), Some(jobs)) => Some(jobs.0.register(
            slot,
            (**owner).clone(),
            Arc::clone(&task),
            &ctx.working_dir,
            false,
        )),
        _ => None,
    };
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
                "aucun code de sortie ({} processus arrêtés{})",
                outcome.terminated.targeted.len(),
                if outcome.terminated.forced.is_empty() {
                    String::new()
                } else {
                    format!(", dont {} de force", outcome.terminated.forced.len())
                }
            )),
        ),
        RunStatus::Cancelled => (ToolStatus::Cancelled, None, Some("annulé".into())),
        RunStatus::Killed { signal } => (
            ToolStatus::Failure,
            None,
            Some(match signal {
                Some(s) => format!("commande tuée par le signal {s}"),
                None => "commande terminée sans statut".into(),
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
                "{name} brut complet ({} octets) : {}",
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
    job_id: Option<String>,
    started: std::time::Instant,
    cwd: &std::path::Path,
    partial: [crate::shell::capture::Captured; 2],
) -> ToolResult {
    let st = task.status();
    let cursor_out = task
        .read(shell::background::Stream::Stdout, 0, 0)
        .total_lines;
    let cursor_err = task
        .read(shell::background::Stream::Stderr, 0, 0)
        .total_lines;
    let mut notes = vec![format!(
        "Commande encore en cours après {} : tâche {} (pid {}) — {}",
        cersei_types::duration::display_ms(started.elapsed()),
        st.id,
        st.pid,
        st.command
    )];
    if let Some(j) = &job_id {
        notes.push(format!(
            "job : {j} (Job status / output / wait / stop) · cwd {}",
            cwd.display()
        ));
    }
    notes.push(format!(
        "La suite : BashTaskOutput (task_id \"{0}\", stream, offset = curseur : stdout depuis la \
         ligne {cursor_out}, stderr depuis la ligne {cursor_err}) ; état : BashTaskStatus ; \
         arrêt : BashTaskStop (task_id \"{0}\").",
        st.id
    ));
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
        "task_id": st.id,
        "job_id": job_id,
        "pid": st.pid,
        "state": st.state.label(),
        "cwd": cwd.display().to_string(),
        "cursors": { "stdout": cursor_out, "stderr": cursor_err },
    }));
    ToolResult::from_report(report)
}

#[cfg(unix)]
async fn start_background(input: &Input, ctx: &ToolContext, config: &ShellConfig) -> ToolResult {
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
    let jobs = ctx.extensions.get::<crate::jobs::JobsHandle>();
    let owner = ctx.extensions.get::<crate::jobs::JobOwner>();
    let mut config = config.clone();
    if let Some(j) = &jobs {
        config.tasks.memory_bytes = j.0.settings.output_buffer_bytes;
        config.tasks.max_line_bytes = j.0.settings.max_line_bytes;
        config.tasks.raw_log_bytes = j.0.settings.raw_log_bytes;
    }
    let slot = match (&jobs, &owner) {
        (Some(j), Some(_)) => match j.0.reserve() {
            Ok(slot) => Some(slot),
            Err(e) => return ToolResult::error(e),
        },
        _ => None,
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
    let job_id = match (slot, &jobs, &owner) {
        (Some(slot), Some(j), Some(o)) => Some(j.0.register(
            slot,
            (**o).clone(),
            Arc::clone(&task),
            &ctx.working_dir,
            true,
        )),
        _ => None,
    };
    let mut lines = vec![format!(
        "Tâche {} démarrée (pid {}) : {}",
        st.id, st.pid, st.command
    )];
    if let Some(j) = &job_id {
        lines.push(format!(
            "job : {j} (Job status / output / wait / stop) · cwd {}",
            ctx.working_dir.display()
        ));
    }
    let status = match &st.state {
        TaskState::Completed { .. } => ToolStatus::Success,
        TaskState::Failed { .. } => ToolStatus::Failure,
        _ => ToolStatus::Running,
    };
    lines.push(format!("état : {}", st.state.label()));
    match &st.readiness {
        Readiness::NotChecked => lines.push(
            "prête : non vérifié (aucun ready_pattern) — un processus lancé n'est pas forcément prêt".into(),
        ),
        Readiness::Waiting => lines.push(format!(
            "prête : pas encore (motif non vu après {})",
            cersei_types::duration::display_ms(started.elapsed())
        )),
        Readiness::Ready { after, line } => {
            lines.push(format!(
                "prête après {} : {line}",
                cersei_types::duration::display_ms(*after)
            ))
        }
    }
    lines.push(format!(
        "Suivi : BashTaskStatus / BashTaskOutput (task_id \"{0}\") ; arrêt : BashTaskStop (task_id \"{0}\").",
        st.id
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
        "task_id": st.id,
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
    report.termination = Some(format!("lancement impossible : {e}"));
    report.duration = Some(started.elapsed());
    if e.kind() == std::io::ErrorKind::NotFound {
        report.suggestion =
            Some("Vérifiez que bash est installé ou disponible dans le PATH.".into());
    }
    ToolResult::from_report(report)
}

#[cfg(feature = "vms")]
async fn run_in_sandbox(
    sandbox: &std::sync::Arc<dyn cersei_vms::Sandbox>,
    command: &str,
    timeout: Duration,
) -> ToolResult {
    // A sandbox runs each command on its own: no persistent state.
    let started = std::time::Instant::now();
    let req = cersei_vms::RunRequest::new(command.to_string()).timeout(timeout);
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
                "exécuté dans le bac à sable {} (sans état persistant entre les commandes)",
                sandbox.id()
            ));
            ToolResult::from_report(report)
        }
        Err(e) => ToolResult::error(format!("Sandbox exec failed: {e}")),
    }
}

// ─── Background task tools ───────────────────────────────────────────────────

fn find_task(
    ctx: &ToolContext,
    id: &str,
) -> std::result::Result<Arc<shell::background::Task>, ToolResult> {
    let sessions = shell::session(&ctx.session_id, &shell_config(ctx));
    sessions.task(id).ok_or_else(|| {
        let ids: Vec<String> = sessions.tasks().iter().map(|t| t.id.clone()).collect();
        crate::tool_feedback::not_found(
            "background task",
            id,
            &ids,
            "Only tasks started in this session with Bash `background: true` (or handed over after `yield_time_ms`) are visible.",
        )
    })
}

fn describe_status(st: &shell::background::TaskStatus) -> String {
    use shell::background::{Readiness, TaskState};
    let code = match &st.state {
        TaskState::Completed { code } => format!(", code {code}"),
        TaskState::Failed { code: Some(c), .. } => format!(", code {c}"),
        TaskState::Failed {
            signal: Some(s), ..
        } => format!(", signal {s}"),
        _ => String::new(),
    };
    let ready = match &st.readiness {
        Readiness::NotChecked => "non vérifiée".to_string(),
        Readiness::Waiting => "pas encore".to_string(),
        Readiness::Ready { after, .. } => {
            format!("oui (après {})", cersei_types::duration::display_ms(*after))
        }
    };
    format!(
        "{} [{}{}] pid {} — {} — prête : {} — lignes stdout {} / stderr {} — {}",
        st.id,
        st.state.label(),
        code,
        st.pid,
        cersei_types::duration::display_ms(st.elapsed),
        ready,
        st.stdout_lines,
        st.stderr_lines,
        st.command
    )
}

pub struct BashTaskStatusTool;

#[async_trait]
impl Tool for BashTaskStatusTool {
    fn name(&self) -> &str {
        "BashTaskStatus"
    }
    fn description(&self) -> &str {
        "Status of this session's background shell tasks (all of them, or one `task_id`): state \
         (running, completed, failed, stopped), exit code once finished, readiness, output size."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": { "task_id": { "type": "string", "description": "A task id (omit for all)" } }
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct In {
            task_id: Option<String>,
        }
        let input: In = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let tasks = match &input.task_id {
            Some(id) => match find_task(ctx, id) {
                Ok(t) => vec![t],
                Err(e) => return e,
            },
            None => shell::session(&ctx.session_id, &shell_config(ctx)).tasks(),
        };
        if tasks.is_empty() {
            return ToolResult::success("Aucune tâche de fond dans cette session.");
        }
        let statuses: Vec<_> = tasks.iter().map(|t| t.status()).collect();
        let text = statuses
            .iter()
            .map(describe_status)
            .collect::<Vec<_>>()
            .join("\n");
        let data: Vec<Value> = statuses
            .iter()
            .map(|s| serde_json::json!({ "task_id": s.id, "state": s.state.label(), "pid": s.pid }))
            .collect();
        ToolResult::success(text).with_metadata(Value::Array(data))
    }
}

pub struct BashTaskOutputTool;

#[async_trait]
impl Tool for BashTaskOutputTool {
    fn name(&self) -> &str {
        "BashTaskOutput"
    }
    fn description(&self) -> &str {
        "Read a page of a background task's output: `stream` stdout or stderr, `offset` = lines to \
         skip (use the returned next offset to continue), `limit` lines (default 200, max 2000)."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "stream": { "type": "string", "enum": ["stdout", "stderr"] },
                "offset": { "type": "integer", "description": "Lines to skip (default 0)" },
                "limit": { "type": "integer", "description": "Lines to return (default 200, max 2000)" }
            },
            "required": ["task_id"]
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        use shell::background::Stream;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct In {
            task_id: String,
            stream: Option<String>,
            offset: Option<u64>,
            limit: Option<usize>,
        }
        let input: In = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let stream = match input.stream.as_deref() {
            None | Some("stdout") => Stream::Stdout,
            Some("stderr") => Stream::Stderr,
            Some(other) => {
                return ToolResult::error(format!(
                    "Unknown stream {other:?}: use stdout or stderr."
                ));
            }
        };
        let task = match find_task(ctx, &input.task_id) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let page = task.read(
            stream,
            input.offset.unwrap_or(0),
            input.limit.unwrap_or(200).clamp(1, 2000),
        );
        let st = task.status();
        let mut out = format!(
            "{} — {} — état : {}\n",
            st.id,
            stream.name(),
            st.state.label()
        );
        if page.dropped_before > 0 {
            out.push_str(&format!(
                "[{} ligne(s) avant cette page ne sont plus en mémoire ; journal brut : {}]\n",
                page.dropped_before,
                page.raw_log.display()
            ));
        }
        for (n, line) in &page.lines {
            out.push_str(&format!("{n:>6} | {line}\n"));
        }
        if let Some(p) = &page.partial {
            out.push_str(&format!("       | {p}   [ligne en cours, non terminée]\n"));
        }
        if page.lines.is_empty() && page.partial.is_none() {
            out.push_str("(aucune ligne à partir de cet offset)\n");
        }
        out.push_str(&format!(
            "[lignes vues : {} ; prochain offset : {}{}]",
            page.total_lines,
            page.next_offset,
            if page.raw_log_capped {
                " ; journal brut plafonné"
            } else {
                ""
            }
        ));
        ToolResult::success(out).with_metadata(serde_json::json!({
            "task_id": st.id,
            "stream": stream.name(),
            "next_offset": page.next_offset,
            "total_lines": page.total_lines,
            "state": st.state.label(),
        }))
    }
}

pub struct BashTaskStopTool;

#[async_trait]
impl Tool for BashTaskStopTool {
    fn name(&self) -> &str {
        "BashTaskStop"
    }
    fn description(&self) -> &str {
        "Stop a background task of this session (graceful, then forced). Stopping a finished task \
         is harmless and reports its final state."
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Shell
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": { "task_id": { "type": "string" } },
            "required": ["task_id"]
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct In {
            task_id: String,
        }
        let input: In = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let task = match find_task(ctx, &input.task_id) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let was_finished = task.state().is_finished();
        let state = task.stop(shell_config(ctx).term_grace).await;
        let st = task.status();
        let msg = if was_finished {
            format!("Déjà terminée : {}", describe_status(&st))
        } else {
            format!("Arrêtée : {}", describe_status(&st))
        };
        ToolResult::success(msg)
            .with_metadata(serde_json::json!({ "task_id": st.id, "state": state.label() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A quick command returns quickly, with all its output: the end of the
    /// output is not waited for once nothing can write to it any more.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_quick_command_does_not_wait_for_the_output_window() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let big = "seq 1 20000";
        BashTool
            .execute(serde_json::json!({ "command": "true" }), &c)
            .await;
        let start = std::time::Instant::now();
        let r = BashTool
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
        let c = ctx(dir.path());
        let r = BashTool
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
    async fn background_commands_are_tasks_with_ids() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        BashTool
            .execute(serde_json::json!({ "command": "true" }), &c)
            .await;
        let bg = BashTool
            .execute(
                serde_json::json!({ "command": "sleep 5", "background": true }),
                &c,
            )
            .await;
        assert_eq!(bg.report.as_ref().unwrap().status, ToolStatus::Running);
        assert!(!bg.is_error);
        assert!(
            bg.metadata.as_ref().unwrap()["task_id"]
                .as_str()
                .unwrap()
                .starts_with("bg-")
        );
        assert_eq!(
            bg.report.as_ref().unwrap().exit_code,
            None,
            "no code while running"
        );
        crate::shell::close_session(&c.session_id).await;
    }

    /// A command still running at the yield returns as a task; its output
    /// and end are read through the task tools, and the deadline still holds.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_yielded_command_is_a_task_read_to_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let r = BashTool
            .execute(
                serde_json::json!({ "command": "echo first; sleep 0.8; echo last", "yield_time_ms": 150 }),
                &c,
            )
            .await;
        let rep = r.report.clone().unwrap();
        assert_eq!(rep.status, ToolStatus::Running, "{}", r.content);
        assert!(!r.is_error);
        let id = r.metadata.as_ref().unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            matches!(&rep.body, ToolBody::Streams { stdout, .. } if stdout.contains("first")),
            "{:?}",
            rep.body
        );
        let task = shell::session(&c.session_id, &shell_config(&c))
            .task(&id)
            .unwrap();
        assert!(task.wait_finished(Duration::from_secs(5)).await);
        let out = BashTaskOutputTool
            .execute(serde_json::json!({ "task_id": id }), &c)
            .await;
        assert!(out.content.contains("last"), "{}", out.content);
        crate::shell::close_session(&c.session_id).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_timeout_still_ends_a_yielded_command() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let r = BashTool
            .execute(
                serde_json::json!({ "command": "sleep 30", "yield_time_ms": 100, "timeout": 700 }),
                &c,
            )
            .await;
        assert_eq!(r.report.as_ref().unwrap().status, ToolStatus::Running);
        let id = r.metadata.as_ref().unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_string();
        let task = shell::session(&c.session_id, &shell_config(&c))
            .task(&id)
            .unwrap();
        let started = std::time::Instant::now();
        assert!(task.wait_finished(Duration::from_secs(5)).await);
        assert!(started.elapsed() < Duration::from_secs(4), "deadline held");
        crate::shell::close_session(&c.session_id).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn progress_is_reported_while_a_command_runs() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let seen = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let s = seen.clone();
        c.extensions.insert(crate::shell::ProgressSink(Arc::new(
            move |_tool: &str, m: &str| s.lock().push(m.to_string()),
        )));
        c.extensions.insert(ShellConfig {
            progress_every: Duration::from_millis(200),
            ..ShellConfig::default()
        });
        let r = BashTool
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
        let r = BashTool
            .execute(
                serde_json::json!({ "command": "sleep 30", "timeout": 300, "yield_time_ms": 300 }),
                &c,
            )
            .await;
        let rep = r.report.clone().unwrap();
        assert_eq!(rep.status, ToolStatus::TimedOut, "{}", r.content);
        assert!(r.metadata.is_none() || r.metadata.as_ref().unwrap().get("task_id").is_none());
        crate::shell::close_session(&c.session_id).await;
    }

    #[test]
    fn unknown_and_misplaced_options_are_refused() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let r = rt.block_on(BashTool.execute(
            serde_json::json!({ "command": "true", "ready_pattern": "x" }),
            &ctx(dir.path()),
        ));
        assert!(
            r.is_error && r.content.contains("background: true"),
            "{}",
            r.content
        );
    }
}
