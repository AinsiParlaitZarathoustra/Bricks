//! The built-in Computer Use extension (Cua Driver): its state, its
//! switch, and the macOS permissions of its process.

use crate::contract::*;
use crate::engine::AppState;
use crate::services::{error, invalid};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn computer_use_status(state: State<'_, AppState>) -> IpcResult<ComputerUseStatus> {
    Ok(state.computer_use.status().await)
}

/// Records the choice, then starts or stops the driver; the status says
/// how it went.
#[tauri::command]
pub async fn computer_use_set_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> IpcResult<ComputerUseStatus> {
    let cu = Arc::clone(&state.computer_use);
    cu.set_enabled(enabled)
        .await
        .map_err(|e| error(IpcErrorCode::Io, e))?;
    Ok(cu.status().await)
}

/// Requests `permission` for the driver's process and opens System
/// Settings at its list.
#[tauri::command]
pub async fn computer_use_request_permission(
    state: State<'_, AppState>,
    permission: ComputerUsePermission,
) -> IpcResult<()> {
    if !cfg!(target_os = "macos") {
        return Err(invalid("les autorisations ne concernent que macOS"));
    }
    state
        .computer_use
        .request_permission(permission)
        .await
        .map_err(|e| error(IpcErrorCode::Internal, e))
}
