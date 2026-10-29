//! The session's durable plan (TodoWrite) through the controller and a
//! scripted model: what the session announces, what it publishes, what its
//! next requests carry, clearing, resume, sessions without a plan file,
//! unreadable files, and sub-agents, whose plans are their own.
//!
//! These are contract tests of the engine, not of any model's behaviour.

#[allow(dead_code)]
mod common;

use bricks_agent::BricksConfig;
use bricks_agent::agents::ProfileSources;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::*;
use bricks_agent::system_prompt::SystemPromptOptions;
use bricks_provider::{CompletionRequest, Provider};
use bricks_tools::todo_write::{PlanState, TodoStatus};
use common::follow::{FollowHandles, Waited};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

// ─── Harness ─────────────────────────────────────────────────────────────────

/// The project as a frontend loads it: prompt options (so every request is
/// assembled from the tools it announces), optionally with sub-agents.
struct Project {
    agents: bool,
}

impl ProjectLoader for Project {
    fn load(&self, wd: &Path) -> Result<ProjectContext, String> {
        let mut bricks = BricksConfig::default();
        bricks.agent.model = Some("test/a".into());
        bricks.semantic.lsp.enabled = false;
        bricks.agents.enabled = self.agents;
        Ok(ProjectContext {
            working_dir: wd.to_path_buf(),
            bricks,
            system_prompt: None,
            prompt_options: Some(SystemPromptOptions {
                is_non_interactive: true,
                working_directory: Some(wd.display().to_string()),
                has_auto_compact: true,
                ..Default::default()
            }),
            long_term_memory: None,
            memory_space: None,
            mcp_servers: Vec::new(),
            agent_profile_sources: self.agents.then(|| ProfileSources {
                project_dir: Some(wd.join(".bricks/agents")),
                user_dir: None,
            }),
        })
    }
}

/// The scripted models. A model waits for the sub-agents it was given, as a
/// real one does before it answers: without it the parent's next request
/// would race the child's for the script's replies.
struct Follow {
    inner: Arc<ScriptedCatalog>,
    waited: Arc<Waited>,
    enabled: Arc<AtomicBool>,
}

impl ModelCatalog for Follow {
    fn models(&self) -> Vec<ModelChoice> {
        self.inner.models()
    }
    fn build(&self, selection: &str, reasoning: Option<&str>) -> Result<Box<dyn Provider>, String> {
        Ok(Box::new(FollowHandles {
            inner: self.inner.build(selection, reasoning)?,
            selection: selection.to_string(),
            waited: Arc::clone(&self.waited),
            enabled: Arc::clone(&self.enabled),
        }))
    }
}

/// A working folder and its sessions folder (`<id>.jsonl`, `<id>.files/`).
struct World {
    wd: PathBuf,
    sessions: PathBuf,
    _root: tempfile::TempDir,
}

impl World {
    fn new(agents: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let wd = std::fs::canonicalize(root.path()).unwrap();
        if agents {
            let profiles = wd.join(".bricks/agents");
            std::fs::create_dir_all(&profiles).unwrap();
            std::fs::write(
                profiles.join("marker.md"),
                "---\nname: marker\ndescription: A marker profile.\n---\nMARK-PROFILE instructions.\n",
            )
            .unwrap();
        }
        let sessions = wd.join(".sessions");
        World {
            wd,
            sessions,
            _root: root,
        }
    }

    fn files(&self, id: &str) -> PathBuf {
        self.sessions.join(format!("{id}.files"))
    }

    fn plan_file(&self, id: &str) -> PathBuf {
        self.files(id).join("plan.json")
    }

    fn config(&self, script: &Arc<Script>, agents: bool) -> EngineConfig {
        let catalog = Arc::new(Follow {
            inner: ScriptedCatalog::new(&["a", "b"], script.clone()),
            waited: Arc::default(),
            enabled: Arc::new(AtomicBool::new(true)),
        });
        let mut cfg = EngineConfig::new(
            self.wd.clone(),
            catalog,
            BricksConfig::default(),
            self.sessions.clone(),
        );
        cfg.interactive = true;
        cfg.project_loader = Some(Arc::new(Project { agents }));
        // Delegation is allowed by the session's policy; nothing else is.
        cfg.permissions = Some(common::default_store(
            self._root.path(),
            &common::allow_tools(&["Agent"]),
        ));
        cfg
    }
}

async fn open(cfg: EngineConfig, session: SessionChoice) -> (Controller, EventStream) {
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session,
            model: None,
            reasoning: None,
        },
    )
    .await
    .expect("the session opens");
    let opened = next(&mut events).await;
    assert_eq!(opened.event.kind(), "session_opened");
    (ctl, events)
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .expect("an event within 10 s")
        .expect("stream open")
}

/// Every envelope up to and including `run_finished`.
async fn until_finished(events: &mut EventStream) -> Vec<Envelope> {
    let mut out = Vec::new();
    loop {
        let e = next(events).await;
        let done = matches!(e.event, Event::RunFinished { .. });
        out.push(e);
        if done {
            return out;
        }
    }
}

/// One prompt and its run to the end. A plan update is queued before the
/// tool call that made it finishes, so it is in these envelopes (checked by
/// `plan_updates_are_queued_before_the_call_and_the_run_end`).
async fn submit(ctl: &Controller, events: &mut EventStream, text: &str) -> Vec<Envelope> {
    ctl.send(Command::Submit {
        prompt: Prompt::text(text),
    })
    .expect("the prompt is accepted");
    until_finished(events).await
}

fn run_id(evs: &[Envelope]) -> String {
    evs.iter()
        .find(|e| e.event.kind() == "run_started")
        .and_then(|e| e.run_id.clone())
        .expect("a run_started with its run id")
}

fn plan_events(evs: &[Envelope]) -> Vec<&Envelope> {
    evs.iter()
        .filter(|e| e.event.kind() == "plan_updated")
        .collect()
}

fn plan_of(e: &Envelope) -> &PlanState {
    match &e.event {
        Event::PlanUpdated { plan } => plan,
        other => panic!("not a plan_updated: {other:?}"),
    }
}

fn steps(plan: &PlanState) -> Vec<(&str, TodoStatus)> {
    plan.todos
        .iter()
        .map(|t| (t.content.as_str(), t.status))
        .collect()
}

fn tool_result<'a>(evs: &'a [Envelope], id: &str) -> (bool, &'a str) {
    evs.iter()
        .find_map(|e| match &e.event {
            Event::ToolFinished {
                tool_call_id,
                is_error,
                output,
                ..
            } if tool_call_id == id => Some((*is_error, output.as_str())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool_finished for {id}"))
}

fn system(r: &CompletionRequest) -> &str {
    r.system.as_deref().unwrap_or("")
}

fn is_child(r: &CompletionRequest) -> bool {
    system(r).contains("You are a sub-agent working on one task")
}

fn plan_call(id: &str) -> Reply {
    Reply::tool(
        id,
        "TodoWrite",
        json!({
            "todos": [
                {"content": "Lire le code", "status": "completed", "activeForm": "Lecture du code"},
                {"content": "Écrire les tests", "status": "in_progress", "activeForm": "Écriture des tests"}
            ],
            "explanation": "Plan initial"
        }),
    )
}

fn stored(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("plan.json exists"))
        .expect("plan.json is JSON")
}

// ─── a. The announced tools and the prompt ───────────────────────────────────

#[tokio::test]
async fn the_session_agent_announces_todowrite_and_no_plan_mode_tool() {
    let w = World::new(false);
    let script = Script::new(vec![Reply::text("Bonjour.")]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    submit(&ctl, &mut events, "Salut.").await;

    let reqs = script.requests();
    let (_, req) = &reqs[0];
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"TodoWrite"), "{names:?}");
    assert!(
        !names.contains(&"EnterPlanMode") && !names.contains(&"ExitPlanMode"),
        "{names:?}"
    );
    let s = system(req);
    assert!(s.contains("## Plan (TodoWrite)"), "{s}");
    // No plan yet: no plan block.
    assert!(!s.contains("<plan"), "{s}");
    ctl.close().await;
}

// ─── b. One update, published once, kept ───────────────────────────────────

#[tokio::test]
async fn a_plan_update_is_published_once_with_its_run_and_kept_on_disk() {
    let w = World::new(false);
    let script = Script::new(vec![plan_call("t1"), Reply::text("Plan noté.")]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    let evs = submit(&ctl, &mut events, "Planifie.").await;

    let updates = plan_events(&evs);
    assert_eq!(updates.len(), 1, "exactly one plan_updated");
    let plan = plan_of(updates[0]);
    assert_eq!(plan.revision, 1);
    assert_eq!(plan.explanation.as_deref(), Some("Plan initial"));
    assert_eq!(
        steps(plan),
        vec![
            ("Lire le code", TodoStatus::Completed),
            ("Écrire les tests", TodoStatus::InProgress),
        ]
    );
    assert_eq!(
        updates[0].run_id.as_deref(),
        Some(run_id(&evs).as_str()),
        "the envelope carries the run that made the update"
    );
    assert_eq!(
        ctl.snapshot().plan,
        *plan,
        "the snapshot holds the same plan"
    );

    let file = stored(&w.plan_file(&ctl.session_id()));
    assert_eq!(file["revision"], 1);
    assert_eq!(file["explanation"], "Plan initial");
    assert_eq!(file["todos"].as_array().unwrap().len(), 2);
    ctl.close().await;
}

// ─── b'. The order of the stream: update, then its call, then the run ───────

#[tokio::test]
async fn plan_updates_are_queued_before_the_call_and_the_run_end() {
    let w = World::new(false);
    let script = Script::new(vec![plan_call("t1"), Reply::text("Plan noté.")]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    let evs = submit(&ctl, &mut events, "Planifie.").await;

    let seq_of = |pred: &dyn Fn(&Envelope) -> bool, what: &str| -> u64 {
        let found: Vec<u64> = evs.iter().filter(|e| pred(e)).map(|e| e.seq).collect();
        assert_eq!(found.len(), 1, "exactly one {what}: {found:?}");
        found[0]
    };
    let updated = seq_of(&|e| e.event.kind() == "plan_updated", "plan_updated");
    let finished_call = seq_of(
        &|e| {
            matches!(
                &e.event,
                Event::ToolFinished { tool_call_id, name, .. }
                    if tool_call_id == "t1" && name == "TodoWrite"
            )
        },
        "TodoWrite tool_finished",
    );
    let run_end = seq_of(&|e| e.event.kind() == "run_finished", "run_finished");
    assert!(
        updated < finished_call,
        "plan_updated {updated} before its call {finished_call}"
    );
    assert!(
        finished_call < run_end,
        "the call {finished_call} before the run {run_end}"
    );
    // Sequence numbers are contiguous: nothing was queued out of place.
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e.seq, evs[0].seq + i as u64);
    }

    // Nothing about the plan is queued after the run ended.
    while let Some(e) = events.try_next() {
        assert_ne!(e.event.kind(), "plan_updated", "a late plan update: {e:?}");
    }
    ctl.close().await;
}

// ─── c. A new message resets the plan; tool turns within a run keep it ──────

#[tokio::test]
async fn a_new_message_resets_the_plan_and_a_refused_update_keeps_it_empty() {
    let w = World::new(false);
    let refused = Reply::tool(
        "t2",
        "TodoWrite",
        json!({"todos": [
            {"content": "Premier", "status": "in_progress", "activeForm": "Premier"},
            {"content": "Second", "status": "in_progress", "activeForm": "Second"}
        ]}),
    );
    let script = Script::new(vec![
        plan_call("t1"),
        Reply::text("Plan noté."),
        // Second run: its first request, then the refused update, then the answer.
        refused,
        Reply::text("Refusé."),
    ]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    submit(&ctl, &mut events, "Planifie.").await;
    let second = submit(&ctl, &mut events, "Suite.").await;

    let reqs = script.requests();
    assert_eq!(reqs.len(), 4, "two runs: two requests each");
    let same_run = system(&reqs[1].1);
    assert!(same_run.contains("<plan revision=\"1\""));
    assert!(same_run.contains("Écrire les tests"));
    for (i, (_, req)) in reqs.iter().enumerate().skip(2) {
        let s = system(req);
        assert!(
            s.contains("<plan revision=\"2\">\n(plan vide)"),
            "request {i} carries the cleared plan: {s}"
        );
        assert!(
            !s.contains("Écrire les tests") && !s.contains("Plan initial"),
            "request {i} no longer carries the previous steps or explanation"
        );
    }

    let (is_error, output) = tool_result(&second, "t2");
    assert!(is_error, "two steps in progress are refused");
    assert!(output.contains("at most one step"), "{output}");
    assert!(output.contains("unchanged"), "{output}");
    let updates = plan_events(&second);
    assert_eq!(updates.len(), 1, "only the submission clears the plan");
    let cleared = plan_of(updates[0]);
    assert_eq!(cleared.revision, 2);
    assert!(cleared.todos.is_empty());
    assert_eq!(cleared.explanation, None);
    assert_eq!(updates[0].run_id.as_deref(), Some(run_id(&second).as_str()));
    assert!(
        updates[0].seq
            < second
                .iter()
                .find(|e| e.event.kind() == "run_started")
                .unwrap()
                .seq
    );
    assert_eq!(ctl.snapshot().plan, *cleared);
    assert_eq!(stored(&w.plan_file(&ctl.session_id()))["todos"], json!([]));
    ctl.close().await;
}

// ─── d. Clearing keeps the revision and the file ────────────────────────────

#[tokio::test]
async fn a_rejected_submission_keeps_the_plan_and_a_later_message_can_reset_it() {
    let w = World::new(false);
    let script = Script::new(vec![
        plan_call("t1"),
        Reply::text("Plan noté."),
        Reply::text("Sans plan."),
        Reply::text("Toujours sans plan."),
    ]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    submit(&ctl, &mut events, "Planifie.").await;
    let before = ctl.snapshot().plan;
    let file_path = w.plan_file(&ctl.session_id());
    let before_file = std::fs::read(&file_path).unwrap();

    let mut prompt = Prompt::text("Pièce jointe absente.");
    prompt.blocks.push(PromptBlock::File {
        path: "missing.txt".into(),
    });
    assert!(ctl.send(Command::Submit { prompt }).is_err());
    assert_eq!(ctl.snapshot().plan, before);
    assert_eq!(std::fs::read(&file_path).unwrap(), before_file);
    assert_eq!(
        script.requests().len(),
        2,
        "no model request for rejected input"
    );

    let evs = submit(&ctl, &mut events, "Nouveau message.").await;
    assert_eq!(plan_events(&evs).len(), 1);
    let cleared = ctl.snapshot().plan;
    assert_eq!(cleared.revision, 2);
    assert!(cleared.todos.is_empty());
    let evs = submit(&ctl, &mut events, "Encore.").await;
    assert!(
        plan_events(&evs).is_empty(),
        "an already empty plan needs no write"
    );
    assert_eq!(ctl.snapshot().plan, cleared);
    ctl.close().await;
}

#[tokio::test]
async fn a_failed_reset_still_sends_the_message_and_keeps_the_previous_plan() {
    let w = World::new(false);
    let script = Script::new(vec![
        plan_call("t1"),
        Reply::text("Plan noté."),
        Reply::text("Suite faite."),
    ]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    submit(&ctl, &mut events, "Planifie.").await;
    let before = ctl.snapshot().plan;
    let path = w.plan_file(&ctl.session_id());
    // A directory at the destination makes the atomic rename fail.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep"), "x").unwrap();
    let evs = submit(&ctl, &mut events, "Suite.").await;
    assert!(
        evs.iter().any(|e| matches!(
            &e.event,
            Event::RunFinished {
                outcome: RunOutcome::Succeeded,
                ..
            }
        )),
        "the message was sent and answered"
    );
    assert!(plan_events(&evs).is_empty(), "nothing published");
    assert!(
        evs.iter().any(|e| matches!(&e.event,
            Event::Notice { message } if message.contains("previous plan was kept"))),
        "a notice says the plan was kept"
    );
    assert_eq!(ctl.snapshot().plan, before);
    assert_eq!(script.requests().len(), 3);
    assert!(system(&script.requests()[2].1).contains("Écrire les tests"));
    ctl.close().await;
}

// ─── After an interruption, unfinished steps stay ───────────────────────────

#[tokio::test]
async fn an_interrupted_run_keeps_its_plan_for_the_next_message() {
    let w = World::new(false);
    let script = Script::new(vec![plan_call("t1"), Reply::hang(), Reply::text("Repris.")]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    ctl.send(Command::Submit {
        prompt: Prompt::text("Planifie."),
    })
    .unwrap();
    // The plan is written, then the second request hangs: interrupt it.
    loop {
        let e = next(&mut events).await;
        if e.event.kind() == "plan_updated" {
            break;
        }
    }
    while script.requests().len() < 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    ctl.send(Command::Cancel).unwrap();
    let evs = until_finished(&mut events).await;
    assert!(evs.iter().any(|e| matches!(
        &e.event,
        Event::RunFinished {
            outcome: RunOutcome::Cancelled,
            ..
        }
    )));
    let before = ctl.snapshot().plan;

    let evs = submit(&ctl, &mut events, "Continue.").await;
    assert!(plan_events(&evs).is_empty(), "the plan is not cleared");
    assert_eq!(ctl.snapshot().plan, before);
    assert!(
        system(&script.requests()[2].1).contains("Écrire les tests"),
        "the next request still carries the unfinished steps"
    );
    ctl.close().await;
}

#[tokio::test]
async fn clearing_the_plan_publishes_an_empty_revision_and_keeps_the_file() {
    let w = World::new(false);
    let clear = Reply::tool("t2", "TodoWrite", json!({"todos": []}));
    let script = Script::new(vec![
        plan_call("t1"),
        Reply::text("Plan noté."),
        clear,
        Reply::text("Effacé."),
    ]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    submit(&ctl, &mut events, "Planifie.").await;
    let evs = submit(&ctl, &mut events, "Efface.").await;

    let updates = plan_events(&evs);
    assert_eq!(
        updates.len(),
        2,
        "submission reset, then explicit tool clear"
    );
    let plan = plan_of(updates[1]);
    assert_eq!(plan.revision, 3);
    assert!(plan.todos.is_empty());
    assert_eq!(plan.explanation, None);
    assert_eq!(ctl.snapshot().plan, *plan);

    let file_path = w.plan_file(&ctl.session_id());
    assert!(file_path.exists(), "the file is written, not deleted");
    let file = stored(&file_path);
    assert_eq!(file["revision"], 3);
    assert_eq!(file["todos"], json!([]));
    ctl.close().await;
}

// ─── e. Resume, and a second session with its own plan ──────────────────────

#[tokio::test]
async fn a_plan_survives_a_resume_and_a_new_session_has_its_own() {
    let w = World::new(false);
    let script = Script::new(vec![
        plan_call("t1"),
        Reply::text("Plan noté."),
        // After the resume: the unfinished plan is kept, then replaced.
        Reply::tool(
            "t2",
            "TodoWrite",
            json!({"todos": [{"content": "Relire", "status": "pending", "activeForm": "Relecture"}]}),
        ),
        Reply::text("Relu."),
    ]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    submit(&ctl, &mut events, "Planifie.").await;
    let id = ctl.session_id();
    let before = ctl.snapshot().plan;
    ctl.close().await;

    // Same folder, same sessions: the plan is read back, whole, and the
    // first message after the reopening keeps its unfinished steps.
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::Resume(id.clone())).await;
    assert_eq!(ctl.snapshot().plan, before, "durable across a resume");
    let evs = submit(&ctl, &mut events, "Suite.").await;
    let updates = plan_events(&evs);
    assert_eq!(updates.len(), 1, "no clearing after a reopening");
    assert_eq!(plan_of(updates[0]).revision, 2);
    assert_eq!(plan_of(updates[0]).todos[0].content, "Relire");
    assert_eq!(ctl.snapshot().plan, *plan_of(updates[0]));
    let reqs = script.requests();
    assert!(system(&reqs[2].1).contains("Écrire les tests"));
    assert!(system(&reqs[3].1).contains("Relire"));
    ctl.close().await;

    // A different new session of the same project: its own plan, none yet.
    let (other, _events) = open(w.config(&script, false), SessionChoice::New).await;
    assert_ne!(other.session_id(), id);
    assert_eq!(other.snapshot().plan.revision, 0);
    assert!(other.snapshot().plan.todos.is_empty());
    assert!(!w.plan_file(&other.session_id()).exists());
    // And the first one still has its plan, untouched by the second.
    assert_eq!(stored(&w.plan_file(&id))["revision"], 2);
    other.close().await;
}

// ─── Removed tools (Sprint 14.8.1) ──────────────────────────────────────────

/// The fifteen native tools removed in 14.8.1.
const REMOVED: [&str; 15] = [
    "EnterPlanMode",
    "ExitPlanMode",
    "CronCreate",
    "CronList",
    "CronDelete",
    "TaskCreate",
    "TaskGet",
    "TaskUpdate",
    "TaskList",
    "TaskStop",
    "TaskOutput",
    "RemoteTrigger",
    "Config",
    "AskUserQuestion",
    "Sleep",
];

/// The session agent announces none of them, and a call to one of them
/// (an old habit of a model, or an old conversation) is an unknown tool:
/// an error result, no success, nothing run. The conversation that holds
/// it resumes and goes on.
#[tokio::test]
async fn a_removed_tool_is_unknown_and_an_old_call_stays_readable() {
    let w = World::new(false);
    let script = Script::new(vec![
        Reply::tool("old", "TaskCreate", json!({"description": "Deploy"})),
        Reply::text("Fait."),
        Reply::text("Toujours là."),
    ]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    let evs = submit(&ctl, &mut events, "Crée une tâche.").await;
    let finished = evs
        .iter()
        .find_map(|e| match &e.event {
            Event::ToolFinished {
                name,
                is_error,
                output,
                ..
            } if name == "TaskCreate" => Some((*is_error, output.clone())),
            _ => None,
        })
        .expect("the call is reported");
    assert!(
        finished.0,
        "a removed tool never succeeds: {:?}",
        finished.1
    );
    let reqs = script.requests();
    for (_, req) in &reqs {
        let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
        for gone in REMOVED {
            assert!(!names.contains(&gone), "{gone} still announced: {names:?}");
        }
        assert!(
            names.contains(&"TodoWrite") && names.contains(&"Bash"),
            "{names:?}"
        );
        assert!(!system(req).contains("Task tracking"));
    }
    let id = ctl.session_id();
    ctl.close().await;

    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::Resume(id)).await;
    assert!(
        ctl.history()
            .await
            .iter()
            .any(|m| serde_json::to_string(m).unwrap().contains("TaskCreate")),
        "the old call is still in the history"
    );
    let evs = submit(&ctl, &mut events, "Et maintenant ?").await;
    assert!(evs.iter().any(|e| matches!(
        &e.event,
        Event::RunFinished {
            outcome: RunOutcome::Succeeded,
            ..
        }
    )));
    ctl.close().await;
}

// ─── f. A session without a plan file resumes at revision 0 ─────────────────

#[tokio::test]
async fn a_session_without_a_plan_file_resumes_at_revision_zero() {
    let w = World::new(false);
    let script = Script::new(vec![
        Reply::text("Bonjour."),
        plan_call("t1"),
        Reply::text("Plan noté."),
    ]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    submit(&ctl, &mut events, "Salut.").await;
    let id = ctl.session_id();
    ctl.close().await;
    assert!(!w.plan_file(&id).exists(), "no plan was ever written");

    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::Resume(id.clone())).await;
    let snap = ctl.snapshot();
    assert_eq!(snap.plan.revision, 0);
    assert!(snap.plan.todos.is_empty());
    let evs = submit(&ctl, &mut events, "Planifie.").await;
    assert_eq!(plan_of(plan_events(&evs)[0]).revision, 1);
    ctl.close().await;
}

// ─── g. An unreadable plan file is kept and refuses updates ─────────────────

#[tokio::test]
async fn an_unreadable_plan_file_is_kept_and_refuses_updates() {
    let w = World::new(false);
    let refused_update = Reply::tool(
        "t2",
        "TodoWrite",
        json!({"todos": [{"content": "Nouvelle étape", "status": "pending", "activeForm": "Nouvelle"}]}),
    );
    let script = Script::new(vec![
        plan_call("t1"),
        Reply::text("Plan noté."),
        refused_update,
        Reply::text("Refusé."),
    ]);
    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::New).await;
    submit(&ctl, &mut events, "Planifie.").await;
    let id = ctl.session_id();
    ctl.close().await;

    let garbage = "{ this is not a plan";
    std::fs::write(w.plan_file(&id), garbage).unwrap();

    let (ctl, mut events) = open(w.config(&script, false), SessionChoice::Resume(id.clone())).await;
    assert_eq!(ctl.snapshot().plan.revision, 0, "the session still opens");
    let evs = submit(&ctl, &mut events, "Modifie.").await;
    let (is_error, output) = tool_result(&evs, "t2");
    assert!(is_error, "{output}");
    assert!(output.contains("cannot be read"), "{output}");
    assert!(plan_events(&evs).is_empty());
    assert_eq!(
        std::fs::read_to_string(w.plan_file(&id)).unwrap(),
        garbage,
        "the file is left as it was"
    );
    ctl.close().await;
}

// ─── Sub-agents: their plans are their own ──────────────────────────────────

fn agent_call(id: &str) -> Reply {
    Reply::tool(
        id,
        "Agent",
        json!({"task": "Look.", "profile": "marker", "isolation": "shared"}),
    )
}

fn is_parent(r: &CompletionRequest) -> bool {
    !is_child(r)
}

#[tokio::test]
async fn a_child_without_todowrite_gets_no_plan_tool_and_no_plan_block() {
    // The default tools (`bricks_tools::coding`) carry no TodoWrite: the
    // session's plan is the session agent's own, never handed to a child.
    let w = World::new(true);
    let script = Script::new(vec![
        plan_call("t1"),
        agent_call("t2"),
        Reply::text("child result"),
        Reply::text("done"),
    ]);
    let (ctl, mut events) = open(w.config(&script, true), SessionChoice::New).await;
    let evs = submit(&ctl, &mut events, "Délègue.").await;
    assert_eq!(plan_events(&evs).len(), 1);

    let reqs = script.requests();
    let children: Vec<&CompletionRequest> = reqs
        .iter()
        .map(|(_, r)| r)
        .filter(|r| is_child(r))
        .collect();
    assert!(!children.is_empty(), "a child ran");
    let parent = reqs.iter().map(|(_, r)| r).find(|r| !is_child(r)).unwrap();
    assert!(
        parent.tools.iter().any(|t| t.name == "SendMessage"),
        "the session agent can message its sub-agents"
    );
    for child in &children {
        assert!(
            !child.tools.iter().any(|t| t.name == "TodoWrite"),
            "a child announces no TodoWrite"
        );
        // With the default `max_depth = 1`, a child starts no sub-agent:
        // SendMessage would have no one to reach.
        assert!(
            !child.tools.iter().any(|t| t.name == "SendMessage"),
            "a child that cannot delegate gets no SendMessage"
        );
        let s = system(child);
        assert!(!s.contains("<plan"), "{s}");
        assert!(!s.contains("## Plan (TodoWrite)"), "{s}");
        assert!(
            !s.contains("Lire le code"),
            "the parent's steps reach no child"
        );
    }
    let parent_plan = ctl.snapshot().plan;
    assert_eq!(parent_plan.revision, 1);
    assert_eq!(steps(&parent_plan)[0].0, "Lire le code");
    ctl.close().await;
}

#[tokio::test]
async fn a_child_with_todowrite_keeps_its_own_plan() {
    let w = World::new(true);
    let parent_step = Reply::tool(
        "t1",
        "TodoWrite",
        json!({"todos": [{"content": "Étape du parent", "status": "pending", "activeForm": "Étape parent"}]}),
    );
    let child_step = Reply::tool(
        "c1",
        "TodoWrite",
        json!({"todos": [{"content": "Étape enfant", "status": "in_progress", "activeForm": "Étape enfant"}]}),
    );
    let script = Script::new(vec![
        parent_step,
        agent_call("t2"),
        child_step,
        Reply::text("child done"),
        Reply::text("done"),
    ]);
    // The factory of the sub-agents carries TodoWrite too: the child builds
    // its own plan, never the parent's.
    let mut cfg = w.config(&script, true);
    cfg.tools = Arc::new(|| {
        let mut tools = bricks_tools::coding();
        tools.push(Box::new(bricks_tools::todo_write::TodoWriteTool));
        tools
    });
    let (ctl, mut events) = open(cfg, SessionChoice::New).await;
    let evs = submit(&ctl, &mut events, "Délègue.").await;

    // The parent's stream: its own update only.
    let updates = plan_events(&evs);
    assert_eq!(
        updates.len(),
        1,
        "the child's update never reaches the parent"
    );
    assert_eq!(plan_of(updates[0]).revision, 1);
    assert_eq!(steps(plan_of(updates[0]))[0].0, "Étape du parent");
    assert_eq!(steps(&ctl.snapshot().plan)[0].0, "Étape du parent");

    let reqs = script.requests();
    let children: Vec<&CompletionRequest> = reqs
        .iter()
        .map(|(_, r)| r)
        .filter(|r| is_child(r))
        .collect();
    assert!(children.len() >= 2, "the child asked twice");
    // Before its own update, the child sees no parent's step.
    assert!(!system(children[0]).contains("Étape du parent"));
    // After it, its own plan, and never the parent's.
    let after = system(children[children.len() - 1]);
    assert!(after.contains("Étape enfant"), "{after}");
    assert!(!after.contains("Étape du parent"), "{after}");

    // The parent's last request: its own step, never the child's.
    let last_parent = reqs.iter().map(|(_, r)| r).rfind(|r| is_parent(r)).unwrap();
    let p = system(last_parent);
    assert!(p.contains("Étape du parent"), "{p}");
    assert!(!p.contains("Étape enfant"), "{p}");
    ctl.close().await;
}
