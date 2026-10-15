//! Interactive terminals over a real PTY (owner: Haiku 7).
//!
//! User-driven only: nothing in the engine or the agent's tools reaches this
//! module. `manager` holds the sessions, `shell` picks the program,
//! `commands` exposes them to Tauri.

pub mod commands;
mod manager;
mod shell;

#[cfg(test)]
mod tests;

pub use manager::{EventSink, OpenParams, PtyManager};
pub use shell::{ShellSpec, default_shell};

pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager;
    app.manage(PtyManager::default());
    Ok(())
}

/// Kill every PTY's process group.
pub fn shutdown(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(manager) = app.try_state::<PtyManager>() {
        manager.kill_all();
    }
}
