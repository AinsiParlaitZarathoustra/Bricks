//! Computer Use: the Cua Driver built into Bricks, as an MCP server lent
//! to the agent of every session ([`ToolLender`]).
//!
//! The driver is the `CuaDriver.app` Bricks ships (in its resources), else
//! the one installed in `/Applications` or `~/Applications`
//! (`BRICKS_CUA_DRIVER` names another). Turning it on launches its daemon
//! through LaunchServices, so that macOS attributes the Accessibility and
//! Screen Recording grants to the driver itself (`com.trycua.driver`), not
//! to Bricks; then connects `cua-driver mcp`, which talks to that daemon.
//! Turning it off closes the connection and stops the daemon Bricks
//! launched. The choice is the `computer_use_enabled` preference; a run
//! sees the tools as they are when it starts, deferred: their names are
//! offered, their definitions once loaded.

use crate::contract::{
    ComputerUseDriver, ComputerUsePermission, ComputerUseState, ComputerUseStatus, PermissionState,
};
use crate::services::prefs::PrefsStore;
use bricks_agent::ToolLender;
use bricks_mcp::{McpManager, McpServerConfig, McpServerStatus};
use bricks_tools::Tool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

/// The server's name: its tools are `mcp__computer-use__<tool>`.
pub const SERVER: &str = "computer-use";
/// The bundle Bricks looks for.
const BUNDLE: &str = "CuaDriver.app";
/// How long the daemon has to answer after its launch.
const DAEMON_START: Duration = Duration::from_secs(15);
/// The longest a driver command (status, stop, permissions) may take.
const COMMAND_LIMIT: Duration = Duration::from_secs(10);

/// An installed driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Driver {
    /// The `CuaDriver.app` bundle.
    pub app: PathBuf,
    /// Its `cua-driver` executable.
    pub exe: PathBuf,
    /// Whether it is the copy shipped inside Bricks.
    pub bundled: bool,
}

impl Driver {
    fn at(app: PathBuf, bundled: bool) -> Option<Self> {
        let exe = app.join("Contents/MacOS/cua-driver");
        exe.is_file().then_some(Self { app, exe, bundled })
    }
}

/// The driver to use: `BRICKS_CUA_DRIVER` (a bundle), the copy shipped in
/// Bricks' resources, then `/Applications` and `~/Applications`.
pub fn locate() -> Option<Driver> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    if let Some(p) = std::env::var_os("BRICKS_CUA_DRIVER") {
        return Driver::at(PathBuf::from(p), false);
    }
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("Resources").join(BUNDLE)));
    candidates(bundled, dirs::home_dir())
}

fn candidates(bundled: Option<PathBuf>, home: Option<PathBuf>) -> Option<Driver> {
    bundled
        .and_then(|b| Driver::at(b, true))
        .or_else(|| Driver::at(Path::new("/Applications").join(BUNDLE), false))
        .or_else(|| home.and_then(|h| Driver::at(h.join("Applications").join(BUNDLE), false)))
}

/// What is running now.
struct Running {
    driver: Driver,
    manager: Arc<McpManager>,
    /// Bricks launched the daemon (and stops it).
    launched_daemon: bool,
}

/// The built-in Computer Use extension of the application.
pub struct ComputerUse {
    prefs: Arc<PrefsStore>,
    /// Serialises starting and stopping.
    running: tokio::sync::Mutex<Option<Running>>,
    /// The tools lent to the agents (empty when off or not ready).
    tools: parking_lot::RwLock<Vec<Arc<dyn Tool>>>,
    phase: parking_lot::RwLock<Phase>,
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Off,
    Starting,
    Ready,
    Failed(String),
}

/// Deferred: the driver's tools are many and long, so a run announces
/// their names and loads those the agent asks for.
impl ToolLender for ComputerUse {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }

    fn deferred_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.read().clone()
    }
}

impl ComputerUse {
    pub fn new(prefs: Arc<PrefsStore>) -> Arc<Self> {
        Arc::new(Self {
            prefs,
            running: tokio::sync::Mutex::new(None),
            tools: parking_lot::RwLock::new(Vec::new()),
            phase: parking_lot::RwLock::new(Phase::Off),
        })
    }

    /// Starts the driver when the preference says so (the application's
    /// launch). A failure stays in the status.
    pub async fn resume(&self) {
        if self.prefs.get().computer_use_enabled {
            let _ = self.start().await;
        }
    }

    /// Records the choice, then starts or stops the driver. The choice is
    /// kept even when the driver fails to start (the status says why).
    pub async fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut prefs = self.prefs.get();
        if prefs.computer_use_enabled != enabled {
            prefs.computer_use_enabled = enabled;
            self.prefs.set(prefs).map_err(|e| e.message)?;
        }
        if enabled {
            self.start().await;
        } else {
            self.stop().await;
        }
        Ok(())
    }

    async fn start(&self) {
        let mut running = self.running.lock().await;
        if running.is_some() {
            return;
        }
        *self.phase.write() = Phase::Starting;
        match connect().await {
            Ok(r) => {
                let tools = bricks_tools::mcp_tool::tools_of(&r.manager)
                    .await
                    .into_iter()
                    .map(Arc::from)
                    .collect();
                *self.tools.write() = tools;
                *self.phase.write() = Phase::Ready;
                *running = Some(r);
            }
            Err(e) => {
                eprintln!("bricks-desktop: Computer Use not started: {e}");
                *self.phase.write() = Phase::Failed(e);
            }
        }
    }

    /// Closes the connection and stops the daemon Bricks launched.
    pub async fn stop(&self) {
        let mut running = self.running.lock().await;
        self.tools.write().clear();
        *self.phase.write() = Phase::Off;
        if let Some(r) = running.take() {
            r.manager.close().await;
            if r.launched_daemon {
                let _ = driver_command(&r.driver.exe, &["stop"]).await;
            }
        }
    }

    pub async fn status(&self) -> ComputerUseStatus {
        let enabled = self.prefs.get().computer_use_enabled;
        // While it starts, the driver is the one found now.
        let driver = self
            .running
            .try_lock()
            .ok()
            .and_then(|r| r.as_ref().map(|r| r.driver.clone()))
            .or_else(locate);
        let phase = self.phase.read().clone();
        let (state, error) = match (&driver, phase) {
            _ if !cfg!(target_os = "macos") => (ComputerUseState::Unsupported, None),
            (None, _) => (ComputerUseState::Missing, None),
            (_, Phase::Off) => (ComputerUseState::Off, None),
            (_, Phase::Starting) => (ComputerUseState::Starting, None),
            (_, Phase::Ready) => (ComputerUseState::Ready, None),
            (_, Phase::Failed(e)) => (ComputerUseState::Error, Some(e)),
        };
        let permissions = match &driver {
            Some(d) => permissions(&d.exe).await,
            None => None,
        };
        let (accessibility, screen_recording) = permissions.unwrap_or_default();
        ComputerUseStatus {
            enabled,
            state,
            error,
            tools: self.tools.read().len() as u32,
            driver: driver.as_ref().map(|d| ComputerUseDriver {
                path: d.app.display().to_string(),
                version: bundle_version(&d.app),
                bundled: d.bundled,
            }),
            accessibility,
            screen_recording,
        }
    }

    /// Asks the driver to request `permission` (macOS registers it in the
    /// list, its prompt shows), then opens System Settings at that list.
    pub async fn request_permission(
        &self,
        permission: ComputerUsePermission,
    ) -> Result<(), String> {
        if let Some(r) = self.running.lock().await.as_ref() {
            let _ = driver_command(
                &r.driver.exe,
                &[
                    "call",
                    "check_permissions",
                    r#"{"prompt":true,"probe_direct_capture":false}"#,
                ],
            )
            .await;
        }
        let pane = match permission {
            ComputerUsePermission::Accessibility => "Privacy_Accessibility",
            ComputerUsePermission::ScreenRecording => "Privacy_ScreenCapture",
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
        let out = Command::new("open")
            .arg(url)
            .output()
            .await
            .map_err(|e| format!("Réglages Système n’ont pas pu s’ouvrir : {e}"))?;
        if !out.status.success() {
            return Err("Réglages Système n’ont pas pu s’ouvrir".into());
        }
        Ok(())
    }
}

/// Launches the daemon if none answers, then connects the MCP server.
async fn connect() -> Result<Running, String> {
    let driver = locate().ok_or_else(|| "Cua Driver est introuvable".to_string())?;
    let launched_daemon = if daemon_running(&driver.exe).await {
        false
    } else {
        launch_daemon(&driver).await?;
        true
    };
    let config = McpServerConfig::stdio(SERVER, driver.exe.display().to_string(), &["mcp"]);
    let manager = Arc::new(McpManager::connect(&[config]).await);
    if let Some(McpServerStatus::Error(e)) = manager.statuses().await.remove(SERVER) {
        manager.close().await;
        if launched_daemon {
            let _ = driver_command(&driver.exe, &["stop"]).await;
        }
        return Err(format!(
            "le serveur MCP de Cua Driver n’a pas démarré : {e}"
        ));
    }
    Ok(Running {
        driver,
        manager,
        launched_daemon,
    })
}

/// `open -n -g -a <bundle> --args serve --no-permissions-gate`: the daemon
/// is its own responsible process, and reports its grants instead of
/// showing the driver's onboarding window (Bricks shows them).
async fn launch_daemon(driver: &Driver) -> Result<(), String> {
    let out = Command::new("open")
        .args(["-n", "-g", "-a"])
        .arg(&driver.app)
        .args(["--args", "serve", "--no-permissions-gate"])
        .output()
        .await
        .map_err(|e| format!("Cua Driver n’a pas pu être lancé : {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Cua Driver n’a pas pu être lancé : {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let deadline = tokio::time::Instant::now() + DAEMON_START;
    while tokio::time::Instant::now() < deadline {
        if daemon_running(&driver.exe).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("Cua Driver ne répond pas après son lancement".into())
}

async fn daemon_running(exe: &Path) -> bool {
    matches!(driver_command(exe, &["status"]).await, Ok((true, _)))
}

/// Runs `exe args…` within [`COMMAND_LIMIT`]: (success, stdout).
async fn driver_command(exe: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let run = Command::new(exe).args(args).kill_on_drop(true).output();
    match tokio::time::timeout(COMMAND_LIMIT, run).await {
        Ok(Ok(out)) => Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("no answer".into()),
    }
}

/// The grants of the running daemon (`permissions status --json`); `None`
/// when no daemon answers for the driver's own identity.
async fn permissions(exe: &Path) -> Option<(PermissionState, PermissionState)> {
    let (_, out) = driver_command(exe, &["permissions", "status", "--json"])
        .await
        .ok()?;
    parse_permissions(&out)
}

fn parse_permissions(json: &str) -> Option<(PermissionState, PermissionState)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let state = |k: &str| match v.get(k).and_then(serde_json::Value::as_bool) {
        Some(true) => PermissionState::Granted,
        Some(false) => PermissionState::Missing,
        None => PermissionState::Unknown,
    };
    Some((state("accessibility"), state("screen_recording")))
}

/// `CFBundleShortVersionString` of the bundle, read from its `Info.plist`.
fn bundle_version(app: &Path) -> Option<String> {
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
    let key = plist.find("<key>CFBundleShortVersionString</key>")?;
    let rest = &plist[key..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_bundle(dir: &Path, version: &str) -> PathBuf {
        let app = dir.join(BUNDLE);
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/cua-driver"), "").unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                "<plist><dict><key>CFBundleShortVersionString</key>\n\t<string>{version}</string></dict></plist>"
            ),
        )
        .unwrap();
        app
    }

    #[test]
    fn the_bundled_driver_comes_first() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = fake_bundle(&tmp.path().join("Resources"), "1.0");
        let home = tmp.path().join("home");
        fake_bundle(&home.join("Applications"), "2.0");
        let found = candidates(Some(bundled.clone()), Some(home.clone())).unwrap();
        assert_eq!(found.app, bundled);
        assert!(found.bundled);
        assert_eq!(bundle_version(&found.app).as_deref(), Some("1.0"));
        // Without a bundled copy, an installed one is used.
        let missing = tmp.path().join("nowhere").join(BUNDLE);
        if let Some(found) = candidates(Some(missing), Some(home.clone())) {
            assert!(!found.bundled);
            assert_ne!(found.app, bundled);
        }
    }

    #[test]
    fn a_bundle_without_its_executable_is_not_a_driver() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join(BUNDLE);
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        assert!(Driver::at(app, true).is_none());
    }

    #[test]
    fn permissions_come_from_the_daemon_payload() {
        let granted = r#"{"accessibility": true, "screen_recording": false,
            "source": {"bundle_id": "com.trycua.driver"}}"#;
        assert_eq!(
            parse_permissions(granted),
            Some((PermissionState::Granted, PermissionState::Missing))
        );
        let unknown = r#"{"daemon_running": false, "status": "unknown"}"#;
        assert_eq!(
            parse_permissions(unknown),
            Some((PermissionState::Unknown, PermissionState::Unknown))
        );
        assert_eq!(parse_permissions("not json"), None);
    }

    #[tokio::test]
    async fn off_by_default_and_lends_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let prefs = Arc::new(PrefsStore::at(tmp.path().join("preferences.json")));
        let cu = ComputerUse::new(prefs);
        assert!(cu.tools().is_empty());
        let status = cu.status().await;
        assert!(!status.enabled);
        assert_eq!(status.tools, 0);
        assert!(matches!(
            status.state,
            ComputerUseState::Off | ComputerUseState::Missing | ComputerUseState::Unsupported
        ));
    }

    #[tokio::test]
    async fn turning_off_is_persisted_and_lends_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("preferences.json");
        let prefs = Arc::new(PrefsStore::at(path.clone()));
        let mut p = prefs.get();
        p.computer_use_enabled = true;
        prefs.set(p).unwrap();
        let cu = ComputerUse::new(Arc::clone(&prefs));
        cu.set_enabled(false).await.unwrap();
        assert!(!PrefsStore::at(path).get().computer_use_enabled);
        assert!(cu.tools().is_empty());
        assert_ne!(cu.status().await.state, ComputerUseState::Ready);
    }
}
