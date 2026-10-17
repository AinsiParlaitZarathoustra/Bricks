//! Path inspection for the composer (dropped items). Owner: orchestrator.

use crate::contract::IpcResult;

/// `file`, `folder` or `missing` (unreadable counts as missing).
#[tauri::command]
pub async fn path_kind(path: String) -> IpcResult<String> {
    let kind = match tokio::fs::metadata(&path).await {
        Ok(m) if m.is_dir() => "folder",
        Ok(_) => "file",
        Err(_) => "missing",
    };
    Ok(kind.to_string())
}
