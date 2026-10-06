//! Gestures → engine commands, against a scripted engine; and the same
//! agent through the interface and through a direct (headless) submission.

use bricks_tui::state::Cell;
use bricks_tui::ui::{Effect, Ui};
use cersei_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use cersei_agent::control::*;
use cersei_agent::BricksConfig;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typed(ui: &mut Ui, text: &str) {
    for c in text.chars() {
        assert!(ui.on_key(key(KeyCode::Char(c))).is_empty());
    }
}

async fn open(
    dir: &std::path::Path,
    replies: Vec<Reply>,
    rules: ApprovalRules,
) -> (Controller, EventStream) {
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.permissions = rules;
    let cfg = EngineConfig::new(
        dir,
        ScriptedCatalog::new(&["a", "b"], Script::new(replies)),
        bricks,
        dir.join(".s"),
    );
    Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap()
}

async fn execute(ctl: &Controller, effects: Vec<Effect>) {
    for e in effects {
        if let Effect::Send(c) = e {
            ctl.send(c).unwrap();
        }
    }
}

/// Feed events to the interface until the run ends; return all envelopes.
async fn pump(ui: &mut Ui, events: &mut EventStream) -> Vec<Envelope> {
    let mut all = Vec::new();
    loop {
        let e = tokio::time::timeout(Duration::from_secs(10), events.next())
            .await
            .unwrap()
            .unwrap();
        ui.on_event(&e);
        let done = matches!(e.event, Event::RunFinished { .. });
        all.push(e);
        if done {
            return all;
        }
    }
}

fn script() -> Vec<Reply> {
    vec![
        Reply::tool("c1", "Glob", serde_json::json!({"pattern": "*.md"})),
        Reply::chunks(&["Trouvé ", "le README."]),
        Reply::text("Réponse finale."),
    ]
}

#[tokio::test]
async fn the_interface_and_a_direct_submission_drive_the_same_engine_alike() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "# x").unwrap();

    // Through the interface: typed, then Enter.
    let (ctl, mut events) = open(dir.path(), script(), ApprovalRules::default()).await;
    let mut ui = Ui::new(dir.path());
    ui.on_event(&events.next().await.unwrap());
    typed(&mut ui, "Trouve le README");
    let effects = ui.on_key(key(KeyCode::Enter));
    assert_eq!(
        effects,
        vec![Effect::Send(Command::Submit {
            prompt: Prompt::text("Trouve le README")
        })]
    );
    execute(&ctl, effects).await;
    let via_ui = pump(&mut ui, &mut events).await;

    // Directly, as `bricks run` does.
    let (ctl2, mut events2) = open(dir.path(), script(), ApprovalRules::default()).await;
    events2.next().await.unwrap();
    ctl2.send(Command::Submit {
        prompt: Prompt::text("Trouve le README"),
    })
    .unwrap();
    let mut direct = Vec::new();
    loop {
        let e = events2.next().await.unwrap();
        let done = matches!(e.event, Event::RunFinished { .. });
        direct.push(e);
        if done {
            break;
        }
    }
    let kinds = |v: &[Envelope]| v.iter().map(|e| e.event.kind()).collect::<Vec<_>>();
    assert_eq!(kinds(&via_ui), kinds(&direct));
    let text = |v: &[Envelope]| match &v.last().unwrap().event {
        Event::RunFinished { text, outcome, .. } => (text.clone(), *outcome),
        _ => unreachable!(),
    };
    assert_eq!(text(&via_ui), text(&direct));
    assert_eq!(text(&via_ui).0, "Réponse finale.");
    // The presentation state shows what the engine reported.
    assert!(ui
        .app
        .cells
        .iter()
        .any(|c| matches!(c, Cell::Tools { calls } if calls[0].name == "Glob")));
    assert!(!ui.app.running());
}

#[tokio::test]
async fn paste_never_submits_and_a_run_in_progress_keeps_the_text() {
    let dir = tempfile::tempdir().unwrap();
    let (ctl, mut events) = open(dir.path(), vec![Reply::hang()], ApprovalRules::default()).await;
    let mut ui = Ui::new(dir.path());
    ui.on_event(&events.next().await.unwrap());
    ui.on_paste("ligne 1\nligne 2\n");
    assert_eq!(ui.composer.text(), "ligne 1\nligne 2\n", "pasted, not sent");
    typed(&mut ui, "é👍");
    ui.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    typed(&mut ui, "fin");
    let effects = ui.on_key(key(KeyCode::Enter));
    let Effect::Send(Command::Submit { prompt }) = &effects[0] else {
        panic!("{effects:?}")
    };
    assert_eq!(
        prompt.blocks,
        vec![PromptBlock::Text {
            text: "ligne 1\nligne 2\né👍\nfin".into()
        }]
    );
    execute(&ctl, effects).await;
    // Wait for the run to start.
    loop {
        let e = events.next().await.unwrap();
        ui.on_event(&e);
        if matches!(e.event, Event::RunStarted { .. }) {
            break;
        }
    }
    typed(&mut ui, "suivant");
    assert!(
        ui.on_key(key(KeyCode::Enter)).is_empty(),
        "not started in parallel"
    );
    assert_eq!(ui.composer.text(), "suivant", "kept, still editable");
    assert!(ui.message.as_deref().unwrap().contains("in progress"));
    // Ctrl+C reaches the engine.
    let effects = ui.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(effects, vec![Effect::Send(Command::Cancel)]);
    execute(&ctl, effects).await;
    let rest = pump(&mut ui, &mut events).await;
    assert!(matches!(
        rest.last().unwrap().event,
        Event::RunFinished {
            outcome: RunOutcome::Cancelled,
            ..
        }
    ));
}

#[tokio::test]
async fn mentions_and_slash_commands_become_attachments_and_commands() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/dossier avec espaces")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let (ctl, mut events) = open(
        dir.path(),
        vec![Reply::text("vu"), Reply::text("vu")],
        ApprovalRules::default(),
    )
    .await;
    let mut ui = Ui::new(dir.path());
    ui.on_event(&events.next().await.unwrap());
    typed(&mut ui, "Lis @smain");
    let (labels, _) = ui.popup_labels().expect("the file list is open");
    assert_eq!(labels[0], "src/main.rs");
    ui.on_key(key(KeyCode::Tab));
    assert_eq!(ui.composer.text(), "Lis ");
    // A file created afterwards appears once the index is refreshed.
    std::fs::write(dir.path().join("src/nouveau.rs"), "").unwrap();
    typed(&mut ui, "@\"dossier av");
    let (labels, _) = ui.popup_labels().unwrap();
    assert!(
        labels[0].starts_with("src/dossier avec espaces"),
        "{labels:?}"
    );
    ui.on_key(key(KeyCode::Enter));
    typed(&mut ui, "@nouveau");
    assert!(
        ui.popup_labels().unwrap().0.is_empty()
            || ui.popup_labels().unwrap().0[0] != "src/nouveau.rs"
    );
    ui.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_eq!(
        ui.popup_labels().unwrap().0[0],
        "src/nouveau.rs",
        "refreshed"
    );
    ui.on_key(key(KeyCode::Esc));
    for _ in 0.."@nouveau".len() {
        ui.on_key(key(KeyCode::Backspace));
    }
    let effects = ui.on_key(key(KeyCode::Enter));
    let Effect::Send(Command::Submit { prompt }) = &effects[0] else {
        panic!("{effects:?}")
    };
    assert_eq!(
        prompt.blocks,
        vec![
            PromptBlock::Text {
                text: "Lis ".into()
            },
            PromptBlock::File {
                path: "src/main.rs".into()
            },
            PromptBlock::Folder {
                path: "src/dossier avec espaces".into()
            },
        ]
    );
    execute(&ctl, effects).await;
    let evs = pump(&mut ui, &mut events).await;
    let Event::RunStarted { attachments, .. } = &evs[0].event else {
        panic!()
    };
    assert_eq!(attachments.len(), 2);

    // Slash commands: completion, engine commands, presentation actions.
    typed(&mut ui, "/mo");
    let (labels, _) = ui.popup_labels().unwrap();
    assert!(labels[0].starts_with("/model"));
    typed(&mut ui, "del test/b deep");
    let effects = ui.on_key(key(KeyCode::Enter));
    assert_eq!(
        effects,
        vec![Effect::Send(Command::SetModel {
            model: Some("test/b".into()),
            reasoning: Some("deep".into())
        })]
    );
    typed(&mut ui, "/context");
    let effects = ui.on_key(key(KeyCode::Enter));
    assert!(matches!(
        effects[0],
        Effect::Present(bricks_tui::commands::Present::Context)
    ));
    typed(&mut ui, "/nope");
    assert!(ui.on_key(key(KeyCode::Enter)).is_empty());
    assert!(ui.message.as_deref().unwrap().contains("unknown command"));
}

#[tokio::test]
async fn approvals_are_answered_from_the_keyboard() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("créé.txt");
    let replies = vec![
        Reply::tool(
            "w1",
            "Write",
            serde_json::json!({"file_path": target.to_string_lossy(), "content": "ok\n"}),
        ),
        Reply::text("écrit"),
        Reply::text("fin"),
    ];
    let (ctl, mut events) = open(dir.path(), replies, ApprovalRules::default()).await;
    let mut ui = Ui::new(dir.path());
    ui.on_event(&events.next().await.unwrap());
    typed(&mut ui, "écris");
    execute(&ctl, ui.on_key(key(KeyCode::Enter))).await;
    loop {
        let e = events.next().await.unwrap();
        ui.on_event(&e);
        if matches!(e.event, Event::ApprovalRequested { .. }) {
            break;
        }
    }
    assert!(ui.focus_approval);
    // 'd' opens the diff, Esc closes it; 'y' approves.
    assert!(ui.on_key(key(KeyCode::Char('d'))).is_empty());
    assert!(ui.overlay.is_some());
    ui.on_key(key(KeyCode::Esc));
    let effects = ui.on_key(key(KeyCode::Char('y')));
    assert!(matches!(
        &effects[0],
        Effect::Send(Command::Approve {
            decision: Decision::Allow,
            ..
        })
    ));
    execute(&ctl, effects).await;
    let evs = pump(&mut ui, &mut events).await;
    assert!(evs.iter().any(|e| e.event.kind() == "edit_applied"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ok\n");
    assert!(!ui.focus_approval);
    assert_eq!(ui.app.applied.len(), 1);
}
