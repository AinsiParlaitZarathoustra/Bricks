use crate::contract::*;
use crate::engine::AppState;
use crate::services::attachments;
use crate::services::join_error;
use tauri::State;

#[tauri::command]
pub async fn sessions_list(state: State<'_, AppState>, project_id: String) -> IpcResult<Vec<SessionItem>> {
    state.sessions_list(&project_id).await
}

/// Searches titles AND message text of the project's conversations
/// (all projects when `project_id` is `None`).
#[tauri::command]
pub async fn sessions_search(state: State<'_, AppState>, project_id: Option<String>, query: String) -> IpcResult<Vec<SessionSearchHit>> {
    state.sessions_search(project_id.as_deref(), &query).await
}

/// Opens (or attaches to an already open) controller for the session.
#[tauri::command]
pub async fn session_open(state: State<'_, AppState>, request: OpenSessionRequest) -> IpcResult<OpenedSession> {
    state.open_session(request).await
}

/// Saves and closes the controller; refuses with `SessionBusy` while a
/// run is active unless `cancel` is true.
#[tauri::command]
pub async fn session_close(state: State<'_, AppState>, session_id: String, cancel: bool) -> IpcResult<()> {
    state.close_session(&session_id, cancel).await
}

#[tauri::command]
pub async fn session_rename(state: State<'_, AppState>, session_id: String, title: String) -> IpcResult<SessionItem> {
    state.rename_session(&session_id, &title).await
}

/// Closes the controller (cancelling a run) then deletes the stored
/// session.
#[tauri::command]
pub async fn session_delete(state: State<'_, AppState>, session_id: String) -> IpcResult<()> {
    state.delete_session(&session_id).await
}

/// Sends an engine `Command` (JSON as in `docs/cli.md`) to the session.
/// `display` is the typed command of a submit (e.g. `/skill debug`), kept as
/// the label of the run it starts; it is never sent to the engine.
#[tauri::command]
pub async fn session_command(
    state: State<'_, AppState>,
    session_id: String,
    command: serde_json::Value,
    display: Option<String>,
) -> IpcResult<()> {
    state.send_command(&session_id, command, display).await
}

/// `cersei_agent::control::Snapshot`.
#[tauri::command]
pub async fn session_snapshot(state: State<'_, AppState>, session_id: String) -> IpcResult<serde_json::Value> {
    state.snapshot(&session_id)
}

/// Stores pasted/dropped bytes (base64) as an application-owned
/// temporary file usable in a `PromptBlock::Image`/`File`.
#[tauri::command]
pub async fn attachment_save(state: State<'_, AppState>, name: String, media_type: String, data_base64: String) -> IpcResult<SavedAttachment> {
    let dir = state.attachments_dir().to_path_buf();
    tokio::task::spawn_blocking(move || attachments::save(&dir, &name, &media_type, &data_base64))
        .await
        .map_err(join_error)?
}
