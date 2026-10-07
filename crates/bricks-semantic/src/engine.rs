//! The engine: one per workspace, shared by every client through an
//! `Arc<SemanticEngine>`.

use crate::config::SemanticConfig;
use crate::context::{self, rel_path, ContextSettings};
use crate::lexical::{self, WalkFilters};
use crate::lsp::{Instance, Launcher, LspPool, ProcessLauncher, Unavailable};
use crate::planner;
use crate::query::*;
use crate::rank::{self, Candidate};
use crate::render;
use crate::result::*;
use crate::syntax::{self, Lang, TreeCache};
use crate::view::{Document, Snapshot, ViewHandle, ViewKind};
use cersei_lsp::RawLocation;
use futures::future::{BoxFuture, FutureExt, Shared};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Extensions with a grammar (definitions can be found syntactically).
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "mts", "cts", "tsx", "js", "jsx", "mjs", "cjs", "py", "pyi", "go",
];

type SharedResponse = Shared<BoxFuture<'static, Arc<CodeResponse>>>;

struct Inflight {
    id: u64,
    fut: SharedResponse,
    waiters: Arc<AtomicUsize>,
    cancel: CancellationToken,
}

struct CachedResponse {
    response: Arc<CodeResponse>,
    at: Instant,
    used: u64,
}

#[derive(Clone)]
struct Baseline {
    hash: String,
    diags: Vec<(u8, String, String)>,
}

/// Counters for metrics.
#[derive(Default)]
struct Counters {
    queries: AtomicU64,
    result_hits: AtomicU64,
    shared_inflight: AtomicU64,
    lsp_restarts: AtomicU64,
}

/// What the engine holds, for benchmarks and inspectors.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineStats {
    pub root: String,
    pub generation: u64,
    pub queries: u64,
    pub result_cache_entries: usize,
    pub result_cache_hits: u64,
    pub shared_inflight: u64,
    pub tree_cache_entries: usize,
    pub tree_cache_source_bytes: u64,
    pub tree_hits: u64,
    pub tree_misses: u64,
    pub tree_incremental: u64,
    pub lsp_launches: u64,
    pub lsp_restarts: u64,
    pub lsp_instances: Vec<LspInstanceStats>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LspInstanceStats {
    pub server: String,
    pub root: String,
    pub pid: Option<u32>,
    pub open_documents: usize,
    pub requests: u64,
    pub position_encoding: String,
    /// The server reports it is indexing.
    pub indexing: bool,
}

pub struct SemanticEngine {
    root: PathBuf,
    config: SemanticConfig,
    shared: ViewHandle,
    trees: Arc<TreeCache>,
    lsp: LspPool,
    generation: AtomicU64,
    results: Mutex<HashMap<String, CachedResponse>>,
    clock: AtomicU64,
    inflight: Mutex<HashMap<String, Inflight>>,
    blocking: Arc<Semaphore>,
    baselines: Mutex<HashMap<PathBuf, Baseline>>,
    counters: Counters,
}

impl SemanticEngine {
    /// An engine for `root`, starting servers as processes.
    pub fn new(root: impl Into<PathBuf>, config: SemanticConfig) -> Arc<Self> {
        Self::with_launcher(root, config, Arc::new(ProcessLauncher))
    }

    pub fn with_launcher(
        root: impl Into<PathBuf>,
        config: SemanticConfig,
        launcher: Arc<dyn Launcher>,
    ) -> Arc<Self> {
        let root = root.into();
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        Arc::new(Self {
            trees: Arc::new(TreeCache::new(config.tree_cache_entries)),
            lsp: LspPool::new(config.lsp.clone(), launcher),
            blocking: Arc::new(Semaphore::new(config.max_blocking_tasks.max(1))),
            shared: ViewHandle::new(ViewKind::Shared, HashMap::new()),
            generation: AtomicU64::new(1),
            results: Mutex::new(HashMap::new()),
            clock: AtomicU64::new(0),
            inflight: Mutex::new(HashMap::new()),
            baselines: Mutex::new(HashMap::new()),
            counters: Counters::default(),
            config,
            root,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &SemanticConfig {
        &self.config
    }

    /// The shared view (the disk).
    pub fn shared_view(&self) -> ViewHandle {
        self.shared.clone()
    }

    /// A new private view, for a client's unsaved buffers.
    pub fn open_view(&self) -> ViewHandle {
        ViewHandle::new(ViewKind::Private, HashMap::new())
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Files changed (an applied edit, a shell command, an external
    /// editor): cached responses are dropped. A semantic result may depend
    /// on other files than its own, so any change invalidates them all;
    /// trees are keyed by content and need no invalidation.
    pub fn notify_changed(&self, _paths: &[PathBuf]) {
        self.invalidate();
    }

    /// Something may have changed anywhere (a shell command ran).
    pub fn notify_any_change(&self) {
        self.invalidate();
    }

    fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.results.lock().clear();
    }

    pub fn stats(&self) -> EngineStats {
        EngineStats {
            root: self.root.display().to_string(),
            generation: self.generation(),
            queries: self.counters.queries.load(Ordering::Relaxed),
            result_cache_entries: self.results.lock().len(),
            result_cache_hits: self.counters.result_hits.load(Ordering::Relaxed),
            shared_inflight: self.counters.shared_inflight.load(Ordering::Relaxed),
            tree_cache_entries: self.trees.len(),
            tree_cache_source_bytes: self.trees.source_bytes(),
            tree_hits: self.trees.hits.load(Ordering::Relaxed),
            tree_misses: self.trees.misses.load(Ordering::Relaxed),
            tree_incremental: self.trees.incremental.load(Ordering::Relaxed),
            lsp_launches: self.lsp.launches.load(Ordering::Relaxed),
            lsp_restarts: self.counters.lsp_restarts.load(Ordering::Relaxed),
            lsp_instances: self
                .lsp
                .instances()
                .iter()
                .map(|i| LspInstanceStats {
                    server: i.key.server.clone(),
                    root: i.key.root.display().to_string(),
                    pid: i.client.pid(),
                    open_documents: i.open_documents(),
                    requests: i.requests.load(Ordering::Relaxed),
                    position_encoding: i.encoding.as_lsp().to_string(),
                    indexing: i.client.is_busy(),
                })
                .collect(),
        }
    }

    /// Shut down idle servers (also done at the start of each query).
    pub async fn reap_idle(&self) -> usize {
        self.lsp.reap_idle().await
    }

    /// Shut down every server.
    pub async fn shutdown(&self) {
        self.lsp.shutdown_all().await;
    }

    /// Answer a query. Never panics, never hangs past the configured time
    /// limit; failures are in the response status.
    pub async fn query(self: &Arc<Self>, q: CodeQuery, req: &Requester) -> CodeResponse {
        let started = Instant::now();
        self.counters.queries.fetch_add(1, Ordering::Relaxed);
        let _ = self.lsp.reap_idle().await;

        let (roots, out_of_scope) = match self.resolve_scope(&q, req) {
            Ok(v) => v,
            Err(e) => return error_response(e, started),
        };
        if q.text.trim().is_empty() && q.target.is_none() {
            return error_response(
                "empty query: give a text to look for or a target".into(),
                started,
            );
        }
        let view = req.view.clone().unwrap_or_else(|| self.shared.clone());
        let allow_lsp = req.allow_lsp_start && self.config.lsp.enabled;
        let fingerprint = serde_json::to_string(&(
            &q,
            &roots,
            view.id(),
            view.inner.generation.load(Ordering::SeqCst),
            self.generation(),
            allow_lsp,
            &req.active_file,
        ))
        .unwrap_or_default();

        // Cached (semantic) response.
        {
            let mut cache = self.results.lock();
            let ttl = Duration::from_millis(self.config.result_cache_ttl_ms);
            if let Some(c) = cache.get_mut(&fingerprint) {
                if c.at.elapsed() < ttl {
                    c.used = self.clock.fetch_add(1, Ordering::Relaxed);
                    self.counters.result_hits.fetch_add(1, Ordering::Relaxed);
                    let mut r = (*c.response).clone();
                    r.metrics.result_cache_hit = true;
                    r.metrics.elapsed_ms = started.elapsed().as_millis() as u64;
                    return r;
                }
                cache.remove(&fingerprint);
            }
        }

        // Identical work already running: wait for it. Waiters are counted
        // under the map's lock, so the last one leaving and a new one
        // joining cannot interleave.
        let (fut, id, shared) = {
            let mut inflight = self.inflight.lock();
            let (fut, id, waiters, shared) = match inflight.get(&fingerprint) {
                Some(e) => (e.fut.clone(), e.id, Arc::clone(&e.waiters), true),
                None => {
                    let cancel = CancellationToken::new();
                    let engine = Arc::clone(self);
                    let id = self.clock.fetch_add(1, Ordering::Relaxed);
                    let job = Job {
                        q: q.clone(),
                        roots: roots.clone(),
                        out_of_scope: out_of_scope.clone(),
                        view: view.clone(),
                        allow_lsp,
                        active: req.active_file.clone(),
                        cancel: cancel.clone(),
                        fingerprint: fingerprint.clone(),
                        id,
                    };
                    let fut: BoxFuture<'static, Arc<CodeResponse>> =
                        async move { Arc::new(engine.run_job(job).await) }.boxed();
                    let fut = fut.shared();
                    let waiters = Arc::new(AtomicUsize::new(0));
                    inflight.insert(
                        fingerprint.clone(),
                        Inflight {
                            id,
                            fut: fut.clone(),
                            waiters: Arc::clone(&waiters),
                            cancel,
                        },
                    );
                    (fut, id, waiters, false)
                }
            };
            waiters.fetch_add(1, Ordering::SeqCst);
            (fut, id, shared)
        };
        if shared {
            self.counters
                .shared_inflight
                .fetch_add(1, Ordering::Relaxed);
        }
        let _guard = WaiterGuard {
            engine: Arc::clone(self),
            fingerprint: fingerprint.clone(),
            id,
        };
        let out = tokio::select! {
            r = fut => {
                let mut r = (*r).clone();
                r.metrics.shared_inflight = shared;
                r.metrics.elapsed_ms = started.elapsed().as_millis() as u64;
                r
            }
            _ = req.cancel.cancelled() => {
                let mut r = CodeResponse::empty(ResultStatus::Cancelled);
                r.plan.reason = "cancelled by the requester (shared work continues for other clients)".into();
                r.metrics.elapsed_ms = started.elapsed().as_millis() as u64;
                r
            }
        };
        out
    }

    /// Requester scope ∩ query paths, all inside the workspace.
    fn resolve_scope(
        &self,
        q: &CodeQuery,
        req: &Requester,
    ) -> Result<(Vec<PathBuf>, Vec<String>), String> {
        let mut allowed = Vec::new();
        for s in &req.scope {
            let s = normalize(&self.root, s);
            if s.starts_with(&self.root) {
                allowed.push(s);
            }
        }
        if allowed.is_empty() {
            return Err(format!(
                "the requester's scope is outside this workspace ({})",
                self.root.display()
            ));
        }
        if q.scope.paths.is_empty() {
            return Ok((allowed, Vec::new()));
        }
        let mut roots = Vec::new();
        let mut outside = Vec::new();
        for p in &q.scope.paths {
            let abs = normalize(&self.root, Path::new(p));
            if allowed.iter().any(|a| abs.starts_with(a)) {
                roots.push(abs);
            } else {
                outside.push(p.clone());
            }
        }
        if roots.is_empty() {
            return Err(format!(
                "every requested path is outside your scope: {}",
                outside.join(", ")
            ));
        }
        Ok((roots, outside))
    }

    fn in_scope(&self, roots: &[PathBuf], p: &Path) -> bool {
        roots.iter().any(|r| p.starts_with(r))
    }

    async fn run_job(self: Arc<Self>, job: Job) -> CodeResponse {
        let generation_at_start = self.generation();
        let fingerprint = job.fingerprint.clone();
        let id = job.id;
        let mut resp = self.execute(job).await;
        resp.generation = generation_at_start;
        {
            let mut inflight = self.inflight.lock();
            if inflight.get(&fingerprint).is_some_and(|e| e.id == id) {
                inflight.remove(&fingerprint);
            }
        }
        let used_lsp = resp
            .plan
            .steps
            .iter()
            .any(|s| s.backend == Backend::Lsp && s.outcome == StepOutcome::Ok);
        let cacheable = matches!(
            resp.status,
            ResultStatus::Complete | ResultStatus::Ambiguous
        );
        if used_lsp && cacheable && self.generation() == generation_at_start {
            let mut cache = self.results.lock();
            if cache.len() >= self.config.result_cache_entries.max(1) {
                if let Some(oldest) = cache
                    .iter()
                    .min_by_key(|(_, c)| c.used)
                    .map(|(k, _)| k.clone())
                {
                    cache.remove(&oldest);
                }
            }
            cache.insert(
                fingerprint,
                CachedResponse {
                    response: Arc::new(resp.clone()),
                    at: Instant::now(),
                    used: self.clock.fetch_add(1, Ordering::Relaxed),
                },
            );
        }
        resp
    }

    async fn execute(self: &Arc<Self>, job: Job) -> CodeResponse {
        let started = Instant::now();
        let timeout = job
            .q
            .limits
            .timeout_ms
            .unwrap_or(self.config.timeout_ms)
            .min(self.config.timeout_ms);
        let deadline = started + Duration::from_millis(timeout);
        let max_file_bytes = job
            .q
            .limits
            .max_file_bytes
            .unwrap_or(self.config.max_file_bytes)
            .min(self.config.max_file_bytes);
        let snapshot = Arc::new(Snapshot::new(job.view.clone(), max_file_bytes));
        let (intent, reason) = planner::route(&job.q);
        let mut ex = Exec {
            engine: Arc::clone(self),
            q: job.q.clone(),
            snapshot: Arc::clone(&snapshot),
            roots: job.roots.clone(),
            deadline,
            cancel: job.cancel.clone(),
            allow_lsp: job.allow_lsp,
            steps: Vec::new(),
            fallbacks: Vec::new(),
            omissions: Vec::new(),
            metrics: QueryMetrics::default(),
            partial: false,
            continuation: Vec::new(),
            out_of_scope_results: 0,
            server_indexing: false,
        };
        if !job.out_of_scope.is_empty() {
            ex.omissions.push(Omission::OutOfScope {
                paths: job.out_of_scope.clone(),
            });
            ex.partial = true;
        }
        let outcome = tokio::select! {
            o = ex.run(intent) => o,
            _ = tokio::time::sleep_until(deadline.into()) => Outcome::status(ResultStatus::Partial),
        };
        let Outcome {
            mut cands,
            mut status,
            ambiguity,
            diagnostics,
            strategy,
        } = outcome;
        if job.cancel.is_cancelled() {
            status = ResultStatus::Cancelled;
        }
        if Instant::now() >= deadline {
            ex.omissions.push(Omission::Deadline { ms: timeout });
            if status == ResultStatus::Complete {
                status = ResultStatus::Partial;
            }
        }
        if ex.out_of_scope_results > 0 {
            ex.omissions.push(Omission::ItemsOmitted {
                reason: "outside your scope".into(),
                count: ex.out_of_scope_results,
            });
        }
        if ex.partial && status == ResultStatus::Complete {
            status = ResultStatus::Partial;
        }

        // Files that changed while we worked: their items are stale.
        let paths: Vec<PathBuf> = {
            let mut v: Vec<PathBuf> = cands.iter().map(|c| c.doc.path.clone()).collect();
            v.sort();
            v.dedup();
            v
        };
        let changed = snapshot.changed_since(&paths);
        for c in cands.iter_mut() {
            if changed.contains(&c.doc.path) {
                c.freshness = Freshness::Stale {
                    reason:
                        "the file changed during the query; positions refer to the version read"
                            .into(),
                };
            }
        }

        let name_for_rank = match &ambiguity {
            Some(a) => Some(a.name.clone()),
            None => planner::is_identifier_like(&job.q.text)
                .then(|| planner::split_qualified(&job.q.text).0),
        };
        rank::score(
            intent,
            name_for_rank.as_deref(),
            job.active.as_deref(),
            &mut cands,
        );
        let cands = rank::merge_and_sort(cands);
        ex.metrics.candidates = cands.len();

        let budget = job
            .q
            .limits
            .budget_tokens
            .unwrap_or(self.config.default_budget_tokens)
            .min(self.config.max_budget_tokens);
        let max_results = job
            .q
            .limits
            .max_results
            .unwrap_or(self.config.max_results)
            .min(self.config.max_results);
        let mut resp = CodeResponse::empty(status);
        resp.plan = PlanReport {
            intent: intent.as_str().to_string(),
            strategy,
            reason,
            steps: std::mem::take(&mut ex.steps),
            fallbacks: std::mem::take(&mut ex.fallbacks),
        };
        resp.ambiguity = ambiguity;
        resp.diagnostics = diagnostics;
        let header = render::render_header(&resp);
        let settings = ContextSettings {
            // The executor may have narrowed it (an outline shows signatures).
            policy: ex.q.context,
            detail: job.q.detail,
            budget_tokens: budget,
            max_results,
        };
        let root = self.root.clone();
        let trees = Arc::clone(&self.trees);
        let ctx_deadline = deadline.max(Instant::now() + Duration::from_millis(500));
        let built = {
            let _permit = self.blocking.acquire().await;
            tokio::task::spawn_blocking(move || {
                context::build(&root, cands, &trees, &settings, ctx_deadline, &header)
            })
            .await
            .unwrap_or_default()
        };
        if built.omitted_budget > 0 {
            ex.omissions.push(Omission::ItemsOmitted {
                reason: "context budget".into(),
                count: built.omitted_budget,
            });
            ex.continuation
                .push("raise budget_tokens, narrow paths, or use detail=compact".into());
        }
        if built.omitted_max > 0 {
            ex.omissions.push(Omission::ItemsOmitted {
                reason: format!("max_results ({max_results})"),
                count: built.omitted_max,
            });
            ex.continuation
                .push("narrow the scope (paths, extensions) to see the rest".into());
        }
        ex.metrics.files_parsed += built.files_parsed;
        ex.metrics.tree_cache_hits += built.tree_hits;
        resp.items = built.items;
        resp.budget = built.budget;
        resp.omissions = std::mem::take(&mut ex.omissions);
        let mut continuation = std::mem::take(&mut ex.continuation);
        continuation.dedup();
        resp.continuation = continuation;
        resp.metrics = ex.metrics;
        resp.metrics.elapsed_ms = started.elapsed().as_millis() as u64;
        resp
    }

    // ── Diagnostics baselines ────────────────────────────────────────────

    fn compare_baseline(
        &self,
        path: &Path,
        hash: &str,
        diags: &[(u8, String, String)],
    ) -> Option<(usize, usize, usize, String)> {
        let mut baselines = self.baselines.lock();
        let prev = baselines.get(path).cloned();
        baselines.insert(
            path.to_path_buf(),
            Baseline {
                hash: hash.to_string(),
                diags: diags.to_vec(),
            },
        );
        let prev = prev.filter(|b| b.hash != hash)?;
        // Multiset comparison on (severity, code, message): positions move
        // with edits, the diagnostic stays the same.
        let mut before: BTreeMap<&(u8, String, String), usize> = BTreeMap::new();
        for d in &prev.diags {
            *before.entry(d).or_default() += 1;
        }
        let mut introduced = 0;
        let mut preexisting = 0;
        for d in diags {
            match before.get_mut(d) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    preexisting += 1;
                }
                _ => introduced += 1,
            }
        }
        let resolved = before.values().sum();
        Some((introduced, preexisting, resolved, prev.hash))
    }
}

struct WaiterGuard {
    engine: Arc<SemanticEngine>,
    fingerprint: String,
    id: u64,
}

impl Drop for WaiterGuard {
    /// The last waiter gone: nobody needs the work any more. It is
    /// cancelled and forgotten, so a later identical query starts afresh.
    fn drop(&mut self) {
        let mut inflight = self.engine.inflight.lock();
        let last = match inflight.get(&self.fingerprint) {
            Some(e) if e.id == self.id => e.waiters.fetch_sub(1, Ordering::SeqCst) == 1,
            _ => false,
        };
        if last {
            if let Some(e) = inflight.remove(&self.fingerprint) {
                e.cancel.cancel();
            }
        }
    }
}

struct Job {
    q: CodeQuery,
    roots: Vec<PathBuf>,
    out_of_scope: Vec<String>,
    view: ViewHandle,
    allow_lsp: bool,
    active: Option<PathBuf>,
    cancel: CancellationToken,
    fingerprint: String,
    id: u64,
}

fn error_response(message: String, started: Instant) -> CodeResponse {
    let mut r = CodeResponse::empty(ResultStatus::Error { message });
    r.metrics.elapsed_ms = started.elapsed().as_millis() as u64;
    r
}

/// Resolve `p` (relative to `root`) and `..` lexically; canonical when it
/// exists.
fn normalize(root: &Path, p: &Path) -> PathBuf {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    if let Ok(c) = std::fs::canonicalize(&joined) {
        return c;
    }
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

struct Outcome {
    cands: Vec<Candidate>,
    status: ResultStatus,
    ambiguity: Option<Ambiguity>,
    diagnostics: Option<DiagnosticsReport>,
    strategy: String,
}

impl Outcome {
    fn status(status: ResultStatus) -> Self {
        Self {
            cands: Vec::new(),
            status,
            ambiguity: None,
            diagnostics: None,
            strategy: String::new(),
        }
    }
}

/// One query's execution state.
struct Exec {
    engine: Arc<SemanticEngine>,
    q: CodeQuery,
    snapshot: Arc<Snapshot>,
    roots: Vec<PathBuf>,
    deadline: Instant,
    cancel: CancellationToken,
    allow_lsp: bool,
    steps: Vec<Step>,
    fallbacks: Vec<String>,
    omissions: Vec<Omission>,
    metrics: QueryMetrics,
    /// Some work stopped at a limit.
    partial: bool,
    continuation: Vec<String>,
    out_of_scope_results: usize,
    /// The server was still indexing when asked.
    server_indexing: bool,
}

/// A resolved target.
struct At {
    doc: Arc<Document>,
    offset: usize,
}

impl Exec {
    fn step(
        &mut self,
        backend: Backend,
        action: &str,
        outcome: StepOutcome,
        t: Instant,
        note: Option<String>,
    ) {
        self.steps.push(Step {
            backend,
            action: action.to_string(),
            outcome,
            duration_ms: t.elapsed().as_millis() as u64,
            note,
        });
    }

    fn limit(&self, asked: Option<usize>, cap: usize) -> usize {
        asked.unwrap_or(cap).min(cap)
    }

    async fn run(&mut self, intent: Intent) -> Outcome {
        match intent {
            Intent::TextSearch | Intent::Auto => self.text_search().await,
            Intent::FindSymbol => {
                let o = self.find_symbol().await;
                // `auto` on an identifier: no definition → text search.
                if self.q.intent == Intent::Auto && o.cands.is_empty() {
                    self.fallbacks
                        .push("no definition with that name: text search instead".into());
                    let mut t = self.text_search().await;
                    t.strategy = format!("{} → text", o.strategy);
                    return t;
                }
                o
            }
            Intent::Definition | Intent::References => self.navigate(intent).await,
            Intent::Understand => match self.q.target {
                Some(Target::File { .. }) => self.outline().await,
                _ => self.understand().await,
            },
            Intent::Diagnostics => self.diagnostics().await,
        }
    }

    fn filters(&self, code_only: bool) -> WalkFilters {
        let mut f = WalkFilters {
            exclude: self.q.scope.exclude.clone(),
            extensions: self.q.scope.extensions.clone(),
            ..Default::default()
        };
        if code_only {
            if f.extensions.is_empty() {
                f.extensions = CODE_EXTENSIONS.iter().map(|s| s.to_string()).collect();
            } else {
                f.extensions
                    .retain(|e| CODE_EXTENSIONS.contains(&e.trim_start_matches('.')));
            }
        }
        f
    }

    /// Run a lexical search (blocking pool), recording limits.
    async fn lexical(
        &mut self,
        pattern: &str,
        mode: MatchMode,
        case_insensitive: bool,
        filters: WalkFilters,
        action: &str,
    ) -> Option<lexical::LexicalOutcome> {
        let t = Instant::now();
        let matcher = match lexical::build_matcher(pattern, mode, case_insensitive) {
            Ok(m) => m,
            Err(e) => {
                self.step(
                    Backend::Lexical,
                    action,
                    StepOutcome::Failed,
                    t,
                    Some(e.clone()),
                );
                return None;
            }
        };
        let cfg = &self.engine.config;
        let max_files = self.limit(self.q.limits.max_files, cfg.max_files);
        let max_matches = self.limit(self.q.limits.max_matches, cfg.max_matches);
        let snapshot = Arc::clone(&self.snapshot);
        let roots = self.roots.clone();
        let deadline = self.deadline;
        let cancel = self.cancel.clone();
        let permit = Arc::clone(&self.engine.blocking).acquire_owned().await;
        let res = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            lexical::search(
                &snapshot,
                &matcher,
                &roots,
                &filters,
                max_files,
                max_matches,
                deadline,
                &cancel,
            )
        })
        .await;
        let out = match res {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                self.step(Backend::Lexical, action, StepOutcome::Failed, t, Some(e));
                return None;
            }
            Err(e) => {
                self.step(
                    Backend::Lexical,
                    action,
                    StepOutcome::Failed,
                    t,
                    Some(e.to_string()),
                );
                return None;
            }
        };
        self.metrics.files_listed += out.files_listed;
        self.metrics.files_searched += out.files_searched;
        self.metrics.bytes_searched += out.bytes_searched;
        let mut limited = false;
        if out.file_limit_hit {
            self.omissions.push(Omission::LimitReached {
                limit: "max_files".into(),
                value: max_files as u64,
            });
            limited = true;
        }
        if out.match_limit_hit {
            self.omissions.push(Omission::LimitReached {
                limit: "max_matches".into(),
                value: max_matches as u64,
            });
            limited = true;
        }
        if out.deadline_hit {
            limited = true;
        }
        for (reason, files) in &out.skipped {
            self.omissions.push(Omission::FilesSkipped {
                reason: reason.clone(),
                count: files.len(),
                examples: files
                    .iter()
                    .take(3)
                    .map(|f| rel_path(&self.engine.root, Path::new(f)))
                    .collect(),
            });
        }
        if limited {
            self.partial = true;
            self.continuation
                .push("narrow the search (paths, extensions, a more specific pattern)".into());
        }
        let outcome = if out.cancelled {
            StepOutcome::Cancelled
        } else if limited {
            StepOutcome::Partial
        } else {
            StepOutcome::Ok
        };
        let note = format!(
            "{} files, {} matches",
            out.files_searched,
            out.matches.len()
        );
        self.step(Backend::Lexical, action, outcome, t, Some(note));
        Some(out)
    }

    async fn text_search(&mut self) -> Outcome {
        let mode = self.q.mode;
        let text = self.q.text.clone();
        let ci = self.q.case_insensitive;
        let f = self.filters(false);
        let action = match mode {
            MatchMode::Literal => "literal search",
            MatchMode::Regex => "regex search",
        };
        let Some(out) = self.lexical(&text, mode, ci, f, action).await else {
            return Outcome {
                status: ResultStatus::Error {
                    message: "the search could not run (see steps)".into(),
                },
                ..Outcome::status(ResultStatus::Partial)
            };
        };
        let method = match mode {
            MatchMode::Literal => "literal match",
            MatchMode::Regex => "regex match",
        };
        let cands = out
            .matches
            .into_iter()
            .map(|m| {
                Candidate::new(
                    m.doc,
                    m.start,
                    m.end,
                    Relation::TextMention,
                    Certainty::Textual,
                    Provenance {
                        backend: Backend::Lexical,
                        method: method.into(),
                    },
                )
            })
            .collect();
        Outcome {
            cands,
            status: ResultStatus::Complete,
            ambiguity: None,
            diagnostics: None,
            strategy: "text".into(),
        }
    }

    /// Definitions named `name` (qualifier optional), from the syntax trees
    /// of files that mention the name.
    async fn definitions_named(&mut self, qualified: &str) -> Vec<Candidate> {
        let (name, qualifier) = planner::split_qualified(qualified);
        if name.is_empty() {
            return Vec::new();
        }
        let pattern = format!(r"\b{}\b", planner::regex_escape(&name));
        let ci = self.q.case_insensitive;
        let f = self.filters(true);
        let Some(out) = self
            .lexical(&pattern, MatchMode::Regex, ci, f, "candidate files")
            .await
        else {
            return Vec::new();
        };
        let mut docs: Vec<Arc<Document>> = Vec::new();
        for m in out.matches {
            if docs.last().map(|d| d.path != m.doc.path).unwrap_or(true) {
                docs.push(m.doc);
            }
        }
        let max_parse = self.engine.config.max_parsed_files;
        if docs.len() > max_parse {
            self.omissions.push(Omission::LimitReached {
                limit: "max_parsed_files".into(),
                value: max_parse as u64,
            });
            self.partial = true;
            docs.truncate(max_parse);
        }
        let t = Instant::now();
        let trees = Arc::clone(&self.engine.trees);
        let deadline = self.deadline;
        let cancel = self.cancel.clone();
        let permit = Arc::clone(&self.engine.blocking).acquire_owned().await;
        let name2 = name.clone();
        let res = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut found = Vec::new();
            let (mut parsed, mut hits, mut failed) = (0usize, 0usize, 0usize);
            for d in docs {
                if cancel.is_cancelled() || Instant::now() >= deadline {
                    break;
                }
                match trees.parse(&d, deadline) {
                    Ok((p, hit)) => {
                        if hit {
                            hits += 1;
                        } else {
                            parsed += 1;
                        }
                        for def in syntax::definitions(&p) {
                            let same = if ci {
                                def.name.eq_ignore_ascii_case(&name2)
                            } else {
                                def.name == name2
                            };
                            let container_ok = match &qualifier {
                                Some(q) => def.container.as_deref() == Some(q.as_str()),
                                None => true,
                            };
                            if same && container_ok {
                                found.push((Arc::clone(&d), def, p.has_errors));
                            }
                        }
                    }
                    Err(_) => failed += 1,
                }
            }
            (found, parsed, hits, failed)
        })
        .await;
        let Ok((found, parsed, hits, failed)) = res else {
            self.step(Backend::Syntax, "definitions", StepOutcome::Failed, t, None);
            return Vec::new();
        };
        self.metrics.files_parsed += parsed;
        self.metrics.tree_cache_hits += hits;
        let note = format!("{} definition(s) in {} file(s)", found.len(), parsed + hits);
        self.step(
            Backend::Syntax,
            "definitions",
            if failed > 0 {
                StepOutcome::Partial
            } else {
                StepOutcome::Ok
            },
            t,
            Some(note),
        );
        found
            .into_iter()
            .map(|(doc, def, errors)| {
                let mut c = Candidate::new(
                    doc,
                    def.name_start,
                    def.name_end,
                    Relation::Candidate,
                    Certainty::Syntactic,
                    Provenance {
                        backend: Backend::Syntax,
                        method: format!("tree-sitter {}", def.node_kind),
                    },
                );
                c.symbol = Some(SymbolRef {
                    name: def.name.clone(),
                    kind: def.kind.clone(),
                });
                c.signature = Some(match &def.container {
                    Some(k) => format!("{} (in {k})", def.signature),
                    None => def.signature.clone(),
                });
                c.scope = Some((def.start, def.end, def.node_kind.clone()));
                if errors {
                    c.documentation = Some("note: the file has syntax errors".into());
                }
                c
            })
            .collect()
    }

    async fn find_symbol(&mut self) -> Outcome {
        let text = self.q.text.clone();
        if !planner::is_identifier_like(&text) {
            self.fallbacks
                .push("not an identifier: text search instead of symbol search".into());
            return self.text_search().await;
        }
        let mut cands = self.definitions_named(&text).await;
        let confirmed = self.confirm_with_workspace_symbols(&text, &mut cands).await;
        let defs = cands
            .iter()
            .filter(|c| matches!(c.relation, Relation::Candidate | Relation::Definition))
            .count();
        let ambiguity = (defs > 1).then(|| Ambiguity {
            name: text.clone(),
            candidates: defs,
            hint: "pass one item id as `target` (or a file/line) to choose".into(),
        });
        if defs == 1 {
            for c in cands
                .iter_mut()
                .filter(|c| c.relation == Relation::Candidate)
            {
                c.relation = Relation::Definition;
            }
        }
        Outcome {
            cands,
            status: ResultStatus::Complete,
            ambiguity,
            diagnostics: None,
            strategy: if confirmed {
                "syntax + lsp workspace/symbol".into()
            } else {
                "syntax".into()
            },
        }
    }

    /// With a server already running (none is started for this), confirm
    /// syntactic definitions with `workspace/symbol` and add the ones the
    /// syntax cannot see (macro-generated...). Returns whether it ran.
    async fn confirm_with_workspace_symbols(
        &mut self,
        qualified: &str,
        cands: &mut Vec<Candidate>,
    ) -> bool {
        let (name, _) = planner::split_qualified(qualified);
        let inst = cands
            .iter()
            .find_map(|c| self.engine.lsp.running_for(&c.doc.path, &self.engine.root))
            .or_else(|| self.engine.lsp.instances().into_iter().next());
        let Some(inst) = inst else {
            return false;
        };
        if !inst
            .client
            .capabilities()
            .is_some_and(|c| c.workspace_symbol)
        {
            return false;
        }
        if !self.snapshot.view.is_shared() {
            return false;
        }
        let t = Instant::now();
        self.metrics.lsp_requests += 1;
        let r = self
            .engine
            .lsp
            .request(
                &inst,
                "workspace/symbol",
                json!({ "query": name }),
                &self.cancel,
            )
            .await;
        self.metrics.lsp_ms += t.elapsed().as_millis() as u64;
        let v = match r {
            Ok(v) => v,
            Err(e) => {
                self.step(
                    Backend::Lsp,
                    "workspace/symbol",
                    StepOutcome::Failed,
                    t,
                    Some(e.to_string()),
                );
                return false;
            }
        };
        let mut added = 0;
        let mut upgraded = 0;
        for sym in v.as_array().into_iter().flatten() {
            if sym["name"].as_str() != Some(name.as_str()) {
                continue;
            }
            let Some(uri) = sym["location"]["uri"].as_str() else {
                continue;
            };
            let path = PathBuf::from(cersei_lsp::uri_to_path(uri));
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if !self.engine.in_scope(&self.roots, &path) {
                self.out_of_scope_results += 1;
                continue;
            }
            let Ok(doc) = self.snapshot.document(&path) else {
                continue;
            };
            // A `WorkspaceSymbol` may come without a range: the file only.
            let range = cersei_lsp::parse_locations(&sym["location"])
                .into_iter()
                .next();
            let start = range
                .as_ref()
                .and_then(|r| doc.mapper().from_lsp(&r.range.start, inst.encoding).ok());
            let end = range
                .as_ref()
                .and_then(|r| doc.mapper().from_lsp(&r.range.end, inst.encoding).ok());
            let prov = Provenance {
                backend: Backend::Lsp,
                method: "workspace/symbol".into(),
            };
            let matched = cands.iter_mut().find(|c| {
                c.doc.path == path
                    && match (start, end, &c.scope) {
                        (Some(s), Some(e), Some((a, b, _))) => s < *b && *a < e.max(s + 1),
                        (Some(s), _, _) => c.start == s,
                        (None, _, _) => true,
                    }
            });
            match matched {
                Some(c) => {
                    c.certainty = Certainty::Confirmed;
                    if !c.provenance.contains(&prov) {
                        c.provenance.push(prov);
                    }
                    upgraded += 1;
                }
                None => {
                    let Some(s) = start else { continue };
                    let e = end.unwrap_or(s).max(s);
                    // Servers list re-exports (`pub use x::name`) as symbols:
                    // inside an import, it is a reference, not a definition.
                    let in_import = self
                        .parse_doc(&doc)
                        .is_some_and(|p| syntax::inside_import(&p, s));
                    let relation = if in_import {
                        Relation::Reference
                    } else {
                        Relation::Candidate
                    };
                    let mut c = Candidate::new(doc, s, e, relation, Certainty::Confirmed, prov);
                    if in_import {
                        c.signature = Some("re-export / import".into());
                    }
                    c.symbol = Some(SymbolRef {
                        name: name.clone(),
                        kind: cersei_lsp::symbol_kind_name(sym["kind"].as_u64().unwrap_or(0))
                            .to_string(),
                    });
                    cands.push(c);
                    added += 1;
                }
            }
        }
        self.step(
            Backend::Lsp,
            "workspace/symbol",
            StepOutcome::Ok,
            t,
            Some(format!("{upgraded} confirmed, {added} added")),
        );
        true
    }

    /// A file's outline: document symbols from the server when it can
    /// answer, else the syntax tree's definitions. Signatures only.
    async fn outline(&mut self) -> Outcome {
        let at = match self.q.target.clone().map(|t| self.resolve_target(&t)) {
            Some(Ok(at)) => at,
            Some(Err(e)) => return Outcome::status(ResultStatus::Error { message: e }),
            None => {
                return Outcome::status(ResultStatus::Error {
                    message: "no file".into(),
                })
            }
        };
        if self.q.context == ContextPolicy::Auto {
            self.q.context = ContextPolicy::None;
        }
        let lsp_reason: Option<String>;
        match self.instance(&at.doc).await {
            Ok(inst)
                if inst
                    .client
                    .capabilities()
                    .is_some_and(|c| c.document_symbol) =>
            {
                let t = Instant::now();
                let r = match self.engine.lsp.sync(&inst, &at.doc).await {
                    Ok(synced) => {
                        self.metrics.lsp_requests += 1;
                        let r = self
                            .engine
                            .lsp
                            .request(
                                &inst,
                                "textDocument/documentSymbol",
                                json!({ "textDocument": { "uri": synced.uri } }),
                                &self.cancel,
                            )
                            .await;
                        drop(synced);
                        r
                    }
                    Err(e) => Err(e),
                };
                self.metrics.lsp_ms += t.elapsed().as_millis() as u64;
                match r {
                    Ok(v) => {
                        self.step(
                            Backend::Lsp,
                            "textDocument/documentSymbol",
                            StepOutcome::Ok,
                            t,
                            None,
                        );
                        let mut cands = Vec::new();
                        self.collect_document_symbols(&inst, &at.doc, &v, None, &mut cands);
                        return Outcome {
                            cands,
                            status: ResultStatus::Complete,
                            ambiguity: None,
                            diagnostics: None,
                            strategy: "lsp outline".into(),
                        };
                    }
                    Err(e) => {
                        self.step(
                            Backend::Lsp,
                            "textDocument/documentSymbol",
                            StepOutcome::Failed,
                            t,
                            Some(e.to_string()),
                        );
                        lsp_reason = Some(e.to_string());
                    }
                }
            }
            Ok(_) => {
                lsp_reason = Some("the server does not support textDocument/documentSymbol".into())
            }
            Err(reason) => lsp_reason = Some(reason),
        }
        if let Some(r) = &lsp_reason {
            self.fallbacks.push(format!(
                "semantic outline unavailable ({r}): syntax outline"
            ));
        }
        let Some(p) = self.parse_doc(&at.doc) else {
            return Outcome::status(ResultStatus::Unavailable {
                reason: format!(
                    "no outline: {} and no grammar for this file",
                    lsp_reason.unwrap_or_default()
                ),
            });
        };
        let cands = syntax::definitions(&p)
            .into_iter()
            .map(|d| {
                let mut c = Candidate::new(
                    Arc::clone(&at.doc),
                    d.name_start,
                    d.name_end,
                    Relation::Definition,
                    Certainty::Syntactic,
                    Provenance {
                        backend: Backend::Syntax,
                        method: format!("tree-sitter {}", d.node_kind),
                    },
                );
                c.signature = Some(match &d.container {
                    Some(k) => format!("{} (in {k})", d.signature),
                    None => d.signature.clone(),
                });
                c.symbol = Some(SymbolRef {
                    name: d.name,
                    kind: d.kind,
                });
                c
            })
            .collect();
        Outcome {
            cands,
            status: ResultStatus::Partial,
            ambiguity: None,
            diagnostics: None,
            strategy: "syntax outline".into(),
        }
    }

    fn collect_document_symbols(
        &mut self,
        inst: &Instance,
        doc: &Arc<Document>,
        v: &Value,
        container: Option<&str>,
        out: &mut Vec<Candidate>,
    ) {
        for sym in v.as_array().into_iter().flatten() {
            let name = sym["name"].as_str().unwrap_or("?").to_string();
            // `DocumentSymbol` (selectionRange) or `SymbolInformation` (location).
            let range = if sym["selectionRange"].is_object() {
                &sym["selectionRange"]
            } else if sym["range"].is_object() {
                &sym["range"]
            } else {
                &sym["location"]["range"]
            };
            let pos = |k: &str| -> Option<usize> {
                let p = cersei_lsp::Position {
                    line: range[k]["line"].as_u64()? as u32,
                    character: range[k]["character"].as_u64()? as u32,
                };
                doc.mapper().from_lsp(&p, inst.encoding).ok()
            };
            if let (Some(s), Some(e)) = (pos("start"), pos("end")) {
                let mut c = Candidate::new(
                    Arc::clone(doc),
                    s,
                    e.max(s),
                    Relation::Definition,
                    Certainty::Confirmed,
                    Provenance {
                        backend: Backend::Lsp,
                        method: "textDocument/documentSymbol".into(),
                    },
                );
                c.symbol = Some(SymbolRef {
                    name: name.clone(),
                    kind: cersei_lsp::symbol_kind_name(sym["kind"].as_u64().unwrap_or(0))
                        .to_string(),
                });
                let line = doc
                    .mapper()
                    .line_of(s)
                    .ok()
                    .and_then(|l| doc.mapper().line_text(l).ok());
                let sig = line
                    .map(|l| syntax::truncate_chars(l.trim(), 160))
                    .unwrap_or_default();
                c.signature = Some(match container {
                    Some(k) => format!("{sig} (in {k})"),
                    None => sig,
                });
                out.push(c);
            }
            if sym["children"].is_array() {
                self.collect_document_symbols(inst, doc, &sym["children"], Some(&name), out);
            }
        }
    }

    // ── Targets ──────────────────────────────────────────────────────────

    fn resolve_target(&mut self, t: &Target) -> Result<At, String> {
        let path = match t {
            Target::Position { path, .. } | Target::Offset { path, .. } | Target::File { path } => {
                path.clone()
            }
            Target::Item { id } => {
                parse_item_id(id)
                    .ok_or_else(|| format!("malformed item id `{id}`"))?
                    .0
            }
        };
        let abs = normalize(&self.engine.root, Path::new(&path));
        if !self.engine.in_scope(&self.roots, &abs) {
            return Err(format!("`{path}` is outside your scope"));
        }
        let doc = self
            .snapshot
            .document(&abs)
            .map_err(|e| format!("cannot read `{path}`: {}", e.label()))?;
        let m = doc.mapper();
        let offset = match t {
            Target::Position { line, column, .. } => {
                let l = line.checked_sub(1).ok_or("lines start at 1")?;
                let range = m.line_range(l).map_err(|e| e.to_string())?;
                let text = &doc.text[range.clone()];
                let skip = column.saturating_sub(1) as usize;
                range.start
                    + text
                        .char_indices()
                        .nth(skip)
                        .map(|(i, _)| i)
                        .unwrap_or(text.len())
            }
            Target::Offset { offset, .. } => {
                m.offset_to_line_col(*offset).map_err(|e| e.to_string())?;
                *offset
            }
            Target::File { .. } => 0,
            Target::Item { id } => {
                let (_, start, _, hash) = parse_item_id(id).unwrap_or_default();
                if hash != doc.revision.hash {
                    return Err(format!(
                        "item `{id}` refers to revision {hash}; `{path}` is now at {} — search again",
                        doc.revision.hash
                    ));
                }
                m.offset_to_line_col(start).map_err(|e| e.to_string())?;
                start
            }
        };
        Ok(At { doc, offset })
    }

    fn parse_doc(&mut self, doc: &Document) -> Option<syntax::Parsed> {
        Lang::for_path(&doc.path)?;
        match self.engine.trees.parse(doc, self.deadline) {
            Ok((p, hit)) => {
                if hit {
                    self.metrics.tree_cache_hits += 1;
                } else {
                    self.metrics.files_parsed += 1;
                }
                Some(p)
            }
            Err(_) => None,
        }
    }

    // ── LSP ──────────────────────────────────────────────────────────────

    async fn instance(&mut self, doc: &Document) -> Result<Arc<Instance>, String> {
        if !self.snapshot.view.is_shared() && self.snapshot.view.overlays(&doc.path) {
            return Err(Unavailable::PrivateView.reason());
        }
        let t = Instant::now();
        match self
            .engine
            .lsp
            .instance_for(&doc.path, &self.engine.root, self.allow_lsp)
            .await
        {
            Ok((inst, restarted)) => {
                self.await_ready(&inst).await;
                if restarted {
                    self.engine
                        .counters
                        .lsp_restarts
                        .fetch_add(1, Ordering::Relaxed);
                    self.engine.invalidate();
                    self.step(Backend::Lsp, "restart server", StepOutcome::Ok, t, None);
                }
                self.resync_open_documents(&inst).await;
                Ok(inst)
            }
            Err(u) => Err(u.reason()),
        }
    }

    /// A server that says it is indexing: wait (bounded), else go on and
    /// mark the response partial — its answers may be incomplete.
    async fn await_ready(&mut self, inst: &Instance) {
        let t = Instant::now();
        // Just started and silent so far: give it a moment to say whether
        // it is indexing (a silent server is then taken as ready).
        let settle = Duration::from_millis(self.engine.config.lsp.startup_settle_ms);
        let age = inst.started.elapsed();
        if !inst.client.reports_activity() && age < settle {
            let wait = (settle - age).min(self.deadline.saturating_duration_since(Instant::now()));
            tokio::select! {
                _ = inst.client.wait_first_report(wait) => {}
                _ = self.cancel.cancelled() => {}
            }
        }
        if !inst.client.is_busy() {
            return;
        }
        let what = inst.client.busy_with().join(", ");
        // Keep a fifth of the remaining time to ask anyway and report.
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let wait = Duration::from_millis(self.engine.config.lsp.ready_wait_ms)
            .min(remaining.saturating_sub(remaining / 5));
        let ready = tokio::select! {
            r = inst.client.wait_ready(wait) => r,
            _ = self.cancel.cancelled() => false,
        };
        if ready {
            self.step(
                Backend::Lsp,
                "wait for indexing",
                StepOutcome::Ok,
                t,
                Some(what),
            );
        } else {
            self.step(
                Backend::Lsp,
                "wait for indexing",
                StepOutcome::Partial,
                t,
                Some(what.clone()),
            );
            self.partial = true;
            self.fallbacks.push(format!(
                "the language server is still indexing ({what}): its answers may be incomplete — ask again later"
            ));
            self.continuation
                .push("ask again once the language server has finished indexing".into());
        }
    }

    /// After a change anywhere, bring the server's open documents up to
    /// date with the disk (bounded by `max_open_documents`).
    async fn resync_open_documents(&mut self, inst: &Instance) {
        let generation = self.engine.generation();
        if inst.synced_generation.load(Ordering::SeqCst) >= generation {
            return;
        }
        for p in inst.open_paths() {
            if let Ok(doc) = self.snapshot.document(&p) {
                let _ = self.engine.lsp.sync(inst, &doc).await;
            }
        }
        inst.synced_generation.store(generation, Ordering::SeqCst);
    }

    async fn lsp_request(
        &mut self,
        inst: &Instance,
        at: &At,
        method: &str,
        extra: Value,
    ) -> Result<Value, String> {
        let t = Instant::now();
        let synced = match self.engine.lsp.sync(inst, &at.doc).await {
            Ok(s) => s,
            Err(e) => {
                self.step(
                    Backend::Lsp,
                    "didOpen/didChange",
                    StepOutcome::Failed,
                    t,
                    Some(e.to_string()),
                );
                return Err(e.to_string());
            }
        };
        let pos = at
            .doc
            .mapper()
            .to_lsp(at.offset, inst.encoding)
            .map_err(|e| e.to_string())?;
        let mut params = json!({
            "textDocument": { "uri": synced.uri },
            "position": { "line": pos.line, "character": pos.character },
        });
        if let (Value::Object(p), Value::Object(e)) = (&mut params, extra) {
            p.extend(e);
        }
        self.metrics.lsp_requests += 1;
        let r = self
            .engine
            .lsp
            .request(inst, method, params, &self.cancel)
            .await;
        self.metrics.lsp_ms += t.elapsed().as_millis() as u64;
        drop(synced);
        match r {
            Ok(v) => {
                self.step(
                    Backend::Lsp,
                    method,
                    StepOutcome::Ok,
                    t,
                    Some(format!("version-synced, {}", inst.encoding.as_lsp())),
                );
                Ok(v)
            }
            Err(e) => {
                let outcome = match e {
                    cersei_lsp::LspError::Cancelled => StepOutcome::Cancelled,
                    _ => StepOutcome::Failed,
                };
                self.step(Backend::Lsp, method, outcome, t, Some(e.to_string()));
                Err(e.to_string())
            }
        }
    }

    /// LSP locations → candidates, in scope, mapped on the snapshot.
    fn map_locations(
        &mut self,
        inst: &Instance,
        locs: Vec<RawLocation>,
        relation: Relation,
        method: &str,
    ) -> Vec<Candidate> {
        let mut out = Vec::new();
        let mut unmapped = 0;
        for l in locs {
            let path = PathBuf::from(cersei_lsp::uri_to_path(&l.uri));
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if !self.engine.in_scope(&self.roots, &path) {
                self.out_of_scope_results += 1;
                continue;
            }
            let Ok(doc) = self.snapshot.document(&path) else {
                unmapped += 1;
                continue;
            };
            let m = doc.mapper();
            let (Ok(s), Ok(e)) = (
                m.from_lsp(&l.range.start, inst.encoding),
                m.from_lsp(&l.range.end, inst.encoding),
            ) else {
                unmapped += 1;
                continue;
            };
            let mut c = Candidate::new(
                Arc::clone(&doc),
                s,
                e.max(s),
                relation,
                Certainty::Confirmed,
                Provenance {
                    backend: Backend::Lsp,
                    method: method.to_string(),
                },
            );
            if inst.has_version(&l.uri, &doc.revision.hash) == Some(false) {
                c.freshness = Freshness::Stale {
                    reason: "the server answered for another version of this file".into(),
                };
            }
            let name = &doc.text[s..e.max(s)];
            if planner::is_identifier_like(name) {
                c.symbol = Some(SymbolRef {
                    name: name.to_string(),
                    kind: "symbol".into(),
                });
            }
            // A definition: attach its syntax node.
            if matches!(relation, Relation::Definition | Relation::Declaration) {
                if let Some(p) = self.parse_doc(&doc) {
                    if let Some(def) = syntax::definitions(&p)
                        .into_iter()
                        .find(|d| d.name_start <= s && s < d.name_end.max(d.name_start + 1))
                    {
                        c.scope = Some((def.start, def.end, def.node_kind.clone()));
                        c.signature = Some(def.signature.clone());
                        c.symbol = Some(SymbolRef {
                            name: def.name,
                            kind: def.kind,
                        });
                        c.provenance.push(Provenance {
                            backend: Backend::Syntax,
                            method: format!("tree-sitter {}", def.node_kind),
                        });
                    }
                }
            }
            out.push(c);
        }
        if unmapped > 0 {
            self.omissions.push(Omission::ItemsOmitted {
                reason: "location not in the current text of its file (stale or unreadable)".into(),
                count: unmapped,
            });
        }
        out
    }

    /// The name at a target (syntax first, then the word around).
    fn name_at(&mut self, at: &At) -> Option<String> {
        if let Some(p) = self.parse_doc(&at.doc) {
            if let Some((_, _, n)) = syntax::identifier_at(&p, at.offset) {
                return Some(n);
            }
        }
        let t = &at.doc.text;
        let is_w = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
        let start = t[..at.offset]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_w(*c))
            .last()
            .map(|(i, _)| i)
            .unwrap_or(at.offset);
        let end = t[at.offset..]
            .char_indices()
            .find(|(_, c)| !is_w(*c))
            .map(|(i, _)| at.offset + i)
            .unwrap_or(t.len());
        let w = &t[start..end];
        planner::is_identifier_like(w).then(|| w.to_string())
    }

    /// Definition / references.
    async fn navigate(&mut self, intent: Intent) -> Outcome {
        let at = match &self.q.target.clone() {
            Some(t) => match self.resolve_target(t) {
                Ok(at) => at,
                Err(e) => return Outcome::status(ResultStatus::Error { message: e }),
            },
            None => {
                // A name only: resolve candidates first; never pick one.
                let text = self.q.text.clone();
                let defs = if planner::is_identifier_like(&text) {
                    self.definitions_named(&text).await
                } else {
                    Vec::new()
                };
                match defs.len() {
                    0 => {
                        if intent == Intent::References && planner::is_identifier_like(&text) {
                            let name = planner::split_qualified(&text).0;
                            return self
                                .mentions_fallback(&name, "no definition found by name")
                                .await;
                        }
                        return Outcome {
                            strategy: "syntax".into(),
                            ..Outcome::status(ResultStatus::Complete)
                        };
                    }
                    1 => {
                        let c = &defs[0];
                        self.fallbacks.push(format!(
                            "target resolved by name to the only definition `{}`",
                            rel_path(&self.engine.root, &c.doc.path)
                        ));
                        At {
                            doc: Arc::clone(&c.doc),
                            offset: c.start,
                        }
                    }
                    n => {
                        return Outcome {
                            cands: defs,
                            status: ResultStatus::Ambiguous,
                            ambiguity: Some(Ambiguity {
                                name: text,
                                candidates: n,
                                hint:
                                    "several definitions match: pass the chosen item id as `target`"
                                        .into(),
                            }),
                            diagnostics: None,
                            strategy: "syntax (candidates)".into(),
                        };
                    }
                }
            }
        };
        let (method, relation, extra, cap) = match intent {
            Intent::Definition => (
                "textDocument/definition",
                Relation::Definition,
                json!({}),
                "definition",
            ),
            _ => (
                "textDocument/references",
                Relation::Reference,
                json!({ "context": { "includeDeclaration": false } }),
                "references",
            ),
        };
        let semantic = match self.instance(&at.doc).await {
            Ok(inst) => {
                let supported = inst.client.capabilities().map(|c| match cap {
                    "definition" => c.definition,
                    _ => c.references,
                });
                if supported == Some(false) {
                    Err(format!("the server does not support {method}"))
                } else {
                    match self.lsp_request(&inst, &at, method, extra).await {
                        Ok(v) => {
                            let locs = cersei_lsp::parse_locations(&v);
                            Ok(self.map_locations(&inst, locs, relation, method))
                        }
                        Err(e) => Err(e),
                    }
                }
            }
            Err(reason) => {
                self.step(
                    Backend::Lsp,
                    method,
                    StepOutcome::Unavailable,
                    Instant::now(),
                    Some(reason.clone()),
                );
                Err(reason)
            }
        };
        // An empty answer from a server that is still indexing proves
        // nothing: complete it with what syntax and text can say.
        let semantic = match semantic {
            Ok(c) if c.is_empty() && self.server_indexing => {
                Err("the server answered nothing while indexing".to_string())
            }
            other => other,
        };
        match semantic {
            Ok(cands) => Outcome {
                cands,
                status: ResultStatus::Complete,
                ambiguity: None,
                diagnostics: None,
                strategy: "lsp → syntax context".into(),
            },
            Err(reason) => {
                if self.cancel.is_cancelled() {
                    return Outcome::status(ResultStatus::Cancelled);
                }
                let Some(name) = self.name_at(&at) else {
                    return Outcome::status(ResultStatus::Unavailable { reason });
                };
                if intent == Intent::Definition {
                    self.fallbacks.push(format!(
                        "semantic definition unavailable ({reason}): definitions named `{name}` by syntax; which one the target uses is not established"
                    ));
                    let defs = self.definitions_named(&name).await;
                    let n = defs.len();
                    Outcome {
                        cands: defs,
                        status: if n > 1 {
                            ResultStatus::Ambiguous
                        } else {
                            ResultStatus::Partial
                        },
                        ambiguity: (n > 1).then(|| Ambiguity {
                            name: name.clone(),
                            candidates: n,
                            hint: "syntax cannot tell which one is used here".into(),
                        }),
                        diagnostics: None,
                        strategy: "syntax (fallback)".into(),
                    }
                } else {
                    self.mentions_fallback(&name, &reason).await
                }
            }
        }
    }

    async fn mentions_fallback(&mut self, name: &str, reason: &str) -> Outcome {
        self.fallbacks.push(format!(
            "semantic references unavailable ({reason}): text mentions of `{name}`, not confirmed references"
        ));
        let pattern = format!(r"\b{}\b", planner::regex_escape(name));
        let f = self.filters(false);
        let ci = self.q.case_insensitive;
        let cands = match self
            .lexical(&pattern, MatchMode::Regex, ci, f, "word search")
            .await
        {
            Some(out) => out
                .matches
                .into_iter()
                .map(|m| {
                    let mut c = Candidate::new(
                        m.doc,
                        m.start,
                        m.end,
                        Relation::TextMention,
                        Certainty::Textual,
                        Provenance {
                            backend: Backend::Lexical,
                            method: "whole-word match".into(),
                        },
                    );
                    c.symbol = Some(SymbolRef {
                        name: name.to_string(),
                        kind: "text".into(),
                    });
                    c
                })
                .collect(),
            None => Vec::new(),
        };
        Outcome {
            cands,
            status: ResultStatus::Partial,
            ambiguity: None,
            diagnostics: None,
            strategy: "text (fallback)".into(),
        }
    }

    /// The place, its definition, and (deeper) some references.
    async fn understand(&mut self) -> Outcome {
        let at = match self.q.target.clone() {
            Some(t) => match self.resolve_target(&t) {
                Ok(at) => at,
                Err(e) => return Outcome::status(ResultStatus::Error { message: e }),
            },
            None => {
                let text = self.q.text.clone();
                if !planner::is_identifier_like(&text) {
                    self.fallbacks
                        .push("no target and not an identifier: text search".into());
                    return self.text_search().await;
                }
                let defs = self.definitions_named(&text).await;
                match defs.len() {
                    0 => {
                        self.fallbacks
                            .push("no definition with that name: text search".into());
                        return self.text_search().await;
                    }
                    1 => At {
                        doc: Arc::clone(&defs[0].doc),
                        offset: defs[0].start,
                    },
                    n => {
                        return Outcome {
                            cands: defs,
                            status: ResultStatus::Ambiguous,
                            ambiguity: Some(Ambiguity {
                                name: text,
                                candidates: n,
                                hint: "pass the chosen item id as `target` to understand one"
                                    .into(),
                            }),
                            diagnostics: None,
                            strategy: "syntax (candidates)".into(),
                        }
                    }
                }
            }
        };
        let mut cands = Vec::new();
        // The enclosing scope of the place itself.
        let mut ctx = Candidate::new(
            Arc::clone(&at.doc),
            at.offset,
            at.offset,
            Relation::Context,
            Certainty::Syntactic,
            Provenance {
                backend: Backend::Syntax,
                method: "enclosing scope".into(),
            },
        );
        if let Some(p) = self.parse_doc(&at.doc) {
            if let Some(sc) = syntax::enclosing(&p, at.offset, at.offset, None) {
                ctx.scope = Some((sc.start, sc.end, sc.node_kind.clone()));
                if let Some(n) = sc.name {
                    ctx.symbol = Some(SymbolRef {
                        name: n,
                        kind: sc.node_kind.clone(),
                    });
                }
            }
        } else {
            ctx.certainty = Certainty::Textual;
            ctx.provenance = vec![Provenance {
                backend: Backend::Lexical,
                method: "lines".into(),
            }];
        }
        let refs_wanted = match self.q.detail {
            Detail::Compact => 0,
            Detail::Normal => 3,
            Detail::Deep => 10,
        };
        let mut semantic_ok = false;
        match self.instance(&at.doc).await {
            Ok(inst) => {
                let caps = inst.client.capabilities().cloned();
                if caps.as_ref().is_some_and(|c| c.hover) {
                    if let Ok(v) = self
                        .lsp_request(&inst, &at, "textDocument/hover", json!({}))
                        .await
                    {
                        ctx.documentation = cersei_lsp::client::hover_text(&v);
                        if ctx.documentation.is_some() {
                            ctx.provenance.push(Provenance {
                                backend: Backend::Lsp,
                                method: "textDocument/hover".into(),
                            });
                        }
                        semantic_ok = true;
                    }
                }
                if caps.as_ref().is_some_and(|c| c.definition) {
                    if let Ok(v) = self
                        .lsp_request(&inst, &at, "textDocument/definition", json!({}))
                        .await
                    {
                        let locs = cersei_lsp::parse_locations(&v);
                        cands.extend(self.map_locations(
                            &inst,
                            locs,
                            Relation::Definition,
                            "textDocument/definition",
                        ));
                        semantic_ok = true;
                    }
                }
                if refs_wanted > 0 && caps.as_ref().is_some_and(|c| c.references) {
                    if let Ok(v) = self
                        .lsp_request(
                            &inst,
                            &at,
                            "textDocument/references",
                            json!({ "context": { "includeDeclaration": false } }),
                        )
                        .await
                    {
                        let locs = cersei_lsp::parse_locations(&v);
                        let total = locs.len();
                        let mut refs = self.map_locations(
                            &inst,
                            locs,
                            Relation::Reference,
                            "textDocument/references",
                        );
                        if refs.len() > refs_wanted {
                            refs.truncate(refs_wanted);
                            self.omissions.push(Omission::ItemsOmitted {
                                reason: format!(
                                    "references beyond {refs_wanted} at this detail level"
                                ),
                                count: total - refs_wanted,
                            });
                            self.continuation.push(
                                "ask intent=references on this target for all of them".into(),
                            );
                        }
                        cands.extend(refs);
                    }
                }
            }
            Err(reason) => {
                self.step(
                    Backend::Lsp,
                    "semantic",
                    StepOutcome::Unavailable,
                    Instant::now(),
                    Some(reason.clone()),
                );
                self.fallbacks.push(format!(
                    "semantic information unavailable ({reason}): syntax context only"
                ));
            }
        }
        cands.insert(0, ctx);
        // Without a server, the definition by name (syntactic, may be a
        // homonym).
        if !semantic_ok {
            if let Some(name) = self.name_at(&at) {
                let defs = self.definitions_named(&name).await;
                if defs.len() > 1 {
                    self.fallbacks.push(format!(
                        "{} definitions named `{name}`: which one applies is not established",
                        defs.len()
                    ));
                }
                cands.extend(defs);
            }
        }
        Outcome {
            cands,
            status: if semantic_ok {
                ResultStatus::Complete
            } else {
                ResultStatus::Partial
            },
            ambiguity: None,
            diagnostics: None,
            strategy: if semantic_ok {
                "syntax scope + lsp (hover, definition, references)".into()
            } else {
                "syntax scope (no server)".into()
            },
        }
    }

    async fn diagnostics(&mut self) -> Outcome {
        let Some(t) = self.q.target.clone() else {
            return Outcome::status(ResultStatus::Error {
                message: "diagnostics need a target file".into(),
            });
        };
        let at = match self.resolve_target(&t) {
            Ok(at) => at,
            Err(e) => return Outcome::status(ResultStatus::Error { message: e }),
        };
        let rel = rel_path(&self.engine.root, &at.doc.path);
        let unavailable = |reason: String| {
            let report = DiagnosticsReport {
                path: rel.clone(),
                state: DiagnosticsState::Unavailable {
                    reason: reason.clone(),
                },
                method: "none".into(),
                errors: 0,
                warnings: 0,
                introduced: None,
                preexisting: None,
                resolved: None,
                baseline_revision: None,
            };
            Outcome {
                cands: Vec::new(),
                status: ResultStatus::Unavailable { reason },
                ambiguity: None,
                diagnostics: Some(report),
                strategy: "lsp diagnostics".into(),
            }
        };
        let inst = match self.instance(&at.doc).await {
            Ok(i) => i,
            Err(reason) => {
                self.step(
                    Backend::Lsp,
                    "diagnostics",
                    StepOutcome::Unavailable,
                    Instant::now(),
                    Some(reason.clone()),
                );
                return unavailable(reason);
            }
        };
        let t0 = Instant::now();
        let synced = match self.engine.lsp.sync(&inst, &at.doc).await {
            Ok(s) => s,
            Err(e) => return unavailable(e.to_string()),
        };
        let wait = Duration::from_millis(self.engine.config.lsp.diagnostics_wait_ms)
            .min(self.deadline.saturating_duration_since(Instant::now()));
        let pull = inst
            .client
            .capabilities()
            .is_some_and(|c| c.diagnostic_pull);
        let (state, raw, method) = if pull {
            match inst.client.pull_diagnostics(&synced.uri, wait).await {
                Ok(raw) => (
                    DiagnosticsState::Analyzed {
                        version: synced.version,
                    },
                    raw,
                    "textDocument/diagnostic (pull)",
                ),
                Err(e) => {
                    self.fallbacks.push(format!(
                        "pull diagnostics failed ({e}): waiting for a publication"
                    ));
                    self.wait_published(&inst, &synced, wait).await
                }
            }
        } else {
            self.wait_published(&inst, &synced, wait).await
        };
        let version = synced.version;
        drop(synced);
        self.metrics.lsp_requests += 1;
        self.metrics.lsp_ms += t0.elapsed().as_millis() as u64;
        self.step(
            Backend::Lsp,
            method,
            match state {
                DiagnosticsState::Analyzed { .. } => StepOutcome::Ok,
                _ => StepOutcome::Partial,
            },
            t0,
            Some(format!("version {version}")),
        );
        let m = at.doc.mapper();
        let mut cands = Vec::new();
        let (mut errors, mut warnings) = (0, 0);
        let mut keys = Vec::new();
        for d in &raw {
            let sev = d["severity"].as_u64().unwrap_or(2) as u8;
            match sev {
                1 => errors += 1,
                2 => warnings += 1,
                _ => {}
            }
            let msg = d["message"].as_str().unwrap_or("").to_string();
            let code = d["code"]
                .as_str()
                .map(String::from)
                .or_else(|| d["code"].as_i64().map(|n| n.to_string()))
                .unwrap_or_default();
            keys.push((sev, code.clone(), msg.clone()));
            let parse = |v: &Value| -> Option<usize> {
                let p = cersei_lsp::Position {
                    line: v["line"].as_u64()? as u32,
                    character: v["character"].as_u64()? as u32,
                };
                m.from_lsp(&p, inst.encoding).ok()
            };
            let (Some(s), Some(e)) = (parse(&d["range"]["start"]), parse(&d["range"]["end"]))
            else {
                continue;
            };
            let mut c = Candidate::new(
                Arc::clone(&at.doc),
                s,
                e.max(s),
                Relation::Diagnostic,
                Certainty::Confirmed,
                Provenance {
                    backend: Backend::Lsp,
                    method: method.to_string(),
                },
            );
            let sev_label = match sev {
                1 => "error",
                2 => "warning",
                3 => "info",
                _ => "hint",
            };
            c.documentation = Some(if code.is_empty() {
                format!("{sev_label}: {msg}")
            } else {
                format!("{sev_label}[{code}]: {msg}")
            });
            if !matches!(state, DiagnosticsState::Analyzed { .. }) {
                c.freshness = Freshness::Stale {
                    reason: "reported for an earlier version".into(),
                };
            }
            cands.push(c);
        }
        let mut report = DiagnosticsReport {
            path: rel,
            state: state.clone(),
            method: method.to_string(),
            errors,
            warnings,
            introduced: None,
            preexisting: None,
            resolved: None,
            baseline_revision: None,
        };
        if matches!(state, DiagnosticsState::Analyzed { .. }) {
            if let Some((i, p, r, h)) =
                self.engine
                    .compare_baseline(&at.doc.path, &at.doc.revision.hash, &keys)
            {
                report.introduced = Some(i);
                report.preexisting = Some(p);
                report.resolved = Some(r);
                report.baseline_revision = Some(h);
            }
        }
        let status = match state {
            DiagnosticsState::Analyzed { .. } => ResultStatus::Complete,
            _ => ResultStatus::Partial,
        };
        Outcome {
            cands,
            status,
            ambiguity: None,
            diagnostics: Some(report),
            strategy: "lsp diagnostics".into(),
        }
    }

    async fn wait_published(
        &mut self,
        inst: &Instance,
        synced: &crate::lsp::Synced,
        wait: Duration,
    ) -> (DiagnosticsState, Vec<Value>, &'static str) {
        let version = synced.version;
        let before = synced.diag_seq_before;
        let fits = |e: &cersei_lsp::DiagnosticsEntry| {
            cersei_lsp::manager::is_for_version(e, version, before)
        };
        let entry = tokio::select! {
            e = inst.client.wait_diagnostics(&synced.uri, wait, fits) => e,
            _ = self.cancel.cancelled() => None,
        };
        match entry {
            Some(e) if fits(&e) => (
                DiagnosticsState::Analyzed { version },
                e.raw,
                if e.version.is_some() {
                    "publishDiagnostics (versioned)"
                } else {
                    "publishDiagnostics (unversioned, received after this version was sent)"
                },
            ),
            Some(e) => (
                DiagnosticsState::Outdated { version: e.version },
                e.raw,
                "publishDiagnostics (earlier version)",
            ),
            None => (
                DiagnosticsState::Pending,
                Vec::new(),
                "publishDiagnostics (none yet)",
            ),
        }
    }
}
