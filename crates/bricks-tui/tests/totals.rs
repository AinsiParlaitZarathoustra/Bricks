//! The `+N −N` line: totals of the changes applied in the open session,
//! where it sits, its colours (shared with diffs and edit results), and
//! the cost label.

use bricks_tui::composer::Composer;
use bricks_tui::state::{App, Cell};
use bricks_tui::view::{self, Overlay as LiveOverlay};
use cersei_agent::control::*;
use cersei_tools::preview::ChangeKind;
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

const GREEN: Color = Color::Rgb(152, 205, 170);
const RED: Color = Color::Rgb(224, 153, 153);

fn env(session: &str, seq: u64, event: Event) -> Envelope {
    Envelope {
        schema: SCHEMA_VERSION,
        session_id: session.into(),
        run_id: Some("r1".into()),
        seq,
        at: 0,
        event,
    }
}

fn opened() -> Event {
    Event::SessionOpened {
        working_dir: "/p".into(),
        model: "local/m".into(),
        reasoning: None,
        resumed: false,
        message_count: 0,
        warnings: vec![],
    }
}

fn file(path: &str, added: usize, removed: usize) -> WrittenFile {
    WrittenFile {
        path: path.into(),
        kind: ChangeKind::Modify,
        added,
        removed,
        binary: false,
    }
}

fn edit(id: &str, agent: Option<&str>, cs: Option<&str>, files: Vec<WrittenFile>) -> Event {
    Event::EditApplied {
        tool_call_id: id.into(),
        tool: "Edit".into(),
        files,
        agent_id: agent.map(String::from),
        changeset_id: cs.map(String::from),
    }
}

fn totals(app: &App) -> (u64, u64) {
    (app.totals.added, app.totals.removed)
}

#[test]
fn applied_changes_add_up_once_per_session() {
    let mut app = App::new();
    app.apply(&env("s1", 1, opened()));
    assert_eq!(totals(&app), (0, 0));
    // Two edits of one file: a sum of operations, not a net diff.
    app.apply(&env(
        "s1",
        2,
        edit("c1", None, None, vec![file("a.rs", 3, 1)]),
    ));
    app.apply(&env(
        "s1",
        3,
        edit("c2", None, None, vec![file("a.rs", 2, 4)]),
    ));
    assert_eq!(totals(&app), (5, 5));
    // The same change delivered twice counts once.
    app.apply(&env(
        "s1",
        4,
        edit("c2", None, None, vec![file("a.rs", 2, 4)]),
    ));
    assert_eq!(totals(&app), (5, 5));
    // A child's call with the same id is another change (ids are per agent).
    app.apply(&env(
        "s1",
        5,
        edit("c2", Some("agent_x"), None, vec![file("b.rs", 1, 0)]),
    ));
    app.apply(&env(
        "s1",
        6,
        edit("c2", Some("agent_x"), None, vec![file("b.rs", 1, 0)]),
    ));
    assert_eq!(totals(&app), (6, 5));
    // A ChangeSet applied, then again: once.
    let cs = || {
        edit(
            "cs_1",
            Some("agent_y"),
            Some("cs_1"),
            vec![file("c.rs", 10, 2)],
        )
    };
    app.apply(&env("s1", 7, cs()));
    app.apply(&env("s1", 8, cs()));
    assert_eq!(totals(&app), (16, 7));
    // Creation, deletion, binary (no invented lines), an empty write.
    let mut bin = file("img.png", 0, 0);
    bin.binary = true;
    let mut created = file("new.rs", 4, 0);
    created.kind = ChangeKind::Create;
    let mut deleted = file("old.rs", 0, 6);
    deleted.kind = ChangeKind::Delete;
    app.apply(&env(
        "s1",
        9,
        edit(
            "c3",
            None,
            None,
            vec![created, deleted, bin, file("same.rs", 0, 0)],
        ),
    ));
    assert_eq!(totals(&app), (20, 13));
    // Not a change: run end, a new run, a compaction, a ChangeSet ready.
    app.apply(&env(
        "s1",
        10,
        Event::Compaction {
            reason: "manual".into(),
            compacted: true,
            outcome: "ok".into(),
        },
    ));
    assert_eq!(totals(&app), (20, 13));
    // A late event of another session changes nothing; a session opened
    // (new or resumed) starts from zero.
    app.apply(&env(
        "s0",
        11,
        edit("late", None, None, vec![file("z.rs", 99, 99)]),
    ));
    assert_eq!(totals(&app), (20, 13));
    app.apply(&env("s2", 12, opened()));
    assert_eq!(totals(&app), (0, 0));
    app.apply(&env(
        "s1",
        13,
        edit("c9", None, None, vec![file("a.rs", 1, 1)]),
    ));
    assert_eq!(totals(&app), (0, 0), "the previous session's event");
}

fn draw(
    app: &App,
    composer: &Composer,
    w: u16,
    h: u16,
    popup: Option<(&[String], usize, &str)>,
) -> Terminal<TestBackend> {
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
    term
}

fn row_of(term: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16)> {
    // From the bottom: the lowest occurrence (an edit result in the body
    // shows the same counts).
    let b = term.backend().buffer();
    for y in (0..b.area.height).rev() {
        let mut line = String::new();
        let mut cols = Vec::new();
        for x in 0..b.area.width {
            let sym = b[(x, y)].symbol();
            for _ in sym.chars() {
                cols.push(x);
            }
            line.push_str(sym);
        }
        if let Some(i) = line.find(needle) {
            let char_idx = line[..i].chars().count();
            return Some((y, cols[char_idx]));
        }
    }
    None
}

fn running_app() -> App {
    let mut app = App::new();
    app.apply(&env("s1", 1, opened()));
    app.apply(&env(
        "s1",
        2,
        Event::RunStarted {
            prompt: "go".into(),
            attachments: vec![],
            model: "local/m".into(),
            reasoning: None,
        },
    ));
    app.apply(&env(
        "s1",
        3,
        edit("c1", None, None, vec![file("a.rs", 428, 18)]),
    ));
    app
}

#[test]
fn the_totals_sit_just_above_the_working_line_in_their_colours() {
    let app = running_app();
    let mut composer = Composer::new();
    composer.insert_str("ligne un\nligne deux\nligne trois");
    let mut term = draw(&app, &composer, 80, 20, None);
    let (ty, tx) = row_of(&term, "+428 −18").expect("totals shown");
    let (wy, _) = row_of(&term, "working").expect("working shown");
    assert_eq!(ty + 1, wy, "immediately above the status line");
    assert!(row_of(&term, "Ctrl+C to cancel").is_some());
    let b = term.backend().buffer();
    assert_eq!(b[(tx, ty)].fg, GREEN, "+N");
    let minus = tx + "+428 ".chars().count() as u16;
    assert_eq!(b[(minus, ty)].symbol(), "−");
    assert_eq!(b[(minus, ty)].fg, RED, "−N");
    assert_ne!(b[(tx + 4, ty)].fg, RED);
    // The input is above, the cursor in it.
    let (iy, _) = row_of(&term, "ligne trois").unwrap();
    assert!(iy < ty);
    let cursor = term.get_cursor_position().unwrap();
    assert_eq!(cursor.y, iy, "the cursor is on the input's last line");
    // At rest: +0 −0 at first, the totals stay after the run.
    let mut idle = App::new();
    idle.apply(&env("s1", 1, opened()));
    let term = draw(&idle, &Composer::new(), 80, 12, None);
    assert!(row_of(&term, "+0 −0").is_some());
    // With a completion list and an approval, nothing overlaps.
    let items = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
    let term = draw(&app, &composer, 80, 20, Some((&items, 0, "files")));
    let (ty, _) = row_of(&term, "+428 −18").unwrap();
    let (wy, _) = row_of(&term, "working").unwrap();
    let (py, _) = row_of(&term, "src/b.rs").unwrap();
    assert!(py < ty && ty + 1 == wy);
    // A small terminal keeps the cancel hint; too small says so.
    let term = draw(&app, &composer, 80, 6, None);
    let (ty, _) = row_of(&term, "+428 −18").unwrap();
    let (wy, _) = row_of(&term, "Ctrl+C").unwrap();
    assert_eq!(ty + 1, wy);
    let term = draw(&app, &composer, 80, 4, None);
    assert!(row_of(&term, "terminal too").is_some());
}

#[test]
fn edit_results_and_diffs_use_the_same_colours() {
    let lines = view::cell_lines(
        &Cell::Edits {
            files: vec![file("a.rs", 3, 1)],
        },
        false,
    );
    let spans = &lines[0].spans;
    let plus = spans.iter().find(|s| s.content == "+3").unwrap();
    let minus = spans.iter().find(|s| s.content == "−1").unwrap();
    assert_eq!((plus.style.fg, minus.style.fg), (Some(GREEN), Some(RED)));
    assert_eq!(view::diff_line("+new").spans[0].style.fg, Some(GREEN));
    assert_eq!(view::diff_line("-old").spans[0].style.fg, Some(RED));
    for header in ["+++ b/a.rs", "--- a/a.rs", "@@ -1 +1 @@", " context"] {
        let fg = view::diff_line(header).spans[0].style.fg;
        assert!(fg != Some(GREEN) && fg != Some(RED), "{header}");
    }
    // Binary: no invented counts.
    let mut bin = file("img.png", 0, 0);
    bin.binary = true;
    let lines = view::cell_lines(&Cell::Edits { files: vec![bin] }, false);
    let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("(binary)") && !text.contains("+0"), "{text}");
}

#[test]
fn a_known_cost_keeps_its_display() {
    let mut app = running_app();
    app.apply(&env(
        "s1",
        9,
        Event::Usage {
            turn: Box::new(cersei_types::Usage {
                cost_usd: Some(0.0123),
                ..Default::default()
            }),
            total: Box::new(cersei_types::Usage {
                cost_usd: Some(0.0123),
                ..Default::default()
            }),
        },
    ));
    let term = draw(&app, &Composer::new(), 80, 12, None);
    assert!(row_of(&term, "$0.0123").is_some());
    assert!(row_of(&term, "Not priced").is_none());
    // No usage yet: no cost at all.
    let term = draw(&running_app(), &Composer::new(), 80, 12, None);
    assert!(row_of(&term, "$").is_none() && row_of(&term, "Not priced").is_none());
}
