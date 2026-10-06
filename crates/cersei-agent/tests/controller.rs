//! The command/event contract, end to end, with a scripted model.

use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::*;
use cersei_agent::BricksConfig;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

struct Env {
    ctl: Controller,
    events: EventStream,
    dir: tempfile::TempDir,
    script: Arc<Script>,
}

async fn open_with(
    replies: Vec<Reply>,
    interactive: bool,
    rules: ApprovalRules,
    capacity: usize,
) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let script = Script::new(replies);
    let catalog = ScriptedCatalog::new(&["a", "b"], script.clone());
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.permissions = rules;
    let mut cfg = EngineConfig::new(dir.path(), catalog, bricks, dir.path().join(".sessions"));
    cfg.interactive = interactive;
    cfg.queue_capacity = capacity;
    let (ctl, events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    Env {
        ctl,
        events,
        dir,
        script,
    }
}

async fn open(replies: Vec<Reply>) -> Env {
    open_with(replies, true, ApprovalRules::default(), 64).await
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

async fn until(events: &mut EventStream, kind: &str) -> Envelope {
    loop {
        let e = next(events).await;
        if e.event.kind() == kind {
            return e;
        }
    }
}

fn kinds(evs: &[Envelope]) -> Vec<&'static str> {
    evs.iter().map(|e| e.event.kind()).collect()
}

fn outcome(e: &Envelope) -> RunOutcome {
    match &e.event {
        Event::RunFinished { outcome, .. } => *outcome,
        other => panic!("not a run_finished: {other:?}"),
    }
}

#[tokio::test]
async fn a_run_streams_in_order_and_ends_once() {
    let mut env = open(vec![
        Reply::tools(vec![
            ("c1", "Read", json!({"file_path": "a.txt"})),
            ("c2", "Read", json!({"file_path": "b.txt"})),
            // Relative paths resolve against the session's working directory.
        ]),
        Reply::text("premier jet"),
        // The engine asks once for a deeper look after an early answer
        // (runner nudge); this is the final answer.
        Reply::chunks(&["Les deux ", "fichiers ", "sont lus."]),
    ])
    .await;
    std::fs::write(env.dir.path().join("a.txt"), "A").unwrap();
    std::fs::write(env.dir.path().join("b.txt"), "B").unwrap();
    let opened = next(&mut env.events).await;
    assert_eq!(opened.event.kind(), "session_opened");
    assert_eq!(opened.seq, 1);
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("Lis a et b"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;

    // Contiguous sequence numbers, one run id, schema version.
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e.seq, i as u64 + 2);
        assert_eq!(e.schema, SCHEMA_VERSION);
        assert!(e.run_id.as_deref().unwrap().starts_with("run_"));
    }
    let k = kinds(&evs);
    assert_eq!(k.first(), Some(&"run_started"));
    assert_eq!(k.iter().filter(|k| **k == "run_finished").count(), 1);
    // Both calls start before they finish, each with its own id.
    let pos = |kind: &str, id: &str| {
        evs.iter()
            .position(|e| match &e.event {
                Event::ToolStarted { tool_call_id, .. } if kind == "start" => tool_call_id == id,
                Event::ToolFinished { tool_call_id, .. } if kind == "end" => tool_call_id == id,
                _ => false,
            })
            .unwrap_or_else(|| panic!("{kind} {id}: {k:?}"))
    };
    assert!(pos("start", "c1") < pos("end", "c1") && pos("start", "c2") < pos("end", "c1"));
    let streamed: String = evs
        .iter()
        .filter_map(|e| match &e.event {
            Event::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(streamed, "premier jetLes deux fichiers sont lus.");
    let Event::RunFinished { text, outcome, .. } = &evs.last().unwrap().event else {
        unreachable!()
    };
    assert_eq!(*outcome, RunOutcome::Succeeded);
    assert_eq!(text, "Les deux fichiers sont lus.");
    assert!(k.contains(&"usage") && k.contains(&"context"), "{k:?}");
    // JSON round trip of everything delivered.
    for e in &evs {
        let line = serde_json::to_string(e).unwrap();
        let back: Envelope = serde_json::from_str(&line).unwrap();
        assert_eq!(back.seq, e.seq);
    }
}

#[tokio::test]
async fn commands_during_a_run_follow_the_rules() {
    let mut env = open(vec![Reply::hang(), Reply::text("après")]).await;
    until(&mut env.events, "session_opened").await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("long"),
        })
        .unwrap();
    until(&mut env.events, "run_started").await;
    for cmd in [
        Command::Submit {
            prompt: Prompt::text("second"),
        },
        Command::Compact,
        Command::ClearContext,
        Command::Resume {
            session_id: "x".into(),
        },
    ] {
        let name = cmd.name();
        let err = env.ctl.send(cmd).unwrap_err();
        assert!(
            err.contains("run") || err.contains("in progress"),
            "{name}: {err}"
        );
        let rejected = until(&mut env.events, "command_rejected").await;
        let Event::CommandRejected { command, .. } = rejected.event else {
            unreachable!()
        };
        assert_eq!(command, name);
    }
    // A model change is accepted and applies from the next turn.
    env.ctl
        .send(Command::SetModel {
            model: Some("test/b".into()),
            reasoning: Some("deep".into()),
        })
        .unwrap();
    let changed = until(&mut env.events, "model_changed").await;
    assert!(
        matches!(changed.event, Event::ModelChanged { ref applies, .. } if applies == "next_turn")
    );
    assert!(env
        .ctl
        .send(Command::SetModel {
            model: Some("test/zzz".into()),
            reasoning: None
        })
        .is_err());
    // Cancellation is immediate, and does not poison the next run.
    env.ctl.send(Command::Cancel).unwrap();
    let fin = until(&mut env.events, "run_finished").await;
    assert_eq!(outcome(&fin), RunOutcome::Cancelled);
    env.ctl.wait_idle().await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("encore"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    assert_eq!(outcome(evs.last().unwrap()), RunOutcome::Succeeded);
    let reqs = env.script.requests();
    assert_eq!(
        reqs.last().unwrap().0,
        "test/b",
        "the new model serves the next run"
    );
    assert_eq!(
        reqs.last()
            .unwrap()
            .1
            .options
            .get::<String>(cersei_provider::REASONING_PROFILE_OPTION)
            .as_deref(),
        Some("deep")
    );
    // The interrupted run left a history the next request could carry.
    let history = env.ctl.history().await;
    assert!(history.len() >= 3, "{}", history.len());
}

/// Nobody reads: the run slows down to the consumer (bounded queue), yet a
/// cancellation goes through, and every event is then delivered in order.
#[tokio::test]
async fn a_slow_consumer_neither_loses_events_nor_blocks_cancellation() {
    let calls: Vec<(String, String, serde_json::Value)> = (0..40)
        .map(|i| {
            (
                format!("g{i}"),
                "Glob".to_string(),
                json!({"pattern": "*.nothing"}),
            )
        })
        .collect();
    let many = Reply {
        tools: calls,
        ..Default::default()
    };
    let mut env = open_with(
        vec![
            many.clone(),
            many.clone(),
            many.clone(),
            many,
            Reply::text("fin"),
        ],
        true,
        ApprovalRules::default(),
        4,
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("beaucoup d'outils"),
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The run is held back by the full queue, not finished.
    assert!(matches!(env.ctl.activity(), Activity::Running { .. }));
    env.ctl.send(Command::Cancel).unwrap();
    tokio::time::timeout(Duration::from_secs(5), env.ctl.wait_idle())
        .await
        .expect("the cancellation went through while nobody read");
    let mut last = 0;
    let mut finished = Vec::new();
    while let Some(e) = env.events.try_next() {
        assert_eq!(e.seq, last + 1, "no gap");
        last = e.seq;
        if let Event::RunFinished { .. } = e.event {
            finished.push(e);
        }
    }
    assert_eq!(finished.len(), 1);
    assert_eq!(outcome(&finished[0]), RunOutcome::Cancelled);
}

/// Read the file (the engine refuses blind overwrites), then write it,
/// then answer (twice: the runner asks once for more after tool use).
fn read_then_write(path: &Path, content: &str) -> Vec<Reply> {
    vec![
        Reply::tool("r1", "Read", json!({"file_path": path.to_string_lossy()})),
        write_call(path, content),
        Reply::text("écrit"),
        Reply::text("fin"),
    ]
}

fn write_call(path: &Path, content: &str) -> Reply {
    Reply::tool(
        "w1",
        "Write",
        json!({"file_path": path.to_string_lossy(), "content": content}),
    )
}

#[tokio::test]
async fn an_approved_change_is_written_after_showing_its_diff() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("note.txt");
    std::fs::write(&target, "ancien\n").unwrap();
    let mut env = open(read_then_write(&target, "nouveau\n")).await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("écris"),
        })
        .unwrap();
    let req = until(&mut env.events, "approval_requested").await;
    let Event::ApprovalRequested { approval } = req.event else {
        unreachable!()
    };
    let preview = approval.preview.clone().expect("a preview");
    assert!(
        preview.files[0].diff.contains("-ancien") && preview.files[0].diff.contains("+nouveau")
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "ancien\n",
        "nothing written yet"
    );
    assert_eq!(env.ctl.snapshot().pending_approvals.len(), 1);
    env.ctl
        .send(Command::Approve {
            approval_id: approval.approval_id.clone(),
            decision: Decision::Allow,
            reason: None,
        })
        .unwrap();
    assert!(
        env.ctl
            .send(Command::Approve {
                approval_id: approval.approval_id,
                decision: Decision::Allow,
                reason: None
            })
            .is_err(),
        "an approval is answered once"
    );
    let evs = until_finished(&mut env.events).await;
    let k = kinds(&evs);
    assert!(
        k.contains(&"approval_resolved") && k.contains(&"edit_applied"),
        "{k:?}"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "nouveau\n");
}

#[tokio::test]
async fn a_rejected_change_is_not_written() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("note.txt");
    std::fs::write(&target, "à garder\n").unwrap();
    let mut env = open(read_then_write(&target, "écrasé\n")).await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("écris"),
        })
        .unwrap();
    let Event::ApprovalRequested { approval } =
        until(&mut env.events, "approval_requested").await.event
    else {
        unreachable!()
    };
    env.ctl
        .send(Command::Approve {
            approval_id: approval.approval_id,
            decision: Decision::Deny,
            reason: Some("non".into()),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    let finished_err = evs.iter().any(|e| matches!(&e.event, Event::ToolFinished { is_error: true, output, .. } if output.contains("non")));
    assert!(finished_err, "{:?}", kinds(&evs));
    assert!(!kinds(&evs).contains(&"edit_applied"));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "à garder\n",
        "the user's file is untouched"
    );
}

#[tokio::test]
async fn without_anyone_to_ask_the_run_stops_with_an_explicit_result() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("x.txt");
    let mut env = open_with(
        vec![write_call(&target, "x"), Reply::text("never")],
        false,
        ApprovalRules::default(),
        64,
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("écris"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    let Event::RunFinished {
        outcome: ended,
        failure,
        approvals_unsatisfied,
        ..
    } = &evs.last().unwrap().event
    else {
        unreachable!()
    };
    assert_eq!(*ended, RunOutcome::Failed);
    assert_eq!(*failure, Some(FailureKind::ApprovalRequired));
    assert_eq!(approvals_unsatisfied[0].tool, "Write");
    assert!(!target.exists());
    // Allowed by the policy: no approval needed, even non-interactive.
    let mut env = open_with(
        vec![write_call(&target, "x"), Reply::text("fait")],
        false,
        ApprovalRules::allow_all(),
        64,
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("écris"),
        })
        .unwrap();
    let evs = until_finished(&mut env.events).await;
    assert_eq!(outcome(evs.last().unwrap()), RunOutcome::Succeeded);
    assert!(target.exists());
}

#[tokio::test]
async fn a_change_approved_for_an_older_state_is_asked_again() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("lib.rs");
    std::fs::write(&target, "fn a() {}\nfn b() {}\n").unwrap();
    let edit = Reply::tool(
        "e1",
        "Edit",
        json!({"file_path": target.to_string_lossy(), "old_string": "fn a() {}", "new_string": "fn a() { 1 }"}),
    );
    let mut env = open(vec![
        Reply::tool("r1", "Read", json!({"file_path": target.to_string_lossy()})),
        edit,
        Reply::text("ok"),
        Reply::text("fin"),
    ])
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("modifie a"),
        })
        .unwrap();
    let Event::ApprovalRequested { approval: first } =
        until(&mut env.events, "approval_requested").await.event
    else {
        unreachable!()
    };
    // The user edits the file while the decision is pending.
    std::fs::write(&target, "fn a() {}\nfn b() { 2 }\n").unwrap();
    env.ctl
        .send(Command::Approve {
            approval_id: first.approval_id.clone(),
            decision: Decision::Allow,
            reason: None,
        })
        .unwrap();
    let Event::ApprovalRequested { approval: second } =
        until(&mut env.events, "approval_requested").await.event
    else {
        unreachable!()
    };
    assert_ne!(second.approval_id, first.approval_id);
    assert!(
        second.preview.as_ref().unwrap().files[0]
            .diff
            .contains("fn b() { 2 }"),
        "recomputed on the new state"
    );
    env.ctl
        .send(Command::Approve {
            approval_id: second.approval_id,
            decision: Decision::Allow,
            reason: None,
        })
        .unwrap();
    until_finished(&mut env.events).await;
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "fn a() { 1 }\nfn b() { 2 }\n",
        "both changes kept"
    );
}

#[tokio::test]
async fn a_session_resumes_with_its_settings_or_says_why_not() {
    // A first answer without tools gets one engine nudge, hence two replies.
    let mut env = open(vec![
        Reply::text("premier"),
        Reply::text("vraiment premier"),
    ])
    .await;
    env.ctl
        .send(Command::SetModel {
            model: Some("test/b".into()),
            reasoning: Some("fast".into()),
        })
        .unwrap();
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("Bonjour, projet été"),
        })
        .unwrap();
    until_finished(&mut env.events).await;
    env.ctl.wait_idle().await;
    let id = env.ctl.session_id();
    let sessions_dir = env.dir.path().join(".sessions");
    let listed = list_sessions(&sessions_dir).await.unwrap();
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].title, "Bonjour, projet été");
    assert_eq!(listed[0].model.as_deref(), Some("test/b"));

    let script = Script::new(vec![Reply::text("suite")]);
    let reopen = |names: &'static [&'static str], wd: &Path| {
        let mut bricks = BricksConfig::default();
        bricks.agent.model = Some("test/a".into());
        EngineConfig::new(
            wd,
            ScriptedCatalog::new(names, script.clone()),
            bricks,
            &sessions_dir,
        )
    };
    let resume = OpenOptions {
        session: SessionChoice::Resume(id.clone()),
        model: None,
        reasoning: None,
    };
    let other_dir = tempfile::tempdir().unwrap();
    let (ctl, mut events) = Controller::open(reopen(&["a", "b"], other_dir.path()), resume.clone())
        .await
        .unwrap();
    let Event::SessionOpened {
        resumed,
        model,
        reasoning,
        message_count,
        warnings,
        working_dir,
    } = next(&mut events).await.event
    else {
        unreachable!()
    };
    assert!(resumed);
    assert_eq!(
        (model.as_str(), reasoning.as_deref()),
        ("test/b", Some("fast"))
    );
    assert!(message_count >= 2);
    assert_eq!(
        working_dir,
        env.dir.path().display().to_string(),
        "its own directory"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("not in the current directory")),
        "{warnings:?}"
    );
    assert_eq!(
        ctl.history().await.len(),
        message_count,
        "the stored history"
    );
    drop(ctl);

    // A model that is no longer configured is not replaced silently.
    let err = Controller::open(reopen(&["a"], env.dir.path()), resume.clone())
        .await
        .err()
        .unwrap();
    assert!(
        err.contains("no longer configured") && err.contains("test/a"),
        "{err}"
    );
    // A working directory that disappeared is reported.
    let gone = tempfile::tempdir().unwrap();
    let mut meta = SessionMeta::load(&sessions_dir.join(format!("{id}.files")))
        .unwrap()
        .unwrap();
    meta.working_dir = gone.path().join("removed");
    meta.save(&sessions_dir.join(format!("{id}.files")))
        .unwrap();
    let err = Controller::open(reopen(&["a", "b"], env.dir.path()), resume)
        .await
        .err()
        .unwrap();
    assert!(err.contains("no longer exists"), "{err}");
    let err = Controller::open(
        reopen(&["a"], env.dir.path()),
        OpenOptions {
            session: SessionChoice::Resume("nope".into()),
            model: None,
            reasoning: None,
        },
    )
    .await
    .err()
    .unwrap();
    assert!(err.contains("no stored session"), "{err}");
}

#[tokio::test]
async fn a_new_session_needs_a_configured_model() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = EngineConfig::new(
        dir.path(),
        ScriptedCatalog::new(&["a"], Script::new(vec![])),
        BricksConfig::default(),
        dir.path().join("s"),
    );
    let opts = OpenOptions {
        session: SessionChoice::New,
        model: None,
        reasoning: None,
    };
    let err = Controller::open(cfg.clone(), opts.clone())
        .await
        .err()
        .unwrap();
    assert!(
        err.contains("no model selected") && err.contains("test/a"),
        "{err}"
    );
    let err = Controller::open(
        cfg,
        OpenOptions {
            model: Some("test/a".into()),
            reasoning: Some("turbo".into()),
            ..opts
        },
    )
    .await
    .err()
    .unwrap();
    assert!(err.contains("no reasoning profile `turbo`"), "{err}");
}

mod maintenance {
    use super::*;
    use async_trait::async_trait;
    use cersei_embeddings::HashingEmbeddings;
    use cersei_memory::structured::extract::{ExtractError, ExtractionRequest, Extractor};
    use cersei_memory::structured::{MemoryConfig, StructuredMemory};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    /// Blocks (until cancelled) while `hold` is set; counts the calls that
    /// completed.
    struct Gate {
        hold: AtomicBool,
        completed: AtomicUsize,
    }

    #[async_trait]
    impl Extractor for Gate {
        fn id(&self) -> String {
            "gate/test".into()
        }
        async fn extract(
            &self,
            _r: &ExtractionRequest,
            cancel: &CancellationToken,
        ) -> Result<String, ExtractError> {
            if self.hold.load(Ordering::SeqCst) {
                cancel.cancelled().await;
                return Err(ExtractError::Cancelled);
            }
            self.completed.fetch_add(1, Ordering::SeqCst);
            Ok(r#"{"entities":[],"facts":[]}"#.into())
        }
    }

    #[tokio::test]
    async fn maintenance_is_its_own_cancellable_phase_and_resumes_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(Gate {
            hold: AtomicBool::new(true),
            completed: AtomicUsize::new(0),
        });
        let memory = Arc::new(
            StructuredMemory::builder(Arc::new(HashingEmbeddings::new(64)))
                .path(dir.path().join("m.grafeo"))
                .config(MemoryConfig::default().with_space("project:t"))
                .extractor(gate.clone())
                .open()
                .unwrap(),
        );
        let script = Script::new(vec![
            Reply::text("réponse"),
            Reply::text("réponse"),
            Reply::text("deux"),
            Reply::text("deux"),
        ]);
        let mut bricks = BricksConfig::default();
        bricks.agent.model = Some("test/a".into());
        let mut cfg = EngineConfig::new(
            dir.path(),
            ScriptedCatalog::new(&["a"], script),
            bricks,
            dir.path().join("s"),
        );
        cfg.long_term_memory = Some(memory.clone());
        cfg.memory_space = Some("project:t".into());
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
        ctl.send(Command::Submit {
            prompt: Prompt::text("retiens ceci"),
        })
        .unwrap();
        let fin = until(&mut events, "run_finished").await;
        assert_eq!(
            outcome(&fin),
            RunOutcome::Succeeded,
            "the answer is delivered first"
        );
        until(&mut events, "memory_maintenance_started").await;
        // The next prompt is not blocked by the maintenance.
        ctl.wait_idle().await;
        assert!(ctl.snapshot().maintenance_running);
        // Cancel goes to the maintenance when no run is active.
        ctl.send(Command::Cancel).unwrap();
        let Event::MemoryMaintenanceFinished {
            outcome: o, report, ..
        } = until(&mut events, "memory_maintenance_finished")
            .await
            .event
        else {
            unreachable!()
        };
        assert_eq!(o, MaintenanceOutcome::Cancelled);
        let report = report.unwrap();
        assert!(report.cancelled && report.pending > 0, "{report:?}");
        assert_eq!(gate.completed.load(Ordering::SeqCst), 0);

        // Next run: the pending episodes are processed once, with the new ones.
        gate.hold.store(false, Ordering::SeqCst);
        ctl.send(Command::Submit {
            prompt: Prompt::text("deuxième"),
        })
        .unwrap();
        let Event::MemoryMaintenanceFinished {
            outcome: o, report, ..
        } = until(&mut events, "memory_maintenance_finished")
            .await
            .event
        else {
            unreachable!()
        };
        assert_eq!(o, MaintenanceOutcome::Completed);
        let report = report.unwrap();
        assert_eq!(report.pending, 0, "{report:?}");
        let calls = gate.completed.load(Ordering::SeqCst);
        assert_eq!(calls, report.extracted);
        assert_eq!(
            calls,
            memory.stats().episodes,
            "each episode extracted exactly once"
        );
        // A further pass has nothing left: no extraction is repeated.
        let again = ctl
            .agent()
            .maintain_memory(&CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!((again.extracted, again.pending), (0, 0));
        assert_eq!(gate.completed.load(Ordering::SeqCst), calls);
    }
}
