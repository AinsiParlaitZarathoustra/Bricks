//! Ask: the model asks the person a question with choices, and waits.
//!
//! The question is published as a [`QuestionRequest`]; a frontend answers it
//! with a choice, a free text (« Autre ») or a skip, through
//! [`AskBroker::respond`]. The first resolution accepted closes it for good.
//!
//! The timer is a person's setting, captured when a message is received
//! ([`AskTimer`], carried by the prompt) and kept for the whole run: `off`
//! (no deadline), `smart` (the model sets `timeout_seconds` for each
//! question) or `custom` (a fixed duration the model cannot change). The
//! engine owns the deadline: at expiry the question is skipped with reason
//! `timeout`, whatever any frontend shows.

use crate::events::AgentEvent;
use async_trait::async_trait;
use bricks_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use bricks_types::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Shortest and longest question timer, in seconds.
pub const MIN_TIMER_SECONDS: u32 = 30;
pub const MAX_TIMER_SECONDS: u32 = 3_600;
/// Choices a question offers (besides « Autre »).
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 6;

/// The question timer a person chose, captured with each message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AskTimer {
    /// No deadline.
    #[default]
    Off,
    /// The model chooses each question's duration.
    Smart,
    /// A fixed duration, imposed on every question.
    Custom { seconds: u32 },
}

impl AskTimer {
    /// Refuses a custom duration out of range.
    pub fn check(self) -> Result<Self, String> {
        match self {
            AskTimer::Custom { seconds }
                if !(MIN_TIMER_SECONDS..=MAX_TIMER_SECONDS).contains(&seconds) =>
            {
                Err(format!(
                    "the question timer must be between {MIN_TIMER_SECONDS} and {MAX_TIMER_SECONDS} seconds (got {seconds})"
                ))
            }
            other => Ok(other),
        }
    }
}

/// One choice of a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A question waiting for the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub question_id: String,
    pub tool_call_id: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    /// Index of the option the model recommends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<usize>,
    /// Duration of the whole question, when a timer applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
    /// When the engine skips it (milliseconds since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<i64>,
}

/// Why a question was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The person chose « Passer ».
    User,
    /// The timer ran out.
    Timeout,
    /// The run was cancelled.
    Cancelled,
    /// Nobody can answer (non-interactive).
    NonInteractive,
}

/// How a question ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum QuestionOutcome {
    /// One of the options.
    Chosen {
        choice: usize,
    },
    /// A free answer (« Autre »).
    Other {
        text: String,
    },
    Skipped {
        reason: SkipReason,
    },
}

/// A person's answer to a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionAnswer {
    Choice { choice: usize },
    Other { text: String },
    Skip,
}

type Waiter = oneshot::Sender<QuestionOutcome>;

/// Where questions go and answers come back. One per session.
#[derive(Default)]
pub struct AskBroker {
    pending: parking_lot::Mutex<HashMap<String, (QuestionRequest, Waiter)>>,
    sink: parking_lot::Mutex<Option<mpsc::Sender<AgentEvent>>>,
    run: parking_lot::Mutex<Option<CancellationToken>>,
    interactive: std::sync::atomic::AtomicBool,
    /// The timer captured with the message the current run answers.
    timer: parking_lot::Mutex<AskTimer>,
}

impl AskBroker {
    pub fn new(interactive: bool) -> Arc<Self> {
        let b = Self::default();
        b.interactive
            .store(interactive, std::sync::atomic::Ordering::SeqCst);
        Arc::new(b)
    }

    /// The timer of the next run (set when its message is received).
    pub fn set_timer(&self, timer: AskTimer) {
        *self.timer.lock() = timer;
    }

    pub fn timer(&self) -> AskTimer {
        *self.timer.lock()
    }

    pub(crate) fn begin_run(&self, sink: mpsc::Sender<AgentEvent>, run: CancellationToken) {
        *self.sink.lock() = Some(sink);
        *self.run.lock() = Some(run);
    }

    /// The run ended: returns the questions still open, closed now (their
    /// tool call was dropped with the run), for the caller to announce.
    pub(crate) fn end_run(&self) -> Vec<QuestionRequest> {
        *self.sink.lock() = None;
        *self.run.lock() = None;
        self.pending.lock().drain().map(|(_, (q, _))| q).collect()
    }

    /// Questions waiting for an answer.
    pub fn pending(&self) -> Vec<QuestionRequest> {
        let mut v: Vec<QuestionRequest> = self
            .pending
            .lock()
            .values()
            .map(|(q, _)| q.clone())
            .collect();
        v.sort_by(|a, b| a.question_id.cmp(&b.question_id));
        v
    }

    /// Answers a pending question. The first resolution wins: an unknown or
    /// already closed question is an error, and nothing is answered twice.
    pub fn respond(&self, question_id: &str, answer: QuestionAnswer) -> Result<(), String> {
        let mut pending = self.pending.lock();
        let Some((request, _)) = pending.get(question_id) else {
            return Err(format!("no pending question `{question_id}`"));
        };
        let outcome = match answer {
            QuestionAnswer::Choice { choice } if choice < request.options.len() => {
                QuestionOutcome::Chosen { choice }
            }
            QuestionAnswer::Choice { choice } => {
                return Err(format!("question `{question_id}` has no choice {choice}"));
            }
            QuestionAnswer::Other { text } if !text.trim().is_empty() => QuestionOutcome::Other {
                text: text.trim().to_string(),
            },
            QuestionAnswer::Other { .. } => return Err("the free answer is empty".into()),
            QuestionAnswer::Skip => QuestionOutcome::Skipped {
                reason: SkipReason::User,
            },
        };
        let (_, waiter) = pending.remove(question_id).expect("checked above");
        waiter
            .send(outcome)
            .map_err(|_| format!("question `{question_id}` is no longer waited for"))
    }

    async fn emit(&self, event: AgentEvent) {
        let sink = self.sink.lock().clone();
        if let Some(tx) = sink {
            let _ = tx.send(event).await;
        }
    }

    /// Publishes a question and waits for its end: an answer, a skip, the
    /// deadline, or the run's cancellation.
    async fn ask(&self, request: QuestionRequest) -> QuestionOutcome {
        let run = self.run.lock().clone();
        if !self.interactive.load(std::sync::atomic::Ordering::SeqCst) || self.sink.lock().is_none()
        {
            return QuestionOutcome::Skipped {
                reason: SkipReason::NonInteractive,
            };
        }
        let (tx, mut rx) = oneshot::channel();
        let id = request.question_id.clone();
        let tool_call_id = request.tool_call_id.clone();
        let timeout = request.timeout_seconds;
        self.pending
            .lock()
            .insert(id.clone(), (request.clone(), tx));
        self.emit(AgentEvent::QuestionAsked(request)).await;
        let cancelled = async {
            match &run {
                Some(r) => r.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let deadline = async {
            match timeout {
                Some(s) => tokio::time::sleep(std::time::Duration::from_secs(u64::from(s))).await,
                None => std::future::pending().await,
            }
        };
        let closed = |reason| QuestionOutcome::Skipped { reason };
        let outcome = tokio::select! {
            biased;
            answer = &mut rx => answer.unwrap_or(closed(SkipReason::Cancelled)),
            _ = deadline => match self.pending.lock().remove(&id) {
                Some(_) => closed(SkipReason::Timeout),
                // An answer was accepted at the last instant: it wins.
                None => rx.try_recv().unwrap_or(closed(SkipReason::Timeout)),
            },
            _ = cancelled => match self.pending.lock().remove(&id) {
                Some(_) => closed(SkipReason::Cancelled),
                None => rx.try_recv().unwrap_or(closed(SkipReason::Cancelled)),
            },
        };
        self.emit(AgentEvent::QuestionResolved {
            question_id: id,
            tool_call_id,
            outcome: outcome.clone(),
        })
        .await;
        outcome
    }
}

/// `Ask`: a question with choices for the person, answered or skipped.
pub struct AskTool {
    broker: Arc<AskBroker>,
}

impl AskTool {
    pub fn new(broker: Arc<AskBroker>) -> Self {
        Self { broker }
    }

    fn description_for(timer: AskTimer) -> String {
        let base = "Ask the person one question with 2 to 6 choices (each with a short label and an \
                    optional description; `recommended` marks one). They can also answer freely or \
                    skip. Use it only for a decision that is theirs and that the context cannot \
                    settle; the call waits for the answer.";
        match timer {
            AskTimer::Off => base.to_string(),
            AskTimer::Smart => format!(
                "{base} Set `timeout_seconds` ({MIN_TIMER_SECONDS}–{MAX_TIMER_SECONDS}) from what is at \
                 stake: without an answer by then, the question is skipped."
            ),
            AskTimer::Custom { seconds } => format!(
                "{base} The person set a fixed duration of {seconds} s per question: without an \
                 answer by then, the question is skipped."
            ),
        }
    }

    fn schema_for(timer: AskTimer) -> Value {
        let mut properties = serde_json::json!({
            "question": { "type": "string", "minLength": 1 },
            "options": {
                "type": "array",
                "minItems": MIN_OPTIONS,
                "maxItems": MAX_OPTIONS,
                "items": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string", "minLength": 1 },
                        "description": { "type": "string" }
                    },
                    "required": ["label"],
                    "additionalProperties": false
                }
            },
            "recommended": { "type": "integer", "minimum": 0, "description": "Index of the recommended option." }
        });
        let mut required = vec!["question", "options"];
        if timer == AskTimer::Smart {
            properties["timeout_seconds"] = serde_json::json!({
                "type": "integer",
                "minimum": MIN_TIMER_SECONDS,
                "maximum": MAX_TIMER_SECONDS
            });
            required.push("timeout_seconds");
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskInput {
    question: String,
    options: Vec<QuestionOption>,
    #[serde(default)]
    recommended: Option<usize>,
    #[serde(default)]
    timeout_seconds: Option<u32>,
}

#[async_trait]
impl Tool for AskTool {
    fn name(&self) -> &str {
        "Ask"
    }

    fn description(&self) -> &str {
        "Ask the person one question with choices; the call waits for the answer."
    }

    fn input_schema(&self) -> Value {
        Self::schema_for(self.broker.timer())
    }

    /// The definition follows the timer captured with the current message.
    fn to_definition(&self) -> ToolDefinition {
        let timer = self.broker.timer();
        ToolDefinition {
            name: "Ask".to_string(),
            description: Self::description_for(timer),
            input_schema: Self::schema_for(timer),
        }
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Custom
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let i: AskInput = match bricks_tools::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let question = i.question.trim().to_string();
        if question.is_empty() {
            return ToolResult::error("`question` is empty.");
        }
        if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&i.options.len()) {
            return ToolResult::error(format!(
                "`options` needs {MIN_OPTIONS} to {MAX_OPTIONS} choices (got {}).",
                i.options.len()
            ));
        }
        if i.options.iter().any(|o| o.label.trim().is_empty()) {
            return ToolResult::error("every option needs a non-empty `label`.");
        }
        if let Some(r) = i.recommended
            && r >= i.options.len()
        {
            return ToolResult::error(format!(
                "`recommended` is {r}, but the options go from 0 to {}.",
                i.options.len() - 1
            ));
        }
        let timeout = match (self.broker.timer(), i.timeout_seconds) {
            (AskTimer::Off, None) => None,
            (AskTimer::Off, Some(_)) => {
                return ToolResult::error(
                    "`timeout_seconds` is not available: questions have no timer.",
                );
            }
            (AskTimer::Custom { .. }, Some(_)) => {
                return ToolResult::error(
                    "`timeout_seconds` is not available: the person imposed the question's duration.",
                );
            }
            (AskTimer::Custom { seconds }, None) => Some(seconds),
            (AskTimer::Smart, None) => {
                return ToolResult::error(format!(
                    "`timeout_seconds` is required ({MIN_TIMER_SECONDS}–{MAX_TIMER_SECONDS})."
                ));
            }
            (AskTimer::Smart, Some(s)) if !(MIN_TIMER_SECONDS..=MAX_TIMER_SECONDS).contains(&s) => {
                return ToolResult::error(format!(
                    "`timeout_seconds` must be between {MIN_TIMER_SECONDS} and {MAX_TIMER_SECONDS} (got {s})."
                ));
            }
            (AskTimer::Smart, Some(s)) => Some(s),
        };
        let options: Vec<QuestionOption> = i
            .options
            .into_iter()
            .map(|o| QuestionOption {
                label: o.label.trim().to_string(),
                description: o
                    .description
                    .map(|d| d.trim().to_string())
                    .filter(|d| !d.is_empty()),
            })
            .collect();
        let tool_call_id = ctx
            .extensions
            .get::<bricks_tools::CurrentToolCall>()
            .map(|c| c.0.clone())
            .unwrap_or_default();
        let request = QuestionRequest {
            question_id: format!("q_{}", uuid::Uuid::new_v4().simple()),
            tool_call_id,
            question,
            options: options.clone(),
            recommended: i.recommended,
            timeout_seconds: timeout,
            deadline_at: timeout
                .map(|s| chrono::Utc::now().timestamp_millis() + i64::from(s) * 1000),
        };
        let outcome = self.broker.ask(request).await;
        let text = match &outcome {
            QuestionOutcome::Chosen { choice } => {
                format!(
                    "The person chose option {choice}: {}.",
                    options[*choice].label
                )
            }
            QuestionOutcome::Other { text } => {
                format!("The person answered in their own words: {text}")
            }
            QuestionOutcome::Skipped { reason } => match reason {
                SkipReason::User => "The person skipped the question.".to_string(),
                SkipReason::Timeout => format!(
                    "No answer within {} s: the question was skipped (timeout). Go on without their \
                     answer, or say what you need from them.",
                    timeout.unwrap_or_default()
                ),
                SkipReason::Cancelled => {
                    "The question was closed: the run was cancelled.".to_string()
                }
                SkipReason::NonInteractive => {
                    "Nobody can answer here (non-interactive): the question was skipped."
                        .to_string()
                }
            },
        };
        ToolResult::success(text).with_metadata(serde_json::to_value(&outcome).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bricks_tools::permissions::AllowAll;
    use bricks_tools::{CostTracker, Extensions, Tool};
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn question(id: &str, options: usize, timeout: Option<u32>) -> QuestionRequest {
        QuestionRequest {
            question_id: id.to_string(),
            tool_call_id: format!("tc-{id}"),
            question: "Quelle option ?".to_string(),
            options: (0..options)
                .map(|i| QuestionOption {
                    label: format!("Option {i}"),
                    description: None,
                })
                .collect(),
            recommended: None,
            timeout_seconds: timeout,
            deadline_at: None,
        }
    }

    /// Opens a run on `broker`: its events come back on the receiver.
    fn open_run(broker: &AskBroker) -> (mpsc::Receiver<AgentEvent>, CancellationToken) {
        let (tx, rx) = mpsc::channel(16);
        let token = CancellationToken::new();
        broker.begin_run(tx, token.clone());
        (rx, token)
    }

    /// Publishes `request` in a task, and returns once it is announced.
    async fn publish(
        broker: &Arc<AskBroker>,
        rx: &mut mpsc::Receiver<AgentEvent>,
        request: QuestionRequest,
    ) -> tokio::task::JoinHandle<QuestionOutcome> {
        let b = Arc::clone(broker);
        let handle = tokio::spawn(async move { b.ask(request).await });
        assert!(
            matches!(rx.recv().await, Some(AgentEvent::QuestionAsked(_))),
            "the question is announced first"
        );
        handle
    }

    #[tokio::test]
    async fn respond_refuses_an_unknown_question_a_missing_choice_and_an_empty_text() {
        let broker = AskBroker::new(true);
        let (mut rx, _token) = open_run(&broker);
        let handle = publish(&broker, &mut rx, question("q1", 3, None)).await;

        let unknown = broker.respond("q-nope", QuestionAnswer::Skip).unwrap_err();
        assert!(
            unknown.contains("no pending question `q-nope`"),
            "{unknown}"
        );
        let far = broker
            .respond("q1", QuestionAnswer::Choice { choice: 3 })
            .unwrap_err();
        assert!(far.contains("has no choice 3"), "{far}");
        let blank = broker
            .respond(
                "q1",
                QuestionAnswer::Other {
                    text: " \n\t ".into(),
                },
            )
            .unwrap_err();
        assert_eq!(blank, "the free answer is empty");
        assert_eq!(
            broker.pending().len(),
            1,
            "refused answers leave the question open"
        );

        broker
            .respond("q1", QuestionAnswer::Choice { choice: 2 })
            .unwrap();
        assert_eq!(handle.await.unwrap(), QuestionOutcome::Chosen { choice: 2 });
    }

    #[tokio::test]
    async fn the_first_resolution_wins_and_a_second_one_is_refused() {
        let broker = AskBroker::new(true);
        let (mut rx, _token) = open_run(&broker);
        let handle = publish(&broker, &mut rx, question("q1", 2, None)).await;

        broker
            .respond(
                "q1",
                QuestionAnswer::Other {
                    text: "  rien de ce qui précède  ".into(),
                },
            )
            .unwrap();
        let again = broker
            .respond("q1", QuestionAnswer::Choice { choice: 0 })
            .unwrap_err();
        assert!(again.contains("no pending question"), "{again}");
        let skipped_later = broker.respond("q1", QuestionAnswer::Skip).unwrap_err();
        assert!(
            skipped_later.contains("no pending question"),
            "{skipped_later}"
        );

        let expected = QuestionOutcome::Other {
            text: "rien de ce qui précède".into(),
        };
        assert_eq!(handle.await.unwrap(), expected);
        match rx.recv().await {
            Some(AgentEvent::QuestionResolved {
                question_id,
                tool_call_id,
                outcome,
            }) => {
                assert_eq!(question_id, "q1");
                assert_eq!(tool_call_id, "tc-q1");
                assert_eq!(outcome, expected, "the close says what was accepted");
            }
            _ => panic!("the close is announced after the question"),
        }
    }

    #[tokio::test]
    async fn skipping_is_the_users_reason() {
        let broker = AskBroker::new(true);
        let (mut rx, _token) = open_run(&broker);
        let handle = publish(&broker, &mut rx, question("q1", 2, None)).await;
        broker.respond("q1", QuestionAnswer::Skip).unwrap();
        assert_eq!(
            handle.await.unwrap(),
            QuestionOutcome::Skipped {
                reason: SkipReason::User
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_deadline_skips_with_reason_timeout_after_exactly_its_duration() {
        let broker = AskBroker::new(true);
        let (mut rx, _token) = open_run(&broker);
        let started = tokio::time::Instant::now();

        let outcome = broker.ask(question("q1", 2, Some(30))).await;

        assert_eq!(
            outcome,
            QuestionOutcome::Skipped {
                reason: SkipReason::Timeout
            }
        );
        assert_eq!(started.elapsed(), Duration::from_secs(30));
        assert!(broker.pending().is_empty(), "an expired question is closed");
        assert!(
            broker
                .respond("q1", QuestionAnswer::Choice { choice: 0 })
                .is_err(),
            "a closed question takes no answer"
        );
        assert!(matches!(
            rx.recv().await,
            Some(AgentEvent::QuestionAsked(_))
        ));
        assert!(matches!(
            rx.recv().await,
            Some(AgentEvent::QuestionResolved {
                outcome: QuestionOutcome::Skipped {
                    reason: SkipReason::Timeout
                },
                ..
            })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_in_time_wins_and_the_deadline_never_fires() {
        let broker = AskBroker::new(true);
        let (mut rx, _token) = open_run(&broker);
        let handle = publish(&broker, &mut rx, question("q1", 2, Some(30))).await;
        broker
            .respond("q1", QuestionAnswer::Choice { choice: 1 })
            .unwrap();
        assert_eq!(handle.await.unwrap(), QuestionOutcome::Chosen { choice: 1 });
    }

    #[tokio::test]
    async fn cancelling_the_run_closes_a_waiting_question_as_cancelled() {
        let broker = AskBroker::new(true);
        let (mut rx, token) = open_run(&broker);
        let handle = publish(&broker, &mut rx, question("q1", 2, None)).await;
        token.cancel();
        assert_eq!(
            handle.await.unwrap(),
            QuestionOutcome::Skipped {
                reason: SkipReason::Cancelled
            }
        );
        assert!(broker.pending().is_empty());
    }

    #[tokio::test]
    async fn ending_the_run_closes_its_waiting_questions_as_cancelled() {
        let broker = AskBroker::new(true);
        let (mut rx, _token) = open_run(&broker);
        let handle = publish(&broker, &mut rx, question("q1", 2, None)).await;
        broker.end_run();
        assert!(broker.pending().is_empty());
        assert_eq!(
            handle.await.unwrap(),
            QuestionOutcome::Skipped {
                reason: SkipReason::Cancelled
            }
        );
        assert!(
            broker
                .respond("q1", QuestionAnswer::Skip)
                .unwrap_err()
                .contains("no pending question")
        );
    }

    #[tokio::test]
    async fn without_a_person_to_answer_nothing_is_published() {
        let quiet = AskBroker::new(false);
        let (mut rx, _token) = open_run(&quiet);
        assert_eq!(
            quiet.ask(question("q1", 2, None)).await,
            QuestionOutcome::Skipped {
                reason: SkipReason::NonInteractive
            }
        );
        assert!(rx.try_recv().is_err(), "no event for a non-interactive run");
        assert!(quiet.pending().is_empty());

        // Interactive, but no run is open: nobody is listening either.
        let idle = AskBroker::new(true);
        assert_eq!(
            idle.ask(question("q2", 2, None)).await,
            QuestionOutcome::Skipped {
                reason: SkipReason::NonInteractive
            }
        );
        assert!(idle.pending().is_empty());
    }

    #[test]
    fn the_timer_is_off_until_a_message_sets_it() {
        let broker = AskBroker::new(true);
        assert_eq!(broker.timer(), AskTimer::Off);
        broker.set_timer(AskTimer::Custom { seconds: 45 });
        assert_eq!(broker.timer(), AskTimer::Custom { seconds: 45 });
        broker.set_timer(AskTimer::Smart);
        assert_eq!(broker.timer(), AskTimer::Smart);
    }

    fn ctx() -> ToolContext {
        ToolContext {
            working_dir: PathBuf::from("."),
            session_id: "s-ask".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    fn input(timeout: Option<u32>) -> Value {
        let mut v = json!({
            "question": "  Quelle base ?  ",
            "options": [{ "label": " Postgres " }, { "label": "SQLite", "description": "   " }],
            "recommended": 1
        });
        if let Some(s) = timeout {
            v["timeout_seconds"] = json!(s);
        }
        v
    }

    #[tokio::test(start_paused = true)]
    async fn the_timer_decides_the_deadline_of_each_question() {
        let cases: [(AskTimer, Option<u32>, Option<u32>); 3] = [
            (AskTimer::Off, None, None),
            (AskTimer::Smart, Some(120), Some(120)),
            (AskTimer::Custom { seconds: 45 }, None, Some(45)),
        ];
        for (timer, model_timeout, expected) in cases {
            let broker = AskBroker::new(true);
            broker.set_timer(timer);
            let (mut rx, _token) = open_run(&broker);
            let tool = Arc::new(AskTool::new(Arc::clone(&broker)));
            let call = ctx().for_call("tc-timer");
            let runner = Arc::clone(&tool);
            let task =
                tokio::spawn(async move { runner.execute(input(model_timeout), &call).await });

            let Some(AgentEvent::QuestionAsked(q)) = rx.recv().await else {
                panic!("no question announced for {timer:?}");
            };
            assert_eq!(q.timeout_seconds, expected, "{timer:?}");
            assert_eq!(q.tool_call_id, "tc-timer");
            match expected {
                Some(s) => {
                    let left =
                        q.deadline_at.expect("a deadline") - chrono::Utc::now().timestamp_millis();
                    let full = i64::from(s) * 1000;
                    assert!(
                        (full - 1000..=full).contains(&left),
                        "{timer:?}: {left} ms left"
                    );
                }
                None => assert_eq!(q.deadline_at, None, "{timer:?}"),
            }
            assert_eq!(q.question, "Quelle base ?", "the question is trimmed");
            assert_eq!(q.options[0].label, "Postgres", "labels are trimmed");
            assert_eq!(
                q.options[1].description, None,
                "a blank description is dropped"
            );
            assert_eq!(q.recommended, Some(1));

            broker
                .respond(&q.question_id, QuestionAnswer::Skip)
                .unwrap();
            let result = task.await.unwrap();
            assert!(!result.is_error, "{timer:?}: {}", result.content);
            assert_eq!(
                result.metadata,
                Some(json!({ "state": "skipped", "reason": "user" }))
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_question_tells_the_model_to_go_on() {
        let broker = AskBroker::new(true);
        broker.set_timer(AskTimer::Smart);
        let (_rx, _token) = open_run(&broker);
        let tool = AskTool::new(Arc::clone(&broker));

        let result = tool.execute(input(Some(30)), &ctx()).await;

        assert!(!result.is_error);
        assert_eq!(
            result.content,
            "No answer within 30 s: the question was skipped (timeout). Go on without their answer, or say what you need from them."
        );
        assert_eq!(
            result.metadata,
            Some(json!({ "state": "skipped", "reason": "timeout" }))
        );
    }
}
