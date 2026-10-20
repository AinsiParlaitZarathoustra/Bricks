//! What the desktop keeps of each run once it ends: its answer text, its shell
//! commands and its file changes. The history holds the messages, not these.
//!
//! `<bricks_home>/desktop/run-records/<session_id>.jsonl` holds one
//! [`RunRecord`] per line. A run is written again when its record changes
//! (a background job or a sub-agent finishing later): every line is a version
//! of the run, the last one wins, and the runs keep the order of their first
//! line. A damaged line is skipped on read and never fatal.
//!
//! A deleted session's file is removed and its later records are refused, so
//! a run still draining its events cannot recreate it.
//!
//! [`Recorder`] builds the records from the envelopes of one session: it routes
//! each event to its run (sub-agents and jobs by their owner) and says when a
//! run's record must be written.

use super::io_error;
use super::run_labels::{append_line, check_session_id};
use crate::contract::{IpcResult, RunCommand, RunFile, RunRecord};
use bricks_agent::control::{Envelope, Event, RunOutcome, WrittenFile, cap_text};
use bricks_tools::preview::ChangeKind;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

/// Format of a record line.
pub const RECORD_VERSION: u32 = 1;

/// Output kept for one command.
const COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

/// Runs kept by a session's recorder: the current one and the recent ones,
/// which still take late sub-agent and job events.
const RUNS_KEPT: usize = 8;

/// The shell tools whose calls are commands (background jobs are followed
/// through their Bash call, never counted on their own).
const SHELL_TOOLS: [&str; 4] = ["Bash", "bash", "Shell", "PowerShell"];

pub fn is_shell_tool(name: &str) -> bool {
    SHELL_TOOLS.contains(&name)
}

pub struct RunRecordStore {
    dir: PathBuf,
    /// Serialises appends and deletions; sessions deleted by this process.
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    deleted: HashSet<String>,
}

impl RunRecordStore {
    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            state: Mutex::new(State::default()),
        }
    }

    /// The runs of a session, oldest first, each one at its last version. A
    /// missing file (an older session) gives none.
    pub fn list(&self, session_id: &str) -> IpcResult<Vec<RunRecord>> {
        check_session_id(session_id)?;
        let path = self.path(session_id);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error("enregistrements des exécutions", e)),
        };
        let mut skipped = 0usize;
        let mut runs: Vec<RunRecord> = Vec::new();
        let mut position: HashMap<String, usize> = HashMap::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(record) = serde_json::from_str::<RunRecord>(line) else {
                skipped += 1;
                continue;
            };
            match position.get(&record.run_id) {
                Some(&at) => runs[at] = record,
                None => {
                    position.insert(record.run_id.clone(), runs.len());
                    runs.push(record);
                }
            }
        }
        if skipped > 0 {
            eprintln!(
                "bricks-desktop: {skipped} damaged line(s) skipped in {}",
                path.display()
            );
        }
        Ok(runs)
    }

    /// Appends a version of a run. Returns `false` when the session was
    /// deleted. Blocking I/O: call from `spawn_blocking`.
    pub fn append(&self, session_id: &str, record: &RunRecord) -> IpcResult<bool> {
        check_session_id(session_id)?;
        let state = self.state.lock();
        if state.deleted.contains(session_id) {
            return Ok(false);
        }
        let line = serde_json::to_string(record)
            .map_err(|e| super::error(crate::contract::IpcErrorCode::Internal, e.to_string()))?;
        append_line(&self.path(session_id), &line)
            .map_err(|e| io_error("enregistrements des exécutions", e))?;
        Ok(true)
    }

    /// Removes the records of a deleted session and refuses later ones.
    /// Blocking I/O: call from `spawn_blocking`.
    pub fn delete(&self, session_id: &str) -> IpcResult<()> {
        check_session_id(session_id)?;
        let mut state = self.state.lock();
        state.deleted.insert(session_id.to_string());
        match std::fs::remove_file(self.path(session_id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error("enregistrements des exécutions", e)),
        }
    }

    /// Whether the session has a records file (for the deletion check).
    pub fn exists(&self, session_id: &str) -> bool {
        check_session_id(session_id).is_ok() && self.path(session_id).exists()
    }

    fn path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.jsonl"))
    }
}

/// The outcome's name in a record.
fn outcome_name(outcome: RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Succeeded => "succeeded",
        RunOutcome::Incomplete => "incomplete",
        RunOutcome::Failed => "failed",
        RunOutcome::Cancelled => "cancelled",
    }
}

fn kind_name(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Create => "create",
        ChangeKind::Modify => "modify",
        ChangeKind::Delete => "delete",
    }
}

/// The status of a finished background job, as a command's status.
fn job_status(state: &str) -> &'static str {
    match state {
        "completed" => "success",
        "failed" => "failure",
        "stopped" => "cancelled",
        _ => "unknown",
    }
}

/// The command text of a shell tool's input (`""` when there is none).
fn command_of(input: &serde_json::Value) -> String {
    input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A background job started by a command, until it is matched to one.
struct JobSeen {
    job_id: String,
    /// The sub-agent that started it; `None` for the session agent.
    owner: Option<String>,
    command: String,
    /// `(state, duration_ms, logs)` once it ended.
    ended: Option<(String, u64, Vec<String>)>,
}

/// One run, from its start to the last change of its record.
pub struct RunBuilder {
    run_id: String,
    /// User-role messages before the run, when they were counted.
    base: Option<u32>,
    prompt: String,
    started_at: i64,
    finished_at: Option<i64>,
    outcome: Option<String>,
    files: Vec<RunFile>,
    file_at: HashMap<String, usize>,
    /// `(sub-agent, tool call, path)` of every operation already counted.
    operations: HashSet<(Option<String>, String, String)>,
    commands: Vec<RunCommand>,
    command_at: HashMap<(Option<String>, String), usize>,
    /// Command text of the shell calls started and not finished yet.
    started: HashMap<(Option<String>, String), String>,
    jobs: Vec<JobSeen>,
    /// Text segments of the answer, in order.
    segments: Vec<String>,
    current: String,
}

impl RunBuilder {
    pub fn new(run_id: String, base: Option<u32>, prompt: String, started_at: i64) -> Self {
        Self {
            run_id,
            base,
            prompt,
            started_at,
            finished_at: None,
            outcome: None,
            files: Vec::new(),
            file_at: HashMap::new(),
            operations: HashSet::new(),
            commands: Vec::new(),
            command_at: HashMap::new(),
            started: HashMap::new(),
            jobs: Vec::new(),
            segments: Vec::new(),
            current: String::new(),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn base(&self) -> Option<u32> {
        self.base
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }

    /// Ends the current text segment (another item of the timeline follows).
    fn cut(&mut self) {
        if !self.current.is_empty() {
            self.segments.push(std::mem::take(&mut self.current));
        }
    }

    fn answer_text(&self) -> String {
        let mut parts: Vec<&str> = self.segments.iter().map(String::as_str).collect();
        if !self.current.is_empty() {
            parts.push(&self.current);
        }
        parts
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The record of the run as it is now (`user_index` is resolved by the
    /// caller, from the history).
    pub fn to_record(&self, user_index: Option<u32>) -> RunRecord {
        RunRecord {
            version: RECORD_VERSION,
            run_id: self.run_id.clone(),
            user_index,
            started_at: self.started_at,
            finished_at: self.finished_at,
            outcome: self.outcome.clone(),
            files: self.files.clone(),
            commands: self.commands.clone(),
            assistant_text: self.answer_text(),
        }
    }

    fn text(&mut self, text: &str) {
        self.current.push_str(text);
    }

    /// A tool call started: its command text is kept for its end.
    fn tool_started(
        &mut self,
        agent: Option<&str>,
        tool_call_id: &str,
        name: &str,
        input: &serde_json::Value,
    ) {
        self.cut();
        if is_shell_tool(name) {
            let key = (agent.map(String::from), tool_call_id.to_string());
            self.started.insert(key, command_of(input));
        }
    }

    /// A tool call ended. Refusals (`executed == false`) are not commands.
    #[allow(clippy::too_many_arguments)]
    fn tool_finished(
        &mut self,
        agent: Option<&str>,
        tool_call_id: &str,
        name: &str,
        is_error: bool,
        duration_ms: u64,
        output: Option<&str>,
        executed: Option<bool>,
        status: Option<&str>,
    ) -> bool {
        if !is_shell_tool(name) {
            return false;
        }
        let owner = agent.map(String::from);
        let key = (owner.clone(), tool_call_id.to_string());
        let command = self.started.remove(&key).unwrap_or_default();
        if executed == Some(false) {
            return false;
        }
        let status = status.unwrap_or("unknown").to_string();
        let output = output.map(|o| cap_text(o, COMMAND_OUTPUT_BYTES).0);
        let mut command_record = RunCommand {
            root_run_id: self.run_id.clone(),
            agent_id: owner.clone(),
            tool_call_id: tool_call_id.to_string(),
            name: name.to_string(),
            command,
            status,
            is_error,
            duration_ms: Some(duration_ms),
            output,
            job_id: None,
        };
        // A command that handed a job over is matched to it, and takes the
        // job's end when the job already ended.
        if command_record.status == "running"
            && let Some(job) = self.jobs.iter().find(|j| {
                j.owner == owner
                    && j.command == command_record.command
                    && !claimed(&self.commands, &j.job_id)
            })
        {
            command_record.job_id = Some(job.job_id.clone());
            if let Some((state, duration, logs)) = &job.ended {
                apply_job_end(&mut command_record, state, *duration, logs);
            }
        }
        match self
            .command_at
            .get(&(owner.clone(), tool_call_id.to_string()))
        {
            Some(&at) => self.commands[at] = command_record,
            None => {
                self.command_at
                    .insert((owner, tool_call_id.to_string()), self.commands.len());
                self.commands.push(command_record);
            }
        }
        true
    }

    /// A background job started for the command of `owner`.
    fn job_started(&mut self, job_id: &str, owner: Option<String>, command: &str) {
        if self.jobs.iter().any(|j| j.job_id == job_id) {
            return;
        }
        self.jobs.push(JobSeen {
            job_id: job_id.to_string(),
            owner,
            command: command.to_string(),
            ended: None,
        });
    }

    /// A background job ended: its command takes the end, if it is matched.
    fn job_finished(
        &mut self,
        job_id: &str,
        state: &str,
        duration_ms: u64,
        logs: &[String],
    ) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|j| j.job_id == job_id) else {
            return false;
        };
        job.ended = Some((state.to_string(), duration_ms, logs.to_vec()));
        let Some(command) = self
            .commands
            .iter_mut()
            .find(|c| c.job_id.as_deref() == Some(job_id))
        else {
            return true;
        };
        apply_job_end(command, state, duration_ms, logs);
        true
    }

    /// The files an applied change wrote. Each operation counts once per
    /// `(sub-agent, tool call, path)`.
    fn edit(&mut self, agent: Option<&str>, tool_call_id: &str, files: &[WrittenFile]) -> bool {
        self.cut();
        let mut changed = false;
        for file in files {
            let owner = agent.map(String::from);
            if !self
                .operations
                .insert((owner.clone(), tool_call_id.to_string(), file.path.clone()))
            {
                continue;
            }
            changed = true;
            let absolute = file.absolute.clone();
            let key = format!(
                "{}|{}",
                self.run_id,
                absolute.as_deref().unwrap_or(&file.path)
            );
            let at = match self.file_at.get(&key) {
                Some(&at) => at,
                None => {
                    self.files.push(RunFile {
                        key: key.clone(),
                        path: file.path.clone(),
                        absolute: absolute.clone(),
                        kind: kind_name(file.kind).to_string(),
                        added: 0,
                        removed: 0,
                        binary: false,
                        operations: 0,
                        agent_ids: Vec::new(),
                    });
                    self.file_at.insert(key, self.files.len() - 1);
                    self.files.len() - 1
                }
            };
            let record = &mut self.files[at];
            record.added += file.added as u64;
            record.removed += file.removed as u64;
            record.binary |= file.binary;
            record.operations += 1;
            record.kind = kind_name(file.kind).to_string();
            if record.absolute.is_none() {
                record.absolute = absolute;
            }
            if let Some(id) = agent
                && !record.agent_ids.iter().any(|a| a == id)
            {
                record.agent_ids.push(id.to_string());
            }
        }
        changed
    }

    /// The run ended: its text is complete, and a shell call of the session
    /// agent that never reported an end is recorded as `unknown`.
    fn finish(&mut self, outcome: &str, at: i64) {
        self.cut();
        let pending: Vec<((Option<String>, String), String)> = self
            .started
            .iter()
            .filter(|((agent, _), _)| agent.is_none())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for ((agent, id), command) in pending {
            self.started.remove(&(agent.clone(), id.clone()));
            self.command_at
                .insert((agent.clone(), id.clone()), self.commands.len());
            self.commands.push(RunCommand {
                root_run_id: self.run_id.clone(),
                agent_id: agent,
                tool_call_id: id,
                name: "Bash".into(),
                command,
                status: "unknown".into(),
                is_error: false,
                duration_ms: None,
                output: None,
                job_id: None,
            });
        }
        self.outcome = Some(outcome.to_string());
        self.finished_at = Some(at);
    }
}

/// Whether a command already took the job `job_id`.
fn claimed(commands: &[RunCommand], job_id: &str) -> bool {
    commands.iter().any(|c| c.job_id.as_deref() == Some(job_id))
}

fn apply_job_end(command: &mut RunCommand, state: &str, duration_ms: u64, logs: &[String]) {
    command.status = job_status(state).to_string();
    command.is_error = command.status != "success";
    command.duration_ms = Some(duration_ms);
    if !logs.is_empty() {
        command.output = Some(cap_text(&logs.join("\n"), COMMAND_OUTPUT_BYTES).0);
    }
}

/// The runs of one session being recorded.
#[derive(Default)]
pub struct Recorder {
    runs: VecDeque<RunBuilder>,
    /// Sub-agent id → the root run it works for.
    agents: HashMap<String, String>,
    /// Background job id → the root run it belongs to.
    jobs: HashMap<String, String>,
}

impl Recorder {
    /// A run started. `base` is the number of user messages before it, when
    /// it was counted (see `RunBuilder::base`).
    pub fn start(&mut self, run_id: &str, base: Option<u32>, prompt: &str, at: i64) {
        self.runs.push_back(RunBuilder::new(
            run_id.to_string(),
            base,
            prompt.to_string(),
            at,
        ));
        while self.runs.len() > RUNS_KEPT {
            self.runs.pop_front();
        }
    }

    /// The record of a run, when it is still kept.
    pub fn run(&self, run_id: &str) -> Option<&RunBuilder> {
        self.runs.iter().find(|r| r.run_id == run_id)
    }

    /// Feeds one envelope. Returns the run whose record changed after its end:
    /// its record must be written again.
    pub fn observe(&mut self, env: &Envelope) -> Option<String> {
        let run_id = self.route(env)?;
        let owner = self.owner_of(env);
        let at = env.at;
        let run = self.runs.iter_mut().find(|r| r.run_id == run_id)?;
        let changed = match &env.event {
            Event::TextDelta { text } => {
                run.text(text);
                false
            }
            Event::ToolStarted {
                tool_call_id,
                name,
                input,
            } => {
                run.tool_started(None, tool_call_id, name, input);
                false
            }
            Event::AgentToolStarted {
                agent_id,
                tool_call_id,
                name,
                input,
            } => {
                run.tool_started(Some(agent_id), tool_call_id, name, input);
                false
            }
            Event::ToolFinished {
                tool_call_id,
                name,
                is_error,
                duration_ms,
                output,
                executed,
                status,
            } => run.tool_finished(
                None,
                tool_call_id,
                name,
                *is_error,
                *duration_ms,
                Some(output),
                *executed,
                status.as_deref(),
            ),
            Event::AgentToolFinished {
                agent_id,
                tool_call_id,
                name,
                is_error,
                duration_ms,
                executed,
                status,
                output,
                ..
            } => run.tool_finished(
                Some(agent_id),
                tool_call_id,
                name,
                *is_error,
                *duration_ms,
                output.as_deref(),
                *executed,
                status.as_deref(),
            ),
            Event::EditApplied {
                tool_call_id,
                files,
                agent_id,
                ..
            } => run.edit(agent_id.as_deref(), tool_call_id, files),
            Event::JobStarted {
                job_id, command, ..
            } => {
                run.job_started(job_id, owner, command);
                false
            }
            Event::JobFinished {
                job_id,
                state,
                duration_ms,
                logs,
                ..
            } => run.job_finished(job_id, state, *duration_ms, logs),
            Event::ApprovalRequested { .. }
            | Event::Notice { .. }
            | Event::Compaction { .. }
            | Event::ContextCleared { .. }
            | Event::SkillActivationFailed { .. } => {
                run.cut();
                false
            }
            Event::RunFinished { outcome, .. } => {
                run.finish(outcome_name(*outcome), at);
                true
            }
            _ => false,
        };
        (changed && run.is_finished()).then_some(run_id)
    }

    /// The run an envelope belongs to: a sub-agent's events go to the run it
    /// works for, a job's to the run that started it.
    fn route(&mut self, env: &Envelope) -> Option<String> {
        match &env.event {
            Event::AgentSpawned { agent } => {
                self.agents
                    .insert(agent.agent_id.clone(), agent.root_run_id.clone());
                None
            }
            Event::AgentToolStarted { agent_id, .. }
            | Event::AgentToolFinished { agent_id, .. }
            | Event::EditApplied {
                agent_id: Some(agent_id),
                ..
            } => self
                .agents
                .get(agent_id)
                .cloned()
                .or_else(|| env.run_id.clone()),
            Event::JobStarted {
                job_id,
                root_run_id,
                ..
            } => {
                self.jobs.insert(job_id.clone(), root_run_id.clone());
                Some(root_run_id.clone())
            }
            Event::JobFinished { job_id, .. } => self.jobs.get(job_id).cloned(),
            _ => env.run_id.clone(),
        }
    }

    /// The owner of a job started by this envelope: the sub-agent when the
    /// job belongs to one, else `None` (the session agent).
    fn owner_of(&self, env: &Envelope) -> Option<String> {
        match &env.event {
            Event::JobStarted { agent_id, .. } if self.agents.contains_key(agent_id) => {
                Some(agent_id.clone())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bricks_agent::control::{Envelope, Event, SCHEMA_VERSION};

    fn record(run: &str, text: &str, at: i64) -> RunRecord {
        RunRecord {
            version: RECORD_VERSION,
            run_id: run.into(),
            user_index: Some(1),
            started_at: at,
            finished_at: Some(at + 5),
            outcome: Some("succeeded".into()),
            files: vec![],
            commands: vec![],
            assistant_text: text.into(),
        }
    }

    #[test]
    fn a_run_appended_twice_is_listed_once_at_its_last_version_in_its_first_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunRecordStore::at(dir.path().join("run-records"));
        assert!(
            store.list("s1").unwrap().is_empty(),
            "a missing file is no run"
        );
        store.append("s1", &record("run_a", "premier", 1)).unwrap();
        store.append("s1", &record("run_b", "deuxième", 2)).unwrap();
        store
            .append("s1", &record("run_a", "premier, corrigé", 1))
            .unwrap();
        let listed = store.list("s1").unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].run_id, "run_a");
        assert_eq!(listed[0].assistant_text, "premier, corrigé");
        assert_eq!(listed[1].run_id, "run_b");
        assert!(
            store.list("s2").unwrap().is_empty(),
            "other sessions are apart"
        );
    }

    #[test]
    fn a_damaged_line_is_skipped_and_the_next_record_starts_on_a_new_line() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunRecordStore::at(dir.path().join("run-records"));
        store.append("s1", &record("run_a", "un", 1)).unwrap();
        let file = dir.path().join("run-records").join("s1.jsonl");
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str("{\"run_id\":\"run_x\",\"assis");
        std::fs::write(&file, text).unwrap();
        assert_eq!(
            store.list("s1").unwrap().len(),
            1,
            "the damaged line is skipped"
        );
        store.append("s1", &record("run_b", "deux", 2)).unwrap();
        let listed = store.list("s1").unwrap();
        assert_eq!(
            listed.iter().map(|r| r.run_id.as_str()).collect::<Vec<_>>(),
            ["run_a", "run_b"]
        );
    }

    #[test]
    fn a_record_without_the_newer_fields_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunRecordStore::at(dir.path().join("run-records"));
        std::fs::create_dir_all(dir.path().join("run-records")).unwrap();
        let old = r#"{"run_id":"run_a","started_at":1}"#;
        std::fs::write(
            dir.path().join("run-records").join("s1.jsonl"),
            format!("{old}\n"),
        )
        .unwrap();
        let listed = store.list("s1").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].user_index, None);
        assert!(listed[0].commands.is_empty() && listed[0].files.is_empty());
        assert_eq!(listed[0].assistant_text, "");
    }

    #[test]
    fn a_deleted_session_loses_its_records_and_gets_no_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunRecordStore::at(dir.path().join("run-records"));
        store.append("s1", &record("run_a", "un", 1)).unwrap();
        assert!(store.exists("s1"));
        store.delete("s1").unwrap();
        assert!(!store.exists("s1"));
        assert!(!store.append("s1", &record("run_late", "tard", 2)).unwrap());
        assert!(
            !store.exists("s1"),
            "a late record does not recreate the file"
        );
        store.delete("s1").unwrap();
    }

    #[test]
    fn a_missing_session_id_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunRecordStore::at(dir.path().join("run-records"));
        assert!(store.list("../escape").is_err());
        assert!(store.delete("a/b").is_err());
    }

    fn env(seq: u64, run: Option<&str>, event: Event) -> Envelope {
        Envelope {
            schema: SCHEMA_VERSION,
            session_id: "s1".into(),
            run_id: run.map(String::from),
            seq,
            at: 1_000 + seq as i64,
            event,
        }
    }

    fn started(rec: &mut Recorder, run: &str) {
        rec.start(run, Some(0), "Fais-le", 1_000);
    }

    fn bash_call(id: &str, command: &str) -> (Event, Event) {
        (
            Event::ToolStarted {
                tool_call_id: id.into(),
                name: "Bash".into(),
                input: serde_json::json!({ "command": command }),
            },
            Event::ToolFinished {
                tool_call_id: id.into(),
                name: "Bash".into(),
                is_error: false,
                duration_ms: 40,
                output: "ok\n".into(),
                executed: Some(true),
                status: Some("success".into()),
            },
        )
    }

    fn written(path: &str, added: usize, removed: usize) -> WrittenFile {
        WrittenFile {
            path: path.into(),
            kind: ChangeKind::Modify,
            added,
            removed,
            binary: false,
            absolute: Some(format!("/work/demo/{path}")),
            patch: Some("@@".into()),
            patch_truncated: false,
        }
    }

    #[test]
    fn the_answer_is_its_segments_joined_by_a_blank_line_and_cut_by_tools_and_edits() {
        let mut rec = Recorder::default();
        started(&mut rec, "run-1");
        let mut seq = 0;
        let mut feed = |rec: &mut Recorder, e: Event| {
            seq += 1;
            rec.observe(&env(seq, Some("run-1"), e))
        };
        feed(
            &mut rec,
            Event::TextDelta {
                text: "  Je lis.\n".into(),
            },
        );
        let (start, end) = bash_call("tc-1", "ls");
        feed(&mut rec, start);
        feed(&mut rec, end);
        feed(
            &mut rec,
            Event::TextDelta {
                text: "Voici".into(),
            },
        );
        feed(
            &mut rec,
            Event::TextDelta {
                text: " le contenu.".into(),
            },
        );
        feed(
            &mut rec,
            Event::EditApplied {
                tool_call_id: "tc-2".into(),
                tool: "Write".into(),
                files: vec![written("a.txt", 1, 0)],
                agent_id: None,
                changeset_id: None,
            },
        );
        feed(
            &mut rec,
            Event::TextDelta {
                text: "Fini.".into(),
            },
        );
        let done = feed(
            &mut rec,
            Event::RunFinished {
                outcome: RunOutcome::Succeeded,
                failure: None,
                error: None,
                termination: None,
                text: "Fini.".into(),
                turns: 2,
                approvals_unsatisfied: vec![],
            },
        );
        assert_eq!(done.as_deref(), Some("run-1"), "the end changes the record");
        let r = rec.run("run-1").unwrap().to_record(Some(0));
        assert_eq!(r.assistant_text, "Je lis.\n\nVoici le contenu.\n\nFini.");
        assert_eq!(r.outcome.as_deref(), Some("succeeded"));
        assert_eq!(
            (r.started_at, r.finished_at),
            (1_000, Some(1_000 + seq as i64))
        );
    }

    #[test]
    fn a_refused_command_is_not_a_command_and_a_job_takes_its_end() {
        let mut rec = Recorder::default();
        started(&mut rec, "run-1");
        let refused = Event::ToolFinished {
            tool_call_id: "tc-0".into(),
            name: "Bash".into(),
            is_error: true,
            duration_ms: 0,
            output: "Permission denied".into(),
            executed: Some(false),
            status: Some("failure".into()),
        };
        rec.observe(&env(
            1,
            Some("run-1"),
            Event::ToolStarted {
                tool_call_id: "tc-0".into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "rm x"}),
            },
        ));
        rec.observe(&env(2, Some("run-1"), refused));
        // A background command: running, then its job.
        rec.observe(&env(
            3,
            Some("run-1"),
            Event::ToolStarted {
                tool_call_id: "tc-1".into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "serve"}),
            },
        ));
        rec.observe(&env(
            4,
            Some("run-1"),
            Event::JobStarted {
                job_id: "job_1".into(),
                agent_id: "root".into(),
                root_run_id: "run-1".into(),
                command: "serve".into(),
                cwd: "/w".into(),
                pid: 7,
            },
        ));
        rec.observe(&env(
            5,
            Some("run-1"),
            Event::ToolFinished {
                tool_call_id: "tc-1".into(),
                name: "Bash".into(),
                is_error: false,
                duration_ms: 900,
                output: "started".into(),
                executed: Some(true),
                status: Some("running".into()),
            },
        ));
        assert!(
            rec.observe(&env(
                6,
                Some("run-1"),
                Event::RunFinished {
                    outcome: RunOutcome::Succeeded,
                    failure: None,
                    error: None,
                    termination: None,
                    text: String::new(),
                    turns: 1,
                    approvals_unsatisfied: vec![]
                }
            ))
            .is_some()
        );
        let r = rec.run("run-1").unwrap().to_record(None);
        assert_eq!(r.commands.len(), 1, "the refusal is not a command");
        assert_eq!(r.commands[0].status, "running");
        assert_eq!(r.commands[0].job_id.as_deref(), Some("job_1"));
        // The job ends after the run: its command takes the end and the record changes.
        let late = rec.observe(&env(
            7,
            None,
            Event::JobFinished {
                job_id: "job_1".into(),
                state: "completed".into(),
                code: Some(0),
                signal: None,
                duration_ms: 5_000,
                logs: vec!["fin".into()],
            },
        ));
        assert_eq!(late.as_deref(), Some("run-1"));
        let r = rec.run("run-1").unwrap().to_record(None);
        assert_eq!(r.commands[0].status, "success");
        assert_eq!(r.commands[0].duration_ms, Some(5_000));
        assert_eq!(r.commands[0].output.as_deref(), Some("fin"));
    }

    #[test]
    fn a_sub_agent_edit_counts_once_per_call_and_path_and_keeps_its_agent() {
        let mut rec = Recorder::default();
        started(&mut rec, "run-1");
        let file = written("src/a.rs", 2, 1);
        let edit = Event::EditApplied {
            tool_call_id: "tc-9".into(),
            tool: "Write".into(),
            files: vec![file.clone()],
            agent_id: Some("ag-1".into()),
            changeset_id: None,
        };
        rec.observe(&env(1, Some("run-1"), edit.clone()));
        rec.observe(&env(2, Some("run-1"), edit));
        rec.observe(&env(
            3,
            Some("run-1"),
            Event::EditApplied {
                tool_call_id: "tc-10".into(),
                tool: "Write".into(),
                files: vec![written("src/a.rs", 1, 0)],
                agent_id: None,
                changeset_id: None,
            },
        ));
        let r = rec.run("run-1").unwrap().to_record(None);
        assert_eq!(r.files.len(), 1);
        let f = &r.files[0];
        assert_eq!(f.key, "run-1|/work/demo/src/a.rs");
        assert_eq!((f.added, f.removed, f.operations), (3, 1, 2));
        assert_eq!(f.agent_ids, vec!["ag-1".to_string()]);
    }

    #[test]
    fn two_files_with_the_same_name_in_two_folders_are_two_files() {
        let mut rec = Recorder::default();
        started(&mut rec, "run-1");
        let mut a = written("x/mod.rs", 1, 0);
        a.absolute = Some("/work/demo/x/mod.rs".into());
        let mut b = written("y/mod.rs", 1, 0);
        b.absolute = Some("/work/demo/y/mod.rs".into());
        rec.observe(&env(
            1,
            Some("run-1"),
            Event::EditApplied {
                tool_call_id: "tc-1".into(),
                tool: "Write".into(),
                files: vec![a, b],
                agent_id: None,
                changeset_id: None,
            },
        ));
        let r = rec.run("run-1").unwrap().to_record(None);
        assert_eq!(r.files.len(), 2);
    }

    #[test]
    fn a_shell_call_without_an_end_is_unknown_once_the_run_ends() {
        let mut rec = Recorder::default();
        started(&mut rec, "run-1");
        rec.observe(&env(
            1,
            Some("run-1"),
            Event::ToolStarted {
                tool_call_id: "tc-1".into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "sleep 9"}),
            },
        ));
        rec.observe(&env(
            2,
            Some("run-1"),
            Event::RunFinished {
                outcome: RunOutcome::Cancelled,
                failure: None,
                error: None,
                termination: None,
                text: String::new(),
                turns: 0,
                approvals_unsatisfied: vec![],
            },
        ));
        let r = rec.run("run-1").unwrap().to_record(None);
        assert_eq!(r.commands.len(), 1);
        assert_eq!(r.commands[0].status, "unknown");
        assert_eq!(r.commands[0].command, "sleep 9");
    }

    #[test]
    fn a_sub_agent_command_is_its_own_entry_of_the_run() {
        let mut rec = Recorder::default();
        started(&mut rec, "run-1");
        rec.observe(&env(
            1,
            Some("run-1"),
            Event::AgentSpawned {
                agent: Box::new(sub_agent_info("ag-1", "run-1")),
            },
        ));
        rec.observe(&env(
            2,
            None,
            Event::AgentToolStarted {
                agent_id: "ag-1".into(),
                tool_call_id: "tc-1".into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "cargo test"}),
            },
        ));
        // Its end comes after the run: it still belongs to the run.
        let changed = rec.observe(&env(
            3,
            None,
            Event::AgentToolFinished {
                agent_id: "ag-1".into(),
                tool_call_id: "tc-1".into(),
                name: "Bash".into(),
                is_error: false,
                duration_ms: 3_000,
                executed: Some(true),
                status: Some("success".into()),
                output: Some("ok".into()),
                output_truncated: false,
            },
        ));
        assert_eq!(changed, None, "a run not yet ended is not written");
        let r = rec.run("run-1").unwrap().to_record(None);
        assert_eq!(r.commands[0].agent_id.as_deref(), Some("ag-1"));
        assert_eq!(r.commands[0].status, "success");
    }

    fn sub_agent_info(agent: &str, root: &str) -> bricks_agent::agents::SpawnInfo {
        serde_json::from_value(serde_json::json!({
            "agent_id": agent,
            "root_run_id": root,
            "profile": "explore",
            "profile_source": "builtin",
            "profile_revision": "r1",
            "model": { "requested": "m", "applied": "m" },
            "reasoning": { "requested": "low", "applied": "low" },
            "workspace": "/work/demo",
            "isolation": "none",
            "task": "t",
            "created_at": "2026-01-01T00:00:00Z",
            "background": false,
            "depth": 1
        }))
        .expect("a spawn info")
    }

    #[test]
    fn the_recorder_keeps_only_the_recent_runs() {
        let mut rec = Recorder::default();
        for i in 0..(RUNS_KEPT + 2) {
            rec.start(&format!("run-{i}"), None, "p", i as i64);
        }
        assert!(rec.run("run-0").is_none(), "the oldest runs are let go");
        assert!(rec.run(&format!("run-{}", RUNS_KEPT + 1)).is_some());
    }

    #[test]
    fn cut_output_is_marked_on_a_character_boundary() {
        let (cut, truncated) = cap_text(&"é".repeat(COMMAND_OUTPUT_BYTES), COMMAND_OUTPUT_BYTES);
        assert!(truncated);
        assert!(cut.len() <= COMMAND_OUTPUT_BYTES);
    }
}
