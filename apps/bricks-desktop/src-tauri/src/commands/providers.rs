//! Provider and model editing (owner: Haiku 3, Sprint 14.2). Every write:
//! checks the revision the desktop read, edits only the target table, validates
//! the whole file with the provider crate, writes atomically, then reloads the
//! catalog. Each command returns the file as it now is, reloaded.
use crate::contract::*;
use crate::engine::catalog::DesktopCatalog;
use crate::engine::{AppState, credentials_available};
use crate::services::join_error;
use crate::services::providers::{self, view};
use crate::services::providers_toml::Edit;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;
use tauri::State;

/// The providers file as it is now, without any secret value.
pub(crate) fn current_file(path: &Path) -> ProvidersFile {
    view(path, &credentials_available)
}

/// Applies one edit under the edit lock, then reloads the catalog from the
/// file as written. A failed edit changes neither the file nor the catalog.
pub(crate) fn apply_edit(
    path: &Path,
    lock: &Mutex<()>,
    catalog: &DesktopCatalog,
    revision: &str,
    edit: &Edit,
) -> IpcResult<ProvidersFile> {
    let _guard = lock.lock();
    providers::commit(path, revision, edit)?;
    let reloaded = bricks_config::load_registry(Some(path)).map(Arc::new);
    catalog.replace(reloaded);
    Ok(current_file(path))
}

async fn run_edit(state: &AppState, revision: String, edit: Edit) -> IpcResult<ProvidersFile> {
    let path = state.providers_path.clone();
    let lock = Arc::clone(&state.providers_lock);
    let catalog = Arc::clone(&state.catalog);
    tokio::task::spawn_blocking(move || apply_edit(&path, &lock, &catalog, &revision, &edit))
        .await
        .map_err(join_error)?
}

#[tauri::command]
pub async fn providers_config(state: State<'_, AppState>) -> IpcResult<ProvidersFile> {
    let path = state.providers_path.clone();
    tokio::task::spawn_blocking(move || current_file(&path))
        .await
        .map_err(join_error)
}

#[tauri::command]
pub async fn provider_update(
    state: State<'_, AppState>,
    provider_id: String,
    patch: ProviderPatch,
    revision: String,
) -> IpcResult<ProvidersFile> {
    run_edit(
        &state,
        revision,
        Edit::ProviderUpdate { provider_id, patch },
    )
    .await
}

#[tauri::command]
pub async fn model_update(
    state: State<'_, AppState>,
    provider_id: String,
    model_id: String,
    patch: ModelPatch,
    revision: String,
) -> IpcResult<ProvidersFile> {
    run_edit(
        &state,
        revision,
        Edit::ModelUpdate {
            provider_id,
            model_id,
            patch,
        },
    )
    .await
}

#[tauri::command]
pub async fn provider_delete(
    state: State<'_, AppState>,
    provider_id: String,
    revision: String,
) -> IpcResult<ProvidersFile> {
    run_edit(&state, revision, Edit::ProviderDelete { provider_id }).await
}

#[tauri::command]
pub async fn model_delete(
    state: State<'_, AppState>,
    provider_id: String,
    model_id: String,
    revision: String,
) -> IpcResult<ProvidersFile> {
    run_edit(
        &state,
        revision,
        Edit::ModelDelete {
            provider_id,
            model_id,
        },
    )
    .await
}

#[cfg(test)]
mod tests;
