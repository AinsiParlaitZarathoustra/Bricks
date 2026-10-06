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
    assert!(
        s.contains("cost unknown (no price)"),
        "unknown, not zero: {s}"
    );
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
    let mut p = Overlay::Picker {
        title: "model".into(),
        items: vec![
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
        filter: String::new(),
        selected: 0,
        action: overlay::PickAction::Model,
    };
    if let Overlay::Picker { filter, .. } = &mut p {
        filter.push_str("oth");
    }
    assert_eq!(p.visible().len(), 1);
    term.draw(|f| overlay::draw(f, &p)).unwrap();
    assert!(screen(&term).contains("other/x"));
}
