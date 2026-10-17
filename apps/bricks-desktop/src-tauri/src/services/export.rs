//! Writes the TypeScript bindings of the IPC contract into the frontend:
//! `apps/bricks-desktop/src/lib/ipc/bindings/<Type>.ts`.
//!
//! Run with `cargo test -p bricks-desktop export_bindings`. Every type of
//! `contract.rs` is listed here; a new contract type goes in the list.

use crate::contract::*;
use std::path::PathBuf;
use ts_rs::TS;

fn bindings_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lib/ipc/bindings")
}

#[test]
fn export_bindings() {
    let dir = bindings_dir();
    std::fs::create_dir_all(&dir).expect("bindings folder");
    macro_rules! export {
        ($($t:ty),* $(,)?) => {
            $(
                <$t as TS>::export_all_to(&dir)
                    .unwrap_or_else(|e| panic!("{}: {e}", stringify!($t)));
            )*
        };
    }
    export!(
        IpcError,
        IpcErrorCode,
        AppInfo,
        ProviderInfo,
        Project,
        SessionItem,
        SearchMatch,
        SessionSearchHit,
        OpenSessionRequest,
        OpenedSession,
        HistoryMessage,
        HistoryRole,
        HistoryBlock,
        DesktopEvent,
        SavedAttachment,
        ChangeRecord,
        ChangeFile,
        ChangeJournalInfo,
        GitStatus,
        GitEntry,
        GitArea,
        FileDiff,
        GitCommit,
        PtyOpenRequest,
        PtyEvent,
        PtyInfo,
        Preferences,
        Theme,
        SlashCommandInfo,
        SlashAction,
        SlashPresent,
        SessionToolInfo,
        McpServerInfo,
        SessionConfigInfo,
        ConfigEntry,
        ProjectFileEntry,
        RunLabel,
        ToolApproval,
        ProvidersFile,
        ProviderView,
        ModelView,
        ModelProfileView,
        ProviderPatch,
        ModelPatch,
        ThemeColors,
        ColorSet,
        ReasoningProfileEdit,
        SkillsCatalog,
        SkillSourceView,
    );
}
