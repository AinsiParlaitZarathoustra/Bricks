//! Skills catalogue for the settings page, outside any session
//!. Reads the sources through
//! `services::skills`; grants no trust.
use crate::contract::*;
use crate::engine::AppState;
use crate::services::join_error;
use std::sync::Arc;
use tauri::State;

/// `project_id`: include that project's sources (listed, never trusted by
/// this call). `reload`: rediscover the sources first.
#[tauri::command]
pub async fn skills_catalog(
    state: State<'_, AppState>,
    project_id: Option<String>,
    query: String,
    page: u32,
    reload: bool,
) -> IpcResult<SkillsCatalog> {
    let project = match &project_id {
        Some(id) => Some(state.project_path(id)?),
        None => None,
    };
    let store = Arc::clone(&state.skills);
    tokio::task::spawn_blocking(move || {
        store.catalog(project_id, project.as_deref(), &query, page, reload)
    })
    .await
    .map_err(join_error)?
}
