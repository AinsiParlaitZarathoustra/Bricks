//! Session inspection adapters: tools, MCP, configuration, locating a
//! stored session, safe quit.
//!
//! Every adapter reads the engine's own views (`Controller::tools`,
//! `mcp_statuses`, `snapshot`, the project's `BricksConfig`) and returns only
//! what the TUI inspectors show. No provider key, token, authentication
//! header or environment value leaves this module: the provider registry is
//! read for the path of its file only.
use crate::contract::*;
use crate::engine::live::{OpenSession, session_item};
use crate::engine::{self, AppState};
use crate::pty;
use crate::services::conversations::{item_from_summary, path_text};
use crate::services::{error, io_error, join_error, not_found};
use bricks_agent::BricksConfig;
use bricks_agent::control::{Controller, Snapshot, list_sessions};
use bricks_agent::permission::{PermissionEffect, PermissionProfile, SessionPolicy};
use bricks_tools::PermissionLevel;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::State;

/// Longest first line of a tool description kept for the table.
const DESCRIPTION_MAX_CHARS: usize = 400;

/// The open session under `session_id`, or `NotFound`.
pub(crate) fn open_or_not_found(
    state: &AppState,
    session_id: &str,
) -> IpcResult<Arc<OpenSession>> {
    state
        .open_entry(session_id)
        .ok_or_else(|| not_found("cette conversation n'est pas ouverte"))
}

/// Tools of an open session with the decision each one gets: the verdict of
/// the session's permission engine, taken on the level the tool declares.
pub(crate) fn tools_of(controller: &Controller) -> IpcResult<Vec<SessionToolInfo>> {
    let mut out = Vec::new();
    for info in controller.tools() {
        let level = permission_level(&info.level).ok_or_else(|| {
            error(
                IpcErrorCode::Internal,
                format!("niveau de permission inconnu : {}", info.level),
            )
        })?;
        out.push(SessionToolInfo {
            name: info.name,
            description: first_line(&info.description),
            level: level_name(level).to_string(),
            decision: effect_name(info.verdict.effect).to_string(),
            reason: info.verdict.reason,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The MCP servers of the session with the state the engine reports.
pub(crate) async fn mcp_of(controller: &Controller) -> Vec<McpServerInfo> {
    controller
        .mcp_statuses()
        .await
        .into_iter()
        .map(|(name, status)| McpServerInfo { name, status })
        .collect()
}

/// The effective configuration of an open session, without secrets.
/// `providers_origin` is the path of the providers file the registry was
/// read from, when there is one.
pub(crate) async fn config_of(
    controller: &Controller,
    providers_origin: Option<&str>,
) -> IpcResult<SessionConfigInfo> {
    let snap = controller.snapshot();
    let project = controller.project();
    let (provider, model) = split_selection(&snap.model);
    let providers_file = providers_origin.map(PathBuf::from);
    let working_dir = project.working_dir.clone();
    let rules_dir = user_rules_dir();
    let sources = tokio::task::spawn_blocking(move || {
        config_sources(providers_file.as_deref(), &working_dir, rules_dir.as_deref())
    })
    .await
    .map_err(join_error)?;
    Ok(SessionConfigInfo {
        session_id: snap.session_id.clone(),
        working_dir: path_text(&snap.working_dir),
        model: model.to_string(),
        provider: provider.to_string(),
        reasoning: snap.reasoning.clone(),
        effective_reasoning: snap.effective_reasoning.clone(),
        interactive: snap.interactive,
        memory_space: snap.memory_space.clone(),
        permission_policy: snap.permission_policy.clone(),
        permission_revision: snap.permission_revision,
        agents_enabled: project.bricks.agents.enabled,
        sources,
        settings: settings_of(&snap, &project.bricks),
    })
}

/// A stored session (open or not) with the registered project of its
/// working folder. `NotFound` when no stored or open session has this id.
pub(crate) async fn locate_of(state: &AppState, session_id: &str) -> IpcResult<SessionItem> {
    if let Some(entry) = state.open_entry(session_id) {
        return Ok(session_item(&entry).await);
    }
    let stored = list_sessions(state.sessions_dir())
        .await
        .map_err(|e| io_error("conversations", e))?;
    let summary = stored
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| not_found("conversation inconnue"))?;
    let owner = summary
        .working_dir
        .as_deref()
        .and_then(|dir| state.project_id_of_dir(dir));
    Ok(item_from_summary(summary, owner, false, false))
}

// ─── Tauri commands ──────────────────────────────────────────────────────────

/// Tools available to an open session, with their level and decision.
#[tauri::command]
pub async fn session_tools(state: State<'_, AppState>, session_id: String) -> IpcResult<Vec<SessionToolInfo>> {
    let entry = open_or_not_found(&state, &session_id)?;
    tools_of(&entry.controller)
}

/// MCP servers of an open session and their state (empty when none).
#[tauri::command]
pub async fn session_mcp(state: State<'_, AppState>, session_id: String) -> IpcResult<Vec<McpServerInfo>> {
    let entry = open_or_not_found(&state, &session_id)?;
    Ok(mcp_of(&entry.controller).await)
}

/// Effective configuration of an open session, without secrets.
#[tauri::command]
pub async fn session_config(state: State<'_, AppState>, session_id: String) -> IpcResult<SessionConfigInfo> {
    let entry = open_or_not_found(&state, &session_id)?;
    let providers = state.registry().ok().map(|r| r.origin().to_string());
    config_of(&entry.controller, providers.as_deref()).await
}

/// A stored session, open or not, with the registered project of its folder.
#[tauri::command]
pub async fn session_locate(state: State<'_, AppState>, session_id: String) -> IpcResult<SessionItem> {
    locate_of(&state, &session_id).await
}

/// Set once a quit has begun: a second call (a double click, a second
/// request) does nothing more than the first.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Native, safe quit: cancels runs, saves sessions, stops PTYs, exits.
/// Runs the same shutdown as the application's exit request, off the UI
/// thread, then exits. Safe to call twice.
#[tauri::command]
pub async fn app_quit(app: tauri::AppHandle) -> IpcResult<()> {
    if !claim_quit(&QUITTING) {
        return Ok(());
    }
    let handle = app.clone();
    let shutdown = tokio::task::spawn_blocking(move || {
        engine::shutdown(&handle);
        pty::shutdown(&handle);
    })
    .await;
    if let Err(e) = &shutdown {
        eprintln!("bricks-desktop: shutdown failed: {e}");
    }
    app.exit(0);
    shutdown.map_err(join_error)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Marks the quit as begun. `true` for the first caller only.
fn claim_quit(flag: &AtomicBool) -> bool {
    flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// The wire name of a permission level (as the engine's approval events).
fn level_name(level: PermissionLevel) -> &'static str {
    match level {
        PermissionLevel::None => "none",
        PermissionLevel::ReadOnly => "read_only",
        PermissionLevel::Write => "write",
        PermissionLevel::Execute => "execute",
        PermissionLevel::Dangerous => "dangerous",
        PermissionLevel::Forbidden => "forbidden",
    }
}

/// A level as `Controller::tools` names it (`format!("{:?}")`, lowercased),
/// or as the approval events name it.
fn permission_level(name: &str) -> Option<PermissionLevel> {
    Some(match name {
        "none" => PermissionLevel::None,
        "readonly" | "read_only" => PermissionLevel::ReadOnly,
        "write" => PermissionLevel::Write,
        "execute" => PermissionLevel::Execute,
        "dangerous" => PermissionLevel::Dangerous,
        "forbidden" => PermissionLevel::Forbidden,
        _ => return None,
    })
}

/// The session's permission profile, as the configuration shows it.
fn profile_label(policy: &SessionPolicy) -> &'static str {
    if !policy.chosen {
        return "non choisi (ASK)";
    }
    match policy.profile {
        PermissionProfile::Ask => "ASK",
        PermissionProfile::Execute => "EXECUTE",
        PermissionProfile::Custom => "CUSTOM",
    }
}

fn effect_name(effect: PermissionEffect) -> &'static str {
    match effect {
        PermissionEffect::Allow => "allow",
        PermissionEffect::Ask => "ask",
        PermissionEffect::Deny => "deny",
    }
}

/// The first line of a description, bounded.
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= DESCRIPTION_MAX_CHARS {
        line.to_string()
    } else {
        let cut: String = line.chars().take(DESCRIPTION_MAX_CHARS).collect();
        format!("{cut}…")
    }
}

/// `provider_id/model_id` split at its first `/`.
fn split_selection(selection: &str) -> (&str, &str) {
    match selection.split_once('/') {
        Some((provider, model)) => (provider, model),
        None => ("", selection),
    }
}

/// `~/.bricks/rules`, where the engine reads user rule files.
fn user_rules_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".bricks").join("rules"))
}

/// The configuration files read for a session, in the order they apply:
/// providers, the project's `bricks.toml`, the user's rule files. Only files
/// that exist are listed.
fn config_sources(
    providers: Option<&Path>,
    working_dir: &Path,
    rules_dir: Option<&Path>,
) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(p) = providers.filter(|p| p.is_file()) {
        found.push(path_text(p));
    }
    let project = working_dir.join("bricks.toml");
    if project.is_file() {
        found.push(path_text(&project));
    }
    if let Some(dir) = rules_dir {
        let mut rules: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "toml"))
            .collect();
        rules.sort();
        found.extend(rules.iter().map(|p| path_text(p)));
    }
    found
}

/// The other effective settings the TUI shows, as label/value pairs. Only
/// numbers, modes and paths: free text from the configuration is not
/// copied, because a malformed line can carry a value the person typed
/// there. Diagnostics are counted, not quoted, for the same reason.
fn settings_of(snap: &Snapshot, bricks: &BricksConfig) -> Vec<ConfigEntry> {
    let entry = |label: &str, value: String| ConfigEntry {
        label: label.to_string(),
        value,
    };
    let or_default = |v: Option<String>, fallback: &str| v.unwrap_or_else(|| fallback.to_string());
    let compression = serde_json::to_value(bricks.compression_level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string));
    vec![
        entry("Permissions", profile_label(&snap.permission_policy).to_string()),
        entry("Budget de contexte", format!("{} jetons", snap.context.input_limit)),
        entry("Marge de sécurité", format!("{} jetons", snap.context.margin)),
        entry("Réserve pour la sortie", format!("{} jetons", snap.context.reserved_output)),
        entry(
            "Compression",
            or_default(
                compression.map(|c| match c.as_str() {
                    "off" => "désactivée".to_string(),
                    "minimal" => "minimale".to_string(),
                    "aggressive" => "agressive".to_string(),
                    other => other.to_string(),
                }),
                "par défaut",
            ),
        ),
        entry(
            "Sorties brutes",
            or_default(
                bricks.raw_output_dir.as_deref().map(path_text),
                "non définies",
            ),
        ),
        entry(
            "Raisonnement par défaut (configuration)",
            or_default(bricks.agent.reasoning.clone(), "non défini"),
        ),
        entry(
            "Jetons de sortie maximum (configuration)",
            or_default(bricks.agent.max_tokens.map(|n| n.to_string()), "par défaut"),
        ),
        entry("Sous-agents simultanés", bricks.agents.max_concurrent.to_string()),
        entry("Profondeur maximale des sous-agents", bricks.agents.max_depth.to_string()),
        entry("Tâches de fond simultanées", bricks.background.max_jobs.to_string()),
        entry("Fichiers analysés par la recherche sémantique", bricks.semantic.max_files.to_string()),
        entry("Avertissements de configuration", bricks.diagnostics.len().to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::OpenSessionRequest;
    use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
    use bricks_agent::control::{Command, EngineConfig, OpenOptions, Prompt, SessionChoice};
    use bricks_agent::permission::CustomRules;
    use bricks_provider::ProviderRegistry;
    use std::time::Duration;

    /// A home folder, a project folder (canonical) and the sessions folder,
    /// under one temporary root that lives as long as the fixture.
    struct Fixture {
        _root: tempfile::TempDir,
        home: PathBuf,
        wd: PathBuf,
        sessions: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let wd_raw = root.path().join("projet");
        let sessions = home.join("sessions");
        std::fs::create_dir_all(&wd_raw).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        let wd = std::fs::canonicalize(&wd_raw).unwrap();
        Fixture { _root: root, home, wd, sessions }
    }

    fn state_of(fx: &Fixture) -> AppState {
        AppState::assemble(fx.home.clone(), Err("aucun registre dans ce test".into()))
    }

    /// A scripted controller of the project folder, with `bricks_toml` as
    /// its project settings. Its event stream is drained in the background.
    async fn scripted(fx: &Fixture, bricks_toml: &str, script: Arc<Script>, interactive: bool) -> Controller {
        let mut cfg = EngineConfig::new(
            &fx.wd,
            ScriptedCatalog::new(&["a"], script),
            BricksConfig::from_texts(Some(bricks_toml), &[]),
            fx.sessions.clone(),
        );
        cfg.interactive = interactive;
        let (controller, mut stream) = Controller::open(
            cfg,
            OpenOptions {
                session: SessionChoice::New,
                model: Some("test/a".into()),
                reasoning: None,
            },
        )
        .await
        .expect("open");
        tokio::spawn(async move { while stream.next().await.is_some() {} });
        controller
    }

    /// Waits until `check` holds (polling), or fails after a few seconds.
    async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..400 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    fn tool<'a>(tools: &'a [SessionToolInfo], name: &str) -> &'a SessionToolInfo {
        tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no tool {name}"))
    }

    fn setting(info: &SessionConfigInfo, label: &str) -> String {
        info.settings
            .iter()
            .find(|e| e.label == label)
            .map(|e| e.value.clone())
            .unwrap_or_else(|| panic!("no setting {label}"))
    }

    #[tokio::test]
    async fn tools_carry_the_levels_and_the_verdicts_of_the_session_policy() {
        let fx = fixture();
        // Not chosen yet: reading is allowed, the rest is asked. The old
        // `[permissions]` section of bricks.toml is not applied.
        let ctl = scripted(
            &fx,
            "[permissions]\nwrite = \"allow\"\n\n[permissions.tools]\nRead = \"deny\"\n",
            Script::new(vec![]),
            false,
        )
        .await;
        let tools = tools_of(&ctl).unwrap();
        assert!(tools.len() > 5, "the session's tools are listed");
        assert!(tools.windows(2).all(|w| w[0].name <= w[1].name), "sorted by name");
        let read = tool(&tools, "Read");
        assert_eq!((read.level.as_str(), read.decision.as_str()), ("read_only", "allow"));
        let edit = tool(&tools, "Edit");
        assert_eq!((edit.level.as_str(), edit.decision.as_str()), ("write", "ask"));
        let bash = tool(&tools, "Bash");
        assert_eq!((bash.level.as_str(), bash.decision.as_str()), ("execute", "ask"));
        assert!(tools.iter().all(|t| !t.reason.is_empty()), "every verdict says why");
        assert!(tools.iter().all(|t| !t.description.contains('\n')), "one line each");
        let info = config_of(&ctl, None).await.unwrap();
        assert_eq!(setting(&info, "Permissions"), "non choisi (ASK)");
        assert!(!info.permission_policy.chosen);

        // CUSTOM: writes allowed, Bash refused by its own rule.
        let mut custom = SessionPolicy::chosen(PermissionProfile::Custom);
        custom.custom = CustomRules {
            write: PermissionEffect::Allow,
            ..CustomRules::default()
        };
        custom.custom.tools.insert("Bash".into(), PermissionEffect::Deny);
        ctl.set_permission_policy(custom).unwrap();
        let tools = tools_of(&ctl).unwrap();
        assert_eq!(tool(&tools, "Edit").decision, "allow");
        assert_eq!(tool(&tools, "Bash").decision, "deny");
        assert_eq!(tool(&tools, "Read").decision, "allow");
        let info = config_of(&ctl, None).await.unwrap();
        assert_eq!(setting(&info, "Permissions"), "CUSTOM");
        assert_eq!(info.permission_revision, 1);
        ctl.close().await;
    }

    #[tokio::test]
    async fn a_policy_sent_as_a_command_is_the_one_shown() {
        let fx = fixture();
        let ctl = scripted(&fx, "", Script::new(vec![]), true).await;
        ctl.send(Command::SetPermissionPolicy {
            policy: SessionPolicy::chosen(PermissionProfile::Execute),
        })
        .unwrap();
        eventually("the policy to change", || ctl.snapshot().permission_revision == 1).await;
        let tools = tools_of(&ctl).unwrap();
        assert!(tools.iter().all(|t| t.decision == "allow" || t.level == "forbidden"));
        let info = config_of(&ctl, None).await.unwrap();
        assert_eq!(setting(&info, "Permissions"), "EXECUTE");
        assert_eq!(info.permission_policy.profile, PermissionProfile::Execute);
        ctl.close().await;
    }

    #[tokio::test]
    async fn mcp_is_empty_when_no_server_is_configured() {
        let fx = fixture();
        let ctl = scripted(&fx, "", Script::new(vec![]), false).await;
        assert!(mcp_of(&ctl).await.is_empty());
        ctl.close().await;
    }

    #[tokio::test]
    async fn the_configuration_never_carries_a_provider_key() {
        const SECRET: &str = "sk-FAKE-SECRET-4242";
        let fx = fixture();
        // A provider with an inline key, read from a real providers file.
        let providers = fx.home.join("providers.toml");
        std::fs::write(
            &providers,
            format!(
                r#"schema_version = 1

[[providers]]
id = "p"
name = "Fournisseur test"
endpoint = "https://api.example.com/v1"
protocol = "chat_completions"
auth = "bearer"
api_key = "{SECRET}"

[[providers.models]]
id = "m"
name = "Modèle test"
api_model = "vendor-m"
input_modalities = ["text"]
output_modalities = ["text"]
streaming = true
tool_calls = true

[providers.models.limits]
max_input_tokens = 96000
max_output_tokens = 8000
"#
            ),
        )
        .unwrap();
        let registry = ProviderRegistry::load(Some(&providers)).expect("providers file");
        let origin = registry.origin().to_string();
        let state = AppState::assemble(fx.home.clone(), Ok(Arc::new(registry)));
        let project = state.projects.add(fx.wd.to_str().unwrap()).unwrap().id;
        let opened = state
            .open_session(OpenSessionRequest {
                project_id: project,
                session_id: None,
                model: Some("p/m".into()),
                reasoning: None,
            })
            .await
            .expect("open with the real provider");
        let entry = state.open_entry(&opened.session.id).expect("open");

        let info = config_of(&entry.controller, Some(&origin)).await.unwrap();
        assert_eq!(info.provider, "p");
        assert_eq!(info.model, "m");
        assert_eq!(info.working_dir, fx.wd.display().to_string());
        assert!(info.sources.iter().any(|s| s == &origin), "providers file listed: {:?}", info.sources);
        let text = serde_json::to_string(&info).unwrap();
        assert!(!text.contains(SECRET), "a provider key reached the configuration view");
        assert!(!text.to_lowercase().contains("api_key"), "no key field in the view");
        assert!(!text.contains("Authorization"), "no authentication header");
        state.close_session(&opened.session.id, true).await.unwrap();
    }

    #[tokio::test]
    async fn configuration_reports_the_settings_without_quoting_diagnostics() {
        const SECRET: &str = "sk-FAKE-IN-BRICKS-TOML-9";
        let fx = fixture();
        // A misspelt key next to a value that looks like a secret: the table
        // falls back to its defaults (the engine's rule), the diagnostic
        // counts as one, and its text is not shown.
        let bricks = format!(
            "[agents]\nenabled = true\nmax_concurrent = 3\n\n[background]\nmax_jobs = 5\nbogus = \"{SECRET}\"\n"
        );
        // The same text is on disk, where the engine reads a project's settings.
        std::fs::write(fx.wd.join("bricks.toml"), &bricks).unwrap();
        let ctl = scripted(&fx, &bricks, Script::new(vec![]), false).await;
        let info = config_of(&ctl, None).await.unwrap();
        assert!(info.agents_enabled);
        assert!(info.sources.iter().any(|s| s.ends_with("bricks.toml")), "{:?}", info.sources);
        assert_eq!(setting(&info, "Sous-agents simultanés"), "3");
        assert_eq!(setting(&info, "Tâches de fond simultanées"), "16", "[background] is rejected: its default");
        assert_eq!(setting(&info, "Avertissements de configuration"), "1");
        let text = serde_json::to_string(&info).unwrap();
        assert!(!text.contains(SECRET), "a value of bricks.toml was quoted");
        // The session's policy: not chosen yet, writes are asked.
        assert_eq!(info.permission_policy, SessionPolicy::unchosen());
        assert!(!info.interactive);
        ctl.close().await;
    }

    #[tokio::test]
    async fn an_unopened_session_is_not_found() {
        let fx = fixture();
        let state = state_of(&fx);
        let err = open_or_not_found(&state, "absente").err().expect("not open");
        assert_eq!(err.code, IpcErrorCode::NotFound);
        let err = locate_of(&state, "absente").await.unwrap_err();
        assert_eq!(err.code, IpcErrorCode::NotFound);
    }

    #[tokio::test]
    async fn a_stored_session_is_located_with_its_project() {
        let fx = fixture();
        let state = state_of(&fx);
        let project = state.projects.add(fx.wd.to_str().unwrap()).unwrap().id;

        // A finished run stores the session in the sessions folder.
        let ctl = scripted(&fx, "", Script::new(vec![Reply::text("ok")]), false).await;
        ctl.send(Command::Submit { prompt: Prompt::text("bonjour") }).unwrap();
        ctl.wait_idle().await;
        let id = ctl.session_id();
        ctl.close().await;

        let located = locate_of(&state, &id).await.unwrap();
        assert_eq!(located.id, id);
        assert!(!located.open, "closed");
        assert_eq!(located.project_id.as_deref(), Some(project.as_str()), "its registered folder");
        assert_eq!(located.working_dir.as_deref(), Some(fx.wd.display().to_string().as_str()));

        // Once its folder is no longer a registered project, it has none.
        state.forget_project(&project).await.unwrap();
        let located = locate_of(&state, &id).await.unwrap();
        assert_eq!(located.project_id, None);
    }

    #[tokio::test]
    async fn an_open_session_is_located_by_its_controller() {
        let fx = fixture();
        let registry = ProviderRegistry::from_toml_str(
            r#"schema_version = 1

[[providers]]
id = "p"
name = "Fournisseur test"
endpoint = "https://api.example.com/v1"
protocol = "chat_completions"
auth = "none"

[[providers.models]]
id = "m"
name = "Modèle test"
api_model = "vendor-m"
input_modalities = ["text"]
output_modalities = ["text"]
streaming = true
tool_calls = true

[providers.models.limits]
max_input_tokens = 96000
max_output_tokens = 8000
"#,
            "test",
        )
        .expect("registry");
        let state = AppState::assemble(fx.home.clone(), Ok(Arc::new(registry)));
        let project = state.projects.add(fx.wd.to_str().unwrap()).unwrap().id;
        let opened = state
            .open_session(OpenSessionRequest {
                project_id: project.clone(),
                session_id: None,
                model: Some("p/m".into()),
                reasoning: None,
            })
            .await
            .expect("open");
        let located = locate_of(&state, &opened.session.id).await.unwrap();
        assert!(located.open);
        assert_eq!(located.project_id.as_deref(), Some(project.as_str()));
        state.close_session(&opened.session.id, true).await.unwrap();
    }

    #[test]
    fn quit_is_claimed_once() {
        let flag = AtomicBool::new(false);
        assert!(claim_quit(&flag), "the first quit runs");
        assert!(!claim_quit(&flag), "a second one does nothing more");
        assert!(!claim_quit(&flag));
    }

    #[test]
    fn levels_and_decisions_have_the_engine_names() {
        for (name, level) in [
            ("none", PermissionLevel::None),
            ("readonly", PermissionLevel::ReadOnly),
            ("read_only", PermissionLevel::ReadOnly),
            ("write", PermissionLevel::Write),
            ("execute", PermissionLevel::Execute),
            ("dangerous", PermissionLevel::Dangerous),
            ("forbidden", PermissionLevel::Forbidden),
        ] {
            assert_eq!(permission_level(name), Some(level), "{name}");
        }
        assert_eq!(permission_level("inconnu"), None);
        assert_eq!(level_name(PermissionLevel::ReadOnly), "read_only");
        assert_eq!(effect_name(PermissionEffect::Deny), "deny");
        assert_eq!(split_selection("p/m/v2"), ("p", "m/v2"));
        assert_eq!(split_selection("seul"), ("", "seul"));
        assert!(first_line(&"x".repeat(1000)).ends_with('…'));
        assert_eq!(first_line("  Lire un fichier\nsuite"), "Lire un fichier");
    }

    #[test]
    fn config_sources_list_only_the_files_that_exist() {
        let dir = tempfile::tempdir().unwrap();
        let wd = dir.path().join("projet");
        let rules = dir.path().join("rules");
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::create_dir_all(&rules).unwrap();
        let providers = dir.path().join("providers.toml");
        std::fs::write(&providers, "schema_version = 1").unwrap();
        std::fs::write(rules.join("b.toml"), "").unwrap();
        std::fs::write(rules.join("a.toml"), "").unwrap();
        std::fs::write(rules.join("notes.txt"), "").unwrap();

        // No project bricks.toml and no providers file on disk: the user's rules only.
        let got = config_sources(Some(&dir.path().join("absent.toml")), &wd, Some(&rules));
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].ends_with("a.toml") && got[1].ends_with("b.toml"));

        std::fs::write(wd.join("bricks.toml"), "").unwrap();
        let got = config_sources(Some(&providers), &wd, Some(&rules));
        assert_eq!(got.len(), 4, "{got:?}");
        assert!(got[0].ends_with("providers.toml"));
        assert!(got[1].ends_with("bricks.toml"));

        assert_eq!(config_sources(None, &wd, None).len(), 1, "the project file only");
    }
}
