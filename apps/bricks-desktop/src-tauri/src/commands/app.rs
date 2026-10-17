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
    save_preferences(&state, prefs).await
}

/// Saves the preferences; the tool-approval preference is then in force for
/// every session and sub-agent, at their next decision. Nothing changes when
/// the file cannot be written.
pub(crate) async fn save_preferences(state: &AppState, prefs: Preferences) -> IpcResult<()> {
    let mode = crate::engine::approval_mode(prefs.tool_approval);
    let store = Arc::clone(&state.prefs);
    tokio::task::spawn_blocking(move || store.set(prefs))
        .await
        .map_err(join_error)??;
    state.approval.set(mode);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cersei_agent::control::ApprovalMode;

    #[tokio::test]
    async fn saving_the_approval_preference_applies_it_at_once_and_at_startup() {
        let home = tempfile::tempdir().unwrap();
        let state = AppState::assemble(home.path().to_path_buf(), Err("pas de registre".into()));
        assert_eq!(state.approval.get(), ApprovalMode::Standard, "standard by default");

        let prefs = Preferences {
            tool_approval: crate::contract::ToolApproval::NoConfirmation,
            ..Preferences::default()
        };
        save_preferences(&state, prefs).await.unwrap();
        assert_eq!(state.approval.get(), ApprovalMode::NoConfirmation);

        let next_start = AppState::assemble(home.path().to_path_buf(), Err("pas de registre".into()));
        assert_eq!(
            next_start.approval.get(),
            ApprovalMode::NoConfirmation,
            "the saved preference is applied at startup"
        );
    }
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
