//! Bricks Desktop: Tauri commands and services over the Bricks engine.
//! The command list below is the IPC contract (`contract.rs` for types);
//! owner: the orchestrator.

pub mod commands;
pub mod contract;
pub mod engine;
pub mod git;
pub mod paths;
pub mod pty;
pub mod services;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            engine::setup(app)?;
            pty::setup(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Application, preferences (Haiku 1)
            commands::app::app_info,
            commands::app::models_list,
            commands::app::prefs_get,
            commands::app::prefs_set,
            commands::app::events_subscribe,
            commands::app::events_unsubscribe,
            // Projects (Haiku 1)
            commands::projects::projects_list,
            commands::projects::project_add,
            commands::projects::project_remove,
            commands::projects::project_rename,
            // Conversations (Haiku 1)
            commands::sessions::sessions_list,
            commands::sessions::sessions_search,
            commands::sessions::session_open,
            commands::sessions::session_close,
            commands::sessions::session_rename,
            commands::sessions::session_delete,
            commands::sessions::session_command,
            commands::sessions::session_snapshot,
            commands::sessions::attachment_save,
            // Change journal (Haiku 1)
            commands::changes::change_log_list,
            commands::changes::change_log_diff,
            commands::changes::change_log_info,
            // Git (Haiku 7)
            git::commands::git_status,
            git::commands::git_diff,
            git::commands::git_log,
            git::commands::git_show,
            // Terminal (Haiku 7)
            pty::commands::pty_open,
            pty::commands::pty_write,
            pty::commands::pty_resize,
            pty::commands::pty_kill,
            pty::commands::pty_list,
            // Paths (orchestrator)
            paths::path_kind,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Bricks")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                engine::shutdown(app);
                pty::shutdown(app);
            }
        });
}
