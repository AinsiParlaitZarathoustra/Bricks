//! A session's project follows the session: resuming a session of another
//! folder loads that folder's project (rules, prompt, semantic settings)
//! before anything is switched; a failure leaves the open session intact.

use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::*;
use cersei_agent::BricksConfig;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A project, read from its folder: `write_allowed` allows writes,
/// `broken` makes it fail; the prompt and the semantic limits name it.
struct Folders;

impl ProjectLoader for Folders {
    fn load(&self, wd: &Path) -> Result<ProjectContext, String> {
        if wd.join("broken").exists() {
            return Err(format!("broken project in {}", wd.display()));
        }
        let name = wd.file_name().unwrap().to_string_lossy().into_owned();
        let mut bricks = BricksConfig::default();
        bricks.agent.model = Some("test/a".into());
        bricks.semantic.lsp.enabled = false;
        bricks.semantic.max_results = if name == "B" { 7 } else { 9 };
        if wd.join("exec_allowed").exists() {
            bricks.permissions.execute = Action::Allow;
        }
        bricks.permissions.write = if wd.join("write_allowed").exists() {
            Action::Allow
        } else {
            Action::Ask
        };
        Ok(ProjectContext {
            working_dir: wd.to_path_buf(),
            bricks,
            system_prompt: Some(format!("PROJECT-{name}")),
            long_term_memory: None,
            memory_space: None,
            mcp_servers: Vec::new(),
            agent_profile_sources: None,
        })
    }
}

struct World {
    root: tempfile::TempDir,
    script: Arc<Script>,
}

impl World {
    fn new(replies: Vec<Reply>) -> Self {
        let root = tempfile::tempdir().unwrap();
        for d in ["A", "B"] {
            std::fs::create_dir(root.path().join(d)).unwrap();
        }
        World {
            root,
            script: Script::new(replies),
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.path().join(name).canonicalize().unwrap()
    }

    async fn open(
        &self,
        folder: &str,
        session: SessionChoice,
    ) -> Result<(Controller, EventStream), String> {
        let catalog = ScriptedCatalog::new(&["a", "b"], self.script.clone());
        let mut cfg = EngineConfig::new(
            self.dir(folder),
            catalog,
            BricksConfig::default(),
            self.root.path().join("sessions"),
        );
        cfg.project_loader = Some(Arc::new(Folders));
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
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .expect("an event within 10 s")
        .expect("stream open")
}

async fn until(events: &mut EventStream, kind: &str) -> Envelope {
    loop {
        let e = next(events).await;
        if e.event.kind() == kind {
            return e;
        }
    }
}

async fn submit(ctl: &Controller, events: &mut EventStream, text: &str) -> Vec<Envelope> {
    ctl.send(Command::Submit {
        prompt: Prompt::text(text),
    })
    .unwrap();
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

/// A stored session of folder `name`.
async fn session_of(w: &World, name: &str) -> String {
    let (ctl, mut ev) = w.open(name, SessionChoice::New).await.unwrap();
    next(&mut ev).await;
    submit(&ctl, &mut ev, "hello").await;
    let id = ctl.session_id();
    ctl.close().await;
    id
}

#[tokio::test]
async fn resuming_another_projects_session_loads_that_project() {
    let w = World::new(vec![
        Reply::text("B created"),
        // In A: a write allowed for the session.
        Reply::tool("w0", "Write", json!({"file_path": "a.txt", "content": "a"})),
        Reply::text("A wrote"),
        // After the resume, in B.
        Reply::tool(
            "w1",
            "Write",
            json!({"file_path": "rel.txt", "content": "b"}),
        ),
        Reply::text("B wrote"),
    ]);
    std::fs::write(w.dir("B").join("write_allowed"), "").unwrap();
    let id_b = session_of(&w, "B").await;

    let (ctl, mut ev) = w.open("A", SessionChoice::New).await.unwrap();
    next(&mut ev).await;
    assert_eq!(ctl.project().system_prompt.as_deref(), Some("PROJECT-A"));
    // A asks before writing; allowed for the session.
    ctl.send(Command::Submit {
        prompt: Prompt::text("write"),
    })
    .unwrap();
    let Event::ApprovalRequested { approval } = until(&mut ev, "approval_requested").await.event
    else {
        unreachable!()
    };
    ctl.send(Command::Approve {
        approval_id: approval.approval_id.clone(),
        decision: Decision::AllowForSession,
        reason: None,
    })
    .unwrap();
    until(&mut ev, "run_finished").await;
    assert_eq!(
        ctl.snapshot().allowed_for_session,
        vec!["Write".to_string()]
    );
    let id_a = ctl.session_id();

    // The person resumes B (the interface's /resume).
    ctl.send(Command::Resume {
        session_id: id_b.clone(),
    })
    .unwrap();
    let Event::SessionOpened {
        working_dir,
        resumed,
        ..
    } = until(&mut ev, "session_opened").await.event
    else {
        unreachable!()
    };
    assert!(resumed);
    let b = w.dir("B");
    assert_eq!(working_dir, b.display().to_string());
    let snap = ctl.snapshot();
    assert_eq!(snap.session_id, id_b);
    assert_eq!(snap.working_dir, b);
    assert_eq!(snap.approval_rules.write, Action::Allow, "B's rules");
    assert!(
        snap.allowed_for_session.is_empty(),
        "A's session approvals stay with A"
    );
    assert_eq!(ctl.project().system_prompt.as_deref(), Some("PROJECT-B"));
    assert_eq!(
        ctl.semantic_engine().config().max_results,
        7,
        "B's semantic settings"
    );
    assert_eq!(ctl.agent().working_dir(), b.as_path());

    // A write in B: no question (B allows it), relative to B.
    let evs = submit(&ctl, &mut ev, "write").await;
    assert!(!evs.iter().any(|e| e.event.kind() == "approval_requested"));
    assert_eq!(std::fs::read_to_string(b.join("rel.txt")).unwrap(), "b");
    assert!(!w.dir("A").join("rel.txt").exists());
    let reqs = w.script.requests();
    let sys = reqs.last().unwrap().1.system.clone().unwrap_or_default();
    assert!(
        sys.contains("PROJECT-B") && !sys.contains("PROJECT-A"),
        "{sys}"
    );
    // A's session is still A's.
    assert_ne!(id_a, id_b);
}

#[tokio::test]
async fn a_resume_that_cannot_be_prepared_changes_nothing() {
    let w = World::new(vec![Reply::text("B created")]);
    let id_b = session_of(&w, "B").await;
    let (ctl, mut ev) = w.open("A", SessionChoice::New).await.unwrap();
    next(&mut ev).await;
    let before = ctl.snapshot();

    // B's project cannot be read.
    std::fs::write(w.dir("B").join("broken"), "").unwrap();
    ctl.send(Command::Resume {
        session_id: id_b.clone(),
    })
    .unwrap();
    let e = next(&mut ev).await;
    let Event::CommandRejected { command, reason } = e.event else {
        panic!("{:?}", e.event)
    };
    assert_eq!(command, "resume");
    assert!(reason.contains("broken project"), "{reason}");
    let after = ctl.snapshot();
    assert_eq!(
        (after.session_id, after.working_dir),
        (before.session_id.clone(), before.working_dir.clone())
    );
    assert_eq!(ctl.project().system_prompt.as_deref(), Some("PROJECT-A"));

    // B's folder is gone.
    std::fs::remove_dir_all(w.dir("B")).unwrap();
    ctl.send(Command::Resume { session_id: id_b }).unwrap();
    let Event::CommandRejected { reason, .. } = next(&mut ev).await.event else {
        panic!()
    };
    assert!(reason.contains("no longer exists"), "{reason}");
    assert_eq!(ctl.snapshot().session_id, before.session_id);
    // Nothing else was published (no session_opened).
    assert!(ev.try_next().is_none());
}

#[tokio::test]
async fn codescout_stays_in_the_resumed_projects_folder() {
    let w = World::new(vec![
        Reply::text("B created"),
        Reply::tools(vec![
            (
                "s1",
                "CodeScout",
                json!({"query": "SECRET_A", "paths": ["../A"]}),
            ),
            (
                "s2",
                "CodeScout",
                json!({"query": "SECRET_A", "paths": ["/"]}),
            ),
            ("s3", "CodeScout", json!({"query": "SECRET_A"})),
            (
                "s4",
                "CodeScout",
                json!({"query": "SECRET_A", "paths": ["link"]}),
            ),
            ("s5", "CodeScout", json!({"query": "VISIBLE_B"})),
        ]),
        Reply::text("done"),
    ]);
    std::fs::write(w.dir("A").join("secret.rs"), "const SECRET_A: u8 = 1;\n").unwrap();
    std::fs::write(w.dir("B").join("lib.rs"), "const VISIBLE_B: u8 = 2;\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(w.dir("A"), w.dir("B").join("link")).unwrap();
    let id_b = session_of(&w, "B").await;
    let (ctl, mut ev) = w.open("A", SessionChoice::New).await.unwrap();
    next(&mut ev).await;
    ctl.send(Command::Resume { session_id: id_b }).unwrap();
    until(&mut ev, "session_opened").await;
    let evs = submit(&ctl, &mut ev, "search").await;
    let out = |id: &str| {
        evs.iter()
            .find_map(|e| match &e.event {
                Event::ToolFinished {
                    tool_call_id,
                    output,
                    ..
                } if tool_call_id == id => Some(output.clone()),
                _ => None,
            })
            .unwrap()
    };
    for id in ["s1", "s2"] {
        assert!(out(id).contains("outside your scope"), "{id}: {}", out(id));
    }
    for id in ["s1", "s2", "s3", "s4"] {
        assert!(
            !out(id).contains("secret.rs"),
            "{id} reached A: {}",
            out(id)
        );
    }
    assert!(out("s5").contains("lib.rs"), "{}", out("s5"));
}

#[cfg(unix)]
#[tokio::test]
async fn the_previous_sessions_jobs_end_with_the_resume() {
    let w = World::new(vec![
        Reply::text("B created"),
        Reply::tool(
            "b1",
            "Bash",
            json!({"command": "sleep 300", "background": true}),
        ),
        Reply::text("started"),
    ]);
    std::fs::write(w.dir("A").join("exec_allowed"), "").unwrap();
    let id_b = session_of(&w, "B").await;
    let (ctl, mut ev) = w.open("A", SessionChoice::New).await.unwrap();
    next(&mut ev).await;
    let evs = submit(&ctl, &mut ev, "start a job").await;
    let pid = evs
        .iter()
        .find_map(|e| match &e.event {
            Event::JobStarted { pid, .. } => Some(*pid),
            _ => None,
        })
        .expect("a job");
    assert!(cersei_tools::shell::procs::is_running(pid));
    ctl.send(Command::Resume {
        session_id: id_b.clone(),
    })
    .unwrap();
    until(&mut ev, "session_opened").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !cersei_tools::shell::procs::is_running(pid),
        "A's job stopped"
    );
    // Nothing of A's job reports as B's session.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Some(e) = ev.try_next() {
        assert!(!e.event.kind().starts_with("job_"), "{:?}", e.event);
    }
}
