//! Provider and model editing. Every write:
//! checks the revision the desktop read, edits only the target table, validates
//! the whole file with the provider crate, writes atomically, then reloads the
//! catalog. Each command returns the file as it now is, reloaded.
use crate::contract::*;
use crate::engine::catalog::DesktopCatalog;
use crate::engine::{AppState, credentials_available, key_saved};
use crate::services::{error, invalid, join_error};
use bricks_provider::keys::KeyStore;
use crate::services::providers::{self, view};
use crate::services::providers_toml::Edit;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;
use tauri::State;

/// The providers file as it is now, without any secret value.
pub(crate) fn current_file(path: &Path, keys: &dyn KeyStore) -> ProvidersFile {
    view(path, &|p| (key_saved(p, keys), credentials_available(p, keys)))
}

/// Applies one edit under the edit lock, then reloads the catalog from the
/// file as written. A failed edit changes neither the file nor the catalog.
pub(crate) fn apply_edit(
    path: &Path,
    lock: &Mutex<()>,
    catalog: &DesktopCatalog,
    keys: &dyn KeyStore,
    revision: &str,
    edit: &Edit,
) -> IpcResult<ProvidersFile> {
    let _guard = lock.lock();
    providers::commit(path, revision, edit)?;
    // A deleted provider's key goes with it.
    if let Edit::ProviderDelete { provider_id } = edit {
        let _ = keys.delete(provider_id);
    }
    let reloaded = bricks_config::load_registry(Some(path)).map(Arc::new);
    catalog.replace(reloaded);
    Ok(current_file(path, keys))
}

async fn run_edit(state: &AppState, revision: String, edit: Edit) -> IpcResult<ProvidersFile> {
    let path = state.providers_path.clone();
    let lock = Arc::clone(&state.providers_lock);
    let catalog = Arc::clone(&state.catalog);
    let keys = Arc::clone(&state.keys);
    tokio::task::spawn_blocking(move || {
        apply_edit(&path, &lock, &catalog, keys.as_ref(), &revision, &edit)
    })
    .await
    .map_err(join_error)?
}

#[tauri::command]
pub async fn providers_config(state: State<'_, AppState>) -> IpcResult<ProvidersFile> {
    let path = state.providers_path.clone();
    let keys = Arc::clone(&state.keys);
    tokio::task::spawn_blocking(move || current_file(&path, keys.as_ref()))
        .await
        .map_err(join_error)
}

/// Saves `key` as the API key of `provider_id` (`None`: removes it). The key
/// goes to the secure storage only; it is never written to a file, logged or
/// returned.
pub(crate) fn write_key(
    path: &Path,
    keys: &dyn KeyStore,
    provider_id: &str,
    key: Option<&str>,
) -> IpcResult<ProvidersFile> {
    let known = current_file(path, keys)
        .providers
        .iter()
        .any(|p| p.id == provider_id);
    if !known {
        return Err(crate::services::not_found(crate::i18n::tr!(
            crate::i18n::msg::UNKNOWN_PROVIDER
        )));
    }
    match key.map(str::trim) {
        Some("") => return Err(invalid(crate::i18n::tr!(crate::i18n::msg::API_KEY_EMPTY))),
        Some(k) => keys.set(provider_id, k),
        None => keys.delete(provider_id),
    }
    .map_err(|_| {
        error(
            IpcErrorCode::Io,
            crate::i18n::tr!(crate::i18n::msg::KEYCHAIN_UNAVAILABLE),
        )
    })?;
    Ok(current_file(path, keys))
}

#[tauri::command]
pub async fn provider_key_set(
    state: State<'_, AppState>,
    provider_id: String,
    key: String,
) -> IpcResult<ProvidersFile> {
    let path = state.providers_path.clone();
    let keys = Arc::clone(&state.keys);
    let file =
        tokio::task::spawn_blocking(move || write_key(&path, keys.as_ref(), &provider_id, Some(&key)))
            .await
            .map_err(join_error)??;
    state.catalog.touch();
    Ok(file)
}

#[tauri::command]
pub async fn provider_key_delete(
    state: State<'_, AppState>,
    provider_id: String,
) -> IpcResult<ProvidersFile> {
    let path = state.providers_path.clone();
    let keys = Arc::clone(&state.keys);
    let file = tokio::task::spawn_blocking(move || write_key(&path, keys.as_ref(), &provider_id, None))
        .await
        .map_err(join_error)??;
    state.catalog.touch();
    Ok(file)
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

#[tauri::command]
pub async fn provider_create(
    state: State<'_, AppState>,
    provider_id: String,
    patch: ProviderPatch,
    revision: String,
) -> IpcResult<ProvidersFile> {
    run_edit(
        &state,
        revision,
        Edit::ProviderCreate { provider_id, patch },
    )
    .await
}

#[tauri::command]
pub async fn model_create(
    state: State<'_, AppState>,
    provider_id: String,
    model_id: String,
    patch: ModelPatch,
    revision: String,
) -> IpcResult<ProvidersFile> {
    run_edit(
        &state,
        revision,
        Edit::ModelCreate {
            provider_id,
            model_id,
            patch,
        },
    )
    .await
}

#[tauri::command]
pub async fn providers_fill_missing_prices(
    state: State<'_, AppState>,
    revision: String,
) -> IpcResult<ProvidersFile> {
    run_edit(&state, revision, Edit::FillMissingPrices).await
}

#[cfg(test)]
mod tests;
