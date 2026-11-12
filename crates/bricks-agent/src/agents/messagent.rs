//! Messagent: the internal mail of the team on a mission. Every agent of the
//! session writes to one or more agents by id (`main`, `sub_0000`…) or to
//! the whole team, reads the team's common thread, and lists its members.
//!
//! A message is delivered to its recipients' queues and read between two
//! turns, after the tool calls of the turn in progress (it never interrupts
//! a command). An agent waiting (`AgentControl wait`) wakes up; an ended
//! agent is not restarted by a message. The person never writes here: their
//! instructions go to `main`.
//!
//! Bounds: 16 KiB per message, 32 waiting per recipient (refused beyond,
//! never cut or dropped), and chains of [`MAX_CHAIN`] replies at most.

use super::runtime::{AgentRuntime, RuntimeHandle};
use super::tool::viewer;
use crate::events::{InjectRefusal, MAX_INJECTED_BYTES, MAX_PENDING_MESSAGES};
use async_trait::async_trait;
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// The session's own agent, the orchestrator.
pub const MAIN: &str = "main";
/// Consecutive replies of one exchange, at most.
pub const MAX_CHAIN: u32 = 8;
/// One message's text, at most: its header still fits one delivery.
pub const MAX_TEXT_BYTES: usize = MAX_INJECTED_BYTES - 512;

/// One message of the team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub from: String,
    /// The recipients; empty for the whole team.
    pub to: Vec<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Replies before this one in its exchange (0: it starts one).
    pub chain: u32,
}

impl Post {
    fn header(&self) -> String {
        let to = if self.to.is_empty() {
            "team".to_string()
        } else {
            self.to.join(", ")
        };
        let reply = self
            .reply_to
            .as_deref()
            .map(|r| format!(", reply to {r}"))
            .unwrap_or_default();
        format!("[Messagent {} from {} to {to}{reply}]", self.id, self.from)
    }

    /// The text a recipient reads.
    pub fn delivered(&self) -> String {
        format!("{}\n{}", self.header(), self.text)
    }
}

/// Where a message goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Team,
    Agents(Vec<String>),
}

/// Who is in the team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub state: String,
    /// Its own worktree, readable by the others (Read, Grep, CodeScout
    /// `workspace`, CodeStats); `None` in the shared workspace.
    pub worktree: Option<String>,
}

/// The mail's own state: the team thread, read positions, the
/// orchestrator's queue between its runs.
#[derive(Default)]
pub(crate) struct Mail {
    /// The orchestrator's queue while one of its runs is going.
    pub(crate) main_controls: Option<Arc<crate::events::StreamControls>>,
    /// What reached the orchestrator while none of its runs was going.
    pub(crate) backlog: Vec<String>,
    thread: Vec<Post>,
    cursors: HashMap<String, usize>,
    /// Team posts already delivered to each member: `read` does not give them again.
    delivered: HashMap<String, std::collections::HashSet<String>>,
    /// Every post by id, for the chains of replies.
    posts: HashMap<String, Post>,
    next: u32,
}

impl Mail {
    fn next_id(&mut self) -> String {
        self.next += 1;
        format!("m{:04}", self.next)
    }
}

/// Why a delivery did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryError {
    Unknown,
    /// Ended or not started: nothing reads it, and a message restarts nothing.
    NotRunning(String),
    Refused(InjectRefusal),
}

impl DeliveryError {
    pub fn describe(&self, id: &str) -> String {
        match self {
            Self::Unknown => format!("no agent `{id}` in this team"),
            Self::NotRunning(state) => format!(
                "{id} is {state}: a message does not restart it (resume it with AgentControl)"
            ),
            Self::Refused(InjectRefusal::Empty) => "the message is empty".into(),
            Self::Refused(InjectRefusal::TooLarge) => {
                format!("the message is over {MAX_INJECTED_BYTES} bytes")
            }
            Self::Refused(InjectRefusal::QueueFull) => {
                format!("{id} already has {MAX_PENDING_MESSAGES} messages waiting")
            }
            Self::Refused(InjectRefusal::Finished) => format!("{id} has just ended"),
        }
    }
}

impl AgentRuntime {
    /// Queue `text` for `to`, read between its next two turns. The
    /// orchestrator keeps it for its next run when none is going.
    pub(crate) fn deliver(&self, to: &str, text: String) -> Result<(), DeliveryError> {
        let done = if to == MAIN {
            let mut mail = self.mail.lock();
            let queued = mail
                .main_controls
                .as_ref()
                .map(|c| c.try_inject(text.clone()));
            match queued {
                Some(Ok(())) => Ok(()),
                Some(Err(InjectRefusal::Finished)) | None => {
                    if text.trim().is_empty() {
                        Err(DeliveryError::Refused(InjectRefusal::Empty))
                    } else if text.len() > MAX_INJECTED_BYTES {
                        Err(DeliveryError::Refused(InjectRefusal::TooLarge))
                    } else if mail.backlog.len() >= MAX_PENDING_MESSAGES {
                        Err(DeliveryError::Refused(InjectRefusal::QueueFull))
                    } else {
                        mail.backlog.push(text);
                        Ok(())
                    }
                }
                Some(Err(e)) => Err(DeliveryError::Refused(e)),
            }
        } else {
            let Some(record) = self.record(to) else {
                return Err(DeliveryError::Unknown);
            };
            match self.controls_of(to) {
                None => Err(DeliveryError::NotRunning(record.state.as_str().into())),
                Some(c) => c.try_inject(text).map_err(|e| match e {
                    InjectRefusal::Finished => {
                        DeliveryError::NotRunning(record.state.as_str().into())
                    }
                    e => DeliveryError::Refused(e),
                }),
            }
        };
        if done.is_ok() {
            self.bump();
        }
        done
    }

    /// The orchestrator's queue for the run that starts (`Some`) or ends
    /// (`None`); what waited for it is moved into the new run.
    pub(crate) fn set_main_controls(&self, controls: Option<Arc<crate::events::StreamControls>>) {
        let mut mail = self.mail.lock();
        if let Some(c) = &controls {
            let waiting = std::mem::take(&mut mail.backlog);
            for (i, text) in waiting.iter().enumerate() {
                if c.try_inject(text.clone()).is_err() {
                    mail.backlog.extend(waiting[i..].iter().cloned());
                    break;
                }
            }
        }
        mail.main_controls = controls;
    }

    /// Deliveries waiting for `id` (not read yet).
    pub(crate) fn waiting_for(&self, id: &str) -> usize {
        if id == MAIN {
            let mail = self.mail.lock();
            mail.backlog.len()
                + mail
                    .main_controls
                    .as_ref()
                    .map_or(0, |c| c.pending_messages())
        } else {
            self.controls_of(id).map_or(0, |c| c.pending_messages())
        }
    }

    /// Post a message from `from`. Each recipient's queue gets it; a team
    /// post also goes to the common thread. Refused when it is empty, too
    /// large, continues a chain past [`MAX_CHAIN`], or reaches nobody.
    pub fn post(
        &self,
        from: &str,
        target: Target,
        text: String,
        reply_to: Option<String>,
    ) -> Result<(Post, Vec<String>), String> {
        if text.trim().is_empty() {
            return Err("the message is empty; nothing was sent.".into());
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(format!(
                "the message is over {MAX_TEXT_BYTES} bytes ({}); nothing was sent.",
                text.len()
            ));
        }
        let chain = match &reply_to {
            None => 0,
            Some(r) => match self.mail.lock().posts.get(r) {
                Some(p) => p.chain + 1,
                None => return Err(format!("no message `{r}` to reply to; nothing was sent.")),
            },
        };
        if chain > MAX_CHAIN {
            let origin = reply_to.clone().unwrap_or_default();
            if from != MAIN {
                let _ = self.deliver(
                    MAIN,
                    format!(
                        "[Messagent] {from} tried to continue the exchange of {origin} past \
                         {MAX_CHAIN} replies; the message was refused. The agents go on with \
                         their other work; step in if the exchange is stuck."
                    ),
                );
            }
            return Err(format!(
                "this exchange already has {MAX_CHAIN} replies in a row: the message was refused \
                 and the orchestrator was told. Go on with your task."
            ));
        }
        let recipients: Vec<String> = match &target {
            Target::Team => {
                let mut ids = vec![MAIN.to_string()];
                ids.extend(
                    self.records()
                        .into_iter()
                        .filter(|r| !r.state.is_terminal())
                        .map(|r| r.info.agent_id),
                );
                ids.retain(|id| id != from);
                ids
            }
            Target::Agents(ids) => {
                let mut out = Vec::new();
                for id in ids {
                    if id == from {
                        return Err("a message to yourself is not sent.".into());
                    }
                    if id != MAIN && self.record(id).is_none() {
                        return Err(format!("no agent `{id}` in this team; nothing was sent."));
                    }
                    if !out.contains(id) {
                        out.push(id.clone());
                    }
                }
                out
            }
        };
        if recipients.is_empty() {
            return Err("nobody else is in the team now; nothing was sent.".into());
        }
        let post = {
            let mut mail = self.mail.lock();
            let post = Post {
                id: mail.next_id(),
                from: from.to_string(),
                to: match &target {
                    Target::Team => Vec::new(),
                    Target::Agents(_) => recipients.clone(),
                },
                text,
                reply_to,
                chain,
            };
            mail.posts.insert(post.id.clone(), post.clone());
            if target == Target::Team {
                mail.thread.push(post.clone());
            }
            post
        };
        let mut missed = Vec::new();
        for to in &recipients {
            match self.deliver(to, post.delivered()) {
                // Delivered: `read` does not give it again.
                Ok(()) if target == Target::Team => {
                    self.mail
                        .lock()
                        .delivered
                        .entry(to.clone())
                        .or_default()
                        .insert(post.id.clone());
                }
                Ok(()) => {}
                Err(e) => missed.push(e.describe(to)),
            }
        }
        if missed.len() == recipients.len() && target != Target::Team {
            return Err(format!("nothing was delivered: {}.", missed.join("; ")));
        }
        Ok((post, missed))
    }

    /// The team posts `reader` has not read yet (all of them with `all`).
    pub fn read_thread(&self, reader: &str, all: bool) -> Vec<Post> {
        let mut mail = self.mail.lock();
        let start = if all {
            0
        } else {
            mail.cursors.get(reader).copied().unwrap_or(0)
        };
        let seen = mail.delivered.get(reader).cloned().unwrap_or_default();
        let posts: Vec<Post> = mail.thread[start.min(mail.thread.len())..]
            .iter()
            .filter(|p| all || !seen.contains(&p.id))
            .cloned()
            .collect();
        let end = mail.thread.len();
        mail.cursors.insert(reader.to_string(), end);
        posts
    }

    /// The orchestrator, then every sub-agent of the session.
    pub fn members(&self) -> Vec<Member> {
        let mut out = vec![Member {
            id: MAIN.into(),
            name: "Orchestrateur".into(),
            state: "running".into(),
            worktree: None,
        }];
        out.extend(self.records().into_iter().map(|r| Member {
            name: member_name(&r.info),
            worktree: (r.info.isolation == "worktree").then(|| r.info.workspace.clone()),
            id: r.info.agent_id,
            state: r.state.as_str().into(),
        }));
        out
    }
}

/// A sub-agent's display name: its label, else its profile.
pub fn member_name(info: &super::spawn::SpawnInfo) -> String {
    info.description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent {}", info.profile))
}

/// `Messagent`: write to agents of the team, read its thread, list it.
pub struct MessagentTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    action: String,
    #[serde(default)]
    to: Option<Value>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reply_to: Option<String>,
    #[serde(default)]
    all: bool,
}

fn runtime(ctx: &ToolContext) -> Option<Arc<AgentRuntime>> {
    ctx.extensions
        .get::<RuntimeHandle>()
        .map(|h| Arc::clone(&h.0))
}

fn target(to: Option<Value>) -> Result<Target, String> {
    match to {
        Some(Value::String(s)) if s == "team" => Ok(Target::Team),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Target::Agents(vec![s])),
        Some(Value::Array(items)) if !items.is_empty() => items
            .into_iter()
            .map(|v| match v {
                Value::String(s) if !s.trim().is_empty() && s != "team" => Ok(s),
                _ => Err("`to` lists agent ids (main, sub_0000…), or is \"team\"".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Target::Agents),
        _ => Err("`send` needs `to`: an id, a list of ids, or \"team\"".into()),
    }
}

#[async_trait]
impl Tool for MessagentTool {
    fn name(&self) -> &str {
        "Messagent"
    }

    fn description(&self) -> &str {
        "The team's mail. `send` (to: an id, a list of ids, or \"team\"; content; reply_to: the \
         message answered): read by its recipients between two turns. `read`: the team thread's \
         new messages (all: the whole thread). `members`: the ids, names and worktrees of the \
         team (another's worktree is read-only for you). Post what moves the work forward; no \
         acknowledgement without content."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["send", "read", "members"] },
                "to": {
                    "description": "send: `main`, a `sub_0000` id, a list of ids, or \"team\" (everyone else present).",
                    "anyOf": [
                        { "type": "string" },
                        { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                    ]
                },
                "content": { "type": "string", "description": "send: the message (16 KiB at most)." },
                "reply_to": { "type": "string", "description": "send: the id of the message answered (m0001…)." },
                "all": { "type": "boolean", "description": "read: the whole thread instead of the new messages." }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let input: Input = match bricks_tools::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let Some(rt) = runtime(ctx) else {
            return ToolResult::error("no team in this session: nothing was sent or read.");
        };
        let me = viewer(ctx).unwrap_or_else(|| MAIN.to_string());
        match input.action.as_str() {
            "send" => {
                let target = match target(input.to) {
                    Ok(t) => t,
                    Err(e) => return ToolResult::error(e),
                };
                let content = input.content.unwrap_or_default();
                match rt.post(&me, target, content, input.reply_to) {
                    Ok((post, missed)) => {
                        let mut text = format!(
                            "{} sent to {}.",
                            post.id,
                            if post.to.is_empty() {
                                "the team".to_string()
                            } else {
                                post.to.join(", ")
                            }
                        );
                        if !missed.is_empty() {
                            text.push_str(&format!(" Not delivered: {}.", missed.join("; ")));
                        }
                        ToolResult::success(text)
                    }
                    Err(e) => ToolResult::error(e),
                }
            }
            "read" => {
                let posts = rt.read_thread(&me, input.all);
                if posts.is_empty() {
                    return ToolResult::success("No new message in the team thread.");
                }
                ToolResult::success(
                    posts
                        .iter()
                        .map(Post::delivered)
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                )
            }
            "members" => ToolResult::success(
                rt.members()
                    .iter()
                    .map(|m| {
                        format!(
                            "- {} · {} · {}{}{}",
                            m.id,
                            m.name,
                            m.state,
                            m.worktree
                                .as_deref()
                                .map(|w| format!(" · worktree {w}"))
                                .unwrap_or_default(),
                            if m.id == me { " (you)" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            other => ToolResult::error(format!("unknown action `{other}`: send, read, members")),
        }
    }
}
