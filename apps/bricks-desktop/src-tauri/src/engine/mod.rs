//! Engine assembly and the controller registry (owner: Haiku 1).
//!
//! One `Controller` per open session, in [`AppState`]. Each has one task that
//! consumes its events ([`live`]), and the windows subscribe to all of them
//! through the [`hub`]. Conversations are opened, closed and changed in
//! [`sessions`].

pub mod catalog;
pub mod hub;
pub mod live;
pub mod sessions;

use crate::contract::{AppInfo, IpcErrorCode, IpcResult, ProviderInfo, ToolApproval};
use crate::services::error;
use crate::services::attachments;
use crate::services::changes::ChangeJournal;
use crate::services::conversations::path_text;
use crate::services::paths::check_folder;
use crate::services::prefs::PrefsStore;
use crate::services::projects::ProjectStore;
use crate::services::run_labels::RunLabelStore;
use crate::services::skills::SkillsCatalogStore;
use cersei_agent::control::{ApprovalMode, ApprovalOverride, ModelCatalog};
use cersei_provider::ProviderRegistry;
use cersei_provider::config::{AuthMode, ProviderConfig};
use catalog::DesktopCatalog;
use hub::Hub;
use live::OpenSession;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long quitting waits for runs to finish their save.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

/// Shared application state, managed by Tauri.
pub struct AppState {
    /// The model catalog every session reads: the provider registry, swapped
    /// when `providers.toml` changes.
    pub(crate) catalog: Arc<DesktopCatalog>,
    /// The providers file the registry is read from.
    pub(crate) providers_path: PathBuf,
    /// Serialises the edits of the providers file (check, write, reload).
    pub(crate) providers_lock: Arc<parking_lot::Mutex<()>>,
    /// The tool-approval override shared by every session and sub-agent.
    pub(crate) approval: Arc<ApprovalOverride>,
    /// Configuration problems of the projects and stores, to show.
    config_errors: Vec<String>,
    home: PathBuf,
    sessions_dir: PathBuf,
    attachments_dir: PathBuf,
    pub(crate) projects: Arc<ProjectStore>,
    pub(crate) prefs: Arc<PrefsStore>,
    /// The skills catalogue of the settings page (Sprint 14.3), outside any session.
    pub(crate) skills: Arc<SkillsCatalogStore>,
    /// Presentation labels of runs started from typed commands (Sprint 14.4).
    pub(crate) run_labels: Arc<RunLabelStore>,
    pub(crate) hub: Arc<Hub>,
    /// The open sessions, by session id.
    sessions: Mutex<HashMap<String, Arc<OpenSession>>>,
    /// Serialises the operations that create or retire controllers, so a
    /// session cannot be opened twice or deleted while it is being opened.
    lifecycle: tokio::sync::Mutex<()>,
}

impl AppState {
    /// Assembles the state from the Bricks home: the provider registry is
    /// loaded once here. Nothing fails: problems are kept in
    /// `config_errors` and the application keeps running.
    pub fn load() -> Self {
        let providers_path = providers_file();
        let registry = bricks_config::load_registry(Some(&providers_path)).map(Arc::new);
        Self::assemble_at(bricks_config::bricks_home(), registry, providers_path)
    }

    /// The state for a given home and registry (tests, other roots). The
    /// providers file is the one under `home`.
    pub fn assemble(home: PathBuf, registry: Result<Arc<ProviderRegistry>, String>) -> Self {
        let providers_path = home.join("providers.toml");
        Self::assemble_at(home, registry, providers_path)
    }

    /// The state for a home, a registry and the providers file it was read
    /// from. The saved tool-approval preference is applied from the start.
    pub fn assemble_at(
        home: PathBuf,
        registry: Result<Arc<ProviderRegistry>, String>,
        providers_path: PathBuf,
    ) -> Self {
        let desktop = home.join("desktop");
        let projects = ProjectStore::load(desktop.join("projects.json"));
        let prefs = PrefsStore::at(desktop.join("preferences.json"));
        let approval = ApprovalOverride::new(approval_mode(prefs.get().tool_approval));
        let journal = Arc::new(ChangeJournal::at(desktop.join("changes")));
        Self {
            catalog: DesktopCatalog::new(registry),
            providers_path,
            providers_lock: Arc::new(parking_lot::Mutex::new(())),
            approval,
            config_errors: projects.notes().to_vec(),
            sessions_dir: home.join("sessions"),
            attachments_dir: desktop.join("attachments"),
            prefs: Arc::new(prefs),
            skills: Arc::new(SkillsCatalogStore::new(dirs::home_dir())),
            run_labels: Arc::new(RunLabelStore::at(desktop.join("run-labels"))),
            projects: Arc::new(projects),
            hub: Arc::new(Hub::new(journal)),
            sessions: Mutex::new(HashMap::new()),
            lifecycle: tokio::sync::Mutex::new(()),
            home,
        }
    }

    pub fn app_info(&self) -> AppInfo {
        let registry = self.catalog.registry();
        let providers = match &registry {
            Ok(r) => r
                .config()
                .providers
                .iter()
                .map(|p| ProviderInfo {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    has_credentials: credentials_available(p),
                    model_count: u32::try_from(p.models.len()).unwrap_or(u32::MAX),
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        let mut config_errors = Vec::new();
        if let Err(e) = &registry {
            config_errors.push(e.clone());
        }
        config_errors.extend(self.config_errors.iter().cloned());
        AppInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            bricks_home: path_text(&self.home),
            sessions_dir: path_text(&self.sessions_dir),
            platform: std::env::consts::OS.to_string(),
            providers,
            config_errors,
        }
    }

    /// `cersei_agent::control::ModelChoice[]` from the configured catalog.
    pub fn models(&self) -> IpcResult<serde_json::Value> {
        self.registry()?;
        let models = ModelCatalog::models(self.catalog.as_ref());
        serde_json::to_value(models).map_err(|e| error(IpcErrorCode::Internal, e.to_string()))
    }

    /// The registry in force now (a broken file is a `Config` error).
    pub fn registry(&self) -> IpcResult<Arc<ProviderRegistry>> {
        self.catalog
            .registry()
            .map_err(|e| error(IpcErrorCode::Config, e))
    }

    /// Canonical folder of a registered project. Used by the Git and
    /// terminal services; errors with `NotFound` / `FolderMissing`.
    pub fn project_path(&self, project_id: &str) -> IpcResult<PathBuf> {
        let project = self.projects.get(project_id)?;
        let path = PathBuf::from(&project.path);
        check_folder(&path).map_err(|e| {
            error(
                IpcErrorCode::FolderMissing,
                format!("le dossier {} n'est plus accessible : {e}", project.path),
            )
        })?;
        Ok(path)
    }

    pub(crate) fn attachments_dir(&self) -> &Path {
        &self.attachments_dir
    }

    pub(crate) fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    /// The open session under `session_id`.
    pub(crate) fn open_entry(&self, session_id: &str) -> Option<Arc<OpenSession>> {
        self.sessions.lock().get(session_id).cloned()
    }

    /// The project whose folder is `dir` (the same folder, once canonical).
    pub(crate) fn project_id_of_dir(&self, dir: &Path) -> Option<String> {
        self.projects
            .list()
            .into_iter()
            .find(|p| cersei_agent::control::same_workspace(Path::new(&p.path), dir))
            .map(|p| p.id)
    }

    pub(crate) fn entries_of_project(&self, project_id: &str) -> Vec<Arc<OpenSession>> {
        self.sessions
            .lock()
            .values()
            .filter(|e| e.project_id == project_id)
            .cloned()
            .collect()
    }
}

/// The providers file the registry reads: `$BRICKS_HOME/providers.toml`, or
/// the default `~/.bricks/providers.toml` (the same choice as the registry).
pub fn providers_file() -> PathBuf {
    bricks_config::providers_file_from_env()
        .or_else(cersei_provider::config::default_config_path)
        .unwrap_or_else(|| bricks_config::bricks_home().join("providers.toml"))
}

/// The engine's approval mode for the desktop's preference.
pub fn approval_mode(tool_approval: ToolApproval) -> ApprovalMode {
    match tool_approval {
        ToolApproval::Standard => ApprovalMode::Standard,
        ToolApproval::AlwaysAsk => ApprovalMode::AlwaysAsk,
        ToolApproval::NoConfirmation => ApprovalMode::NoConfirmation,
    }
}

/// Whether a provider can be called: no credential is needed (`auth =
/// "none"`), or one is configured inline or in the named environment
/// variable. The value itself never leaves Rust.
pub(crate) fn credentials_available(p: &ProviderConfig) -> bool {
    if matches!(p.auth, AuthMode::None) {
        return true;
    }
    let inline = p.api_key.as_ref().is_some_and(|k| !k.is_blank());
    let from_env = p
        .api_key_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .is_some_and(|v| !v.trim().is_empty());
    inline || from_env
}

pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager;
    let state = AppState::load();
    let dir = state.attachments_dir().to_path_buf();
    // Old pasted files go, in the background: startup does not wait for them.
    tauri::async_runtime::spawn_blocking(move || {
        attachments::remove_older_than(&dir, attachments::RETENTION);
    });
    app.manage(state);
    Ok(())
}

/// Cancel runs, save sessions, close controllers.
pub fn shutdown(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(state) = app.try_state::<AppState>() {
        tauri::async_runtime::block_on(state.shutdown(SHUTDOWN_BUDGET));
    }
}

#[cfg(test)]
mod tests;
