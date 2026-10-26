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
            commands::app::app_icon_show,
            commands::app::permissions_default_get,
            commands::app::permissions_default_set,
            commands::app::events_subscribe,
            commands::app::events_unsubscribe,
            // Projects (Haiku 1)
            commands::projects::projects_list,
            commands::projects::project_add,
            commands::projects::project_remove,
            commands::projects::project_rename,
            commands::projects::project_relocate,
            // Conversations (Haiku 1)
            commands::sessions::sessions_list,
            commands::sessions::sessions_search,
            commands::sessions::session_run_user_index,
            commands::sessions::session_tool_image,
            commands::sessions::session_open,
            commands::sessions::session_close,
            commands::sessions::session_rename,
            commands::sessions::session_delete,
            commands::sessions::session_command,
            commands::sessions::session_snapshot,
            commands::sessions::attachment_save,
            // Attachment copies: drafts, promotion into a session, paths, preview (Sprint 14.7 Lot C)
            commands::attachments::attachment_import_bytes,
            commands::attachments::attachment_import_path,
            commands::attachments::discard_draft_attachment,
            commands::attachments::discard_session_attachment,
            commands::attachments::discard_draft,
            commands::attachments::promote_draft,
            commands::attachments::session_attachment_paths,
            commands::attachments::attachment_preview,
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
            // Slash commands
            commands::slash::slash_commands,
            commands::slash::slash_parse,
            // Session inspection
            commands::inspect::session_tools,
            commands::inspect::session_mcp,
            commands::inspect::session_config,
            commands::inspect::session_locate,
            commands::inspect::app_quit,
            // File index for @ mentions
            commands::files::project_files,
            // Read-only file windows of message references, SVG save dialog (Sprint 14.5)
            commands::project_file::project_file_read,
            commands::project_file::save_svg_file,
            // Reveal in the Finder a file a sub-agent changed (Sprint 14.6)
            commands::agent_file::reveal_agent_file,
            // Reader of the conversation: windows of the files it names (Sprint 14.6.2)
            commands::reader_file::reader_file_read,
            // Provider/model editing
            commands::providers::providers_config,
            commands::providers::provider_update,
            commands::providers::model_update,
            commands::providers::provider_delete,
            commands::providers::model_delete,
            commands::providers::provider_create,
            commands::providers::model_create,
            commands::providers::providers_fill_missing_prices,
            // Skills of the settings: catalogue and changes to the store
            commands::skills::skills_catalog,
            commands::skills::skills_create,
            commands::skills::skills_import,
            commands::skills::skills_remove,
            // Read adapters over engine agent state
            commands::adk::session_agents,
            commands::adk::session_changesets,
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
