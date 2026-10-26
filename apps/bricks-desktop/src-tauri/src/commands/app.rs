use crate::contract::*;
use crate::engine::AppState;
use crate::services::join_error;
use std::sync::Arc;
use tauri::{State, ipc::Channel};

#[tauri::command]
pub async fn app_info(state: State<'_, AppState>) -> IpcResult<AppInfo> {
    Ok(state.app_info())
}

/// `bricks_agent::control::ModelChoice[]` from the configured catalog.
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

/// Shows the light or the dark icon in the Dock (macOS; elsewhere nothing).
#[tauri::command]
pub fn app_icon_show(app: tauri::AppHandle, dark: bool) -> IpcResult<()> {
    #[cfg(target_os = "macos")]
    {
        let shown = app.run_on_main_thread(move || {
            use objc2::{AllocAnyThread, MainThreadMarker};
            use objc2_app_kit::{NSApplication, NSImage};
            use objc2_foundation::NSData;
            const LIGHT: &[u8] = include_bytes!("../../icons/app-light.png");
            const DARK: &[u8] = include_bytes!("../../icons/app-dark.png");
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let data = NSData::with_bytes(if dark { DARK } else { LIGHT });
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                let app = NSApplication::sharedApplication(mtm);
                unsafe { app.setApplicationIconImage(Some(&image)) };
            }
        });
        shown.map_err(|e| crate::services::error(crate::contract::IpcErrorCode::Internal, e.to_string()))?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, dark);
    Ok(())
}

#[tauri::command]
pub async fn prefs_set(state: State<'_, AppState>, prefs: Preferences) -> IpcResult<()> {
    save_preferences(&state, prefs).await
}

/// Saves the preferences. Nothing changes when the file cannot be written.
pub(crate) async fn save_preferences(state: &AppState, prefs: Preferences) -> IpcResult<()> {
    let store = Arc::clone(&state.prefs);
    tokio::task::spawn_blocking(move || store.set(prefs))
        .await
        .map_err(join_error)?
}

/// The permission policy new sessions start from.
#[tauri::command]
pub async fn permissions_default_get(state: State<'_, AppState>) -> IpcResult<PermissionsDefault> {
    let store = state.permissions.clone();
    tokio::task::spawn_blocking(move || match store.default_policy() {
        Ok(policy) => PermissionsDefault { policy, error: None },
        Err(e) => PermissionsDefault { policy: None, error: Some(e) },
    })
    .await
    .map_err(join_error)
}

/// Sets the policy new sessions start from. Open sessions keep theirs.
#[tauri::command]
pub async fn permissions_default_set(
    state: State<'_, AppState>,
    policy: bricks_agent::permission::SessionPolicy,
) -> IpcResult<()> {
    set_default_permissions(&state, policy).await
}

pub(crate) async fn set_default_permissions(
    state: &AppState,
    mut policy: bricks_agent::permission::SessionPolicy,
) -> IpcResult<()> {
    policy.chosen = true;
    let store = state.permissions.clone();
    tokio::task::spawn_blocking(move || store.set_default(&policy))
        .await
        .map_err(join_error)?
        .map_err(|e| crate::services::error(IpcErrorCode::Io, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bricks_agent::permission::{PermissionProfile, SessionPolicy};

    #[tokio::test]
    async fn the_default_permissions_are_chosen_once_and_kept() {
        let home = tempfile::tempdir().unwrap();
        let state = AppState::assemble(home.path().to_path_buf(), Err("pas de registre".into()));
        assert_eq!(state.permissions.default_policy().unwrap(), None, "not chosen yet");

        let mut policy = SessionPolicy::chosen(PermissionProfile::Execute);
        policy.chosen = false;
        set_default_permissions(&state, policy).await.unwrap();
        let next_start = AppState::assemble(home.path().to_path_buf(), Err("pas de registre".into()));
        let kept = next_start.permissions.default_policy().unwrap().expect("saved");
        assert_eq!(kept.profile, PermissionProfile::Execute);
        assert!(kept.chosen, "a saved default is a choice");
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
