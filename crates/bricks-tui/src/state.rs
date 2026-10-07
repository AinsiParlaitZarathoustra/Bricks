//! Presentation state, built only from the engine's events (and the stored
//! history on resume). It holds what to draw, never what the engine owns:
//! the conversation itself stays in the session store.

use cersei_agent::control::*;
use cersei_agent::ContextStatus;
use cersei_types::Usage;
use std::time::Instant;

/// Tool outputs kept for inspection (the head; the engine stores the
/// originals with the session).
pub const KEPT_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallStatus {
    Running,
    Ok,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub status: CallStatus,
    pub duration_ms: u64,
    pub output: String,
    pub output_bytes: usize,
    pub progress: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Cell {
    User {
        text: String,
        attachments: Vec<String>,
    },
    Assistant {
        text: String,
        open: bool,
    },
    /// Reasoning the provider exposed. Nothing is shown when it sent none.
    Thinking {
        text: String,
        open: bool,
    },
    Tools {
        calls: Vec<ToolCall>,
    },
    Approval {
        request: ApprovalRequest,
        resolution: Option<(Decision, DecidedBy)>,
    },
    Edits {
        files: Vec<WrittenFile>,
    },
    Notice(String),
    Error(String),
    /// Results of `/search` (computed by the engine, shown as is).
    Search {
        query: String,
        status: String,
        hits: Vec<cersei_agent::control::SearchHit>,
        omitted: usize,
        notes: Vec<String>,
        elapsed_ms: u64,
    },
    RunEnd {
        outcome: RunOutcome,
        error: Option<String>,
        seconds: f64,
    },
}

impl Cell {
    /// Still changing: not ready for the scrollback.
    pub fn is_open(&self) -> bool {
        match self {
            Cell::Assistant { open, .. } | Cell::Thinking { open, .. } => *open,
            Cell::Tools { calls } => calls.iter().any(|c| c.status == CallStatus::Running),
            Cell::Approval { resolution, .. } => resolution.is_none(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub session_id: String,
    pub working_dir: String,
    pub model: String,
    pub reasoning: Option<String>,
    pub context: Option<ContextStatus>,
    pub total: Option<Usage>,
    pub turns_usage: Vec<Usage>,
}

#[derive(Debug, Clone)]
pub struct Recall {
    pub items: usize,
    pub tokens: u64,
    pub omitted: usize,
    pub budget: usize,
}

#[derive(Debug, Clone)]
pub enum Maintenance {
    Running,
    Done {
        outcome: MaintenanceOutcome,
        report: Option<cersei_memory::MaintenanceReport>,
        error: Option<String>,
    },
}

#[derive(Default)]
pub struct App {
    pub cells: Vec<Cell>,
    /// Cells already written to the scrollback.
    pub committed: usize,
    pub status: Status,
    pub run: Option<(String, Instant)>,
    pub pending: Vec<ApprovalRequest>,
    /// Changes written in this session, in order.
    pub applied: Vec<(String, Vec<WrittenFile>)>,
    pub recall: Option<Recall>,
    pub maintenance: Option<Maintenance>,
    pub warnings: Vec<String>,
    /// The agent wrote files: the mention index must be refreshed.
    pub files_changed: bool,
    pub last_seq: u64,
}

fn summary(input: &serde_json::Value) -> String {
    for key in [
        "file_path",
        "path",
        "command",
        "pattern",
        "url",
        "query",
        "patch",
    ] {
        if let Some(v) = input.get(key).and_then(|v| v.as_str()) {
            return v.lines().next().unwrap_or("").chars().take(100).collect();
        }
    }
    String::new()
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn running(&self) -> bool {
        self.run.is_some()
    }

    fn close_open_text(&mut self) {
        for c in self.cells.iter_mut().rev() {
            match c {
                Cell::Assistant { open, .. } | Cell::Thinking { open, .. } => *open = false,
                _ => {}
            }
        }
    }

    fn calls_mut(&mut self, id: &str) -> Option<&mut ToolCall> {
        self.cells.iter_mut().rev().find_map(|c| match c {
            Cell::Tools { calls } => calls.iter_mut().find(|t| t.id == id),
            _ => None,
        })
    }

    /// Show a submitted prompt (the engine echoes it as `run_started`).
    pub fn push_user(&mut self, text: &str, attachments: Vec<String>) {
        self.cells.push(Cell::User {
            text: text.to_string(),
            attachments,
        });
    }

    pub fn note(&mut self, text: impl Into<String>) {
        self.cells.push(Cell::Notice(text.into()));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.cells.push(Cell::Error(text.into()));
    }

    /// Cells ready for the scrollback: the finished prefix.
    pub fn ready_to_commit(&self) -> usize {
        let mut n = self.committed;
        while n < self.cells.len() && !self.cells[n].is_open() {
            n += 1;
        }
        n
    }

    pub fn apply(&mut self, env: &Envelope) {
        self.last_seq = env.seq;
        self.status.session_id = env.session_id.clone();
        match &env.event {
            Event::SessionOpened {
                working_dir,
                model,
                reasoning,
                warnings,
                resumed,
                message_count,
            } => {
                self.status.working_dir = working_dir.clone();
                self.status.model = model.clone();
                self.status.reasoning = reasoning.clone();
                self.warnings.extend(warnings.iter().cloned());
                for w in warnings {
                    self.note(format!("warning: {w}"));
                }
                if *resumed {
                    self.note(format!(
                        "resumed session {} ({message_count} messages)",
                        env.session_id
                    ));
                }
            }
            Event::RunStarted {
                prompt,
                attachments,
                model,
                reasoning,
            } => {
                self.run = Some((env.run_id.clone().unwrap_or_default(), Instant::now()));
                self.status.model = model.clone();
                self.status.reasoning = reasoning.clone();
                let labels = attachments
                    .iter()
                    .map(|a| {
                        let mut l = format!("{} {} ({} B)", a.kind, a.path, a.bytes);
                        if let Some(n) = &a.note {
                            l.push_str(&format!(" — {n}"));
                        }
                        l
                    })
                    .collect();
                self.push_user(prompt, labels);
            }
            Event::TextDelta { text } => match self.cells.last_mut() {
                Some(Cell::Assistant {
                    text: t,
                    open: true,
                }) => t.push_str(text),
                _ => {
                    self.close_open_text();
                    self.cells.push(Cell::Assistant {
                        text: text.clone(),
                        open: true,
                    });
                }
            },
            Event::ThinkingDelta { text } => match self.cells.last_mut() {
                Some(Cell::Thinking {
                    text: t,
                    open: true,
                }) => t.push_str(text),
                _ => {
                    self.close_open_text();
                    self.cells.push(Cell::Thinking {
                        text: text.clone(),
                        open: true,
                    });
                }
            },
            Event::ToolStarted {
                tool_call_id,
                name,
                input,
            } => {
                self.close_open_text();
                let call = ToolCall {
                    id: tool_call_id.clone(),
                    name: name.clone(),
                    summary: summary(input),
                    status: CallStatus::Running,
                    duration_ms: 0,
                    output: String::new(),
                    output_bytes: 0,
                    progress: None,
                };
                match self.cells.last_mut() {
                    Some(Cell::Tools { calls }) => calls.push(call),
                    _ => self.cells.push(Cell::Tools { calls: vec![call] }),
                }
            }
            Event::ToolProgress {
                tool_call_id,
                name,
                message,
            } => {
                let target = self.cells.iter_mut().rev().find_map(|c| match c {
                    Cell::Tools { calls } => calls.iter_mut().find(|t| {
                        t.status == CallStatus::Running
                            && match tool_call_id {
                                Some(id) => &t.id == id,
                                None => &t.name == name,
                            }
                    }),
                    _ => None,
                });
                if let Some(t) = target {
                    t.progress = Some(message.clone());
                }
            }
            Event::ToolFinished {
                tool_call_id,
                is_error,
                duration_ms,
                output,
                ..
            } => {
                if let Some(t) = self.calls_mut(tool_call_id) {
                    t.status = if *is_error {
                        CallStatus::Failed
                    } else {
                        CallStatus::Ok
                    };
                    t.duration_ms = *duration_ms;
                    t.output_bytes = output.len();
                    let mut cut = output.len().min(KEPT_OUTPUT_BYTES);
                    while !output.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    t.output = output[..cut].to_string();
                    t.progress = None;
                }
            }
            Event::ApprovalRequested { approval } => {
                self.pending.push(approval.clone());
                self.cells.push(Cell::Approval {
                    request: approval.clone(),
                    resolution: None,
                });
            }
            Event::ApprovalResolved {
                approval_id,
                decision,
                by,
                ..
            } => {
                self.pending.retain(|p| &p.approval_id != approval_id);
                for c in self.cells.iter_mut().rev() {
                    if let Cell::Approval {
                        request,
                        resolution,
                    } = c
                    {
                        if &request.approval_id == approval_id {
                            *resolution = Some((*decision, *by));
                            break;
                        }
                    }
                }
            }
            Event::EditApplied { tool, files, .. } => {
                self.applied.push((tool.clone(), files.clone()));
                self.files_changed = true;
                self.cells.push(Cell::Edits {
                    files: files.clone(),
                });
            }
            Event::MemoryRecalled {
                items,
                tokens,
                omitted,
                budget,
            } => {
                self.recall = Some(Recall {
                    items: *items,
                    tokens: *tokens,
                    omitted: *omitted,
                    budget: *budget,
                });
            }
            Event::Context { status } => self.status.context = Some((**status).clone()),
            Event::Usage { turn, total } => {
                self.status.turns_usage.push((**turn).clone());
                self.status.total = Some((**total).clone());
            }
            Event::Compaction {
                outcome, compacted, ..
            } => {
                self.note(if *compacted {
                    format!("context compacted: {outcome}")
                } else {
                    format!("no compaction: {outcome}")
                });
            }
            Event::ModelChanged {
                model,
                reasoning,
                applies,
            } => {
                self.status.model = model.clone();
                self.status.reasoning = reasoning.clone();
                let when = if applies == "next_turn" {
                    "from the next turn"
                } else {
                    "for the next prompt"
                };
                self.note(format!(
                    "model {model}{} {when}",
                    reasoning
                        .as_ref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default()
                ));
            }
            Event::ContextCleared { messages_removed } => {
                self.status.context = None;
                self.note(format!(
                    "context cleared ({messages_removed} messages kept in the raw history)"
                ));
            }
            Event::SessionSaved => {}
            Event::Notice { message } => self.note(message.clone()),
            Event::RunFinished {
                outcome,
                error,
                approvals_unsatisfied,
                ..
            } => {
                self.close_open_text();
                // Tools interrupted by a cancellation are not running any more.
                for c in self.cells.iter_mut() {
                    if let Cell::Tools { calls } = c {
                        for t in calls.iter_mut().filter(|t| t.status == CallStatus::Running) {
                            t.status = CallStatus::Failed;
                            t.output = "interrupted".into();
                        }
                    }
                    if let Cell::Approval { resolution, .. } = c {
                        if resolution.is_none() {
                            *resolution = Some((Decision::Deny, DecidedBy::Cancelled));
                        }
                    }
                }
                self.pending.clear();
                let seconds = self
                    .run
                    .take()
                    .map(|(_, t)| t.elapsed().as_secs_f64())
                    .unwrap_or_default();
                let mut error = error.clone();
                if !approvals_unsatisfied.is_empty() {
                    let tools: Vec<&str> = approvals_unsatisfied
                        .iter()
                        .map(|a| a.tool.as_str())
                        .collect();
                    error = Some(format!(
                        "{} (needed: {})",
                        error.unwrap_or_default(),
                        tools.join(", ")
                    ));
                }
                self.cells.push(Cell::RunEnd {
                    outcome: *outcome,
                    error,
                    seconds,
                });
            }
            Event::MemoryMaintenanceStarted => self.maintenance = Some(Maintenance::Running),
            Event::MemoryMaintenanceFinished {
                outcome,
                report,
                error,
            } => {
                if *outcome == MaintenanceOutcome::Failed {
                    self.error(format!(
                        "long-term memory maintenance failed: {}",
                        error
                            .clone()
                            .or_else(|| report.as_ref().map(|r| r.errors.join("; ")))
                            .unwrap_or_default()
                    ));
                }
                self.maintenance = Some(Maintenance::Done {
                    outcome: *outcome,
                    report: report.clone(),
                    error: error.clone(),
                });
            }
            Event::SearchResults {
                query,
                status,
                hits,
                omitted,
                notes,
                elapsed_ms,
            } => self.cells.push(Cell::Search {
                query: query.clone(),
                status: status.clone(),
                hits: hits.clone(),
                omitted: *omitted,
                notes: notes.clone(),
                elapsed_ms: *elapsed_ms,
            }),
            Event::CommandRejected { reason, .. } => self.error(reason.clone()),
        }
    }

    /// Cells of a stored conversation (on resume): text only, tool calls
    /// summarised; what the engine stored is the authority.
    pub fn load_history(&mut self, messages: &[cersei_types::Message]) {
        use cersei_types::{ContentBlock, MessageContent, Role};
        for m in messages {
            let blocks: Vec<ContentBlock> = match &m.content {
                MessageContent::Text(t) => vec![ContentBlock::Text { text: t.clone() }],
                MessageContent::Blocks(b) => b.clone(),
            };
            let mut calls = Vec::new();
            for b in blocks {
                match b {
                    ContentBlock::Text { text } if m.role == Role::User => {
                        if !text.starts_with("[system]") {
                            self.push_user(&text, Vec::new());
                        }
                    }
                    ContentBlock::Text { text } => {
                        self.cells.push(Cell::Assistant { text, open: false })
                    }
                    ContentBlock::ToolUse { id, name, input } => calls.push(ToolCall {
                        id,
                        name,
                        summary: summary(&input),
                        status: CallStatus::Ok,
                        duration_ms: 0,
                        output: String::new(),
                        output_bytes: 0,
                        progress: None,
                    }),
                    _ => {}
                }
            }
            if !calls.is_empty() {
                self.cells.push(Cell::Tools { calls });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(seq: u64, event: Event) -> Envelope {
        Envelope {
            schema: SCHEMA_VERSION,
            session_id: "s".into(),
            run_id: Some("r".into()),
            seq,
            at: 0,
            event,
        }
    }

    #[test]
    fn cells_follow_the_run_and_commit_only_when_finished() {
        let mut app = App::new();
        let evs = vec![
            Event::RunStarted {
                prompt: "go".into(),
                attachments: vec![],
                model: "m".into(),
                reasoning: None,
            },
            Event::ThinkingDelta { text: "je ".into() },
            Event::ThinkingDelta {
                text: "réfléchis".into(),
            },
            Event::TextDelta {
                text: "Je lis".into(),
            },
            Event::ToolStarted {
                tool_call_id: "a".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": "x.rs"}),
            },
            Event::ToolStarted {
                tool_call_id: "b".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": "y.rs"}),
            },
        ];
        for (i, e) in evs.into_iter().enumerate() {
            app.apply(&env(i as u64 + 1, e));
        }
        assert_eq!(app.cells.len(), 4, "user, thinking, text, one tool group");
        assert!(matches!(&app.cells[1], Cell::Thinking { text, .. } if text == "je réfléchis"));
        assert_eq!(app.ready_to_commit(), 3, "the running tools stay live");
        app.apply(&env(
            7,
            Event::ToolFinished {
                tool_call_id: "b".into(),
                name: "Read".into(),
                is_error: false,
                duration_ms: 3,
                output: "ok".into(),
            },
        ));
        assert_eq!(app.ready_to_commit(), 3);
        app.apply(&env(
            8,
            Event::ToolFinished {
                tool_call_id: "a".into(),
                name: "Read".into(),
                is_error: true,
                duration_ms: 3,
                output: "boom".into(),
            },
        ));
        assert_eq!(app.ready_to_commit(), 4);
        app.apply(&env(
            9,
            Event::TextDelta {
                text: "Fini".into(),
            },
        ));
        assert_eq!(app.ready_to_commit(), 4, "the streaming answer stays live");
        app.apply(&env(
            10,
            Event::RunFinished {
                outcome: RunOutcome::Succeeded,
                failure: None,
                error: None,
                termination: Some(cersei_agent::Termination::Completed),
                text: "Fini".into(),
                turns: 2,
                approvals_unsatisfied: vec![],
            },
        ));
        assert_eq!(app.ready_to_commit(), app.cells.len());
        assert!(!app.running());
    }

    #[test]
    fn a_resumed_history_becomes_cells() {
        use cersei_types::{ContentBlock, Message};
        let mut app = App::new();
        app.load_history(&[
            Message::user("Bonjour"),
            Message::assistant_blocks(vec![
                ContentBlock::Text {
                    text: "Je regarde.".into(),
                },
                ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "Read".into(),
                    input: serde_json::json!({"file_path": "a.rs"}),
                },
            ]),
            Message::user("[system] nudge"),
            Message::assistant("Voilà."),
        ]);
        let kinds: Vec<&str> = app
            .cells
            .iter()
            .map(|c| match c {
                Cell::User { .. } => "user",
                Cell::Assistant { .. } => "assistant",
                Cell::Tools { .. } => "tools",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["user", "assistant", "tools", "assistant"],
            "engine nudges are not shown as user turns"
        );
        assert_eq!(app.ready_to_commit(), app.cells.len());
    }

    #[test]
    fn a_cancelled_run_leaves_nothing_running() {
        let mut app = App::new();
        app.apply(&env(
            1,
            Event::RunStarted {
                prompt: "go".into(),
                attachments: vec![],
                model: "m".into(),
                reasoning: None,
            },
        ));
        app.apply(&env(
            2,
            Event::ToolStarted {
                tool_call_id: "a".into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "sleep 100"}),
            },
        ));
        app.apply(&env(
            3,
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
        assert_eq!(app.ready_to_commit(), app.cells.len());
        let Cell::Tools { calls } = &app.cells[1] else {
            panic!()
        };
        assert_eq!(calls[0].status, CallStatus::Failed);
    }
}
