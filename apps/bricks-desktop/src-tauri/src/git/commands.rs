use crate::contract::*;
use crate::engine::AppState;
use super::ops;
use tauri::State;

#[tauri::command]
pub async fn git_status(state: State<'_, AppState>, project_id: String) -> IpcResult<GitStatus> {
    let dir = state.project_path(&project_id)?;
    ops::status(&dir).await
}

/// Diff of one path: `area` staged → index vs HEAD, unstaged → worktree
/// vs index, untracked → whole file as added.
#[tauri::command]
pub async fn git_diff(
    state: State<'_, AppState>,
    project_id: String,
    path: String,
    area: GitArea,
) -> IpcResult<FileDiff> {
    let dir = state.project_path(&project_id)?;
    ops::diff(&dir, &path, area).await
}

#[tauri::command]
pub async fn git_log(
    state: State<'_, AppState>,
    project_id: String,
    limit: u32,
    skip: u32,
) -> IpcResult<Vec<GitCommit>> {
    let dir = state.project_path(&project_id)?;
    ops::log(&dir, limit, skip).await
}

/// Per-file patches of one commit.
#[tauri::command]
pub async fn git_show(
    state: State<'_, AppState>,
    project_id: String,
    rev: String,
) -> IpcResult<Vec<FileDiff>> {
    let dir = state.project_path(&project_id)?;
    ops::show(&dir, &rev).await
}
