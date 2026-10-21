//! Attachment copies as the interface uses them: import (pasted bytes, a
//! path), discard, promotion into a session, the paths a submit sends, and
//! preview. The rules are in `services::attachments`; the sessions' binding in
//! `engine::attachments`.

use crate::contract::{
    AttachmentPreview, AttachmentRef, AttachmentScope, ImportedAttachment, IpcResult,
    ResolvedAttachment,
};
use crate::engine::AppState;
use tauri::State;

/// Imports pasted bytes (base64; a data URL is accepted) under `name`, into
/// `draft_id` or into a new draft. The type is found from the content: an
/// image of at most 5 MiB, UTF-8 text of at most 2 MiB; other formats are
/// refused by name.
#[tauri::command]
pub async fn attachment_import_bytes(
    state: State<'_, AppState>,
    draft_id: Option<String>,
    name: String,
    data_base64: String,
) -> IpcResult<ImportedAttachment> {
    state
        .import_attachment_bytes(draft_id, name, data_base64)
        .await
}

/// Imports the file at the absolute `path` (a regular file, or a symbolic link
/// to one), into `draft_id` or into a new draft. A folder is not copied.
#[tauri::command]
pub async fn attachment_import_path(
    state: State<'_, AppState>,
    draft_id: Option<String>,
    path: String,
) -> IpcResult<ImportedAttachment> {
    state.import_attachment_path(draft_id, path).await
}

/// Frees the copy of a draft. Idempotent.
#[tauri::command]
pub async fn discard_draft_attachment(
    state: State<'_, AppState>,
    draft_id: String,
    attachment_id: String,
) -> IpcResult<()> {
    state
        .discard_draft_attachment(draft_id, attachment_id)
        .await
}

/// Removes a copy of a session that no sent message refers to (refused with
/// `rejected` otherwise). Idempotent for a copy already gone.
#[tauri::command]
pub async fn discard_session_attachment(
    state: State<'_, AppState>,
    session_id: String,
    attachment_id: String,
) -> IpcResult<()> {
    state
        .discard_session_attachment(session_id, attachment_id)
        .await
}

/// Removes a draft and every copy it holds. Idempotent.
#[tauri::command]
pub async fn discard_draft(state: State<'_, AppState>, draft_id: String) -> IpcResult<()> {
    state.discard_draft(draft_id).await
}

/// Moves a draft's copies into the session, before its submit. Returns the
/// copies as the session holds them; nothing when the draft is empty.
#[tauri::command]
pub async fn promote_draft(
    state: State<'_, AppState>,
    draft_id: String,
    session_id: String,
) -> IpcResult<Vec<AttachmentRef>> {
    state.promote_draft(draft_id, session_id).await
}

/// The absolute paths of a session's copies, in the order given: what a
/// submit passes to the engine as `file` or `image` blocks.
#[tauri::command]
pub async fn session_attachment_paths(
    state: State<'_, AppState>,
    session_id: String,
    attachment_ids: Vec<String>,
) -> IpcResult<Vec<ResolvedAttachment>> {
    state
        .session_attachment_paths(session_id, attachment_ids)
        .await
}

/// A copy for display: an image as a data URL (at most 5 MiB), a text as its
/// beginning (at most 256 KiB).
#[tauri::command]
pub async fn attachment_preview(
    state: State<'_, AppState>,
    scope: AttachmentScope,
    owner_id: String,
    attachment_id: String,
) -> IpcResult<AttachmentPreview> {
    state
        .attachment_preview(scope, owner_id, attachment_id)
        .await
}
