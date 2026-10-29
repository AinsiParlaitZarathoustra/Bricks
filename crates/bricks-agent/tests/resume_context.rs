//! Resuming a stored session: its history is in the agent when the session
//! opens, so the first context status counts it, and the first run sends it
//! once and announces it once. Scripted model, temporary directories only.

use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog, ScriptedProvider};
use bricks_agent::control::*;
use bricks_agent::events::AgentEvent;
use bricks_agent::{Agent, BricksConfig, Provenance};
use bricks_memory::{JsonlMemory, Memory};
use bricks_types::Message;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Words per message: each stored message is a few kilo-characters long, so
/// the history is clearly larger than prompt and tools.
const PAD: usize = 300;

fn text(tag: &str) -> String {
    format!("{tag} {}", "detail ".repeat(PAD))
}

fn config(wd: &Path, sessions: &Path, script: Arc<Script>) -> EngineConfig {
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    let mut cfg = EngineConfig::new(
        wd,
        ScriptedCatalog::new(&["a", "b"], script),
        bricks,
        sessions,
    );
    cfg.interactive = true;
    cfg
}

async fn open(cfg: EngineConfig, session: SessionChoice) -> Result<(Controller, EventStream), String> {
    Controller::open(
        cfg,
        OpenOptions {
            session,
            model: None,
            reasoning: None,
        },
    )
    .await
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .expect("an event within 10 s")
        .expect("stream open")
}

/// The first envelope: `(resumed, message_count, warnings)`.
async fn opened(events: &mut EventStream) -> (bool, usize, Vec<String>) {
    match next(events).await.event {
        Event::SessionOpened {
            resumed,
            message_count,
            warnings,
            ..
        } => (resumed, message_count, warnings),
        other => panic!("the first event is not session_opened: {other:?}"),
    }
}

/// Events up to and including `run_finished`; returns the final event.
async fn run_finished(events: &mut EventStream) -> Event {
    loop {
        let e = next(events).await;
        if matches!(e.event, Event::RunFinished { .. }) {
            return e.event;
        }
    }
}

async fn submit_and_finish(ctl: &Controller, events: &mut EventStream, prompt: &str) -> Event {
    ctl.send(Command::Submit {
        prompt: Prompt::text(prompt),
    })
    .unwrap();
    run_finished(events).await
}

/// The active history of `id` as stored in `sessions`.
async fn stored_active(sessions: &Path, id: &str) -> Vec<Message> {
    JsonlMemory::new(sessions)
        .load_histories(id)
        .await
        .expect("the stored history loads")
        .active
}

/// A session of `runs` answered prompts, written by a first controller that
/// is then closed. Returns its id and its stored active history.
async fn stored_session(sessions: &Path, wd: &Path, runs: usize) -> (String, Vec<Message>) {
    let script = Script::new(
        (0..runs)
            .map(|i| Reply::text(&text(&format!("answer {i}"))))
            .collect(),
    );
    let (ctl, mut events) = open(config(wd, sessions, script), SessionChoice::New)
        .await
        .unwrap();
    opened(&mut events).await;
    for i in 0..runs {
        let fin = submit_and_finish(&ctl, &mut events, &text(&format!("question {i}"))).await;
        assert!(
            matches!(fin, Event::RunFinished { outcome: RunOutcome::Succeeded, .. }),
            "run {i}: {fin:?}"
        );
    }
    ctl.wait_idle().await;
    let id = ctl.session_id();
    let stored = stored_active(sessions, &id).await;
    assert_eq!(stored.len(), 2 * runs, "one prompt and one answer per run");
    drop(ctl);
    (id, stored)
}

fn texts(messages: &[Message]) -> Vec<Option<String>> {
    messages
        .iter()
        .map(|m| m.get_text().map(str::to_string))
        .collect()
}

/// The exact messages a request carries: the stored history, then `prompt`.
fn expected_request(stored: &[Message], prompt: &str) -> Vec<Option<String>> {
    texts(stored)
        .into_iter()
        .chain(std::iter::once(Some(prompt.to_string())))
        .collect()
}

// ─── Agent-level helpers (SessionLoaded is only visible on the agent) ───────

fn agent(
    script: Arc<Script>,
    memory_dir: &Path,
    session: &str,
    raw_dir: &Path,
    log: Arc<Mutex<Vec<AgentEvent>>>,
) -> Agent {
    Agent::builder()
        .provider(ScriptedProvider::new("test/a", script))
        .memory(JsonlMemory::new(memory_dir))
        .session_id(session)
        .raw_output_dir(raw_dir)
        .on_event(move |e| log.lock().unwrap().push(e.clone()))
        .build()
        .unwrap()
}

fn loaded_counts(log: &Arc<Mutex<Vec<AgentEvent>>>) -> Vec<usize> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::SessionLoaded { message_count, .. } => Some(*message_count),
            _ => None,
        })
        .collect()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// 1. The very first snapshot of a reopened session already counts the whole
///    stored history, as an estimate until the server measures it.
#[tokio::test]
async fn a_resumed_session_counts_its_whole_history_from_the_first_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join(".sessions");
    let (id, stored) = stored_session(&sessions, dir.path(), 5).await;
    let stored_chars: usize = stored.iter().filter_map(|m| m.get_text()).map(str::len).sum();

    let (fresh, _events) = open(
        config(dir.path(), &sessions, Script::new(vec![])),
        SessionChoice::New,
    )
    .await
    .unwrap();
    let base = fresh.snapshot().context.context_used;

    let (resumed, mut events) = open(
        config(dir.path(), &sessions, Script::new(vec![])),
        SessionChoice::Resume(id),
    )
    .await
    .unwrap();
    // Nothing has run yet: the snapshot is taken right after the open.
    let used = resumed.snapshot().context.context_used;
    assert_eq!(used.provenance, Provenance::Estimated, "{used:?}");
    assert_eq!(used.measured_tokens, 0, "no server measurement yet");
    assert!(
        used.tokens >= base.tokens + (stored_chars / 8) as u64,
        "history not counted: resumed {} vs new {} for {stored_chars} chars",
        used.tokens,
        base.tokens
    );

    // Once the server answers, the occupation is measured again.
    opened(&mut events).await;
    let fin = submit_and_finish(&resumed, &mut events, "encore").await;
    assert!(matches!(fin, Event::RunFinished { outcome: RunOutcome::Succeeded, .. }));
    assert_ne!(
        resumed.snapshot().context.context_used.provenance,
        Provenance::Estimated,
        "a server measurement replaces the estimate"
    );
}

/// 2. The first run sends each stored message exactly once, in order, then the
///    new prompt; the next run adds only its own two messages.
#[tokio::test]
async fn the_first_run_sends_the_stored_history_once() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join(".sessions");
    let (id, stored) = stored_session(&sessions, dir.path(), 4).await;

    let script = Script::new(vec![Reply::text("reprise"), Reply::text("encore")]);
    let (ctl, mut events) = open(
        config(dir.path(), &sessions, script.clone()),
        SessionChoice::Resume(id),
    )
    .await
    .unwrap();
    opened(&mut events).await;

    let fin = submit_and_finish(&ctl, &mut events, "continue").await;
    assert!(matches!(fin, Event::RunFinished { outcome: RunOutcome::Succeeded, .. }));
    let first = script.requests();
    assert_eq!(first.len(), 1, "one request for the first run");
    assert_eq!(
        texts(&first[0].1.messages),
        expected_request(&stored, "continue"),
        "the stored history once, then the prompt"
    );

    let fin = submit_and_finish(&ctl, &mut events, "again").await;
    assert!(matches!(fin, Event::RunFinished { outcome: RunOutcome::Succeeded, .. }));
    let requests = script.requests();
    let mut second = stored.clone();
    second.push(Message::user("continue"));
    second.push(Message::assistant("reprise"));
    assert_eq!(
        texts(&requests[1].1.messages),
        expected_request(&second, "again"),
        "the second run adds only its own messages"
    );
}

/// 2 (continued). At agent level, `SessionLoaded` is emitted once, with the
///    stored count, by the first run; the second run emits none.
#[tokio::test]
async fn a_restored_agent_announces_its_history_once() {
    let mem = tempfile::tempdir().unwrap();
    let raw = tempfile::tempdir().unwrap();
    let writer = agent(
        Script::new(
            (0..3)
                .map(|i| Reply::text(&text(&format!("answer {i}"))))
                .collect(),
        ),
        mem.path(),
        "s",
        raw.path(),
        Arc::new(Mutex::new(Vec::new())),
    );
    for i in 0..3 {
        writer.run(&text(&format!("question {i}"))).await.unwrap();
    }
    drop(writer);
    let stored = stored_active(mem.path(), "s").await;
    assert_eq!(stored.len(), 6);

    let script = Script::new(vec![Reply::text("one"), Reply::text("two")]);
    let log = Arc::new(Mutex::new(Vec::new()));
    let b = agent(script.clone(), mem.path(), "s", raw.path(), log.clone());
    assert_eq!(b.restore_history().await.unwrap(), Some(6));
    assert!(loaded_counts(&log).is_empty(), "announced by the run, not at restore");

    b.run("continue").await.unwrap();
    assert_eq!(loaded_counts(&log), vec![6], "announced once, with the stored count");
    b.run("again").await.unwrap();
    assert_eq!(loaded_counts(&log), vec![6], "the second run announces nothing");

    let requests = script.requests();
    assert_eq!(
        texts(&requests[0].1.messages),
        expected_request(&stored, "continue")
    );
    let mut second = stored.clone();
    second.push(Message::user("continue"));
    second.push(Message::assistant("one"));
    assert_eq!(
        texts(&requests[1].1.messages),
        expected_request(&second, "again")
    );
}

/// 3. The opened event's message count is the stored active history length,
///    and the held history agrees.
#[tokio::test]
async fn the_opened_message_count_is_the_stored_active_history() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join(".sessions");
    let (id, stored) = stored_session(&sessions, dir.path(), 3).await;

    let (ctl, mut events) = open(
        config(dir.path(), &sessions, Script::new(vec![])),
        SessionChoice::Resume(id),
    )
    .await
    .unwrap();
    let (resumed, message_count, warnings) = opened(&mut events).await;
    assert!(resumed);
    assert_eq!(message_count, stored.len());
    assert!(
        !warnings.iter().any(|w| w.contains("history not loaded")),
        "{warnings:?}"
    );
    assert_eq!(ctl.history().await.len(), message_count);
}

/// 4. A new session is unaffected: no stored history, prompt and tools only,
///    no history announced.
#[tokio::test]
async fn a_new_session_is_unaffected() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join(".sessions");
    let (ctl, mut events) = open(
        config(dir.path(), &sessions, Script::new(vec![])),
        SessionChoice::New,
    )
    .await
    .unwrap();
    let (resumed, message_count, warnings) = opened(&mut events).await;
    assert!(!resumed);
    assert_eq!(message_count, 0);
    assert!(!warnings.iter().any(|w| w.contains("history not loaded")));
    assert!(ctl.history().await.is_empty());
    let used = ctl.snapshot().context.context_used;
    assert_eq!(used.provenance, Provenance::Estimated);

    // At agent level: nothing to restore, and the first run announces nothing.
    let mem = tempfile::tempdir().unwrap();
    let raw = tempfile::tempdir().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let script = Script::new(vec![Reply::text("hi")]);
    let a = agent(script.clone(), mem.path(), "fresh", raw.path(), log.clone());
    assert_eq!(a.restore_history().await.unwrap(), None);
    a.run("hello").await.unwrap();
    assert!(loaded_counts(&log).is_empty(), "no SessionLoaded for a new session");
    assert_eq!(
        texts(&script.requests()[0].1.messages),
        expected_request(&[], "hello"),
        "only the prompt is sent"
    );
}

/// 5. An unreadable stored history does not prevent opening: the warning is
///    reported, nothing is sent, and the first run fails with the storage
///    error.
#[tokio::test]
async fn an_unreadable_history_opens_with_a_warning_and_the_run_fails() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join(".sessions");
    let (id, _stored) = stored_session(&sessions, dir.path(), 2).await;

    // Break the checkpoint: its digest no longer matches its body.
    let path = sessions.join(format!("{id}.jsonl"));
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(
        body.starts_with("{\"bricks_checkpoint\""),
        "not a checkpoint: {}",
        &body[..body.len().min(60)]
    );
    std::fs::write(&path, format!("{body}not a message\n")).unwrap();

    let script = Script::new(vec![Reply::text("should not be used")]);
    let (ctl, mut events) = open(
        config(dir.path(), &sessions, script.clone()),
        SessionChoice::Resume(id),
    )
    .await
    .expect("the session still opens");
    let (resumed, message_count, warnings) = opened(&mut events).await;
    assert!(resumed);
    assert_eq!(message_count, 0);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("history not loaded") && w.contains("checkpoint digest mismatch")),
        "{warnings:?}"
    );
    assert!(ctl.history().await.is_empty());

    let fin = submit_and_finish(&ctl, &mut events, "again").await;
    match fin {
        Event::RunFinished {
            outcome,
            failure,
            error,
            ..
        } => {
            assert_eq!(outcome, RunOutcome::Failed);
            assert!(matches!(failure, Some(FailureKind::Error)), "{failure:?}");
            let error = error.unwrap_or_default();
            assert!(error.contains("checkpoint digest mismatch"), "{error}");
        }
        other => panic!("not a run_finished: {other:?}"),
    }
    assert!(script.requests().is_empty(), "no request is sent over a broken history");
}
