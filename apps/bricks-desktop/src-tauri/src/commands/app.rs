use crate::contract::*;
use crate::engine::AppState;
use crate::services::join_error;
use std::sync::Arc;
use tauri::{State, ipc::Channel};

#[tauri::command]
pub async fn app_info(state: State<'_, AppState>) -> IpcResult<AppInfo> {
    Ok(state.app_info())
}

/// `cersei_agent::control::ModelChoice[]` from the configured catalog.
#[tauri::command]
pub async fn models_list(state: State<'_, AppState>) -> IpcResult<serde_json::Value> {
    state.models()
}

#[tauri::command]
pub async fn prefs_get(state: State<'_, AppState>) -> IpcResult<Preferences> {
    let prefs = Arc::clone(&state.prefs);
    tokio::task::spawn_blocking(move || prefs.get())
        .await
        .map_err(join_error)
}

#[tauri::command]
pub async fn prefs_set(state: State<'_, AppState>, prefs: Preferences) -> IpcResult<()> {
    let store = Arc::clone(&state.prefs);
    tokio::task::spawn_blocking(move || store.set(prefs))
        .await
        .map_err(join_error)?
}

/// Subscribes the window to every session's events; returns the
/// subscription id.
#[tauri::command]
pub async fn events_subscribe(state: State<'_, AppState>, channel: Channel<DesktopEvent>) -> IpcResult<String> {
    Ok(state.hub.subscribe(channel))
}

#[tauri::command]
pub async fn events_unsubscribe(state: State<'_, AppState>, subscription_id: String) -> IpcResult<()> {
    // Unknown ids are not an error: the subscription is gone either way.
    state.hub.unsubscribe(&subscription_id);
    Ok(())
}
