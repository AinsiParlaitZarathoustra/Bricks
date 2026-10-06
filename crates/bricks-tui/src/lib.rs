//! The interactive terminal interface of Bricks.
//!
//! It renders the engine's events and turns gestures into commands of the
//! engine contract (`cersei_agent::control`). It never runs providers,
//! tools, compaction or memory itself.
//!
//! Presentation: an inline viewport at the bottom of the terminal (live
//! cells, composer, status bar). Finished cells are written above it, into
//! the terminal's own scrollback, once — the transcript stays in the
//! terminal history after Bricks exits. Inspectors and pickers open on the
//! alternate screen and leave the transcript untouched. One loop owns the
//! terminal: nothing else writes to stdout while it runs.

pub mod commands;
pub mod composer;
pub mod inspect;
pub mod markdown;
pub mod mentions;
pub mod overlay;
pub mod state;
pub mod term;
pub mod ui;
pub mod view;

use cersei_agent::control::{Command, Controller, Event, EventStream};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::Event as TermEvent;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::Stdout;
use ui::{Effect, Ui};

pub struct TuiOptions {
    /// Open the session picker first (`bricks resume` without an id).
    pub pick_session: bool,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Height of the live area.
fn live_height() -> u16 {
    let rows = ratatui::crossterm::terminal::size()
        .map(|(_, r)| r)
        .unwrap_or(24);
    rows.saturating_sub(1).clamp(1, 16)
}

fn inline_terminal() -> std::io::Result<Term> {
    Terminal::with_options(
        CrosstermBackend::new(std::io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(live_height()),
        },
    )
}

/// Write finished cells above the live area, once, in chunks.
fn commit(term: &mut Term, ui: &mut Ui) -> std::io::Result<()> {
    let ready = ui.app.ready_to_commit();
    if ready <= ui.app.committed {
        return Ok(());
    }
    let width = term.size()?.width as usize;
    let mut lines = Vec::new();
    for cell in &ui.app.cells[ui.app.committed..ready] {
        lines.extend(markdown::wrap_all(
            &view::cell_lines(cell, false),
            width.max(1),
        ));
    }
    ui.app.committed = ready;
    for chunk in lines.chunks(200) {
        let chunk = chunk.to_vec();
        term.insert_before(chunk.len() as u16, |buf| {
            Paragraph::new(chunk).render(buf.area, buf);
        })?;
    }
    Ok(())
}

/// Terminal input, read on a thread of its own. It polls in short slices
/// instead of blocking on the input: between slices the terminal reader is
/// free, so the cursor-position queries the inline viewport needs (start,
/// resize, insertion) get their answer. (crossterm's async `EventStream`
/// holds the reader while it waits, and those queries then time out.)
fn spawn_input() -> (
    tokio::sync::mpsc::Receiver<std::io::Result<TermEvent>>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || {
        while !stopped.load(std::sync::atomic::Ordering::SeqCst) {
            match ratatui::crossterm::event::poll(std::time::Duration::from_millis(50)) {
                Ok(true) => {
                    let ev = ratatui::crossterm::event::read();
                    let failed = ev.is_err();
                    if tx.blocking_send(ev).is_err() || failed {
                        break;
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    break;
                }
            }
        }
    });
    (rx, stop)
}

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

async fn run_effects(ui: &mut Ui, ctl: &Controller, effects: Vec<Effect>) -> bool {
    use commands::Present;
    for e in effects {
        match e {
            Effect::Quit => return true,
            Effect::Send(cmd) => {
                if let Command::Submit { .. } = &cmd {
                    ui.message = None;
                }
                if let Err(why) = ctl.send(cmd) {
                    ui.message = Some(why);
                }
            }
            Effect::ChooseProfile(model) => {
                let Some(m) = ctl.models().into_iter().find(|m| m.selection == model) else {
                    ui.message = Some(format!("model `{model}` is no longer configured"));
                    continue;
                };
                if m.profiles.is_empty() {
                    if let Err(why) = ctl.send(Command::SetModel {
                        model: Some(model),
                        reasoning: None,
                    }) {
                        ui.message = Some(why);
                    }
                } else {
                    ui.overlay = Some(Ui::profile_picker(&m));
                }
            }
            Effect::Present(p) => {
                let snapshot = ctl.snapshot();
                ui.overlay = Some(match p {
                    Present::ModelPicker => Ui::model_picker(&ctl.models(), &snapshot.model),
                    Present::SessionPicker => match ctl.sessions().await {
                        Ok(list) => inspect::sessions(list, &snapshot.session_id),
                        Err(e) => overlay::Overlay::plain("sessions", &e),
                    },
                    Present::Memory => inspect::memory(&snapshot, &ui.app),
                    Present::Context => inspect::context(&snapshot),
                    Present::Cost => inspect::costs(&snapshot, &ui.app),
                    Present::Session => inspect::session(&snapshot),
                    Present::Diff => inspect::diff(ui),
                    Present::Tools => inspect::tools(ctl),
                    Present::Mcp => inspect::mcp(ctl.mcp_statuses().await),
                    Present::Config => inspect::config(&snapshot),
                    Present::Help => inspect::help(),
                    Present::Quit | Present::Attach(..) => continue,
                });
            }
        }
    }
    false
}

/// Run the interface until the user quits. The terminal is restored on
/// every exit path, including errors and panics.
pub async fn run(ctl: Controller, mut events: EventStream, opts: TuiOptions) -> Result<(), String> {
    let working_dir = ctl.snapshot().working_dir;
    let mut ui = Ui::new(&working_dir);
    let guard = term::TermGuard::enter().map_err(|e| format!("cannot set up the terminal: {e}"))?;
    let mut term = inline_terminal().map_err(|e| e.to_string())?;
    let mut full: Option<Term> = None;
    let (mut input, input_stop) = spawn_input();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(120));
    let mut spin = 0usize;
    if opts.pick_session {
        if let Ok(list) = ctl.sessions().await {
            ui.overlay = Some(inspect::sessions(list, &ctl.session_id()));
        }
    }
    let result: Result<(), String> = loop {
        // Draw: the overlay on the alternate screen, or the live area.
        let drawn = if let Some(o) = &ui.overlay {
            if full.is_none() {
                let _ = execute!(std::io::stdout(), EnterAlternateScreen);
                full = Terminal::new(CrosstermBackend::new(std::io::stdout())).ok();
            }
            match full.as_mut() {
                Some(t) => t.draw(|f| overlay::draw(f, o)).map(|_| ()),
                None => Ok(()),
            }
        } else {
            if full.take().is_some() {
                // The main screen comes back as it was: the live area is
                // still there, so no clear (and no cursor query) is needed.
                let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
            }
            commit(&mut term, &mut ui).and_then(|_| {
                let labels = ui.popup_labels();
                let popup = labels
                    .as_ref()
                    .map(|(items, title)| (items.as_slice(), ui.popup_sel, *title));
                let o = view::Overlay {
                    popup,
                    spinner: SPINNER[spin % SPINNER.len()],
                    maintenance: ui.maintenance_running,
                    show_thinking: ui.show_thinking,
                    focus_approval: ui.focus_approval,
                    message: ui.message.as_deref(),
                };
                term.draw(|f| view::draw_live(f, &ui.app, &ui.composer, &o))
                    .map(|_| ())
            })
        };
        if let Err(e) = drawn {
            break Err(format!("terminal error: {e}"));
        }

        let effects = tokio::select! {
            ev = input.recv() => match ev {
                Some(Ok(TermEvent::Key(k))) => ui.on_key(k),
                Some(Ok(TermEvent::Paste(s))) => { ui.on_paste(&s); Vec::new() }
                Some(Ok(TermEvent::Resize(_, _))) => {
                    if full.is_none() {
                        let _ = term.autoresize();
                    } else if let Some(t) = full.as_mut() {
                        let _ = t.autoresize();
                    }
                    Vec::new()
                }
                Some(Ok(_)) => Vec::new(),
                Some(Err(e)) => break Err(format!("terminal input: {e}")),
                None => break Ok(()),
            },
            env = events.next() => match env {
                Some(env) => {
                    ui.on_event(&env);
                    if let Event::SessionOpened { resumed: true, .. } = &env.event {
                        let history = ctl.history().await;
                        ui.app.load_history(&history);
                    }
                    Vec::new()
                }
                None => break Ok(()),
            },
            _ = tick.tick() => { spin += 1; Vec::new() }
        };
        if run_effects(&mut ui, &ctl, effects).await {
            break Ok(());
        }
    };
    input_stop.store(true, std::sync::atomic::Ordering::SeqCst);
    if full.take().is_some() {
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }
    // Leave the live area on screen (it is the end of the transcript) and
    // the shell prompt below it.
    let area = term.get_frame().area();
    let _ = execute!(
        std::io::stdout(),
        ratatui::crossterm::cursor::MoveTo(0, area.bottom().saturating_sub(1)),
        ratatui::crossterm::style::Print("\r\n")
    );
    drop(term);
    drop(guard);
    ctl.close().await;
    result
}
