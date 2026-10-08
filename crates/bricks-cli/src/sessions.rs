//! `bricks sessions`: the project's stored sessions (`--all`: every one).
//! Reading only: no provider, model or key is needed, and nothing stored
//! is changed.

use crate::args::Global;
use crate::exit;
use cersei_agent::control::{list_sessions_in, SessionScope};
use std::path::Path;

pub async fn run(global: &Global, launch: &Path, json: bool, all: bool) -> i32 {
    let scope = if all {
        SessionScope::All
    } else {
        match crate::setup::workspace(global, launch) {
            Ok(w) => SessionScope::Workspace(w),
            Err(e) => {
                eprintln!("bricks: {e}");
                return exit::USAGE;
            }
        }
    };
    let dir = crate::setup::sessions_dir();
    let sessions = match list_sessions_in(&dir, &scope).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bricks: sessions in {}: {e}", dir.display());
            return exit::FAILED;
        }
    };
    if json {
        for s in &sessions {
            println!("{}", serde_json::to_string(s).unwrap_or_default());
        }
        return exit::OK;
    }
    if sessions.is_empty() {
        match &scope {
            SessionScope::Workspace(w) => println!(
                "No stored session for {}. `bricks sessions --all` lists every project's.",
                w.display()
            ),
            SessionScope::All => println!("No stored session in {}.", dir.display()),
        }
        return exit::OK;
    }
    if let SessionScope::Workspace(w) = &scope {
        println!("Sessions of {} (`--all`: every project)", w.display());
    }
    for s in &sessions {
        let when = chrono::DateTime::from_timestamp_millis(s.updated_at)
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        println!(
            "{}  {when}  {:>4} msg  {}  {}",
            s.id,
            s.message_count,
            s.model.as_deref().unwrap_or("?"),
            if s.title.is_empty() {
                "(untitled)"
            } else {
                &s.title
            }
        );
        match &s.working_dir {
            Some(wd) if wd.is_dir() => println!("    {}", wd.display()),
            Some(wd) => println!("    {} (folder no longer found)", wd.display()),
            None => println!("    (no folder recorded: older session)"),
        }
    }
    exit::OK
}
