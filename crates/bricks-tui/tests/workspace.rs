//! The interface follows the open session's project: completion index,
//! folder shown, session picker (this project / every project).

use bricks_tui::inspect;
use bricks_tui::overlay::Overlay;
use bricks_tui::ui::Ui;
use bricks_tui::view::short_path;
use cersei_agent::control::SessionSummary;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::path::{Path, PathBuf};

fn summary(id: &str, title: &str, dir: Option<&Path>) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        title: title.into(),
        created_at: 0,
        updated_at: 1_790_000_000_000,
        message_count: 3,
        working_dir: dir.map(Path::to_path_buf),
        model: Some("local/m".into()),
        reasoning: None,
    }
}

fn screen(o: &Overlay) -> String {
    let mut t = Terminal::new(TestBackend::new(110, 12)).unwrap();
    t.draw(|f| bricks_tui::overlay::draw(f, o)).unwrap();
    let b = t.backend().buffer().clone();
    (0..b.area.height)
        .map(|y| {
            (0..b.area.width)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn completion_follows_the_resumed_project() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("A");
    let b = root.path().join("B");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("seulement_a.rs"), "").unwrap();
    std::fs::write(b.join("seulement_b.rs"), "").unwrap();
    let mut ui = Ui::new(&a);
    ui.index.ensure_fresh();
    assert!(!ui.index.search("seulement_a", 5).is_empty());
    ui.set_workspace(&b);
    assert_eq!(ui.working_dir, b);
    assert_eq!(ui.index.root(), b.as_path());
    ui.index.ensure_fresh();
    assert!(
        !ui.index.search("seulement_b", 5).is_empty(),
        "B's file proposed"
    );
    assert!(
        ui.index.search("seulement_a", 5).is_empty(),
        "A's file gone"
    );
}

#[test]
fn the_picker_shows_this_project_then_every_project_with_tab() {
    let root = tempfile::tempdir().unwrap();
    let atlas = root.path().join("atlas");
    std::fs::create_dir(&atlas).unwrap();
    let gone = PathBuf::from("/nonexistent/projet");
    let project = vec![summary("s1", "travail atlas", Some(&atlas))];
    let all = vec![
        summary("s1", "travail atlas", Some(&atlas)),
        summary("s2", "autre", Some(&gone)),
        summary("s3", "", None),
    ];
    let mut ui = Ui::new(&atlas);
    ui.overlay = Some(inspect::sessions(project, all.clone(), "s1", &atlas));
    let s = screen(ui.overlay.as_ref().unwrap());
    assert!(
        s.contains("this project") && s.contains("travail atlas"),
        "{s}"
    );
    assert!(!s.contains("autre"), "{s}");
    assert!(s.contains("Tab: sessions — all projects"), "{s}");
    // Tab: every project, with what is known of each folder.
    ui.on_key(key(KeyCode::Tab));
    let s = screen(ui.overlay.as_ref().unwrap());
    assert!(s.contains("all projects") && s.contains("autre"), "{s}");
    assert!(s.contains("folder no longer found"), "{s}");
    assert!(s.contains("no folder recorded"), "{s}");
    assert!(s.contains("Tab: sessions — this project"), "{s}");
    // Search still works in either view; Enter resumes the chosen one.
    for c in "autre".chars() {
        ui.on_key(key(KeyCode::Char(c)));
    }
    let effects = ui.on_key(key(KeyCode::Enter));
    assert!(format!("{effects:?}").contains("s2"), "{effects:?}");

    // An empty project view says so and points to Tab.
    let o = inspect::sessions(Vec::new(), all, "x", &atlas);
    let s = screen(&o);
    assert!(
        s.contains("no session of this project yet") && s.contains("Tab"),
        "{s}"
    );
    let o = inspect::sessions(Vec::new(), Vec::new(), "x", &atlas);
    assert!(screen(&o).contains("no stored session yet"));
}

#[test]
fn the_project_shown_is_short_but_never_ambiguous() {
    let home = Path::new("/Users/moi");
    assert_eq!(
        short_path("/Users/moi/Documents/atlas", Some(home), 32),
        "~/Documents/atlas"
    );
    assert_eq!(short_path("/Users/moi", Some(home), 32), "~");
    assert_eq!(short_path("/srv/x", Some(home), 32), "/srv/x");
    // Shortened to the same last components: still told apart.
    let a = short_path("/Users/moi/clients/alpha/projets/atlas", Some(home), 16);
    let b = short_path("/Users/moi/clients/beta/projets/atlas", Some(home), 16);
    assert!(
        a.starts_with("…/projets/atlas #") && b.starts_with("…/projets/atlas #"),
        "{a} {b}"
    );
    assert_ne!(a, b);
    assert_eq!(
        a,
        short_path("/Users/moi/clients/alpha/projets/atlas", Some(home), 16),
        "stable"
    );
}
