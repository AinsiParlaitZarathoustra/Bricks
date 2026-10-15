use crate::contract::*;
use crate::engine::AppState;
use crate::services::join_error;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn projects_list(state: State<'_, AppState>) -> IpcResult<Vec<Project>> {
    let store = Arc::clone(&state.projects);
    tokio::task::spawn_blocking(move || store.list())
        .await
        .map_err(join_error)
}

/// Registers a folder (chosen with the native dialog on the frontend).
/// Re-adding a known path returns the existing project.
#[tauri::command]
pub async fn project_add(state: State<'_, AppState>, path: String) -> IpcResult<Project> {
    let store = Arc::clone(&state.projects);
    tokio::task::spawn_blocking(move || store.add(&path))
        .await
        .map_err(join_error)?
}

/// Forgets the project (files and sessions are left untouched).
#[tauri::command]
pub async fn project_remove(state: State<'_, AppState>, project_id: String) -> IpcResult<()> {
    state.forget_project(&project_id).await
}

#[tauri::command]
pub async fn project_rename(state: State<'_, AppState>, project_id: String, name: String) -> IpcResult<Project> {
    let store = Arc::clone(&state.projects);
    tokio::task::spawn_blocking(move || store.rename(&project_id, &name))
        .await
        .map_err(join_error)?
}
