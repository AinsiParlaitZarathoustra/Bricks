//! Read adapters over engine state that the desktop could only reach as text
//! or as events of the current run. They
//! return the engine's own serialized records, read now; nothing is run,
//! changed or granted. The frontend keeps them in its reducer as before.
use crate::commands::inspect::open_or_not_found;
use crate::contract::{IpcErrorCode, IpcResult};
use crate::engine::AppState;
use crate::services::error;
use tauri::State;

/// The sub-agent instances of an open session, as the agent runtime keeps
/// them (`bricks_agent::agents::InstanceRecord`, durable across runs).
/// Empty when the session has no sub-agent runtime.
#[tauri::command]
pub async fn session_agents(state: State<'_, AppState>, session_id: String) -> IpcResult<serde_json::Value> {
    let entry = open_or_not_found(&state, &session_id)?;
    let records = entry
        .controller
        .agent_runtime()
        .map(|rt| rt.records())
        .unwrap_or_default();
    serde_json::to_value(records).map_err(|e| error(IpcErrorCode::Internal, e.to_string()))
}

/// The isolated changesets of the sub-agents of an open session
/// (`bricks_agent::agents::workspace::ChangeSet`), in the runtime's order.
/// Empty when the session has no sub-agent runtime.
#[tauri::command]
pub async fn session_changesets(state: State<'_, AppState>, session_id: String) -> IpcResult<serde_json::Value> {
    let entry = open_or_not_found(&state, &session_id)?;
    let sets = entry
        .controller
        .agent_runtime()
        .map(|rt| rt.workspaces.changesets())
        .unwrap_or_default();
    serde_json::to_value(sets).map_err(|e| error(IpcErrorCode::Internal, e.to_string()))
}
