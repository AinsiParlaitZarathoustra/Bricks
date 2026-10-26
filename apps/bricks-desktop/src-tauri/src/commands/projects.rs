use crate::contract::*;
use crate::engine::AppState;
use crate::services::join_error;
use std::sync::Arc;
use tauri::State;

/// The registered projects, then the built-in one of the conversations
/// without a project.
#[tauri::command]
pub async fn projects_list(state: State<'_, AppState>) -> IpcResult<Vec<Project>> {
    let store = Arc::clone(&state.projects);
    tokio::task::spawn_blocking(move || -> IpcResult<Vec<Project>> {
        let mut projects = store.list();
        projects.push(store.chats()?);
        Ok(projects)
    })
    .await
    .map_err(join_error)?
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

/// Moves the project to another folder (its folder was moved or renamed).
/// Its conversations follow it.
#[tauri::command]
pub async fn project_relocate(state: State<'_, AppState>, project_id: String, path: String) -> IpcResult<Project> {
    state.relocate_project(&project_id, &path).await
}
