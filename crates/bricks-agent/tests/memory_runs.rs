//! Acceptance: the long-term memory of each run, through the controller.
//!
//! The desktop's memory is resolved at the start of each run (a switch, the
//! session's project, the stores). These tests drive real sessions with a
//! scripted model and a resolver that opens the stores of a temporary home
//! with hashing embeddings. They cover: the switch applies at the next run
//! and the running run keeps its memory; memory off (nothing recalled,
//! captured or offered, tools refused); a project captures its exchanges and
//! nothing reaches the global memory; isolation between projects, a global
//! note with its provenance, persistence across a move; no project; a note
//! found in the same run; indexing failure and retry; read-only sessions;
//! clearing refused during a run; history untouched by clearing the memory.

#[allow(dead_code)]
mod common;

use async_trait::async_trait;
use bricks_agent::BricksConfig;
use bricks_agent::agents::ProfileSources;
use bricks_agent::control::scripted::{Reply, Script, ScriptedCatalog};
use bricks_agent::control::*;
use bricks_agent::permission::{PermissionEffect, PermissionProfile, SessionPolicy};
use bricks_embeddings::{EmbeddingError, EmbeddingProvider, HashingEmbeddings};
use bricks_memory::structured::{
    Lease, MemoryConfig, MemoryError, OpenMemories, Processing, ScopedMemory, SharedStore,
    global_space, project_space,
};
use bricks_provider::CompletionRequest;
use bricks_tools::{PermissionLevel, Tool};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ─── The resolver of the desktop, in a test ──────────────────────────────────

/// Hashing embeddings (same identity) that fail once, on the first batch
/// containing the armed text (`fail_on`), then work again.
struct Flaky {
    inner: HashingEmbeddings,
    fail_on: Arc<std::sync::Mutex<Option<String>>>,
}

#[async_trait]
impl EmbeddingProvider for Flaky {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let hit = {
            let mut armed = self.fail_on.lock().unwrap();
            let hit = armed
                .as_deref()
                .is_some_and(|mark| texts.iter().any(|t| t.contains(mark)));
            if hit {
                *armed = None;
            }
            hit
        };
        if hit {
            return Err(EmbeddingError::Api("embedder down".into()));
        }
        self.inner.embed_batch(texts).await
    }
}

/// The memory of each run: `on` is the switch, `project` the session's
/// project (`None`: no project), the stores live in `home/memory`, opened
/// through `stores` (shared by the sessions of one test, as in the process).
struct Resolver {
    home: PathBuf,
    stores: Arc<OpenMemories>,
    project: Option<String>,
    on: Arc<AtomicBool>,
    fail_on: Arc<std::sync::Mutex<Option<String>>>,
    resolves: AtomicUsize,
}

impl Resolver {
    fn new(stores: &Arc<OpenMemories>, home: &Path, project: Option<&str>, on: bool) -> Arc<Self> {
        Arc::new(Self {
            home: home.join("memory"),
            stores: Arc::clone(stores),
            project: project.map(str::to_string),
            on: Arc::new(AtomicBool::new(on)),
            fail_on: Arc::new(std::sync::Mutex::new(None)),
            resolves: AtomicUsize::new(0),
        })
    }

    fn project_path(&self, id: &str) -> PathBuf {
        self.home.join("projects").join(format!("{id}.grafeo"))
    }

    /// The store of this session's project, when it was opened.
    fn project_store(&self) -> Option<SharedStore> {
        let id = self.project.as_ref()?;
        self.stores.get(&self.project_path(id))
    }

    /// The global store, when it was opened.
    fn global_store(&self) -> Option<SharedStore> {
        self.stores.get(&self.home.join("global.grafeo"))
    }
}

impl MemoryResolver for Resolver {
    fn resolve(&self, _session_id: &str) -> RunMemory {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        if !self.on.load(Ordering::SeqCst) {
            return RunMemory::default();
        }
        let config = MemoryConfig::default();
        let open = |path: PathBuf, space: &str| {
            self.stores
                .open(
                    &path,
                    space,
                    Arc::new(Flaky {
                        inner: HashingEmbeddings::new(64),
                        fail_on: Arc::clone(&self.fail_on),
                    }),
                    &config,
                    None,
                )
                .map_err(|e| e.to_string())
        };
        let project = self.project.as_ref().map(|id| {
            open(self.project_path(id), &project_space(id))
        });
        let global = Some(open(self.home.join("global.grafeo"), global_space()));
        let (memory, leases): (ScopedMemory, Vec<Lease>) = ScopedMemory::for_run(
            self.project.clone(),
            project,
            global,
            Processing::default(),
        );
        RunMemory {
            memory: Some(Arc::new(memory)),
            guard: Some(Box::new(leases)),
            warnings: Vec::new(),
        }
    }
}

// ─── Sessions ────────────────────────────────────────────────────────────────

struct Env {
    ctl: Controller,
    events: EventStream,
    script: Arc<Script>,
    #[allow(dead_code)]
    dir: tempfile::TempDir,
}

fn bricks() -> BricksConfig {
    let mut bricks = BricksConfig::default();
    bricks.agent.model = Some("test/a".into());
    bricks.semantic.lsp.enabled = false;
    bricks
}

async fn next(events: &mut EventStream) -> Envelope {
    tokio::time::timeout(Duration::from_secs(20), events.next())
        .await
        .expect("an event within 20 s")
        .expect("stream open")
}

/// A new session in `dir`, with `replies`, the memory of `resolver`, and
/// `policy` as the default policy (`None`: unchosen, reads run, the rest asked).
async fn start(
    dir: tempfile::TempDir,
    replies: Vec<Reply>,
    resolver: &Arc<Resolver>,
    policy: Option<SessionPolicy>,
) -> Env {
    let script = Script::new(replies);
    let catalog = ScriptedCatalog::new(&["a"], script.clone());
    let mut cfg = EngineConfig::new(dir.path(), catalog, bricks(), dir.path().join(".sessions"));
    cfg.agent_profile_sources = Some(ProfileSources {
        project_dir: Some(dir.path().join(".bricks/agents")),
        user_dir: None,
    });
    cfg.memory_resolver = Some(resolver.clone());
    cfg.permissions = policy.map(|p| common::default_store(dir.path(), &p));
    let (ctl, mut events) = Controller::open(
        cfg,
        OpenOptions {
            session: SessionChoice::New,
            model: None,
            reasoning: None,
        },
    )
    .await
    .unwrap();
    next(&mut events).await; // session_opened
    Env {
        ctl,
        events,
        script,
        dir,
    }
}

fn execute() -> SessionPolicy {
    SessionPolicy::chosen(PermissionProfile::Execute)
}

/// Submits `prompt`, calls `on` with each event, and returns every envelope
/// of the run once its maintenance has ended.
async fn run_with(env: &mut Env, prompt: &str, mut on: impl FnMut(&Event)) -> Vec<Envelope> {
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text(prompt),
        })
        .unwrap();
    let mut evs = Vec::new();
    loop {
        let e = next(&mut env.events).await;
        on(&e.event);
        let done = matches!(e.event, Event::RunFinished { .. });
        evs.push(e);
        if done {
            env.ctl.wait_maintenance().await;
            return evs;
        }
    }
}

/// `(is_error, output)` of every finished call of `name`, in order.
fn results(evs: &[Envelope], name: &str) -> Vec<(bool, String)> {
    evs.iter()
        .filter_map(|e| match &e.event {
            Event::ToolFinished {
                name: n,
                is_error,
                output,
                ..
            } if n == name => Some((*is_error, output.clone())),
            _ => None,
        })
        .collect()
}

/// The requests the scripted model received, in order.
fn requests(env: &Env) -> Vec<CompletionRequest> {
    env.script.requests().into_iter().map(|(_, r)| r).collect()
}

fn offered(req: &CompletionRequest) -> Vec<String> {
    req.tools.iter().map(|t| t.name.clone()).collect()
}

fn system(req: &CompletionRequest) -> String {
    req.system.clone().unwrap_or_default()
}

fn exchanges(store: &Option<SharedStore>) -> usize {
    store
        .as_ref()
        .map_or(0, |s| s.memory.counts().unwrap().exchanges)
}

fn notes(store: &Option<SharedStore>) -> usize {
    store
        .as_ref()
        .map_or(0, |s| s.memory.counts().unwrap().notes)
}

/// The id of the note in a refusal or a confirmation of `memory_write`.
fn id_in(output: &str) -> String {
    let after = output
        .split("the note ")
        .nth(1)
        .or_else(|| output.split(" as ").nth(1))
        .expect("an id in the output");
    after
        .split_whitespace()
        .next()
        .unwrap()
        .trim_end_matches('.')
        .to_string()
}

fn home() -> (tempfile::TempDir, Arc<OpenMemories>) {
    (tempfile::tempdir().unwrap(), Arc::new(OpenMemories::default()))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn with_the_memory_off_nothing_is_recalled_captured_or_offered_and_its_tools_are_refused() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), false);
    let mut env = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("m1", "memory_search", json!({"query": "tabs"})),
            Reply::tool("m2", "memory_write", json!({"text": "Prefers tabs"})),
            Reply::text("fini"),
        ],
        &resolver,
        Some(execute()),
    )
    .await;
    let evs = run_with(&mut env, "Quelles préférences ?", |_| {}).await;

    for req in requests(&env) {
        let tools = offered(&req);
        assert!(!tools.iter().any(|t| t.starts_with("memory_")), "{tools:?}");
        assert!(!system(&req).contains("memory_search"), "no guidance either");
        assert!(!system(&req).contains("<long_term_memory"), "nothing recalled");
    }
    let search = results(&evs, "memory_search");
    assert!(search[0].0 && search[0].1.contains("memory_disabled"), "{search:?}");
    let write = results(&evs, "memory_write");
    assert!(write[0].0 && write[0].1.contains("memory_disabled"), "{write:?}");
    assert!(resolver.project_store().is_none(), "no store opened, nothing captured");
    assert!(resolver.global_store().is_none());
}

#[tokio::test]
async fn switching_the_memory_on_applies_at_the_next_run_and_not_to_the_running_one() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), false);
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("note.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let mut env = start(
        work,
        vec![
            Reply::tool("r1", "Read", json!({"file_path": file.to_string_lossy()})),
            Reply::text("lu"),
            Reply::tool("m1", "memory_search", json!({"query": "hello"})),
            Reply::text("fini"),
        ],
        &resolver,
        Some(execute()),
    )
    .await;

    // The switch is turned on while the first run executes its tool.
    let on = Arc::clone(&resolver.on);
    run_with(&mut env, "lis le fichier", move |e| {
        if let Event::ToolStarted { name, .. } = e
            && name == "Read"
        {
            on.store(true, Ordering::SeqCst);
        }
    })
    .await;
    assert_eq!(resolver.resolves.load(Ordering::SeqCst), 1, "resolved once, at its start");
    let first = requests(&env);
    assert_eq!(first.len(), 2);
    for req in &first {
        assert!(!offered(req).iter().any(|t| t.starts_with("memory_")), "the running run keeps its tools");
    }
    assert!(resolver.project_store().is_none(), "the running run captured nothing");

    // The next run has the memory.
    let evs = run_with(&mut env, "Quelle est la donnée ?", |_| {}).await;
    assert_eq!(resolver.resolves.load(Ordering::SeqCst), 2);
    let third = &requests(&env)[2];
    assert!(offered(third).contains(&"memory_search".to_string()));
    assert!(!results(&evs, "memory_search")[0].0, "a search runs");
    assert_eq!(exchanges(&resolver.project_store()), 2, "the next run's exchange is kept");
}

#[tokio::test]
async fn a_run_with_a_project_captures_its_exchange_into_the_project_only() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    let mut env = start(
        tempfile::tempdir().unwrap(),
        vec![Reply::text("Bonjour !")],
        &resolver,
        Some(execute()),
    )
    .await;
    run_with(&mut env, "Salut", |_| {}).await;
    assert_eq!(exchanges(&resolver.project_store()), 2, "user turn and reply");
    assert_eq!(exchanges(&resolver.global_store()), 0, "never the global memory");
}

#[tokio::test]
async fn a_session_without_a_project_captures_nothing_refuses_project_scope_and_never_falls_back() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), None, true);
    let mut env = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("p1", "memory_search", json!({"query": "x", "scope": "project"})),
            Reply::tool("p2", "memory_write", json!({"text": "une note", "scope": "project"})),
            Reply::tool(
                "g1",
                "memory_write",
                json!({"text": "Préfère les réponses courtes", "scope": "global"}),
            ),
            Reply::text("fini"),
        ],
        &resolver,
        Some(execute()),
    )
    .await;
    let evs = run_with(&mut env, "Bonjour", |_| {}).await;

    let search = results(&evs, "memory_search");
    assert!(search[0].0 && search[0].1.contains("no project"), "{search:?}");
    let write = results(&evs, "memory_write");
    assert!(write[0].0 && write[0].1.contains("no project"), "{write:?}");
    assert!(write[1].0 == false, "a global note is still written: {write:?}");
    let global = resolver.global_store().unwrap();
    assert_eq!(notes(&Some(global.clone())), 1, "the project note never fell back to global");
    assert_eq!(exchanges(&Some(global)), 0, "nothing captured without a project");
}

#[tokio::test]
async fn a_note_written_in_a_run_is_found_by_the_next_search_of_the_same_run() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    let mut env = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("w1", "memory_write", json!({"text": "Deploy on Fridays only"})),
            Reply::tool("s1", "memory_search", json!({"query": "deploy fridays"})),
            Reply::text("ok"),
        ],
        &resolver,
        Some(execute()),
    )
    .await;
    let evs = run_with(&mut env, "Note la règle de déploiement", |_| {}).await;
    let write = results(&evs, "memory_write");
    assert!(!write[0].0 && write[0].1.contains("Saved in the project memory as"), "{write:?}");
    let search = results(&evs, "memory_search");
    assert!(!search[0].0, "{search:?}");
    assert!(search[0].1.contains("Deploy on Fridays only"), "{}", search[0].1);
}

#[tokio::test]
async fn isolation_between_projects_provenance_of_a_global_note_and_persistence_across_a_move() {
    let (home_dir, stores) = home();
    let alpha = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    let beta = Resolver::new(&stores, home_dir.path(), Some("pb"), true);
    let mut a = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("w1", "memory_write", json!({"text": "Alpha uses postgres"})),
            Reply::tool(
                "w2",
                "memory_write",
                json!({"text": "Prefers short answers", "scope": "global"}),
            ),
            Reply::text("ok"),
        ],
        &alpha,
        Some(execute()),
    )
    .await;
    run_with(&mut a, "Note ça", |_| {}).await;

    let mut b = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("s1", "memory_search", json!({"query": "postgres", "scope": "project"})),
            Reply::tool("s2", "memory_search", json!({"query": "short answers", "scope": "global"})),
            Reply::text("ok"),
        ],
        &beta,
        Some(execute()),
    )
    .await;
    let evs = run_with(&mut b, "Quelle base ?", |_| {}).await;
    let project = results(&evs, "memory_search")[0].clone();
    assert!(project.1.contains("Nothing in the project memory matches"), "{project:?}");
    let global = results(&evs, "memory_search")[1].clone();
    assert!(global.1.contains("from project pa"), "provenance: {global:?}");

    // The project moved folder: same id, a new session, the memory follows.
    let moved = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    let mut c = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("s3", "memory_search", json!({"query": "postgres"})),
            Reply::text("ok"),
        ],
        &moved,
        Some(execute()),
    )
    .await;
    let evs = run_with(&mut c, "Rappelle", |_| {}).await;
    assert!(results(&evs, "memory_search")[0].1.contains("Alpha uses postgres"));
}

#[tokio::test]
async fn an_indexing_failure_is_said_the_note_is_kept_and_the_retry_does_not_duplicate_it() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    // The first embedding of the note fails; the embedder recovers after it.
    *resolver.fail_on.lock().unwrap() = Some("Deploy on Fridays".into());
    let mut env = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("w1", "memory_write", json!({"text": "Deploy on Fridays"})),
            Reply::tool("w2", "memory_write", json!({"text": "Deploy on Fridays"})),
            Reply::text("ok"),
        ],
        &resolver,
        Some(execute()),
    )
    .await;
    let evs = run_with(&mut env, "Note", |_| {}).await;
    let writes = results(&evs, "memory_write");
    assert!(writes[0].0, "{writes:?}");
    assert!(
        writes[0].1.contains("is saved but not indexed yet"),
        "{}",
        writes[0].1
    );
    assert!(!writes[1].0, "the retry succeeds: {writes:?}");
    assert_eq!(id_in(&writes[0].1), id_in(&writes[1].1), "the same note");
    let stored = resolver
        .project_store()
        .unwrap()
        .memory
        .entries(bricks_memory::structured::browse::EntryFilter::Notes, "", 0, 50)
        .unwrap();
    assert_eq!(stored.total, 1, "no duplicate: {:?}", stored.entries);
    let s = resolver.project_store().unwrap().memory.stats();
    assert_eq!(s.indexed, s.episodes, "every episode, the note included, is indexed");
}

#[tokio::test]
async fn a_read_only_session_does_not_offer_memory_write_nor_its_guidance() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    let policy = common::custom(|c| c.write = PermissionEffect::Deny);
    let mut env = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("w1", "memory_write", json!({"text": "Prefers tabs"})),
            Reply::text("ok"),
        ],
        &resolver,
        Some(policy),
    )
    .await;
    let evs = run_with(&mut env, "Note ça", |_| {}).await;
    let req = &requests(&env)[0];
    let tools = offered(req);
    assert!(tools.contains(&"memory_search".to_string()), "reads stay: {tools:?}");
    assert!(!tools.contains(&"memory_write".to_string()), "{tools:?}");
    assert!(!system(req).contains("memory_write"), "no guidance for writing");
    let write = results(&evs, "memory_write");
    assert!(write[0].0, "a call anyway is refused: {write:?}");
    assert_eq!(notes(&resolver.project_store()), 0);
}

#[tokio::test]
async fn a_run_cannot_be_cleared_under_its_feet_and_clearing_after_it_works() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("note.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let mut env = start(
        work,
        vec![
            Reply::tool("r1", "Read", json!({"file_path": file.to_string_lossy()})),
            Reply::text("lu"),
        ],
        &resolver,
        Some(execute()),
    )
    .await;
    env.ctl
        .send(Command::Submit {
            prompt: Prompt::text("lis"),
        })
        .unwrap();
    let mut refused = false;
    loop {
        let e = next(&mut env.events).await;
        if let Event::ToolStarted { name, .. } = &e.event
            && name == "Read"
        {
            let store = resolver.project_store().expect("the run's store");
            refused = matches!(store.forget_all().await, Err(MemoryError::InUse(_)));
        }
        if matches!(e.event, Event::RunFinished { .. }) {
            break;
        }
    }
    env.ctl.wait_maintenance().await;
    assert!(refused, "clearing refused while the run uses the store");
    let store = resolver.project_store().unwrap();
    assert!(!store.in_use(), "released at the end of the run");
    store.forget_all().await.unwrap();
}

#[tokio::test]
async fn clearing_the_memory_leaves_the_session_history_untouched() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    let mut env = start(
        tempfile::tempdir().unwrap(),
        vec![Reply::text("Bonjour"), Reply::text("Suite")],
        &resolver,
        Some(execute()),
    )
    .await;
    run_with(&mut env, "Premier message", |_| {}).await;
    assert_eq!(exchanges(&resolver.project_store()), 2);
    let before = env.ctl.history().await.len();
    assert!(before >= 2);

    resolver.project_store().unwrap().forget_all().await.unwrap();
    assert_eq!(exchanges(&resolver.project_store()), 0, "the memory is empty");
    assert_eq!(env.ctl.history().await.len(), before, "the conversation is kept");

    run_with(&mut env, "Second message", |_| {}).await;
    let second = serde_json::to_string(&requests(&env)[1].messages).unwrap();
    assert!(second.contains("Premier message"), "the first exchange is still sent");
}

#[tokio::test]
async fn the_memory_recalled_at_the_start_of_a_run_is_within_budget_and_says_its_scope() {
    let (home_dir, stores) = home();
    let resolver = Resolver::new(&stores, home_dir.path(), Some("pa"), true);
    let mut env = start(
        tempfile::tempdir().unwrap(),
        vec![
            Reply::tool("w1", "memory_write", json!({"text": "Deploy on Fridays only"})),
            Reply::text("noté"),
            Reply::text("vendredi"),
        ],
        &resolver,
        Some(execute()),
    )
    .await;
    run_with(&mut env, "Note la règle", |_| {}).await;
    run_with(&mut env, "When do we deploy?", |_| {}).await;
    let sys = system(&requests(&env)[2]);
    let start_at = sys.find("<long_term_memory").expect("recalled into the prompt");
    let block = &sys[start_at..];
    assert!(block.contains("Deploy on Fridays only"), "{block}");
    assert!(block.contains("project memory"), "the scope is said: {block}");
    let tokens = bricks_types::tokens::estimate_text(block).tokens;
    assert!(tokens <= 1200, "{tokens} tokens");
}

// A tool used through its trait, outside any run, with no memory on the context.
#[tokio::test]
async fn the_memory_tools_refuse_without_a_run_memory() {
    use bricks_tools::memory_tools::MemorySearchTool;
    let ctx = bricks_tools::ToolContext {
        working_dir: std::env::temp_dir(),
        session_id: "none".into(),
        permissions: Arc::new(AllowAll),
        cost_tracker: Arc::new(bricks_tools::CostTracker::new()),
        mcp_manager: None,
        extensions: bricks_tools::Extensions::default(),
    };
    let out = MemorySearchTool
        .execute(json!({"query": "x"}), &ctx)
        .await;
    assert!(out.is_error && out.content.contains("memory_disabled"), "{out:?}");
    let _ = (PermissionLevel::ReadOnly, Value::Null);
}

struct AllowAll;

#[async_trait]
impl bricks_tools::permissions::PermissionPolicy for AllowAll {
    async fn check(
        &self,
        _request: &bricks_tools::permissions::PermissionRequest,
    ) -> bricks_tools::permissions::PermissionDecision {
        bricks_tools::permissions::PermissionDecision::Allow
    }
}
