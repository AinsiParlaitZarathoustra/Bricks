//! What a conversation can do with the others, on the person's request: find
//! and read them, know whether they work, know its own context budget, write
//! to one and get its answer, fork one, and move one to another project.
//! The operations are methods of [`AppState`]; [`SessionTool`] is the one
//! agent tool that reaches them, lent to every session.

use super::live::{OpenSession, RunNotice, files_dir, load_meta};
use super::sessions::check_session_id;
use super::{AppState, error};
use crate::contract::{IpcErrorCode, IpcResult, OpenSessionRequest, SessionItem};
use crate::services::conversations::message_text;
use crate::services::mail::{Mail, MailState};
use crate::services::{invalid, not_found, now_ms};
use bricks_agent::control::{Command, Prompt, SessionMeta};
use bricks_memory::{JsonlMemory, Memory, SaveRevision, SessionHistories, session_keys};
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use bricks_types::{ContentBlock, Message, MessageContent, Role, ToolResultContent};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long a message may wait for its conversation to be free.
const DELIVERY_WAIT: Duration = Duration::from_secs(30 * 60);
/// How long the run a message started may take to answer.
const ANSWER_WAIT: Duration = Duration::from_secs(2 * 60 * 60);
/// The default and longest wait for an answer.
const WAIT_DEFAULT_SECS: u64 = 60;
const WAIT_MAX_SECS: u64 = 600;
/// Messages shown by one `read`, by default and at most.
const PAGE_DEFAULT: usize = 20;
const PAGE_MAX: usize = 100;
/// Characters kept of one message in a `read`, and of an answer.
const MESSAGE_CHARS: usize = 4_000;
/// The longest message one conversation may send.
const MAIL_CHARS: usize = 16_000;

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}… [clipped]")
}

fn rejected(message: impl Into<String>) -> crate::contract::IpcError {
    error(IpcErrorCode::Rejected, message)
}

/// A page of a conversation's messages.
pub struct HistoryPage {
    pub total: usize,
    pub offset: usize,
    pub lines: Vec<String>,
}

impl AppState {
    /// Every conversation of every project, most recently updated first.
    pub async fn sessions_all(&self) -> Vec<SessionItem> {
        let mut out = Vec::new();
        for project in self.projects.list() {
            if let Ok(items) = self.sessions_list(&project.id).await {
                out.extend(items);
            }
        }
        if let Ok(chats) = self.projects.get(crate::services::projects::CHATS_PROJECT_ID)
            && !out.iter().any(|i| i.project_id.as_deref() == Some(chats.id.as_str()))
            && let Ok(items) = self.sessions_list(&chats.id).await
        {
            out.extend(items);
        }
        out.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
        out.dedup_by(|a, b| a.id == b.id);
        out
    }

    /// The messages of a conversation from `offset` (its own history when it
    /// is open, else the stored one).
    pub async fn session_page(&self, id: &str, offset: usize, limit: usize) -> IpcResult<HistoryPage> {
        check_session_id(id)?;
        let messages = match self.open_entry(id) {
            Some(entry) => entry.controller.history().await,
            None => {
                if load_meta(self.sessions_dir(), id).await.is_none() {
                    return Err(not_found(format!("no conversation {id}")));
                }
                JsonlMemory::new(self.sessions_dir())
                    .load(id)
                    .await
                    .map_err(|e| error(IpcErrorCode::Io, e.to_string()))?
            }
        };
        let limit = limit.clamp(1, PAGE_MAX);
        let lines = messages
            .iter()
            .enumerate()
            .skip(offset)
            .take(limit)
            .map(|(i, m)| format!("[{i}] {}: {}", role_name(m), clip(&message_summary(m), MESSAGE_CHARS)))
            .collect();
        Ok(HistoryPage {
            total: messages.len(),
            offset,
            lines,
        })
    }

    /// Sends `text` from the conversation `from` to `to`, without waiting:
    /// the message waits for `to` to be free, then starts a run there whose
    /// final answer is the reply.
    pub async fn mail_send(&self, from: &str, to: &str, text: &str) -> IpcResult<Mail> {
        check_session_id(to)?;
        if from == to {
            return Err(invalid("a conversation cannot write to itself"));
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(invalid("the message is empty"));
        }
        if text.chars().count() > MAIL_CHARS {
            return Err(invalid(format!("the message is longer than {MAIL_CHARS} characters")));
        }
        if self.mail.is_replying(from) {
            return Err(rejected(
                "this conversation is answering a message: its final answer is the reply, \
                 and it cannot write to another conversation during this turn",
            ));
        }
        if self.open_entry(from).is_none() && load_meta(self.sessions_dir(), from).await.is_none() {
            return Err(not_found(format!("no conversation {from}")));
        }
        let entry = self.entry_for_mail(to).await?;
        let from_title = match self.open_entry(from) {
            Some(e) => e.controller.snapshot().title,
            None => load_meta(self.sessions_dir(), from)
                .await
                .map(|m| m.title)
                .unwrap_or_default(),
        };
        let to_title = entry.controller.snapshot().title;
        let mail = self.mail.record(from, to, &to_title, text);
        let prompt = format!(
            "Message de la conversation « {} » ({from}) :\n\n{text}\n\nTa réponse finale lui sera transmise : elle la récupère avec l'outil Session.",
            if from_title.is_empty() { "sans titre" } else { from_title.as_str() },
        );
        tauri::async_runtime::spawn(deliver(entry, Arc::clone(&self.mail), mail.id.clone(), prompt));
        Ok(mail)
    }

    /// The open session `id`, opened in its project when it is not.
    async fn entry_for_mail(&self, id: &str) -> IpcResult<Arc<OpenSession>> {
        if let Some(entry) = self.open_entry(id) {
            return Ok(entry);
        }
        let meta = load_meta(self.sessions_dir(), id)
            .await
            .ok_or_else(|| not_found(format!("no conversation {id}")))?;
        let project_id = self
            .project_id_of_dir(&meta.working_dir)
            .ok_or_else(|| rejected("the conversation belongs to no project of Bricks"))?;
        self.open_session(OpenSessionRequest {
            project_id,
            session_id: Some(id.to_string()),
            model: None,
            reasoning: None,
        })
        .await?;
        self.open_entry(id)
            .ok_or_else(|| rejected("the conversation could not be opened"))
    }

    /// Copies the conversation `id` into a new one, child of it: its
    /// histories and files, with the paths into its files rewritten, and
    /// fresh usage counters. Nothing is left behind when it fails.
    pub async fn fork_session(&self, id: &str) -> IpcResult<SessionItem> {
        check_session_id(id)?;
        let meta = load_meta(self.sessions_dir(), id)
            .await
            .ok_or_else(|| not_found(format!("no conversation {id}")))?;
        let new_id = new_session_id();
        let made = self.copy_session(id, &new_id, &meta).await;
        if let Err(e) = made {
            let _ = JsonlMemory::new(self.sessions_dir()).delete(&new_id).await;
            let _ = std::fs::remove_dir_all(files_dir(self.sessions_dir(), &new_id));
            return Err(e);
        }
        let forked = load_meta(self.sessions_dir(), &new_id)
            .await
            .ok_or_else(|| error(IpcErrorCode::Io, "the fork was not saved"))?;
        let count = JsonlMemory::new(self.sessions_dir())
            .load(&new_id)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let item = self.item_from_meta(&forked, count, false, false);
        self.hub.publish(crate::contract::DesktopEvent::SessionUpdated {
            session: item.clone(),
        });
        Ok(item)
    }

    async fn copy_session(&self, id: &str, new_id: &str, meta: &SessionMeta) -> IpcResult<()> {
        let dir = self.sessions_dir().to_path_buf();
        let store = JsonlMemory::new(&dir);
        let io = |e: String| error(IpcErrorCode::Io, e);
        // The files first, so the copied history never points at nothing.
        let (from, to) = (files_dir(&dir, id), files_dir(&dir, new_id));
        tauri::async_runtime::spawn_blocking(move || copy_dir(&from, &to))
            .await
            .map_err(|e| io(e.to_string()))?
            .map_err(|e| io(e.to_string()))?;
        let old_files = format!("{id}.files");
        let new_files = format!("{new_id}.files");
        let moved = |messages: Vec<Message>| -> IpcResult<Vec<Message>> {
            let text = serde_json::to_string(&messages).map_err(|e| io(e.to_string()))?;
            serde_json::from_str(&text.replace(&old_files, &new_files)).map_err(|e| io(e.to_string()))
        };
        let histories = store.load_histories(id).await.map_err(|e| io(e.to_string()))?;
        let histories = SessionHistories {
            active: moved(histories.active)?,
            raw: moved(histories.raw)?,
        };
        store
            .store_histories(new_id, histories, SaveRevision { owner: rand_owner(), seq: 1 })
            .await
            .map_err(|e| io(e.to_string()))?;
        let mut n = 1;
        while dir.join(format!("{}.jsonl", session_keys::snapshot(id, n))).exists() {
            let snapshot = store
                .load(&session_keys::snapshot(id, n))
                .await
                .map_err(|e| io(e.to_string()))?;
            store
                .store(&session_keys::snapshot(new_id, n), &moved(snapshot)?)
                .await
                .map_err(|e| io(e.to_string()))?;
            n += 1;
        }
        let mut forked = meta.clone();
        let now = now_ms();
        forked.id = new_id.to_string();
        forked.title = format!(
            "{} (fork)",
            if meta.title.trim().is_empty() { "Conversation" } else { meta.title.trim() }
        );
        forked.parent = Some(id.to_string());
        forked.created_at = now;
        forked.updated_at = now;
        forked.save(&files_dir(&dir, new_id)).map_err(io)
    }

    /// Moves one conversation to the folder of another project. Refused while
    /// it works; an idle open one is closed first. Only the folder of a
    /// project of Bricks is accepted. On failure nothing changes.
    pub async fn move_session(&self, id: &str, folder: &str) -> IpcResult<SessionItem> {
        check_session_id(id)?;
        let wanted = std::fs::canonicalize(folder)
            .map_err(|_| rejected(format!("{folder} is not a readable folder")))?;
        let project = self
            .projects
            .list()
            .into_iter()
            .find(|p| !p.chats && !p.missing && Path::new(&p.path) == wanted)
            .ok_or_else(|| {
                rejected(format!(
                    "{folder} is not the folder of a project of Bricks: add it as a project first"
                ))
            })?;
        let _gate = self.lifecycle.lock().await;
        let mut meta = load_meta(self.sessions_dir(), id)
            .await
            .ok_or_else(|| not_found(format!("no conversation {id}")))?;
        if let Some(entry) = self.open_entry(id) {
            if entry.is_busy() {
                return Err(error(
                    IpcErrorCode::SessionBusy,
                    "the conversation is working: move it once its turn has ended",
                ));
            }
            self.retire(&entry).await;
        }
        meta.working_dir = PathBuf::from(&project.path);
        meta.touch();
        meta.save(&files_dir(self.sessions_dir(), id))
            .map_err(|e| error(IpcErrorCode::Io, e))?;
        let count = super::live::stored_message_count(self.sessions_dir(), id).await;
        let item = self.item_from_meta(&meta, count, false, false);
        self.hub.publish(crate::contract::DesktopEvent::SessionUpdated {
            session: item.clone(),
        });
        Ok(item)
    }
}

/// Waits for `entry` to be free, submits the message, then follows the run
/// it starts until its end, or the session's.
async fn deliver(
    entry: Arc<OpenSession>,
    mail: Arc<crate::services::mail::Mailbox>,
    id: String,
    prompt: String,
) {
    let mut closed = entry.closed();
    let mut notices = entry.notices();
    let gone = "the conversation was closed or deleted before it answered".to_string();
    let deadline = tokio::time::Instant::now() + DELIVERY_WAIT;
    loop {
        if *closed.borrow() {
            return mail.failed(&id, gone);
        }
        if !entry.is_busy() {
            let command = Command::Submit {
                prompt: Prompt::text(prompt.clone()),
            };
            match entry.send_tagged(command, None, Vec::new(), Some(id.clone())).await {
                Ok(()) => break,
                // Someone else started a run meanwhile: wait again.
                Err(_) if entry.is_busy() => {}
                Err(reason) => return mail.failed(&id, format!("refused: {reason}")),
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return mail.failed(&id, "the conversation stayed busy for 30 minutes".into());
        }
        tokio::select! {
            _ = closed.changed() => {}
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    }
    let answer_deadline = tokio::time::Instant::now() + ANSWER_WAIT;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(answer_deadline) => {
                return mail.failed(&id, "no answer within 2 hours".into());
            }
            changed = closed.changed() => {
                if changed.is_err() || *closed.borrow() {
                    return mail.failed(&id, gone);
                }
            }
            notice = notices.recv() => match notice {
                Ok(RunNotice::Started { tag }) if tag == id => mail.started(&id),
                Ok(RunNotice::Finished { tag, text, outcome }) if tag == id => {
                    // A run cancelled or failed gave no answer: the message failed.
                    return match outcome.as_str() {
                        "cancelled" => mail.failed(&id, "the conversation's run was cancelled before it answered".into()),
                        "failed" => mail.failed(&id, "the conversation's run failed before it answered".into()),
                        _ => mail.answered(&id, text, outcome),
                    };
                }
                Ok(_) => {}
                // Notices were missed: when the conversation is idle again,
                // the end of the message's run was among them.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if !entry.is_busy() {
                        return mail.failed(&id, "the answer was lost; read the conversation for it".into());
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return mail.failed(&id, gone);
                }
            },
        }
    }
}

fn new_session_id() -> String {
    let now = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let rand = uuid::Uuid::new_v4().simple().to_string();
    format!("{now}-{}", &rand[..6])
}

fn rand_owner() -> u64 {
    let bytes = uuid::Uuid::new_v4();
    u64::from_le_bytes(bytes.as_bytes()[..8].try_into().unwrap_or([1; 8]))
}

/// Copies the folder `from` into `to` (created), without `session.json`. An
/// absent `from` copies nothing.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            copy_dir(&src, &dst)?;
        } else if entry.file_name() != "session.json" {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

fn role_name(m: &Message) -> &'static str {
    match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

/// A message as one line of text: its text, and what its other blocks are.
fn message_summary(m: &Message) -> String {
    let MessageContent::Blocks(blocks) = &m.content else {
        return message_text(m);
    };
    let mut parts = Vec::new();
    for b in blocks {
        match b {
            ContentBlock::Text { text } => parts.push(text.clone()),
            ContentBlock::ToolUse { name, input, .. } => {
                let target = input
                    .get("file_path")
                    .or_else(|| input.get("command"))
                    .or_else(|| input.get("pattern"))
                    .and_then(Value::as_str)
                    .map(|s| format!(" {}", clip(s, 120)))
                    .unwrap_or_default();
                parts.push(format!("[tool {name}{target}]"));
            }
            ContentBlock::ToolResult { content, is_error, .. } => {
                let text = match content {
                    ToolResultContent::Text(t) => t.clone(),
                    ToolResultContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                let mark = if is_error.unwrap_or(false) { "error" } else { "result" };
                parts.push(format!("[tool {mark}: {}]", clip(&text, 400)));
            }
            ContentBlock::Image { .. } => parts.push("[image]".into()),
            _ => {}
        }
    }
    parts.join("\n")
}

/// Images in the history: they count in the context but carry no text.
fn image_count(messages: &[Message]) -> u64 {
    let mut n = 0;
    for m in messages {
        if let MessageContent::Blocks(blocks) = &m.content {
            for b in blocks {
                match b {
                    ContentBlock::Image { .. } => n += 1,
                    ContentBlock::ToolResult {
                        content: ToolResultContent::Blocks(inner),
                        ..
                    } => n += inner.iter().filter(|b| matches!(b, ContentBlock::Image { .. })).count() as u64,
                    _ => {}
                }
            }
        }
    }
    n
}

// ─── The tool ────────────────────────────────────────────────────────────────

/// The `Session` tool: one tool, several actions.
pub struct SessionTool {
    app: tauri::AppHandle,
}

/// Lends the `Session` tool to every session.
pub struct SessionLender {
    pub app: tauri::AppHandle,
}

impl bricks_agent::ToolLender for SessionLender {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(SessionTool { app: self.app.clone() })]
    }
}

/// Several lenders as one.
pub struct Lenders(pub Vec<Arc<dyn bricks_agent::ToolLender>>);

impl bricks_agent::ToolLender for Lenders {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.0.iter().flat_map(|l| l.tools()).collect()
    }
    fn deferred_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.0.iter().flat_map(|l| l.deferred_tools()).collect()
    }
}

const DESCRIPTION: &str = "Work with the other conversations of Bricks, only when the person asks for it. \
Actions: `list` (recent conversations), `search` (`query` in titles and messages), `read` (`id`, \
`offset`, `limit`: a page of its messages), `status` (`id`: open, working or idle), `context` (this \
conversation's context budget), `send` (`id`, `message`: writes to it without waiting; returns a \
message id), `wait` (`message_id`, `timeout_seconds` ≤ 600: the answer once given), `messages` (the \
messages this conversation sent and their state: sent, in progress, answered or failed; whether one \
was read is never known), `fork` (`id`: copies it into a new conversation), `move` (`id`, `folder`: \
moves it to the folder of another project; refused while it works). The final answer of the run a \
message starts is its reply: get it with `wait` or `messages`. A conversation answering a message \
cannot send one during that turn. Use Agent for sub-agents, not this tool.";

/// The parameters `Session` accepts, as its schema declares them.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // Checked whole, then read from the call's input.
struct SessionInput {
    action: String,
    id: Option<String>,
    query: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
    message: Option<String>,
    message_id: Option<String>,
    timeout_seconds: Option<u64>,
    folder: Option<String>,
}

#[async_trait::async_trait]
impl Tool for SessionTool {
    fn name(&self) -> &str {
        "Session"
    }
    fn description(&self) -> &str {
        DESCRIPTION
    }
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn permission_level_for(&self, input: &Value) -> PermissionLevel {
        match input.get("action").and_then(Value::as_str) {
            // A move changes where the conversation's later commands run.
            Some("send") | Some("move") => PermissionLevel::Execute,
            Some("fork") => PermissionLevel::Write,
            _ => PermissionLevel::ReadOnly,
        }
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "search", "read", "status", "context", "send", "wait", "messages", "fork", "move"]
                },
                "id": { "type": "string", "description": "A conversation id" },
                "query": { "type": "string" },
                "offset": { "type": "integer", "description": "First message index (read)" },
                "limit": { "type": "integer", "description": "Messages (read) or conversations (list)" },
                "message": { "type": "string" },
                "message_id": { "type": "string" },
                "timeout_seconds": { "type": "integer" },
                "folder": { "type": "string", "description": "Folder of a project of Bricks (move)" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        use tauri::Manager;
        // The parameters are checked whole (names and types) before any is
        // read: a misnamed or mistyped one is refused, never skipped.
        let checked = bricks_tools::tool_feedback::parse_input::<SessionInput, _>(self, &input);
        if let Err(refused) = checked {
            return refused;
        }
        let Some(state) = self.app.try_state::<AppState>() else {
            return ToolResult::error("the conversations are not reachable here");
        };
        let me = ctx.session_id.as_str();
        let s = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
        let n = |key: &str| input.get(key).and_then(Value::as_u64).map(|v| v as usize);
        let need = |key: &str| s(key).filter(|v| !v.trim().is_empty());
        let result = match input.get("action").and_then(Value::as_str).unwrap_or("") {
            "list" => Ok(list_text(&state.sessions_all().await, me, n("limit").unwrap_or(30))),
            "search" => match need("query") {
                None => Err("`query` is required".to_string()),
                Some(q) => state
                    .sessions_search(None, &q)
                    .await
                    .map(|hits| {
                        let items: Vec<SessionItem> = hits.into_iter().map(|h| h.session).collect();
                        list_text(&items, me, n("limit").unwrap_or(30))
                    })
                    .map_err(|e| e.message),
            },
            "read" => match need("id") {
                None => Err("`id` is required".into()),
                Some(id) => state
                    .session_page(&id, n("offset").unwrap_or(0), n("limit").unwrap_or(PAGE_DEFAULT))
                    .await
                    .map(|p| {
                        let next = p.offset + p.lines.len();
                        let mut text = format!(
                            "Conversation {id}: messages {}–{} of {}.\n",
                            p.offset,
                            next.saturating_sub(1).max(p.offset),
                            p.total
                        );
                        text.push_str(&p.lines.join("\n"));
                        if next < p.total {
                            text.push_str(&format!("\n\nMore: read with offset {next}."));
                        }
                        text
                    })
                    .map_err(|e| e.message),
            },
            "status" => status_text(&state, &s("id").unwrap_or_else(|| me.to_string())).await,
            "context" => context_text(&state, me).await,
            "send" => match (need("id"), need("message")) {
                (Some(id), Some(message)) => state
                    .mail_send(me, &id, &message)
                    .await
                    .map(|m| {
                        format!(
                            "Message {} sent to « {} » ({}). It waits for the conversation to be free; \
                             use `wait` with this message_id for the answer.",
                            m.id, m.to_title, m.to
                        )
                    })
                    .map_err(|e| e.message),
                _ => Err("`id` and `message` are required".into()),
            },
            "wait" => match need("message_id") {
                None => Err("`message_id` is required".into()),
                Some(mid) => {
                    let secs = input
                        .get("timeout_seconds")
                        .and_then(Value::as_u64)
                        .unwrap_or(WAIT_DEFAULT_SECS)
                        .min(WAIT_MAX_SECS);
                    match state.mail.get(&mid) {
                        Some(m) if m.from == me => match state.mail.wait(&mid, Duration::from_secs(secs)).await {
                            Some(m) => Ok(mail_text(&m, true)),
                            None => Err(format!("no message {mid}")),
                        },
                        _ => Err(format!("no message {mid} sent by this conversation")),
                    }
                }
            },
            "messages" => {
                let sent = state.mail.sent_by(me);
                Ok(if sent.is_empty() {
                    "This conversation sent no message.".into()
                } else {
                    sent.iter().map(|m| mail_text(m, false)).collect::<Vec<_>>().join("\n\n")
                })
            }
            "fork" => match need("id") {
                None => Err("`id` is required".into()),
                Some(id) => state
                    .fork_session(&id)
                    .await
                    .map(|item| format!("Forked into « {} » ({}).", item.title, item.id))
                    .map_err(|e| e.message),
            },
            "move" => match (need("id"), need("folder")) {
                (Some(id), Some(folder)) if id == me => {
                    let _ = folder;
                    Err("a conversation cannot move itself while it works".into())
                }
                (Some(id), Some(folder)) => state
                    .move_session(&id, &folder)
                    .await
                    .map(|item| {
                        format!(
                            "« {} » now works in {}.",
                            item.title,
                            item.working_dir.unwrap_or_default()
                        )
                    })
                    .map_err(|e| e.message),
                _ => Err("`id` and `folder` are required".into()),
            },
            other => Err(format!(
                "unknown action `{other}`: list, search, read, status, context, send, wait, messages, fork or move"
            )),
        };
        match result {
            Ok(text) => ToolResult::success(text),
            Err(e) => ToolResult::error(e),
        }
    }
}

fn list_text(items: &[SessionItem], me: &str, limit: usize) -> String {
    if items.is_empty() {
        return "No conversation.".into();
    }
    let mut lines: Vec<String> = items
        .iter()
        .take(limit.max(1))
        .map(|i| {
            let state = if i.running {
                "working"
            } else if i.open {
                "open"
            } else {
                "closed"
            };
            let mine = if i.id == me { " (this one)" } else { "" };
            let parent = i
                .parent_id
                .as_deref()
                .map(|p| format!(", fork of {p}"))
                .unwrap_or_default();
            format!(
                "- {} « {} »{mine}: {state}, {} messages, {}{parent}",
                i.id,
                if i.title.is_empty() { "untitled" } else { i.title.as_str() },
                i.message_count,
                i.working_dir.as_deref().unwrap_or("?"),
            )
        })
        .collect();
    if items.len() > limit {
        lines.push(format!("… {} more.", items.len() - limit));
    }
    lines.join("\n")
}

async fn status_text(state: &AppState, id: &str) -> Result<String, String> {
    check_session_id(id).map_err(|e| e.message)?;
    if let Some(entry) = state.open_entry(id) {
        let snap = entry.controller.snapshot();
        let what = if entry.is_running() {
            "working"
        } else if entry.is_busy() {
            "busy (compacting or resuming)"
        } else {
            "open and idle"
        };
        let mut text = format!("« {} » ({id}) is {what}.", snap.title);
        if !snap.pending_approvals.is_empty() || !snap.pending_questions.is_empty() {
            text.push_str(&format!(
                " It waits for the person: {} approval(s), {} question(s).",
                snap.pending_approvals.len(),
                snap.pending_questions.len()
            ));
        }
        return Ok(text);
    }
    match load_meta(state.sessions_dir(), id).await {
        Some(m) => Ok(format!("« {} » ({id}) is closed (not working).", m.title)),
        None => Err(format!("no conversation {id}")),
    }
}

async fn context_text(state: &AppState, me: &str) -> Result<String, String> {
    let entry = state
        .open_entry(me)
        .ok_or_else(|| "this conversation is not open".to_string())?;
    let snap = entry.controller.snapshot();
    let c = &snap.context;
    let used = c.context_used.tokens;
    let limit = c.input_limit;
    let pct = if limit == 0 { 0.0 } else { used as f64 * 100.0 / limit as f64 };
    let usable = limit.saturating_sub(c.margin);
    let policy = bricks_agent::context::ContextPolicy::default();
    let compact_at = (limit as f64 * policy.compact_threshold) as u64;
    let mut text = format!(
        "Context: {used} tokens used of {limit} ({pct:.1} %). Remaining: {} tokens. Safety margin: \
         {} tokens, so {} tokens can still be added before a request is refused. Automatic \
         compaction at about {compact_at} tokens ({:.0} %). Output reserved apart: {} tokens.",
        limit.saturating_sub(used),
        c.margin,
        usable.saturating_sub(used),
        policy.compact_threshold * 100.0,
        c.reserved_output,
    );
    if c.context_used.upper_bound > used {
        text.push_str(&format!(" The count may reach {} tokens.", c.context_used.upper_bound));
    }
    // The split is estimated: text shares of the last request, applied to
    // the tokens used, and a fixed cost per image.
    if let Some(b) = &c.breakdown {
        let images = image_count(&entry.controller.history().await);
        let image_tokens = (images * policy.image_tokens).min(used);
        let text_tokens = used.saturating_sub(image_tokens);
        let ch = b.chars;
        let total = (ch.instructions + ch.conversation + ch.tool_results + ch.tool_definitions + ch.other).max(1);
        let part = |x: u64| text_tokens * x / total;
        text.push_str(&format!(
            "\nEstimated split (not measured): instructions ≈ {}, tool definitions ≈ {}, messages ≈ {}, \
             tool results ≈ {}, other ≈ {}, images ≈ {image_tokens} ({images} image(s)).",
            part(ch.instructions),
            part(ch.tool_definitions),
            part(ch.conversation),
            part(ch.tool_results),
            part(ch.other),
        ));
    }
    Ok(text)
}

fn mail_text(m: &Mail, with_answer: bool) -> String {
    let state = match m.state {
        MailState::Sent => "sent, waiting for the conversation to be free",
        MailState::InProgress => "in progress: the conversation is answering",
        MailState::Answered => "answered",
        MailState::Failed => "failed",
    };
    let mut text = format!("{} to « {} » ({}): {state}.", m.id, m.to_title, m.to);
    if let Some(outcome) = m.outcome.as_deref().filter(|o| *o != "succeeded") {
        text.push_str(&format!(" Its run ended as {outcome}: the answer may be partial or empty."));
    }
    if let Some(reason) = &m.reason {
        text.push_str(&format!(" {reason}."));
    }
    if let Some(answer) = &m.answer {
        if with_answer {
            text.push_str(&format!("\nAnswer:\n{}", clip(answer, MESSAGE_CHARS)));
        } else {
            text.push_str(&format!(" Answer: {}", clip(answer, 200)));
        }
    }
    text
}
