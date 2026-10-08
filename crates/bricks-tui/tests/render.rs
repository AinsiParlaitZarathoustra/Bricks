//! Rendering with ratatui's TestBackend: the live area, the status bar, an
//! approval, a small terminal, inspectors.

use bricks_tui::composer::Composer;
use bricks_tui::overlay::{self, Overlay};
use bricks_tui::state::App;
use bricks_tui::view::{self, Overlay as LiveOverlay};
use cersei_agent::control::*;
use cersei_tools::preview::{file_change, ChangePreview};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn env(seq: u64, event: Event) -> Envelope {
    Envelope {
        schema: SCHEMA_VERSION,
        session_id: "s1".into(),
        run_id: Some("r1".into()),
        seq,
        at: 0,
        event,
    }
}

fn screen(term: &Terminal<TestBackend>) -> String {
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn live(
    app: &App,
    composer: &Composer,
    w: u16,
    h: u16,
    popup: Option<(&[String], usize, &str)>,
) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let o = LiveOverlay {
        popup,
        spinner: "⠋",
        maintenance: false,
        show_thinking: false,
        focus_approval: false,
        message: None,
        hyperlinks: false,
    };
    term.draw(|f| view::draw_live(f, app, composer, &o))
        .unwrap();
    screen(&term)
}

#[test]
fn the_live_area_shows_progress_status_and_composer() {
    let mut app = App::new();
    app.apply(&env(
        1,
        Event::SessionOpened {
            working_dir: "/p".into(),
            model: "local/m".into(),
            reasoning: Some("deep".into()),
            resumed: false,
            message_count: 0,
            warnings: vec![],
        },
    ));
    app.apply(&env(
        2,
        Event::RunStarted {
            prompt: "Analyse".into(),
            attachments: vec![],
            model: "local/m".into(),
            reasoning: Some("deep".into()),
        },
    ));
    app.apply(&env(
        3,
        Event::ToolStarted {
            tool_call_id: "c1".into(),
            name: "Grep".into(),
            input: serde_json::json!({"pattern": "fn main"}),
        },
    ));
    app.apply(&env(
        4,
        Event::Usage {
            turn: Box::new(cersei_types::Usage {
                input_tokens: 10,
                output_tokens: 2,
                ..Default::default()
            }),
            total: Box::new(cersei_types::Usage {
                input_tokens: 10,
                output_tokens: 2,
                ..Default::default()
            }),
        },
    ));
    let mut composer = Composer::new();
    composer.insert_str("prochain prompt é漢");
    let s = live(&app, &composer, 70, 12, None);
    assert!(s.contains("Grep fn main"), "{s}");
    assert!(s.contains("local/m · deep"), "{s}");
    assert!(s.contains("Not priced"), "unknown, not zero: {s}");
    assert!(!s.contains("cost unknown") && !s.contains("$0.0000"), "{s}");
    assert!(s.contains("Ctrl+C to cancel"), "{s}");
    assert!(s.contains("prochain prompt é漢"), "{s}");

    // Small terminal: no panic, a clear message.
    let s = live(&app, &composer, 15, 3, None);
    assert!(s.contains("terminal too"), "{s}");
}

#[test]
fn an_approval_and_a_completion_list_are_visible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "x\n").unwrap();
    let preview = ChangePreview {
        files: vec![file_change("a.txt", &path, Some("x\n"), Some("y\n"))],
        refusal: None,
    };
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
        Event::ApprovalRequested {
            approval: ApprovalRequest {
                approval_id: "ap_1".into(),
                tool_call_id: "c1".into(),
                tool: "Write".into(),
                level: "write".into(),
                description: "Execute tool 'Write'".into(),
                input: serde_json::json!({}),
                preview: Some(preview),
                agent_id: None,
            },
        },
    ));
    let items = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
    let s = live(&app, &Composer::new(), 90, 16, Some((&items, 1, "files")));
    assert!(s.contains("[y] allow"), "{s}");
    assert!(s.contains("(+1 −1)"), "{s}");
    assert!(s.contains("src/lib.rs"), "{s}");
}

#[test]
fn inspectors_scroll_and_pickers_filter() {
    let mut term = Terminal::new(TestBackend::new(60, 10)).unwrap();
    let mut o = Overlay::plain(
        "help",
        &(0..100)
            .map(|i| format!("ligne {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    o.scroll_by(50, 7);
    term.draw(|f| overlay::draw(f, &o)).unwrap();
    let s = screen(&term);
    assert!(s.contains("ligne 50") && !s.contains("ligne 49 "), "{s}");
    let mut p = Overlay::picker(
        "model",
        vec![
            overlay::PickItem {
                label: "local/m".into(),
                detail: "Test".into(),
                value: Some("local/m".into()),
            },
            overlay::PickItem {
                label: "other/x".into(),
                detail: "X".into(),
                value: Some("other/x".into()),
            },
        ],
        overlay::PickAction::Model,
    );
    if let Overlay::Picker { filter, .. } = &mut p {
        filter.push_str("oth");
    }
    assert_eq!(p.visible().len(), 1);
    term.draw(|f| overlay::draw(f, &p)).unwrap();
    assert!(screen(&term).contains("other/x"));
}

fn lines_of(cell: &bricks_tui::state::Cell) -> String {
    view::cell_lines(cell, false)
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn durations_are_milliseconds_and_a_sub_agent_has_its_own_cell() {
    use cersei_agent::agents::{AgentResult, Choice, InstanceState, SpawnInfo};
    let mut app = App::new();
    app.apply(&env(
        1,
        Event::RunStarted {
            prompt: "p".into(),
            attachments: vec![],
            model: "m".into(),
            reasoning: None,
        },
    ));
    for (i, ms) in [0u64, 100, 12_345].into_iter().enumerate() {
        let id = format!("t{i}");
        app.apply(&env(
            2 + i as u64 * 2,
            Event::ToolStarted {
                tool_call_id: id.clone(),
                name: "Grep".into(),
                input: serde_json::json!({"pattern": "process_payment"}),
            },
        ));
        app.apply(&env(
            3 + i as u64 * 2,
            Event::ToolFinished {
                tool_call_id: id,
                name: "Grep".into(),
                is_error: false,
                duration_ms: ms,
                output: String::new(),
            },
        ));
    }
    let tools = lines_of(
        app.cells
            .iter()
            .find(|c| matches!(c, bricks_tui::state::Cell::Tools { .. }))
            .unwrap(),
    );
    assert!(tools.contains("process_payment  0 ms"), "{tools}");
    assert!(tools.contains("  100 ms"), "{tools}");
    assert!(tools.contains("  12345 ms"), "{tools}");
    for old in ["0.0s", "0.1s", "12.3s", "12.35s"] {
        assert!(!tools.contains(old), "{tools}");
    }

    let info = SpawnInfo {
        agent_id: "agent_1".into(),
        parent_id: Some("main_1".into()),
        root_run_id: "run_1".into(),
        tool_call_id: None,
        profile: "inspecteur".into(),
        profile_source: "builtin:inspecteur.md".into(),
        profile_revision: "abc".into(),
        model: Choice {
            requested: "inherit".into(),
            applied: "p/m".into(),
            reason: None,
        },
        reasoning: Choice {
            requested: "high".into(),
            applied: "high".into(),
            reason: None,
        },
        workspace: "/w".into(),
        isolation: "shared".into(),
        task: "Find parse".into(),
        description: None,
        created_at: "2026-10-07T00:00:00Z".into(),
        batch_index: None,
        background: false,
        depth: 1,
        branch: None,
        skills: vec![],
    };
    app.apply(&env(
        20,
        Event::AgentSpawned {
            agent: Box::new(info),
        },
    ));
    app.apply(&env(
        21,
        Event::AgentState {
            agent_id: "agent_1".into(),
            state: InstanceState::Running,
            reason: None,
        },
    ));
    app.apply(&env(
        22,
        Event::AgentToolStarted {
            agent_id: "agent_1".into(),
            tool_call_id: "c1".into(),
            name: "CodeScout".into(),
            input: serde_json::json!({"query": "process_payment"}),
        },
    ));
    let running = lines_of(app.cells.last().unwrap());
    assert!(
        running.contains("agent inspecteur · p/m · high · running"),
        "{running}"
    );
    assert!(running.contains("ms so far"), "{running}");
    app.apply(&env(
        23,
        Event::AgentToolFinished {
            agent_id: "agent_1".into(),
            tool_call_id: "c1".into(),
            name: "CodeScout".into(),
            is_error: false,
            duration_ms: 47,
        },
    ));
    app.apply(&env(
        24,
        Event::AgentFinished {
            result: Box::new(AgentResult {
                agent_id: "agent_1".into(),
                profile: "inspecteur".into(),
                status: "completed".into(),
                termination: None,
                error: None,
                summary: "ok".into(),
                summary_truncated: false,
                files_changed: vec!["src/a.rs".into()],
                commands: vec![],
                warnings: vec![],
                turns: 3,
                tool_calls: 1,
                usage: Default::default(),
                duration_ms: 12_345,
                model: "p/m".into(),
                reasoning: Some("high".into()),
                workspace: "/w".into(),
                transcript: None,
                changeset: None,
                branch: None,
                skills: Vec::new(),
            }),
        },
    ));
    app.apply(&env(
        25,
        Event::AgentState {
            agent_id: "agent_1".into(),
            state: InstanceState::Completed,
            reason: None,
        },
    ));
    let done = lines_of(app.cells.last().unwrap());
    assert!(done.contains("CodeScout process_payment  47 ms"), "{done}");
    assert!(
        done.contains("completed in 12345 ms · 3 turn(s) · files: src/a.rs"),
        "{done}"
    );
    assert!(!done.contains("so far"), "{done}");
}
