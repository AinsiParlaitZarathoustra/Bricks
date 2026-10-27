//! The Mémoire section of the settings: counts, entries, clearing.

use crate::contract::*;
use crate::engine::AppState;
use tauri::State;

#[tauri::command]
pub async fn memory_overview(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> IpcResult<MemoryOverview> {
    state.memory_overview(project_id).await
}

#[tauri::command]
pub async fn memory_counts(
    state: State<'_, AppState>,
    scope: MemoryScope,
    project_id: Option<String>,
) -> IpcResult<MemoryCounts> {
    state.memory_counts(scope, project_id).await
}

/// A page of entries (`limit` 1–50), newest first.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn memory_entries(
    state: State<'_, AppState>,
    scope: MemoryScope,
    project_id: Option<String>,
    filter: MemoryEntryFilter,
    query: String,
    offset: u32,
    limit: u32,
) -> IpcResult<MemoryEntryPage> {
    state
        .memory_entries(scope, project_id, filter, query, offset, limit)
        .await
}

#[tauri::command]
pub async fn memory_delete(
    state: State<'_, AppState>,
    scope: MemoryScope,
    project_id: Option<String>,
) -> IpcResult<()> {
    state.memory_delete(scope, project_id).await
}

#[tauri::command]
pub async fn memory_delete_retired(
    state: State<'_, AppState>,
    project_id: String,
) -> IpcResult<()> {
    state.memory_delete_retired(project_id).await
}
