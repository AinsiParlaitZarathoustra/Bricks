//! Skills of the settings page: the catalogue of the store and the built-in
//! skills, and the changes made to the store. Reads and writes go through
//! `services::skills`, over `bricks_skills::SkillStore`; grant no trust.
use crate::contract::*;
use crate::engine::AppState;
use crate::services::join_error;
use crate::services::skills as skill_service;
use std::path::Path;
use std::sync::Arc;
use tauri::State;

/// `project_id`: echoed back (the catalogue reads no project folder). `reload`:
/// read the store again first.
#[tauri::command]
pub async fn skills_catalog(
    state: State<'_, AppState>,
    project_id: Option<String>,
    query: String,
    page: u32,
    reload: bool,
) -> IpcResult<SkillsCatalog> {
    let store = Arc::clone(&state.skills);
    tokio::task::spawn_blocking(move || store.catalog(project_id, &query, page, reload))
        .await
        .map_err(join_error)?
}

/// Writes a skill from its name, description and instructions into the store.
#[tauri::command]
pub async fn skills_create(
    state: State<'_, AppState>,
    name: String,
    description: String,
    instructions: String,
) -> IpcResult<SkillChange> {
    let store = state.skills.store();
    tokio::task::spawn_blocking(move || {
        skill_service::create_skill(&store, &name, &description, &instructions)
    })
    .await
    .map_err(join_error)?
}

/// Copies the skill folder `path` (chosen by the person) into the store.
#[tauri::command]
pub async fn skills_import(state: State<'_, AppState>, path: String) -> IpcResult<SkillChange> {
    let store = state.skills.store();
    tokio::task::spawn_blocking(move || skill_service::import_skill(&store, Path::new(&path)))
        .await
        .map_err(join_error)?
}

/// Removes the installed skill `name`. Resolved by name in the store only.
#[tauri::command]
pub async fn skills_remove(state: State<'_, AppState>, name: String) -> IpcResult<SkillChange> {
    let store = state.skills.store();
    tokio::task::spawn_blocking(move || skill_service::remove_skill(&store, &name))
        .await
        .map_err(join_error)?
}
