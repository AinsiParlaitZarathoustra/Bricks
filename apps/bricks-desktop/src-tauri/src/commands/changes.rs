use crate::contract::*;
use crate::engine::AppState;
use crate::services::join_error;
use std::sync::Arc;
use tauri::State;

/// The most records one listing returns.
const MAX_LIST: u32 = 2000;
const DEFAULT_LIST: u32 = 200;

/// Journal entries of a project, newest first; one session when
/// `session_id` is given.
#[tauri::command]
pub async fn change_log_list(state: State<'_, AppState>, project_id: String, session_id: Option<String>, limit: Option<u32>) -> IpcResult<Vec<ChangeRecord>> {
    state.projects.get(&project_id)?;
    let journal = Arc::clone(&state.hub.journal);
    let limit = limit.unwrap_or(DEFAULT_LIST).min(MAX_LIST) as usize;
    tokio::task::spawn_blocking(move || journal.list(&project_id, session_id.as_deref(), limit))
        .await
        .map_err(join_error)?
}

/// The stored patch of one file of a record.
#[tauri::command]
pub async fn change_log_diff(state: State<'_, AppState>, record_id: String, path: String) -> IpcResult<FileDiff> {
    let journal = Arc::clone(&state.hub.journal);
    tokio::task::spawn_blocking(move || journal.diff(&record_id, &path))
        .await
        .map_err(join_error)?
}

#[tauri::command]
pub async fn change_log_info(state: State<'_, AppState>, project_id: String) -> IpcResult<ChangeJournalInfo> {
    state.projects.get(&project_id)?;
    let journal = Arc::clone(&state.hub.journal);
    tokio::task::spawn_blocking(move || journal.info(&project_id))
        .await
        .map_err(join_error)?
}
