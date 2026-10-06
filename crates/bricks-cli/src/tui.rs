//! `bricks`, `bricks tui`, `bricks resume [id]`: the interactive interface.

use crate::args::Global;
use crate::exit;
use crate::setup;
use cersei_agent::control::Controller;
use std::io::IsTerminal;

pub async fn run(global: &Global, session: Option<String>, pick_session: bool) -> i32 {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!(
            "bricks: the interactive interface needs a terminal; use `bricks run` for scripts \
             (see `bricks run --help`)"
        );
        return exit::USAGE;
    }
    let cfg = match setup::engine(global, true) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bricks: {e}");
            return exit::USAGE;
        }
    };
    let (ctl, events) = match Controller::open(cfg, setup::open_options(global, session)).await {
        Ok(x) => x,
        Err(e) => {
            eprintln!("bricks: {e}");
            return exit::USAGE;
        }
    };
    let handle = ctl.clone();
    match bricks_tui::run(ctl, events, bricks_tui::TuiOptions { pick_session }).await {
        Ok(()) => {
            // The session shown last (a resume may have switched it).
            let id = handle.session_id();
            eprintln!("Session {id} stored. Resume it with `bricks resume {id}`.");
            exit::OK
        }
        Err(e) => {
            eprintln!("bricks: {e}");
            exit::FAILED
        }
    }
}
