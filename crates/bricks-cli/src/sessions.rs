//! `bricks sessions`.

use crate::exit;
use cersei_agent::control::list_sessions;

pub async fn run(json: bool) -> i32 {
    let dir = crate::setup::sessions_dir();
    let sessions = match list_sessions(&dir).await {
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
        println!("No stored session in {}.", dir.display());
        return exit::OK;
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
        if let Some(wd) = &s.working_dir {
            println!("    {}", wd.display());
        }
    }
    exit::OK
}
