//! The status bar at real widths, through Ratatui's test backend: what is
//! kept as the terminal narrows, Unicode widths, activity and `Ctrl+C`
//! during a run, the `+N −N` line above it, no panic at extreme sizes.

use bricks_tui::composer::Composer;
use bricks_tui::state::App;
use bricks_tui::statusbar;
use bricks_tui::view::{self, Overlay as LiveOverlay};
use cersei_agent::control::*;
use cersei_tools::preview::ChangeKind;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

const GREEN: Color = Color::Rgb(152, 205, 170);
const RED: Color = Color::Rgb(224, 153, 153);

fn env(seq: u64, event: Event) -> Envelope {
    Envelope {
        schema: SCHEMA_VERSION,
        session_id: "s".into(),
        run_id: Some("r".into()),
        seq,
        at: 0,
        event,
    }
}

fn context(used: u64, limit: u64, provenance: &str) -> cersei_agent::context::ContextStatus {
    serde_json::from_value(serde_json::json!({
        "context_used": {"tokens": used, "upper_bound": used, "measured_tokens": 0,
            "estimated_tokens": used, "provenance": provenance, "method": "local"},
        "context_window": {"model": "local/m", "total": 1_000_000, "max_input_tokens": limit, "max_output_tokens": 16384},
        "input_limit": limit, "reserved_output": 16384, "margin": 1000, "total_tokens": 0,
        "totals": {"input_tokens": 0, "cache_read_tokens": 0, "cache_write_tokens": 0, "output_tokens": 0,
            "reasoning_tokens": 0, "requests": 0, "requests_without_usage": 0, "compaction_requests": 0},
        "notes": []
    }))
    .unwrap()
}

/// A session in `dir`, model `name` (profile labels from the catalog),
/// context ≈18.8k / 984k, not priced; running when `running`.
fn app(name: &str, dir: &str, running: bool) -> App {
    let mut app = App::new();
    app.models = vec![ModelChoice {
        selection: "local/m".into(),
        name: name.into(),
        provider: "Local".into(),
        profiles: vec![ProfileChoice {
            id: "high".into(),
            label: "High".into(),
        }],
        default_profile: Some("high".into()),
        max_input_tokens: 983_616,
        context_window_tokens: Some(1_000_000),
        input: vec!["text".into()],
        priced: false,
    }];
    app.apply(&env(
        1,
        Event::SessionOpened {
            working_dir: dir.into(),
            model: "local/m".into(),
            reasoning: None,
            effective_reasoning: Some("high".into()),
            resumed: false,
            message_count: 0,
            warnings: vec![],
        },
    ));
    app.apply(&env(
        2,
        Event::Context {
            status: Box::new(context(18_813, 983_616, "estimated")),
        },
    ));
    let u = cersei_types::Usage {
        input_tokens: 10,
        output_tokens: 2,
        ..Default::default()
    };
    app.apply(&env(
        3,
        Event::Usage {
            turn: Box::new(u.clone()),
            total: Box::new(u),
        },
    ));
    if running {
        app.apply(&env(
            4,
            Event::RunStarted {
                prompt: "go".into(),
                attachments: vec![],
                model: "local/m".into(),
                reasoning: None,
            },
        ));
        app.apply(&env(
            5,
            Event::EditApplied {
                tool_call_id: "c".into(),
                tool: "Edit".into(),
                files: vec![WrittenFile {
                    path: "a.rs".into(),
                    kind: ChangeKind::Modify,
                    added: 12,
                    removed: 3,
                    binary: false,
                }],
                agent_id: None,
                changeset_id: None,
            },
        ));
    }
    app
}

fn text(line: &ratatui::text::Line) -> String {
    line.spans.iter().map(|s| s.content.to_string()).collect()
}

fn draw(app: &App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let o = LiveOverlay {
        popup: None,
        spinner: "⠋",
        maintenance: false,
        show_thinking: false,
        focus_approval: false,
        message: None,
        hyperlinks: false,
    };
    term.draw(|f| view::draw_live(f, app, &Composer::new(), &o))
        .unwrap();
    term
}

fn rows(term: &Terminal<TestBackend>) -> Vec<String> {
    let b = term.backend().buffer();
    (0..b.area.height)
        .map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn the_bar_degrades_with_the_width_and_never_overflows() {
    let idle = app("Example Model", "/Users/me/Documents/bricks", false);
    let run = app("Example Model", "/Users/me/Documents/bricks", true);
    for w in [20usize, 40, 60, 80, 120, 160] {
        for a in [&idle, &run] {
            let line = statusbar::status_line(a, "⠋", false, w);
            let t = text(&line);
            assert!(statusbar::width(&t) <= w, "{w}: {t:?}");
            assert!(!t.contains('⌂'), "{t}");
            if a.running() {
                assert!(t.contains('⠋'), "{w}: activity kept: {t}");
            }
        }
    }
    let wide = text(&statusbar::status_line(&idle, "⠋", false, 160));
    assert!(
        wide.starts_with("Example Model · High │ Context ≈18.8k / 984k (≈1.9%) │ Not priced"),
        "{wide}"
    );
    assert!(wide.contains("bricks"), "{wide}");
    let mid = text(&statusbar::status_line(&idle, "⠋", false, 60));
    assert!(
        mid.contains("Example Model") && mid.contains("Ctx ≈1.9%"),
        "{mid}"
    );
    let run80 = text(&statusbar::status_line(&run, "⠋", false, 80));
    assert!(
        run80.contains("Ctrl+C"),
        "cancel stays visible at 80: {run80}"
    );
    let run20 = text(&statusbar::status_line(&run, "⠋", false, 20));
    assert!(run20.contains('⠋'), "{run20}");
    // A partial or unknown cost never reads as a total.
    assert!(!wide.contains('$'));
}

#[test]
fn long_and_wide_names_are_cut_by_cells() {
    for name in [
        "Modèle très long avec accents éèàù et encore plus de texte",
        "漢字のとても長いモデル名前漢字のとても長いモデル名前",
        "Emoji 👩‍💻🚀👍🏽 model with family 👨‍👩‍👧‍👦 and more",
        "evil\u{1b}[2Jname\u{7}",
    ] {
        let a = app(
            name,
            "/Users/me/Documents/projets/漢字 et 👍🏽/very/deep/folder/bricks",
            true,
        );
        for w in [20usize, 40, 60, 80, 120, 160] {
            let t = text(&statusbar::status_line(&a, "⠋", false, w));
            assert!(statusbar::width(&t) <= w, "{w}: {t:?}");
            assert!(!t.contains('\u{1b}') && !t.contains('\u{7}'), "{t:?}");
        }
    }
}

#[test]
fn the_rendered_bar_keeps_activity_and_totals_at_every_size() {
    let a = app("Example Model", "/Users/me/Documents/bricks", true);
    for w in [20u16, 40, 60, 80, 120, 160] {
        let term = draw(&a, w, 12);
        let r = rows(&term);
        let status = r
            .iter()
            .rposition(|l| l.contains('⠋'))
            .expect("activity row");
        let totals = r
            .iter()
            .rposition(|l| l.contains("+12") && l.contains("−3"))
            .expect("totals row");
        assert_eq!(
            totals + 1,
            status,
            "{w}: +N −N just above the activity line"
        );
        // Colours of the totals, signs included.
        let b = term.backend().buffer();
        let line = &r[totals];
        let col = |needle: &str| line[..line.find(needle).unwrap()].chars().count() as u16;
        assert_eq!(b[(col("+12"), totals as u16)].fg, GREEN);
        assert_eq!(b[(col("−3"), totals as u16)].fg, RED);
    }
}

#[test]
fn extreme_sizes_and_a_resize_during_a_run_do_not_panic() {
    let mut a = app("Example Model 漢字 👍🏽", "/p", true);
    for (w, h) in [
        (1u16, 1u16),
        (2, 30),
        (19, 4),
        (20, 5),
        (500, 3),
        (500, 60),
        (3, 200),
    ] {
        draw(&a, w, h);
    }
    for w in [0usize, 1, 2, 3] {
        let t = text(&statusbar::status_line(&a, "⠋", false, w));
        assert!(statusbar::width(&t) <= w, "{w}: {t:?}");
    }
    // Narrow, wide, narrow again while the run goes on: the activity stays.
    for w in [160u16, 30, 120, 20, 80] {
        let r = rows(&draw(&a, w, 10));
        assert!(r.iter().any(|l| l.contains('⠋')), "{w}");
    }
    // A memory maintenance after the run keeps its own mark.
    a.apply(&env(
        9,
        Event::RunFinished {
            outcome: RunOutcome::Succeeded,
            failure: None,
            error: None,
            termination: None,
            text: String::new(),
            turns: 1,
            approvals_unsatisfied: vec![],
        },
    ));
    let t = text(&statusbar::status_line(&a, "⠋", true, 80));
    assert!(t.contains("updating memory"), "{t}");
}

fn at(a: &App, secs: u64) -> std::time::Instant {
    a.run.as_ref().expect("running").1 + std::time::Duration::from_secs(secs)
}

#[test]
fn working_shows_the_run_elapsed_time_then_the_cancel_hint() {
    let a = app("Example Model", "/Users/me/Documents/bricks", true);
    let line =
        |secs: u64, w: usize| text(&statusbar::status_line_at(&a, "⠦", false, w, at(&a, secs)));
    assert!(line(12, 200).contains("⠦ working [12 s] (Ctrl+C to cancel)"));
    assert!(line(65, 200).contains("⠦ working [1 min 05 s] (Ctrl+C to cancel)"));
    assert!(line(3723, 200).contains("⠦ working [1 h 02 min 03 s] (Ctrl+C to cancel)"));
    assert!(line(93_784, 200).contains("⠦ working [1 d 02 h 03 min 04 s] (Ctrl+C to cancel)"));
    assert!(!line(12, 200).contains("—"), "the old form is gone");
    // Narrow: activity, time and cancellation come before the rest.
    for w in [28usize, 32, 36, 40, 50] {
        let t = line(65, w);
        assert!(statusbar::width(&t) <= w, "{w}: {t:?}");
        assert!(
            t.contains("[1 min 05 s]") && t.contains("(Ctrl+C"),
            "{w}: {t}"
        );
    }
    let t = line(65, 33);
    assert!(t.contains("⠦ working [1 min 05 s] (Ctrl+C)"), "{t}");
    // Extreme widths: shortened, never wider than the terminal.
    for w in 0..28usize {
        for secs in [0, 65, 93_784, 40 * 86_400] {
            let t = line(secs, w);
            assert!(statusbar::width(&t) <= w, "{w}: {t:?}");
        }
    }
    let t = line(65, 14);
    assert!(
        t.contains("[1 min 05 s]"),
        "the time survives a small width: {t}"
    );
}

#[test]
fn the_timer_follows_the_run_and_restarts_with_the_next() {
    let mut a = app("Example Model", "/Users/me/Documents/bricks", true);
    let first = a.run.as_ref().unwrap().1;
    // Tool events, approvals and ticks do not reset it.
    a.apply(&env(
        6,
        Event::ToolStarted {
            tool_call_id: "t1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "sleep 1"}),
        },
    ));
    assert_eq!(a.run.as_ref().unwrap().1, first);
    // End of the run (succeeded or cancelled): no timer any more.
    for (seq, outcome) in [(7u64, RunOutcome::Succeeded), (9, RunOutcome::Cancelled)] {
        a.apply(&env(
            seq,
            Event::RunFinished {
                outcome,
                failure: None,
                error: None,
                termination: None,
                text: String::new(),
                turns: 1,
                approvals_unsatisfied: vec![],
            },
        ));
        assert!(a.run.is_none());
        let t = text(&statusbar::status_line(&a, "⠦", false, 200));
        assert!(!t.contains("working") && !t.contains(" s]"), "{t}");
        // A new run starts from zero.
        a.apply(&env(
            seq + 1,
            Event::RunStarted {
                prompt: "again".into(),
                attachments: vec![],
                model: "local/m".into(),
                reasoning: None,
            },
        ));
        let started = a.run.as_ref().unwrap().1;
        assert!(started >= first);
        let t = text(&statusbar::status_line_at(&a, "⠦", false, 200, started));
        assert!(t.contains("working [0 s]"), "{t}");
    }
    // A session opened (resumed or new) has no run, hence no old start.
    a.apply(&env(
        20,
        Event::SessionOpened {
            working_dir: "/p".into(),
            model: "local/m".into(),
            reasoning: None,
            effective_reasoning: None,
            resumed: true,
            message_count: 4,
            warnings: vec![],
        },
    ));
    assert!(a.run.is_none());
    // Maintenance outside a run stays apart from `working`.
    let t = text(&statusbar::status_line(&a, "⠦", true, 200));
    assert!(
        t.contains("updating memory") && !t.contains("working"),
        "{t}"
    );
}
