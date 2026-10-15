use std::sync::Arc;

use tauri::{State, ipc::Channel};

use super::{EventSink, OpenParams, PtyManager, default_shell};
use crate::contract::*;
use crate::engine::AppState;

#[tauri::command]
pub async fn pty_open(
    state: State<'_, AppState>,
    manager: State<'_, PtyManager>,
    request: PtyOpenRequest,
    channel: Channel<PtyEvent>,
) -> IpcResult<PtyInfo> {
    let cwd = state.project_path(&request.project_id)?;
    let sink: EventSink = Arc::new(move |event: PtyEvent| channel.send(event).is_ok());
    let manager = manager.inner().clone();
    let params = OpenParams {
        project_id: request.project_id,
        session_id: request.session_id,
        cwd,
        cols: request.cols,
        rows: request.rows,
    };
    // Spawning a process blocks: keep it off the async runtime.
    tokio::task::spawn_blocking(move || manager.open(params, &default_shell(), sink))
        .await
        .map_err(|e| IpcError {
            code: IpcErrorCode::Internal,
            message: format!("Ouverture du terminal interrompue ({e})."),
        })?
}

/// Keyboard input from xterm.js (UTF-8 text, control characters included).
/// Input is queued in arrival order and written by a per-terminal thread.
#[tauri::command]
pub async fn pty_write(
    manager: State<'_, PtyManager>,
    pty_id: String,
    data: String,
) -> IpcResult<()> {
    manager.write(&pty_id, &data)
}

#[tauri::command]
pub async fn pty_resize(
    manager: State<'_, PtyManager>,
    pty_id: String,
    cols: u16,
    rows: u16,
) -> IpcResult<()> {
    manager.resize(&pty_id, cols, rows)
}

/// Idempotent: killing an unknown or finished terminal succeeds.
#[tauri::command]
pub async fn pty_kill(manager: State<'_, PtyManager>, pty_id: String) -> IpcResult<()> {
    manager.kill(&pty_id);
    Ok(())
}

#[tauri::command]
pub async fn pty_list(
    manager: State<'_, PtyManager>,
    project_id: Option<String>,
) -> IpcResult<Vec<PtyInfo>> {
    Ok(manager.list(project_id.as_deref()))
}
