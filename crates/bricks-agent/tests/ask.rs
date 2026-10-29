//! `Ask`, end to end: the question timer (off, smart, custom) in the tool
//! definitions and in the Bricks foundation, the validation of the model's
//! input, the wire shapes the desktop reads, and sessions whose scripted model
//! asks: answers, skips, refusals, cancellation and deadlines.
//!
//! Controller tests use a temporary project and sessions folder only; no file
//! of the user's `~/.bricks` is read or written.

#[allow(dead_code)]
mod common;

use bricks_agent::BricksConfig;
use bricks_agent::agents::ProfileSources;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::*;
use bricks_agent::system_prompt::{
    SystemPromptOptions, bricks_foundation, bricks_foundation_with,
};
use bricks_tools::permissions::AllowAll;
use bricks_tools::{CostTracker, Extensions, Tool, ToolContext, ToolResult};
use bricks_types::ToolDefinition;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

// ─── The tool, without a session ─────────────────────────────────────────────

fn broker(timer: AskTimer, interactive: bool) -> Arc<AskBroker> {
    let b = AskBroker::new(interactive);
    b.set_timer(timer);
    b
}

fn definition(timer: AskTimer) -> ToolDefinition {
    AskTool::new(broker(timer, true)).to_definition()
}

fn property_names(def: &ToolDefinition) -> Vec<String> {
    let mut names: Vec<String> = def.input_schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

fn required_names(def: &ToolDefinition) -> Vec<String> {
    let mut names: Vec<String> = def.input_schema["required"]
        .as_array()
        .expect("required")
        .iter()
        .map(|v| v.as_str().expect("a name").to_string())
        .collect();
    names.sort();
    names
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

/// A non-interactive broker answers at once, so valid input can be run
/// without waiting: the outcome says why nobody answered.
async fn run_tool(timer: AskTimer, input: Value) -> ToolResult {
    AskTool::new(broker(timer, false))
        .execute(input, &ctx())
        .await
}

fn input(options: Value) -> Value {
    json!({ "question": "Quelle base ?", "options": options })
}

fn two_options() -> Value {
    json!([{ "label": "Postgres" }, { "label": "SQLite" }])
}

#[test]
fn the_timer_bounds_are_30_to_3600_seconds() {
    for ok in [
        AskTimer::Off,
        AskTimer::Smart,
        AskTimer::Custom { seconds: 30 },
        AskTimer::Custom { seconds: 3_600 },
    ] {
        assert_eq!(ok.check(), Ok(ok));
    }
    for bad in [0, 29, 3_601, u32::MAX] {
        let err = AskTimer::Custom { seconds: bad }
            .check()
            .expect_err("out of range");
        assert!(err.contains("between 30 and 3600 seconds"), "{err}");
        assert!(err.contains(&format!("(got {bad})")), "{err}");
    }
}

#[test]
fn off_offers_no_timeout_parameter() {
    let def = definition(AskTimer::Off);
    assert_eq!(def.name, "Ask");
    assert_eq!(property_names(&def), ["options", "question", "recommended"]);
    assert_eq!(required_names(&def), ["options", "question"]);
    assert!(!def.input_schema.to_string().contains("timeout_seconds"));
    assert!(!def.description.contains("timeout_seconds"));
    assert!(!def.description.contains("skipped"), "no timer, nothing skipped");
}

#[test]
fn smart_requires_the_model_to_set_a_timeout_between_30_and_3600() {
    let def = definition(AskTimer::Smart);
    assert_eq!(
        property_names(&def),
        ["options", "question", "recommended", "timeout_seconds"]
    );
    assert!(required_names(&def).contains(&"timeout_seconds".to_string()));
    let timeout = &def.input_schema["properties"]["timeout_seconds"];
    assert_eq!(timeout["type"], "integer");
    assert_eq!(timeout["minimum"], 30);
    assert_eq!(timeout["maximum"], 3_600);
    assert!(
        def.description.contains("`timeout_seconds` (30–3600)"),
        "{}",
        def.description
    );
}

#[test]
fn custom_fixes_the_duration_and_the_parameter_is_gone() {
    let def = definition(AskTimer::Custom { seconds: 45 });
    assert_eq!(property_names(&def), ["options", "question", "recommended"]);
    assert_eq!(required_names(&def), ["options", "question"]);
    assert!(!def.input_schema.to_string().contains("timeout_seconds"));
    assert!(
        def.description.contains("fixed duration of 45 s per question"),
        "{}",
        def.description
    );
}

#[test]
fn every_definition_bounds_the_choices_and_refuses_extra_fields() {
    for timer in [
        AskTimer::Off,
        AskTimer::Smart,
        AskTimer::Custom { seconds: 60 },
    ] {
        let schema = definition(timer).input_schema;
        assert_eq!(schema["additionalProperties"], false, "{timer:?}");
        assert_eq!(schema["properties"]["options"]["minItems"], 2, "{timer:?}");
        assert_eq!(schema["properties"]["options"]["maxItems"], 6, "{timer:?}");
    }
}

#[test]
fn the_definition_follows_the_timer_of_the_current_message() {
    let b = broker(AskTimer::Off, true);
    let tool = AskTool::new(Arc::clone(&b));
    assert!(!tool.to_definition().input_schema.to_string().contains("timeout_seconds"));
    b.set_timer(AskTimer::Smart);
    assert!(tool.to_definition().input_schema.to_string().contains("timeout_seconds"));
    assert_eq!(tool.input_schema(), tool.to_definition().input_schema);
}

#[tokio::test]
async fn the_input_is_validated_before_anything_is_asked() {
    let cases: Vec<(&str, AskTimer, Value, &str)> = vec![
        (
            "an empty question",
            AskTimer::Off,
            json!({ "question": "   ", "options": two_options() }),
            "`question` is empty.",
        ),
        (
            "one option only",
            AskTimer::Off,
            input(json!([{ "label": "Seule" }])),
            "needs 2 to 6 choices (got 1)",
        ),
        (
            "seven options",
            AskTimer::Off,
            input(json!([
                { "label": "a" }, { "label": "b" }, { "label": "c" }, { "label": "d" },
                { "label": "e" }, { "label": "f" }, { "label": "g" }
            ])),
            "needs 2 to 6 choices (got 7)",
        ),
        (
            "a blank label",
            AskTimer::Off,
            input(json!([{ "label": "  " }, { "label": "SQLite" }])),
            "every option needs a non-empty `label`",
        ),
        (
            "a recommended index outside the options",
            AskTimer::Off,
            json!({ "question": "Q", "options": two_options(), "recommended": 2 }),
            "`recommended` is 2, but the options go from 0 to 1.",
        ),
        (
            "a timeout with no timer",
            AskTimer::Off,
            json!({ "question": "Q", "options": two_options(), "timeout_seconds": 60 }),
            "`timeout_seconds` is not available: questions have no timer.",
        ),
        (
            "a timeout under a fixed custom duration",
            AskTimer::Custom { seconds: 45 },
            json!({ "question": "Q", "options": two_options(), "timeout_seconds": 60 }),
            "the person imposed the question's duration.",
        ),
        (
            "a smart question without its timeout",
            AskTimer::Smart,
            json!({ "question": "Q", "options": two_options() }),
            "`timeout_seconds` is required (30–3600).",
        ),
        (
            "a smart timeout under 30 seconds",
            AskTimer::Smart,
            json!({ "question": "Q", "options": two_options(), "timeout_seconds": 29 }),
            "must be between 30 and 3600 (got 29)",
        ),
        (
            "a smart timeout over an hour",
            AskTimer::Smart,
            json!({ "question": "Q", "options": two_options(), "timeout_seconds": 3_601 }),
            "must be between 30 and 3600 (got 3601)",
        ),
        (
            "an unknown field",
            AskTimer::Off,
            json!({ "question": "Q", "options": two_options(), "colour": "bleu" }),
            "",
        ),
    ];
    for (what, timer, input, expected) in cases {
        let result = run_tool(timer, input).await;
        assert!(result.is_error, "{what}: {}", result.content);
        assert!(
            result.content.contains(expected),
            "{what}: {}",
            result.content
        );
    }
}

#[tokio::test]
async fn valid_input_for_each_timer_is_accepted() {
    let cases = [
        (AskTimer::Off, json!({ "question": "Q", "options": two_options() })),
        (
            AskTimer::Smart,
            json!({ "question": "Q", "options": two_options(), "timeout_seconds": 30 }),
        ),
        (
            AskTimer::Smart,
            json!({ "question": "Q", "options": two_options(), "timeout_seconds": 3_600 }),
        ),
        (AskTimer::Custom { seconds: 45 }, json!({ "question": "Q", "options": two_options() })),
    ];
    for (timer, input) in cases {
        let result = run_tool(timer, input).await;
        assert!(!result.is_error, "{timer:?}: {}", result.content);
        // Nobody answers a non-interactive run: the outcome says so.
        assert_eq!(
            result.metadata,
            Some(json!({ "state": "skipped", "reason": "non_interactive" })),
            "{timer:?}"
        );
    }
}

// ─── Wire shapes the desktop reads ───────────────────────────────────────────

#[test]
fn the_wire_shapes_are_the_ones_the_desktop_reads() {
    assert_eq!(serde_json::to_value(AskTimer::Off).unwrap(), json!({ "mode": "off" }));
    assert_eq!(serde_json::to_value(AskTimer::Smart).unwrap(), json!({ "mode": "smart" }));
    assert_eq!(
        serde_json::to_value(AskTimer::Custom { seconds: 45 }).unwrap(),
        json!({ "mode": "custom", "seconds": 45 })
    );
    assert_eq!(
        serde_json::to_value(QuestionOutcome::Chosen { choice: 1 }).unwrap(),
        json!({ "state": "chosen", "choice": 1 })
    );
    assert_eq!(
        serde_json::to_value(QuestionOutcome::Other { text: "x".into() }).unwrap(),
        json!({ "state": "other", "text": "x" })
    );
    for (reason, name) in [
        (SkipReason::User, "user"),
        (SkipReason::Timeout, "timeout"),
        (SkipReason::Cancelled, "cancelled"),
        (SkipReason::NonInteractive, "non_interactive"),
    ] {
        assert_eq!(
            serde_json::to_value(QuestionOutcome::Skipped { reason }).unwrap(),
            json!({ "state": "skipped", "reason": name })
        );
    }
    let answers = [
        (json!({ "kind": "choice", "choice": 0 }), QuestionAnswer::Choice { choice: 0 }),
        (json!({ "kind": "other", "text": "autre" }), QuestionAnswer::Other { text: "autre".into() }),
        (json!({ "kind": "skip" }), QuestionAnswer::Skip),
    ];
    for (wire, answer) in answers {
        assert_eq!(serde_json::from_value::<QuestionAnswer>(wire.clone()).unwrap(), answer);
        assert_eq!(serde_json::to_value(&answer).unwrap(), wire);
    }
}

#[test]
fn the_protocol_names_the_question_events_and_the_answer_command() {
    let asked = Event::QuestionAsked {
        question: QuestionRequest {
            question_id: "q-1".into(),
            tool_call_id: "tc-1".into(),
            question: "Q".into(),
            options: vec![QuestionOption { label: "A".into(), description: None }],
            recommended: None,
            timeout_seconds: Some(30),
            deadline_at: Some(1_000),
        },
    };
    let wire = serde_json::to_value(&asked).unwrap();
    assert_eq!(wire["type"], "question_asked");
    assert_eq!(wire["question"]["timeout_seconds"], 30);
    assert_eq!(asked.kind(), "question_asked");

    let resolved = Event::QuestionResolved {
        question_id: "q-1".into(),
        tool_call_id: "tc-1".into(),
        outcome: QuestionOutcome::Skipped { reason: SkipReason::Timeout },
    };
    assert_eq!(resolved.kind(), "question_resolved");
    assert_eq!(
        serde_json::to_value(&resolved).unwrap()["outcome"],
        json!({ "state": "skipped", "reason": "timeout" })
    );

    let answer = Command::AnswerQuestion {
        question_id: "q-1".into(),
        answer: QuestionAnswer::Skip,
    };
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({ "type": "answer_question", "question_id": "q-1", "answer": { "kind": "skip" } })
    );
}

#[test]
fn a_prompt_carries_its_timer_when_it_has_one() {
    let plain = serde_json::to_value(Prompt::text("bonjour")).unwrap();
    assert!(plain.get("ask_timer").is_none(), "absent when not set");
    let timed: Prompt = serde_json::from_value(json!({
        "blocks": [],
        "ask_timer": { "mode": "custom", "seconds": 45 }
    }))
    .unwrap();
    assert_eq!(timed.ask_timer, Some(AskTimer::Custom { seconds: 45 }));
    assert_eq!(Prompt::text("bonjour").ask_timer, None);
}

// ─── The Bricks foundation ───────────────────────────────────────────────────

#[test]
fn the_ask_bullet_follows_the_timer_and_only_comes_with_the_tool() {
    let ask = ["Ask".to_string()];
    let off = bricks_foundation_with(&ask, AskTimer::Off);
    assert!(off.contains("- Utilise Ask seulement pour une décision"), "{off}");
    assert!(!off.contains("timeout_seconds"));

    let smart = bricks_foundation_with(&ask, AskTimer::Smart);
    assert!(smart.contains("Choisis `timeout_seconds` selon l'enjeu"), "{smart}");
    assert!(!smart.contains("Le délai de"));

    let custom = bricks_foundation_with(&ask, AskTimer::Custom { seconds: 45 });
    assert!(custom.contains("Le délai de 45 s est imposé"), "{custom}");
    assert!(!custom.contains("Choisis `timeout_seconds`"));

    let without = bricks_foundation_with(&["Read".to_string()], AskTimer::Smart);
    assert!(!without.contains("Utilise Ask"), "no Ask tool, no Ask bullet");

    assert_eq!(
        bricks_foundation(&ask),
        off,
        "the plain foundation is the Off one"
    );
}

// ─── Through the controller, with a scripted model ───────────────────────────

/// The project as a frontend loads it: the Bricks prompt is built for the
/// tools each session announces (`prompt_options`), as bricks-config does.
struct Frontend;

impl ProjectLoader for Frontend {
    fn load(&self, wd: &Path) -> Result<ProjectContext, String> {
        Ok(ProjectContext {
            working_dir: wd.to_path_buf(),
            bricks: bricks(),
            system_prompt: None,
            prompt_options: Some(SystemPromptOptions {
                working_directory: Some(wd.display().to_string()),
                ..SystemPromptOptions::default()
            }),
            long_term_memory: None,
            memory_space: None,
            mcp_servers: Vec::new(),
            agent_profile_sources: Some(ProfileSources {
                project_dir: Some(wd.join(".bricks/agents")),
                user_dir: None,
            }),
        })
    }
}

struct Env {
    ctl: Controller,
    events: EventStream,
    script: Arc<Script>,
    _dir: tempfile::TempDir,
}

fn bricks() -> BricksConfig {
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.semantic.lsp.enabled = false;
    bricks
}

/// A new session with `replies`, interactive or not.
async fn open_with(replies: Vec<Reply>, interactive: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(replies);
    let catalog = ScriptedCatalog::new(&["a", "b"], script.clone());
    let mut cfg = EngineConfig::new(
        dir.path(),
        catalog,
        bricks(),
        dir.path().join(".sessions"),
    );
    cfg.agent_profile_sources = Some(ProfileSources {
        project_dir: Some(dir.path().join(".bricks/agents")),
        user_dir: None,
    });
    cfg.interactive = interactive;
    cfg.project_loader = Some(Arc::new(Frontend));
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    next(&mut events).await; // session_opened
    Env {
        ctl,
        events,
        script,
        _dir: dir,
    }
}

async fn open(replies: Vec<Reply>) -> Env {
    open_with(replies, true).await
}

async fn next_within(events: &mut EventStream, secs: u64) -> Envelope {
    tokio::time::timeout(Duration::from_secs(secs), events.next())
        .await
        .expect("an event in time")
        .expect("stream open")
}

async fn next(events: &mut EventStream) -> Envelope {
    next_within(events, 20).await
}

/// The envelopes up to and including the first one matching `pred`.
async fn until_within(
    events: &mut EventStream,
    secs: u64,
    pred: impl Fn(&Event) -> bool,
) -> Vec<Envelope> {
    let mut out = Vec::new();
    loop {
        let e = next_within(events, secs).await;
        let done = pred(&e.event);
        out.push(e);
        if done {
            return out;
        }
    }
}

async fn until(events: &mut EventStream, pred: impl Fn(&Event) -> bool) -> Vec<Envelope> {
    until_within(events, 20, pred).await
}

fn is_asked(e: &Event) -> bool {
    matches!(e, Event::QuestionAsked { .. })
}

fn is_finished(e: &Event) -> bool {
    matches!(e, Event::RunFinished { .. })
}

fn submit(env: &Env, prompt: Prompt) {
    env.ctl.send(Command::Submit { prompt }).unwrap();
}

fn asked(evs: &[Envelope]) -> QuestionRequest {
    evs.iter()
        .find_map(|e| match &e.event {
            Event::QuestionAsked { question } => Some(question.clone()),
            _ => None,
        })
        .expect("a question was asked")
}

fn resolved(evs: &[Envelope]) -> Option<QuestionOutcome> {
    evs.iter().find_map(|e| match &e.event {
        Event::QuestionResolved { outcome, .. } => Some(outcome.clone()),
        _ => None,
    })
}

/// `(is_error, output)` of every finished call of `name`, in order.
fn tool_results(evs: &[Envelope], name: &str) -> Vec<(bool, String)> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::ToolFinished {
                name: n,
                is_error,
                output,
                ..
            } if n == name => Some((*is_error, output.clone())),
            _ => None,
        })
        .collect()
}

fn rejection(evs: &[Envelope]) -> Option<(String, String)> {
    evs.iter().find_map(|e| match &e.event {
        Event::CommandRejected { command, reason } => Some((command.clone(), reason.clone())),
        _ => None,
    })
}

fn run_outcome(evs: &[Envelope]) -> RunOutcome {
    match &evs.last().unwrap().event {
        Event::RunFinished { outcome, .. } => *outcome,
        other => panic!("not a run_finished: {other:?}"),
    }
}

/// The tool definitions the model was offered on its `index`-th request.
fn offered(script: &Script, index: usize) -> Vec<ToolDefinition> {
    script.requests()[index].1.tools.clone()
}

fn ask_definition(script: &Script, index: usize) -> ToolDefinition {
    offered(script, index)
        .into_iter()
        .find(|t| t.name == "Ask")
        .expect("the Ask tool is offered")
}

/// The Ask call the scripted model makes: two choices, the second recommended.
fn ask_call(timeout: Option<u32>) -> Value {
    let mut v = json!({
        "question": "Quelle base de données ?",
        "options": [
            { "label": "Postgres", "description": "serveur partagé" },
            { "label": "SQLite" }
        ],
        "recommended": 1
    });
    if let Some(s) = timeout {
        v["timeout_seconds"] = json!(s);
    }
    v
}

fn secs_left(deadline_at: i64) -> i64 {
    (deadline_at - chrono::Utc::now().timestamp_millis()) / 1000
}

#[tokio::test]
async fn an_answer_is_the_result_of_the_call_and_closes_the_question() {
    let mut env = open(vec![
        Reply::tool("ask-1", "Ask", ask_call(None)),
        Reply::text("Je prends SQLite."),
    ])
    .await;
    submit(&env, Prompt::text("Choisis une base"));

    let before = until(&mut env.events, is_asked).await;
    let q = asked(&before);
    assert_eq!(q.options.len(), 2);
    assert_eq!(q.options[0].description.as_deref(), Some("serveur partagé"));
    assert_eq!(q.recommended, Some(1));
    assert_eq!(q.timeout_seconds, None, "no timer: no duration");
    assert_eq!(q.deadline_at, None);
    assert_eq!(
        env.ctl.snapshot().pending_questions,
        vec![q.clone()],
        "the snapshot lists the question while it waits"
    );

    env.ctl
        .send(Command::AnswerQuestion {
            question_id: q.question_id.clone(),
            answer: QuestionAnswer::Choice { choice: 1 },
        })
        .unwrap();
    let after = until(&mut env.events, is_finished).await;

    assert_eq!(resolved(&after), Some(QuestionOutcome::Chosen { choice: 1 }));
    let results = tool_results(&after, "Ask");
    assert_eq!(results.len(), 1);
    assert!(!results[0].0, "{}", results[0].1);
    assert!(results[0].1.contains("The person chose option 1: SQLite."), "{}", results[0].1);
    assert_eq!(run_outcome(&after), RunOutcome::Succeeded);
    assert!(env.ctl.snapshot().pending_questions.is_empty());
}

#[tokio::test]
async fn a_skip_and_a_free_answer_are_reported_as_the_person_gave_them() {
    let mut env = open(vec![
        Reply::tool("ask-1", "Ask", ask_call(None)),
        Reply::text("Passons."),
        Reply::tool("ask-2", "Ask", ask_call(None)),
        Reply::text("Noté."),
    ])
    .await;

    submit(&env, Prompt::text("Première"));
    let q = asked(&until(&mut env.events, is_asked).await);
    env.ctl
        .send(Command::AnswerQuestion {
            question_id: q.question_id,
            answer: QuestionAnswer::Skip,
        })
        .unwrap();
    let first = until(&mut env.events, is_finished).await;
    assert_eq!(
        resolved(&first),
        Some(QuestionOutcome::Skipped {
            reason: SkipReason::User
        })
    );
    assert!(
        tool_results(&first, "Ask")[0].1.contains("The person skipped the question."),
    );

    submit(&env, Prompt::text("Seconde"));
    let q = asked(&until(&mut env.events, is_asked).await);
    env.ctl
        .send(Command::AnswerQuestion {
            question_id: q.question_id,
            answer: QuestionAnswer::Other {
                text: "  Ni l'un ni l'autre  ".into(),
            },
        })
        .unwrap();
    let second = until(&mut env.events, is_finished).await;
    assert_eq!(
        resolved(&second),
        Some(QuestionOutcome::Other {
            text: "Ni l'un ni l'autre".into()
        }),
        "the free answer is trimmed"
    );
    assert!(
        tool_results(&second, "Ask")[0]
            .1
            .contains("The person answered in their own words: Ni l'un ni l'autre"),
    );
}

#[tokio::test]
async fn an_invalid_answer_is_refused_and_the_question_stays_open() {
    let mut env = open(vec![
        Reply::tool("ask-1", "Ask", ask_call(None)),
        Reply::text("Merci."),
    ])
    .await;
    submit(&env, Prompt::text("Choisis"));
    let q = asked(&until(&mut env.events, is_asked).await);

    let out_of_range = Command::AnswerQuestion {
        question_id: q.question_id.clone(),
        answer: QuestionAnswer::Choice { choice: 7 },
    };
    assert!(env.ctl.send(out_of_range).is_err());
    let refused = until(&mut env.events, |e| matches!(e, Event::CommandRejected { .. })).await;
    let (command, reason) = rejection(&refused).unwrap();
    assert_eq!(command, "answer_question");
    assert!(reason.contains("has no choice 7"), "{reason}");

    let blank = Command::AnswerQuestion {
        question_id: q.question_id.clone(),
        answer: QuestionAnswer::Other { text: "   ".into() },
    };
    assert!(env.ctl.send(blank).is_err());
    let refused = until(&mut env.events, |e| matches!(e, Event::CommandRejected { .. })).await;
    assert!(rejection(&refused).unwrap().1.contains("the free answer is empty"));

    let unknown = Command::AnswerQuestion {
        question_id: "q-unknown".into(),
        answer: QuestionAnswer::Skip,
    };
    assert!(env.ctl.send(unknown).is_err());
    assert_eq!(env.ctl.snapshot().pending_questions, vec![q.clone()]);

    env.ctl
        .send(Command::AnswerQuestion {
            question_id: q.question_id,
            answer: QuestionAnswer::Choice { choice: 0 },
        })
        .unwrap();
    let done = until(&mut env.events, is_finished).await;
    assert_eq!(resolved(&done), Some(QuestionOutcome::Chosen { choice: 0 }));
    assert_eq!(run_outcome(&done), RunOutcome::Succeeded);
}

#[tokio::test]
async fn cancelling_the_run_closes_the_waiting_question_as_cancelled() {
    // The run drops its running calls when it is cancelled, and whether the
    // question's own cancel branch runs first is random (`tokio::select!`):
    // one round in two lost the close, so the rounds show the race reliably.
    for round in 0..20 {
        let mut env = open(vec![Reply::tool("ask-1", "Ask", ask_call(None))]).await;
        submit(&env, Prompt::text("Choisis"));
        let _ = asked(&until(&mut env.events, is_asked).await);

        env.ctl.send(Command::Cancel).unwrap();
        let done = until(&mut env.events, is_finished).await;

        assert_eq!(
            resolved(&done),
            Some(QuestionOutcome::Skipped {
                reason: SkipReason::Cancelled
            }),
            "round {round}: the question is closed with the run"
        );
        assert_eq!(run_outcome(&done), RunOutcome::Cancelled, "round {round}");
        assert!(env.ctl.snapshot().pending_questions.is_empty(), "round {round}");
    }
}

#[tokio::test]
async fn an_interactive_session_offers_ask_and_a_non_interactive_one_does_not() {
    let mut env = open_with(vec![Reply::text("bonjour")], true).await;
    submit(&env, Prompt::text("Salut"));
    until(&mut env.events, is_finished).await;
    let names: Vec<String> = offered(&env.script, 0).into_iter().map(|t| t.name).collect();
    assert!(names.contains(&"Ask".to_string()), "{names:?}");

    let mut quiet = open_with(vec![Reply::text("bonjour")], false).await;
    submit(&quiet, Prompt::text("Salut"));
    until(&mut quiet.events, is_finished).await;
    let names: Vec<String> = offered(&quiet.script, 0).into_iter().map(|t| t.name).collect();
    assert!(!names.contains(&"Ask".to_string()), "{names:?}");
}

#[tokio::test]
async fn a_custom_timer_sets_every_question_and_the_model_cannot_change_it() {
    let mut env = open(vec![
        // Refused at once: the person imposed the duration.
        Reply::tool("ask-1", "Ask", ask_call(Some(60))),
        // Waits, with the imposed duration.
        Reply::tool("ask-2", "Ask", ask_call(None)),
        Reply::text("Fait."),
    ])
    .await;
    submit(
        &env,
        Prompt {
            ask_timer: Some(AskTimer::Custom { seconds: 45 }),
            ..Prompt::text("Décide")
        },
    );

    let before = until(&mut env.events, is_asked).await;
    let refused = tool_results(&before, "Ask");
    assert_eq!(refused.len(), 1);
    assert!(refused[0].0, "the model's duration is refused");
    assert!(refused[0].1.contains("the person imposed"), "{}", refused[0].1);

    let q = asked(&before);
    assert_eq!(q.timeout_seconds, Some(45));
    let left = secs_left(q.deadline_at.expect("a deadline"));
    assert!((44..=45).contains(&left), "{left} s left");

    let definition = ask_definition(&env.script, 0);
    assert!(definition.description.contains("45 s per question"));
    assert!(!definition.input_schema.to_string().contains("timeout_seconds"));
    assert!(
        env.script.requests()[0].1.system.as_deref().unwrap_or_default().contains("Le délai de 45 s est imposé"),
        "the system prompt says the duration is imposed"
    );

    env.ctl
        .send(Command::AnswerQuestion {
            question_id: q.question_id,
            answer: QuestionAnswer::Choice { choice: 0 },
        })
        .unwrap();
    let done = until(&mut env.events, is_finished).await;
    assert_eq!(run_outcome(&done), RunOutcome::Succeeded);
}

#[tokio::test]
async fn a_smart_question_needs_its_own_timeout_and_gets_its_deadline() {
    let mut env = open(vec![
        Reply::tool("ask-1", "Ask", ask_call(None)),
        Reply::tool("ask-2", "Ask", ask_call(Some(120))),
        Reply::text("Fait."),
    ])
    .await;
    submit(
        &env,
        Prompt {
            ask_timer: Some(AskTimer::Smart),
            ..Prompt::text("Décide")
        },
    );

    let before = until(&mut env.events, is_asked).await;
    let refused = tool_results(&before, "Ask");
    assert!(refused[0].0, "{}", refused[0].1);
    assert!(
        refused[0].1.contains("`timeout_seconds` is required (30–3600)."),
        "{}",
        refused[0].1
    );

    let q = asked(&before);
    assert_eq!(q.timeout_seconds, Some(120));
    let left = secs_left(q.deadline_at.expect("a deadline"));
    assert!((119..=120).contains(&left), "{left} s left");
    assert!(
        required_names(&ask_definition(&env.script, 0)).contains(&"timeout_seconds".to_string())
    );

    env.ctl
        .send(Command::AnswerQuestion {
            question_id: q.question_id,
            answer: QuestionAnswer::Skip,
        })
        .unwrap();
    let done = until(&mut env.events, is_finished).await;
    assert_eq!(run_outcome(&done), RunOutcome::Succeeded);
}

#[tokio::test]
async fn an_out_of_range_custom_timer_is_refused_before_anything_is_sent() {
    let mut env = open(vec![]).await;
    for seconds in [10, 3_601] {
        let prompt = Prompt {
            ask_timer: Some(AskTimer::Custom { seconds }),
            ..Prompt::text("Bonjour")
        };
        assert!(env.ctl.send(Command::Submit { prompt }).is_err());
        let refused = until(&mut env.events, |e| matches!(e, Event::CommandRejected { .. })).await;
        let (command, reason) = rejection(&refused).unwrap();
        assert_eq!(command, "submit");
        assert!(
            reason.contains(&format!("between 30 and 3600 seconds (got {seconds})")),
            "{reason}"
        );
    }
    assert!(env.script.requests().is_empty(), "no model request was sent");
    let activity = serde_json::to_value(&env.ctl.snapshot().activity).unwrap();
    assert_eq!(activity["state"], "idle", "the refused message leaves the session idle");

    // The session still runs a message with a valid timer.
    env.script.push(Reply::text("Bonjour."));
    submit(
        &env,
        Prompt {
            ask_timer: Some(AskTimer::Custom { seconds: 30 }),
            ..Prompt::text("Bonjour")
        },
    );
    let done = until(&mut env.events, is_finished).await;
    assert_eq!(run_outcome(&done), RunOutcome::Succeeded);
}

#[tokio::test]
async fn the_timer_of_a_run_holds_until_the_next_message() {
    let mut env = open(vec![
        Reply::tool("ask-1", "Ask", ask_call(None)),
        Reply::text("Suite."),
        Reply::tool("ask-2", "Ask", ask_call(None)),
        Reply::text("Fin."),
    ])
    .await;

    // Run 1: a custom timer.
    submit(
        &env,
        Prompt {
            ask_timer: Some(AskTimer::Custom { seconds: 45 }),
            ..Prompt::text("Première")
        },
    );
    let q = asked(&until(&mut env.events, is_asked).await);
    assert_eq!(q.timeout_seconds, Some(45));

    // A message sent meanwhile, with no timer, is refused and changes nothing.
    let meanwhile = Prompt {
        ask_timer: Some(AskTimer::Off),
        ..Prompt::text("Pendant")
    };
    assert!(env.ctl.send(Command::Submit { prompt: meanwhile }).is_err());
    let refused = until(&mut env.events, |e| matches!(e, Event::CommandRejected { .. })).await;
    assert!(rejection(&refused).unwrap().1.contains("a run is in progress"));

    env.ctl
        .send(Command::AnswerQuestion {
            question_id: q.question_id,
            answer: QuestionAnswer::Choice { choice: 0 },
        })
        .unwrap();
    let first = until(&mut env.events, is_finished).await;
    assert_eq!(run_outcome(&first), RunOutcome::Succeeded);
    assert!(
        ask_definition(&env.script, 1).description.contains("45 s per question"),
        "the run's second request still has the imposed duration"
    );

    // Run 2: no timer, from its own message.
    submit(&env, Prompt::text("Seconde"));
    let q = asked(&until(&mut env.events, is_asked).await);
    assert_eq!(q.timeout_seconds, None, "the next message sets the next timer");
    assert_eq!(q.deadline_at, None);
    let definition = ask_definition(&env.script, 2);
    assert!(!definition.input_schema.to_string().contains("timeout_seconds"));
    assert!(!definition.description.contains("45 s"));

    env.ctl
        .send(Command::AnswerQuestion {
            question_id: q.question_id,
            answer: QuestionAnswer::Skip,
        })
        .unwrap();
    let second = until(&mut env.events, is_finished).await;
    assert_eq!(run_outcome(&second), RunOutcome::Succeeded);
}

#[tokio::test]
async fn a_smart_question_left_unanswered_is_skipped_at_its_deadline() {
    let mut env = open(vec![
        Reply::tool("ask-1", "Ask", ask_call(Some(30))),
        Reply::text("Je continue sans réponse."),
    ])
    .await;
    // The deadline runs on the test clock: the 30 s pass at once when the
    // session waits for nobody else.
    tokio::time::pause();
    submit(
        &env,
        Prompt {
            ask_timer: Some(AskTimer::Smart),
            ..Prompt::text("Décide")
        },
    );

    let before = until_within(&mut env.events, 300, is_asked).await;
    assert_eq!(asked(&before).timeout_seconds, Some(30));

    let rest = until_within(&mut env.events, 300, is_finished).await;
    assert_eq!(
        resolved(&rest),
        Some(QuestionOutcome::Skipped {
            reason: SkipReason::Timeout
        })
    );
    let results = tool_results(&rest, "Ask");
    assert!(!results[0].0, "a timeout is an answer, not an error");
    assert!(results[0].1.contains("No answer within 30 s"), "{}", results[0].1);
    assert_eq!(run_outcome(&rest), RunOutcome::Succeeded);
    assert!(env.ctl.snapshot().pending_questions.is_empty());
}
