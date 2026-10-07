//! Language servers shared by every client of an engine.
//!
//! * One instance per (server, project root), started on demand; concurrent
//!   first uses share one start. A monorepo can have several instances of
//!   the same server, one per project root.
//! * Documents are synchronized before each request (`didOpen`, then
//!   versioned full-content `didChange`) under a per-document lock, held
//!   for the request: the answer refers to the version that was sent. There
//!   is no lock across documents or servers.
//! * A crashed server is restarted a bounded number of times; unused
//!   servers are shut down after a while.

use crate::config::LspSettings;
use crate::position::PositionEncoding;
use crate::view::Document;
use cersei_lsp::{LspClient, LspError, LspResult, LspServerConfig};
use dashmap::DashMap;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OnceCell, OwnedRwLockReadGuard, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

/// Starts servers. The default spawns processes; tests plug in-process
/// servers.
pub trait Launcher: Send + Sync {
    /// Whether the server can be started at all (installed...).
    fn check(&self, config: &LspServerConfig) -> Result<(), String>;
    fn launch(
        &self,
        config: LspServerConfig,
        root: PathBuf,
        timeout: Duration,
    ) -> BoxFuture<'static, LspResult<Arc<LspClient>>>;
}

/// Spawns the configured command. Never downloads or installs anything.
pub struct ProcessLauncher;

impl Launcher for ProcessLauncher {
    fn check(&self, config: &LspServerConfig) -> Result<(), String> {
        which::which(&config.command).map(|_| ()).map_err(|_| {
            format!(
                "`{}` is not installed (install it manually; Bricks never installs servers)",
                config.command
            )
        })
    }

    fn launch(
        &self,
        config: LspServerConfig,
        root: PathBuf,
        timeout: Duration,
    ) -> BoxFuture<'static, LspResult<Arc<LspClient>>> {
        Box::pin(async move {
            let client = Arc::new(LspClient::new(config));
            client.start(&root).await?;
            let outcome = match tokio::time::timeout(timeout, client.initialize()).await {
                Ok(Ok(_)) => return Ok(client),
                Ok(Err(e)) => e,
                Err(_) => LspError::Timeout(timeout),
            };
            // Let the last stderr lines arrive, then explain the failure.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let stderr = client.stderr_tail();
            let _ = client.shutdown().await;
            let last = stderr.trim().lines().last().unwrap_or("").to_string();
            Err(if last.is_empty() {
                outcome
            } else {
                LspError::SpawnFailed(format!("{outcome}: {last}"))
            })
        })
    }
}

/// Why no server answers for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    Disabled,
    NoServer {
        extension: String,
    },
    NotInstalled(String),
    /// The requester may not start a server, and none is running.
    NotRunning,
    StartFailed(String),
    RestartLimit {
        restarts: u32,
    },
    PrivateView,
}

impl Unavailable {
    pub fn reason(&self) -> String {
        match self {
            Self::Disabled => "language servers are disabled ([semantic.lsp] enabled = false)".into(),
            Self::NoServer { extension } => format!("no language server configured for `.{extension}`"),
            Self::NotInstalled(m) => m.clone(),
            Self::NotRunning => "no language server running, and this request may not start one".into(),
            Self::StartFailed(m) => format!("language server failed to start: {m}"),
            Self::RestartLimit { restarts } => {
                format!("language server crashed {restarts} times; not restarted again")
            }
            Self::PrivateView => {
                "language servers serve the shared view only (this view has private or preview content)".into()
            }
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct InstanceKey {
    pub server: String,
    pub root: PathBuf,
}

struct DocState {
    version: i64,
    /// Hash of the content the server has; `None` when not open.
    hash: Option<String>,
    used: u64,
}

/// One running server for one project root.
pub struct Instance {
    pub key: InstanceKey,
    pub client: Arc<LspClient>,
    pub encoding: PositionEncoding,
    docs: Mutex<HashMap<String, Arc<RwLock<DocState>>>>,
    last_used: Mutex<Instant>,
    pub started: Instant,
    clock: AtomicU64,
    pub requests: AtomicU64,
    /// Engine generation up to which open documents were re-synchronized.
    pub synced_generation: AtomicU64,
}

impl Instance {
    /// Paths of the documents open on the server.
    pub fn open_paths(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .docs
            .lock()
            .iter()
            .filter(|(_, l)| l.try_read().map(|s| s.hash.is_some()).unwrap_or(true))
            .map(|(u, _)| PathBuf::from(cersei_lsp::uri_to_path(u)))
            .collect();
        v.sort();
        v
    }

    fn touch(&self) {
        *self.last_used.lock() = Instant::now();
    }

    pub fn open_documents(&self) -> usize {
        self.docs
            .lock()
            .values()
            .filter(|d| d.try_read().map(|s| s.hash.is_some()).unwrap_or(true))
            .count()
    }

    /// Whether the server's copy of `uri` has `hash` (`None`: not open,
    /// so the server reads the disk; `Some(false)`: another version).
    pub fn has_version(&self, uri: &str, hash: &str) -> Option<bool> {
        let lock = self.docs.lock().get(uri).cloned()?;
        let state = lock.try_read().ok()?;
        state.hash.as_deref().map(|h| h == hash)
    }
}

/// A document synchronized for one request: holds the document's read
/// lock until dropped, so its version cannot change meanwhile.
pub struct Synced {
    pub uri: String,
    pub version: i64,
    /// Diagnostics sequence before the version was sent.
    pub diag_seq_before: u64,
    _guard: OwnedRwLockReadGuard<DocState>,
}

/// One start, shared by everyone asking while it runs.
type StartCell = Arc<OnceCell<Result<Arc<Instance>, Unavailable>>>;

pub struct LspPool {
    settings: LspSettings,
    launcher: Arc<dyn Launcher>,
    configs: Vec<LspServerConfig>,
    instances: DashMap<InstanceKey, StartCell>,
    restarts: DashMap<InstanceKey, u32>,
    permits: Arc<Semaphore>,
    pub launches: AtomicU64,
}

impl LspPool {
    pub fn new(settings: LspSettings, launcher: Arc<dyn Launcher>) -> Self {
        let mut configs: Vec<LspServerConfig> = settings.servers.clone();
        for b in cersei_lsp::config::builtin_servers() {
            if !configs.iter().any(|c| c.name == b.name) {
                configs.push(b);
            }
        }
        Self {
            permits: Arc::new(Semaphore::new(settings.max_concurrent_requests.max(1))),
            settings,
            launcher,
            configs,
            instances: DashMap::new(),
            restarts: DashMap::new(),
            launches: AtomicU64::new(0),
        }
    }

    pub fn settings(&self) -> &LspSettings {
        &self.settings
    }

    /// The server configured for a file.
    pub fn server_for(&self, path: &Path) -> Option<&LspServerConfig> {
        let ext = format!(".{}", path.extension()?.to_str()?);
        self.configs.iter().find(|c| c.matches_extension(&ext))
    }

    /// The running, healthy instance for `path`, if any (never starts one).
    pub fn running_for(&self, path: &Path, workspace_root: &Path) -> Option<Arc<Instance>> {
        let cfg = self.server_for(path)?;
        let key = InstanceKey {
            server: cfg.name.clone(),
            root: project_root(path, workspace_root, &cfg.name),
        };
        let cell = self.instances.get(&key)?.clone();
        match cell.get() {
            Some(Ok(i)) if i.client.is_alive() => Some(Arc::clone(i)),
            _ => None,
        }
    }

    /// The instance serving `path`: running, restarted after a crash
    /// (bounded), or started now when `allow_start`. The flag says whether
    /// a (re)start happened (callers then invalidate semantic caches).
    pub async fn instance_for(
        &self,
        path: &Path,
        workspace_root: &Path,
        allow_start: bool,
    ) -> Result<(Arc<Instance>, bool), Unavailable> {
        if !self.settings.enabled {
            return Err(Unavailable::Disabled);
        }
        let cfg = self
            .server_for(path)
            .cloned()
            .ok_or_else(|| Unavailable::NoServer {
                extension: path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_string(),
            })?;
        let key = InstanceKey {
            server: cfg.name.clone(),
            root: project_root(path, workspace_root, &cfg.name),
        };
        let mut restarted = false;
        if let Some(cell) = self.instances.get(&key).map(|c| c.clone()) {
            match cell.get() {
                Some(Ok(i)) if i.client.is_alive() => {
                    i.touch();
                    return Ok((Arc::clone(i), false));
                }
                Some(Ok(_)) => {
                    // Crashed: restart within the limit.
                    let mut n = self.restarts.entry(key.clone()).or_insert(0);
                    *n += 1;
                    let count = *n;
                    drop(n);
                    if count > self.settings.max_restarts {
                        return Err(Unavailable::RestartLimit {
                            restarts: count - 1,
                        });
                    }
                    self.instances.remove(&key);
                    restarted = true;
                }
                Some(Err(e)) => return Err(e.clone()),
                None => {} // starting: wait below
            }
        }
        if !allow_start && !self.instances.contains_key(&key) {
            return Err(Unavailable::NotRunning);
        }
        self.launcher
            .check(&cfg)
            .map_err(Unavailable::NotInstalled)?;
        let cell = self
            .instances
            .entry(key.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let launcher = Arc::clone(&self.launcher);
        let timeout = Duration::from_millis(self.settings.startup_timeout_ms);
        let launches = &self.launches;
        let result = cell
            .get_or_init(|| async {
                launches.fetch_add(1, Ordering::Relaxed);
                match launcher
                    .launch(cfg.clone(), key.root.clone(), timeout)
                    .await
                {
                    Ok(client) => {
                        let encoding = client
                            .capabilities()
                            .and_then(|c| PositionEncoding::from_lsp(&c.position_encoding))
                            .unwrap_or_default();
                        Ok(Arc::new(Instance {
                            key: key.clone(),
                            client,
                            encoding,
                            docs: Mutex::new(HashMap::new()),
                            last_used: Mutex::new(Instant::now()),
                            started: Instant::now(),
                            clock: AtomicU64::new(0),
                            requests: AtomicU64::new(0),
                            synced_generation: AtomicU64::new(0),
                        }))
                    }
                    Err(e) => Err(Unavailable::StartFailed(e.to_string())),
                }
            })
            .await
            .clone();
        let inst = result?;
        inst.touch();
        Ok((inst, restarted))
    }

    /// Make the server's copy of `doc` this exact version, and keep it so
    /// until the returned guard is dropped.
    pub async fn sync(&self, inst: &Instance, doc: &Document) -> LspResult<Synced> {
        let uri = cersei_lsp::path_to_uri(&doc.path);
        let lock = {
            let mut docs = inst.docs.lock();
            Arc::clone(docs.entry(uri.clone()).or_insert_with(|| {
                Arc::new(RwLock::new(DocState {
                    version: 0,
                    hash: None,
                    used: 0,
                }))
            }))
        };
        let tick = inst.clock.fetch_add(1, Ordering::Relaxed);
        // Fast path: the server already has this version.
        {
            let guard = Arc::clone(&lock).read_owned().await;
            if guard.hash.as_deref() == Some(doc.revision.hash.as_str()) {
                let version = guard.version;
                return Ok(Synced {
                    uri,
                    version,
                    diag_seq_before: inst.client.diagnostics_seq(),
                    _guard: guard,
                });
            }
        }
        let mut w = Arc::clone(&lock).write_owned().await;
        let seq = inst.client.diagnostics_seq();
        if w.hash.as_deref() != Some(doc.revision.hash.as_str()) {
            if w.hash.is_none() {
                let lang = inst.client.language_id_for(&doc.path);
                let v = w.version + 1;
                inst.client.did_open(&uri, &lang, v, &doc.text).await?;
                w.version = v;
            } else {
                let v = w.version + 1;
                inst.client.did_change_full(&uri, v, &doc.text).await?;
                w.version = v;
            }
            w.hash = Some(doc.revision.hash.clone());
        }
        w.used = tick;
        let version = w.version;
        let guard = w.downgrade();
        self.evict(inst, &uri).await;
        Ok(Synced {
            uri,
            version,
            diag_seq_before: seq,
            _guard: guard,
        })
    }

    /// Close the least recently used documents beyond the limit.
    async fn evict(&self, inst: &Instance, keep: &str) {
        let max = self.settings.max_open_documents.max(1);
        let candidates: Vec<(String, Arc<RwLock<DocState>>)> = inst
            .docs
            .lock()
            .iter()
            .map(|(u, l)| (u.clone(), Arc::clone(l)))
            .collect();
        let mut open: Vec<(u64, String, Arc<RwLock<DocState>>)> = candidates
            .into_iter()
            .filter_map(|(u, l)| {
                let used = l.try_read().ok().filter(|s| s.hash.is_some())?.used;
                Some((used, u, l))
            })
            .collect();
        if open.len() <= max {
            return;
        }
        open.sort_by_key(|(used, _, _)| *used);
        let excess = open.len() - max;
        for (_, uri, lock) in open.into_iter().filter(|(_, u, _)| u != keep).take(excess) {
            // Busy documents (a request holds them) stay open.
            if let Ok(mut w) = lock.try_write() {
                if inst.client.did_close(&uri).await.is_ok() {
                    w.hash = None;
                }
            }
        }
    }

    /// Send a request through the concurrency limit, with the configured
    /// timeout and the requester's cancellation.
    pub async fn request(
        &self,
        inst: &Instance,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> LspResult<Value> {
        let _permit = tokio::select! {
            p = Arc::clone(&self.permits).acquire_owned() => p.map_err(|_| LspError::ServerExited)?,
            _ = cancel.cancelled() => return Err(LspError::Cancelled),
        };
        inst.touch();
        inst.requests.fetch_add(1, Ordering::Relaxed);
        inst.client
            .request(
                method,
                params,
                Duration::from_millis(self.settings.request_timeout_ms),
                Some(cancel),
            )
            .await
    }

    /// Shut down instances unused for `idle_shutdown_secs`. Returns how many.
    pub async fn reap_idle(&self) -> usize {
        let idle = Duration::from_secs(self.settings.idle_shutdown_secs);
        let stale: Vec<(InstanceKey, Arc<Instance>)> = self
            .instances
            .iter()
            .filter_map(|e| match e.value().get() {
                Some(Ok(i)) if i.last_used.lock().elapsed() >= idle => {
                    Some((e.key().clone(), Arc::clone(i)))
                }
                _ => None,
            })
            .collect();
        for (key, inst) in &stale {
            self.instances.remove(key);
            let _ = inst.client.shutdown().await;
        }
        stale.len()
    }

    pub async fn shutdown_all(&self) {
        let all: Vec<Arc<Instance>> = self
            .instances
            .iter()
            .filter_map(|e| e.value().get().and_then(|r| r.as_ref().ok()).cloned())
            .collect();
        self.instances.clear();
        for i in all {
            let _ = i.client.shutdown().await;
        }
    }

    /// Running instances.
    pub fn instances(&self) -> Vec<Arc<Instance>> {
        let mut v: Vec<Arc<Instance>> = self
            .instances
            .iter()
            .filter_map(|e| e.value().get().and_then(|r| r.as_ref().ok()).cloned())
            .filter(|i| i.client.is_alive())
            .collect();
        v.sort_by(|a, b| (&a.key.server, &a.key.root).cmp(&(&b.key.server, &b.key.root)));
        v
    }
}

/// The project root of `path` for a server: the nearest marker below the
/// workspace root (for Rust, the outermost `Cargo.toml` declaring a
/// `[workspace]`, else the nearest one), or the workspace root.
pub fn project_root(path: &Path, workspace_root: &Path, server: &str) -> PathBuf {
    let markers: &[&str] = match server {
        "rust-analyzer" => &["Cargo.toml"],
        "typescript-language-server" | "deno" | "biome" | "vtsls" => {
            &["tsconfig.json", "jsconfig.json", "package.json"]
        }
        "gopls" => &["go.work", "go.mod"],
        "pyright" | "pylsp" | "basedpyright" | "jedi-language-server" => &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "pyrightconfig.json",
        ],
        _ => &[],
    };
    let mut nearest: Option<PathBuf> = None;
    let mut rust_workspace: Option<PathBuf> = None;
    let mut dir = path.parent();
    while let Some(d) = dir {
        if !d.starts_with(workspace_root) {
            break;
        }
        for m in markers {
            let f = d.join(m);
            if f.is_file() {
                if nearest.is_none() {
                    nearest = Some(d.to_path_buf());
                }
                if *m == "Cargo.toml"
                    && std::fs::read_to_string(&f)
                        .map(|t| t.contains("[workspace]"))
                        .unwrap_or(false)
                {
                    rust_workspace = Some(d.to_path_buf());
                }
                if *m == "go.work" {
                    rust_workspace = Some(d.to_path_buf());
                }
            }
        }
        if d == workspace_root {
            break;
        }
        dir = d.parent();
    }
    rust_workspace
        .or(nearest)
        .unwrap_or_else(|| workspace_root.to_path_buf())
}
