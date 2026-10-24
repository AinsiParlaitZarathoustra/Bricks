//! CodeStats: the code, comment and blank lines of a file or a tree, counted
//! in process with Tokei.
//!
//! The walk is the one Grep and the semantic engine share (`.gitignore` and
//! `.ignore` rules, also outside a git checkout and nested), plus
//! `.tokeignore`; hidden files are counted, VCS metadata directories are not,
//! and directory links are never followed. A requested file goes through the
//! same rules: its parent is walked. Every file is opened without following
//! a final link and checked against what the walk saw (same device and
//! inode, a regular file inside the analysed root) before a byte of it is
//! read; hard links are counted once.
//!
//! Totals always cover the whole analysis: `top_n` only limits the rows
//! shown. Identical analyses running at the same time in one session share
//! one walk; nothing is kept once it ends.

use super::*;
use crate::code_scout::RunCancel;
use crate::tool_report::{ToolBody, ToolReport, ToolStatus};
use bricks_semantic::CancellationToken;
use bricks_semantic::lexical::{WalkFilters, walk_builder};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rayon::prelude::*;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokei::{Config, LanguageType};

/// The ignore file read for this tool only, as Tokei does.
const TOKEIGNORE: &str = ".tokeignore";
/// Bytes a shebang is looked for in (Tokei's own limit).
const SHEBANG_BYTES: usize = 128;
/// Paths quoted per warning cause.
const EXAMPLES: usize = 3;
/// Usually generated: counted unless a rule excludes them, and named once
/// in a diagnostic when they were.
const GENERATED_DIRS: &[&str] = &["target", "node_modules", "dist", "coverage"];

pub struct CodeStatsTool;

/// A probe called on the analysis thread.
#[doc(hidden)]
pub type Hook = Arc<dyn Fn() + Send + Sync>;
/// A probe called with the path of an opened file.
#[doc(hidden)]
pub type PathHook = Arc<dyn Fn(&Path) + Send + Sync>;

/// Test probes, called on the analysis thread (not part of the contract).
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct CodeStatsProbe {
    /// After the walk, before any file is opened.
    pub walked: Option<Hook>,
    /// A file opened and checked, before its text is read.
    pub opened: Option<PathHook>,
    /// A file counted.
    pub counted: Option<Hook>,
}

#[derive(Deserialize, Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
enum Detail {
    Summary,
    #[default]
    Languages,
    Files,
}

impl Detail {
    fn as_str(self) -> &'static str {
        match self {
            Detail::Summary => "summary",
            Detail::Languages => "languages",
            Detail::Files => "files",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: Option<String>,
    #[serde(default)]
    detail: Detail,
    language: Option<String>,
    top_n: Option<i64>,
    #[serde(default)]
    exclude: Vec<String>,
}

/// A validated request: what identifies an analysis.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Params {
    /// The requested file or directory, canonical.
    target: PathBuf,
    is_file: bool,
    detail: Detail,
    language: Option<LanguageType>,
    top_n: usize,
    /// Sorted and deduplicated.
    exclude: Vec<String>,
}

/// Session, the directory paths are shown from, and the request.
type Key = (String, PathBuf, Params);
type SharedOutcome = Shared<BoxFuture<'static, Arc<Outcome>>>;

enum Outcome {
    Done(Value),
    Cancelled,
    Failed(String),
}

struct Inflight {
    id: u64,
    fut: SharedOutcome,
    waiters: Arc<AtomicUsize>,
    cancel: CancellationToken,
}

static INFLIGHT: Lazy<Mutex<HashMap<Key, Inflight>>> = Lazy::new(Default::default);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
/// Analyses still on the blocking pool, wanted or not.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// Callers waiting on the analyses of a session (tests).
#[doc(hidden)]
pub fn waiting_in_session(session: &str) -> usize {
    INFLIGHT
        .lock()
        .iter()
        .filter(|((s, _, _), _)| s == session)
        .map(|(_, e)| e.waiters.load(Ordering::SeqCst))
        .sum()
}

/// Analyses of this process still running on the blocking pool (tests).
#[doc(hidden)]
pub fn analyses_running() -> usize {
    RUNNING.load(Ordering::SeqCst)
}

/// The last caller gone, nobody needs the analysis: it is stopped and
/// forgotten.
struct WaiterGuard {
    key: Key,
    id: u64,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        let mut inflight = INFLIGHT.lock();
        let last = match inflight.get(&self.key) {
            Some(e) if e.id == self.id => e.waiters.fetch_sub(1, Ordering::SeqCst) == 1,
            _ => false,
        };
        if last && let Some(e) = inflight.remove(&self.key) {
            e.cancel.cancel();
        }
    }
}

#[async_trait]
impl Tool for CodeStatsTool {
    fn name(&self) -> &str {
        "CodeStats"
    }

    fn description(&self) -> &str {
        "Code statistics: count the lines of code, comments and blank lines of a file or a \
         directory tree, in total, per language or per file (Tokei, in process). Follows \
         .gitignore, .ignore and .tokeignore, counts hidden files, never follows directory \
         links; a file mixing languages counts for its primary language. `top_n` only limits \
         the rows shown: totals always cover the whole analysis. Check `partial` (and \
         `warnings`) before presenting totals as complete."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FileSystem
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File or directory, relative to the working directory (default: the working directory)." },
                "detail": { "type": "string", "enum": ["summary", "languages", "files"], "description": "summary: totals only; languages (default): one row per language; files: one row per file." },
                "language": { "type": "string", "description": "Count only the files of this primary language (Tokei name: `Rust`, `TypeScript`, `Jupyter Notebooks`)." },
                "top_n": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20, "description": "Rows shown, sorted by code lines. Does not limit the analysis." },
                "exclude": { "type": "array", "items": { "type": "string" }, "description": "Gitignore-style globs to leave out, relative to the analysed directory. Exclusions only: no `!`." }
            }
        })
    }

    async fn execute(&self, raw: Value, ctx: &ToolContext) -> ToolResult {
        let input: Input = match crate::tool_feedback::parse_input(self, &raw) {
            Ok(i) => i,
            Err(e) => return e,
        };
        let params = match validate(input, &ctx.working_dir) {
            Ok(p) => p,
            Err(msg) => return crate::tool_feedback::invalid_input(self, &raw, msg),
        };
        let run_cancel = ctx.extensions.get::<RunCancel>().map(|c| c.0.clone());
        if run_cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            return cancelled();
        }
        let base = std::fs::canonicalize(&ctx.working_dir).unwrap_or(ctx.working_dir.clone());
        let probe = ctx
            .extensions
            .get::<CodeStatsProbe>()
            .map(|p| (*p).clone())
            .unwrap_or_default();
        let key: Key = (ctx.session_id.clone(), base.clone(), params.clone());

        let (fut, id) = {
            let mut inflight = INFLIGHT.lock();
            let (fut, id, waiters) = match inflight.get(&key) {
                Some(e) => (e.fut.clone(), e.id, Arc::clone(&e.waiters)),
                None => {
                    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                    let cancel = CancellationToken::new();
                    let fut = start(key.clone(), id, params, base, probe, cancel.clone());
                    let waiters = Arc::new(AtomicUsize::new(0));
                    inflight.insert(
                        key.clone(),
                        Inflight {
                            id,
                            fut: fut.clone(),
                            waiters: Arc::clone(&waiters),
                            cancel,
                        },
                    );
                    (fut, id, waiters)
                }
            };
            waiters.fetch_add(1, Ordering::SeqCst);
            (fut, id)
        };
        let _guard = WaiterGuard { key, id };

        let outcome = match run_cancel {
            Some(c) => tokio::select! {
                o = fut => Some(o),
                _ = c.cancelled() => None,
            },
            None => Some(fut.await),
        };
        match outcome.as_deref() {
            Some(Outcome::Done(v)) => ToolResult::success(v.to_string()).with_metadata(v.clone()),
            Some(Outcome::Failed(e)) => ToolResult::error(format!("CodeStats failed: {e}")),
            Some(Outcome::Cancelled) | None => cancelled(),
        }
    }
}

/// No totals once stopped: files already being parsed may have finished,
/// the next ones were not opened.
fn cancelled() -> ToolResult {
    let mut r = ToolReport::new(
        ToolStatus::Cancelled,
        ToolBody::Text("CodeStats annulé : aucun total rendu.".into()),
    );
    r.termination = Some("annulé".into());
    ToolResult::from_report(r)
}

/// The shared analysis: started on the blocking pool when first polled,
/// forgotten when it ends.
fn start(
    key: Key,
    id: u64,
    params: Params,
    base: PathBuf,
    probe: CodeStatsProbe,
    cancel: CancellationToken,
) -> SharedOutcome {
    let fut: BoxFuture<'static, Arc<Outcome>> = async move {
        RUNNING.fetch_add(1, Ordering::SeqCst);
        let out = tokio::task::spawn_blocking(move || {
            let out = analyse(&params, &base, &probe, &cancel);
            RUNNING.fetch_sub(1, Ordering::SeqCst);
            out
        })
        .await
        .unwrap_or_else(|e| Outcome::Failed(e.to_string()));
        let mut inflight = INFLIGHT.lock();
        if inflight.get(&key).is_some_and(|e| e.id == id) {
            inflight.remove(&key);
        }
        Arc::new(out)
    }
    .boxed();
    fut.shared()
}

fn validate(input: Input, working_dir: &Path) -> std::result::Result<Params, String> {
    let top_n = match input.top_n {
        None => 20,
        Some(n) if (1..=100).contains(&n) => n as usize,
        Some(n) => return Err(format!("`top_n` must be between 1 and 100, got {n}")),
    };
    let language = match input.language.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(name) => Some(language_named(name)?),
    };
    let mut exclude = Vec::new();
    for g in input.exclude {
        let g = g.trim();
        if g.is_empty() {
            return Err("`exclude` holds an empty pattern".into());
        }
        if g.starts_with('!') {
            return Err(format!(
                "`exclude` only excludes: `{g}` would re-include files (remove the `!`)"
            ));
        }
        exclude.push(g.to_string());
    }
    exclude.sort();
    exclude.dedup();

    let requested = match input.path.as_deref() {
        None | Some("") => working_dir.to_path_buf(),
        Some(p) => working_dir.join(p),
    };
    let target =
        std::fs::canonicalize(&requested).map_err(|e| format!("`{}`: {e}", requested.display()))?;
    let meta = std::fs::metadata(&target).map_err(|e| format!("`{}`: {e}", target.display()))?;
    let is_file = if meta.is_dir() {
        false
    } else if meta.is_file() {
        true
    } else {
        return Err(format!(
            "`{}` is neither a regular file nor a directory",
            requested.display()
        ));
    };
    // Bad globs are refused before any walk.
    let root = analysis_root(&target, is_file);
    let mut ob = ignore::overrides::OverrideBuilder::new(root);
    for g in &exclude {
        ob.add(&format!("!{g}"))
            .map_err(|e| format!("`exclude`: {e}"))?;
    }
    ob.build().map_err(|e| format!("`exclude`: {e}"))?;

    Ok(Params {
        target,
        is_file,
        detail: input.detail,
        language,
        top_n,
        exclude,
    })
}

/// A Tokei language from its display name or its identifier, any case.
fn language_named(name: &str) -> std::result::Result<LanguageType, String> {
    let found = LanguageType::list().iter().map(|(l, _)| *l).find(|l| {
        l.name().eq_ignore_ascii_case(name) || format!("{l:?}").eq_ignore_ascii_case(name)
    });
    found.ok_or_else(|| {
        let needle = name.to_lowercase();
        let close: Vec<&str> = LanguageType::list()
            .iter()
            .map(|(l, _)| l.name())
            .filter(|n| n.to_lowercase().contains(&needle) || needle.contains(&n.to_lowercase()))
            .take(5)
            .collect();
        if close.is_empty() {
            format!("unknown language `{name}` (Tokei names: `Rust`, `TypeScript`, `Python`…)")
        } else {
            format!(
                "unknown language `{name}`; did you mean {}?",
                close.join(", ")
            )
        }
    })
}

/// The directory walked: the target itself, or a file's parent.
fn analysis_root(target: &Path, is_file: bool) -> &Path {
    if is_file {
        target.parent().unwrap_or(target)
    } else {
        target
    }
}

/// How a file's language is known.
#[derive(Clone, Copy)]
enum Detect {
    Lang(LanguageType),
    /// No extension and no known name: the first line may say.
    Shebang,
}

/// A file to count, as the walk saw it.
struct Candidate {
    shown: String,
    /// What is opened: the entry itself, or a file link's canonical target.
    open: PathBuf,
    detect: Detect,
    dev: u64,
    ino: u64,
}

enum FileOutcome {
    Counted {
        lang: LanguageType,
        code: u64,
        comments: u64,
        blanks: u64,
        changed: bool,
    },
    NotCode,
    Skipped(&'static str),
    Cancelled,
}

#[derive(Default, Clone, Copy)]
struct Tally {
    files: u64,
    code: u64,
    comments: u64,
    blanks: u64,
}

impl Tally {
    /// Adds a file; `false` on overflow (the tally then saturates).
    fn add(&mut self, code: u64, comments: u64, blanks: u64) -> bool {
        let mut ok = true;
        let mut add = |a: &mut u64, b: u64| match a.checked_add(b) {
            Some(v) => *a = v,
            None => {
                *a = u64::MAX;
                ok = false;
            }
        };
        add(&mut self.files, 1);
        add(&mut self.code, code);
        add(&mut self.comments, comments);
        add(&mut self.blanks, blanks);
        ok
    }

    fn lines(&self) -> Option<u64> {
        self.code
            .checked_add(self.comments)
            .and_then(|v| v.checked_add(self.blanks))
    }

    fn json(&self, warnings: &mut Warnings) -> Value {
        Value::Object(self.fields(true, warnings))
    }

    /// A result row: the file or the language, and its counters.
    fn row(
        &self,
        path: Option<&str>,
        lang: LanguageType,
        files: bool,
        warnings: &mut Warnings,
    ) -> Value {
        let mut m = serde_json::Map::new();
        if let Some(p) = path {
            m.insert("path".into(), p.into());
        }
        m.insert("language".into(), lang.name().into());
        m.extend(self.fields(files, warnings));
        Value::Object(m)
    }

    fn fields(&self, files: bool, warnings: &mut Warnings) -> serde_json::Map<String, Value> {
        let lines = self.lines().unwrap_or_else(|| {
            warnings.add("overflow", None);
            u64::MAX
        });
        let mut m = serde_json::Map::new();
        if files {
            m.insert("files".into(), self.files.into());
        }
        m.insert("code".into(), self.code.into());
        m.insert("comments".into(), self.comments.into());
        m.insert("blanks".into(), self.blanks.into());
        m.insert("lines".into(), lines.into());
        m
    }
}

/// Warnings grouped by cause, a few paths each.
#[derive(Default)]
struct Warnings(BTreeMap<&'static str, (u64, Vec<String>)>);

impl Warnings {
    fn add(&mut self, cause: &'static str, path: Option<&str>) {
        let e = self.0.entry(cause).or_default();
        e.0 += 1;
        if let Some(p) = path
            && e.1.len() < EXAMPLES
        {
            e.1.push(p.to_string());
        }
    }

    /// Causes that leave something uncounted.
    fn partial(&self) -> bool {
        self.0.keys().any(|c| {
            matches!(
                *c,
                "unreadable"
                    | "vanished"
                    | "changed"
                    | "changed_during_read"
                    | "invalid_notebook"
                    | "walk_error"
                    | "overflow"
            )
        })
    }

    fn json(&self) -> Value {
        Value::Array(
            self.0
                .iter()
                .map(|(cause, (count, examples))| {
                    serde_json::json!({ "cause": cause, "count": count, "examples": examples })
                })
                .collect(),
        )
    }
}

fn shown(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// The whole analysis, on a blocking thread.
fn analyse(
    params: &Params,
    base: &Path,
    probe: &CodeStatsProbe,
    cancel: &CancellationToken,
) -> Outcome {
    if cancel.is_cancelled() {
        return Outcome::Cancelled;
    }
    let root = analysis_root(&params.target, params.is_file).to_path_buf();
    let mut warnings = Warnings::default();
    // An empty private directory: language detection by name looks at a
    // path that does not exist, so Tokei never opens anything itself.
    let scratch = match tempfile::Builder::new()
        .prefix("bricks-codestats")
        .tempdir()
    {
        Ok(d) => d,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let absent = scratch.path().join("absent");

    let filters = WalkFilters {
        exclude: params.exclude.clone(),
        hidden: true,
        ..Default::default()
    };
    let mut wb = match walk_builder(&root, &filters) {
        Ok(b) => b,
        Err(e) => return Outcome::Failed(e),
    };
    // Sorted: the first of several paths to one file is always the same.
    wb.add_custom_ignore_filename(TOKEIGNORE)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b));
    if params.is_file
        && let Some(name) = params.target.file_name().map(|n| n.to_os_string())
    {
        wb.max_depth(Some(1));
        wb.filter_entry(move |e| e.depth() == 0 || e.file_name() == name.as_os_str());
    }

    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut requested_seen = false;
    for entry in wb.build() {
        if cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warnings.add("walk_error", Some(&e.to_string()));
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            continue;
        }
        requested_seen = true;
        let path = entry.path();
        let shown_path = shown(path, base);
        let detect = match detect_by_name(&absent, path) {
            Some(d) => d,
            None => {
                if params.is_file {
                    warnings.add("unrecognized", Some(&shown_path));
                }
                continue;
            }
        };
        if let (Detect::Lang(l), Some(want)) = (detect, params.language)
            && l != want
        {
            continue;
        }
        let (open, meta) = if ft.is_symlink() {
            // Directory links are never followed; a file link counts when
            // its target is a regular file inside the analysed root.
            let Some((target, meta)) = std::fs::canonicalize(path)
                .ok()
                .and_then(|t| std::fs::symlink_metadata(&t).ok().map(|m| (t, m)))
            else {
                warnings.add("broken_link", Some(&shown_path));
                continue;
            };
            if meta.is_dir() {
                warnings.add("directory_link", Some(&shown_path));
                continue;
            }
            if !meta.is_file() {
                warnings.add("not_regular", Some(&shown_path));
                continue;
            }
            if !target.starts_with(&root) {
                warnings.add("link_outside_root", Some(&shown_path));
                continue;
            }
            (target, meta)
        } else if ft.is_file() {
            match entry.metadata() {
                Ok(m) => (path.to_path_buf(), m),
                Err(_) => {
                    warnings.add("vanished", Some(&shown_path));
                    continue;
                }
            }
        } else {
            // FIFOs, sockets, devices: never opened.
            warnings.add("not_regular", Some(&shown_path));
            continue;
        };
        let (dev, ino) = identity(&meta);
        if !seen.insert((dev, ino)) {
            warnings.add("duplicate", Some(&shown_path));
            continue;
        }
        candidates.push(Candidate {
            shown: shown_path,
            open,
            detect,
            dev,
            ino,
        });
    }
    if params.is_file && !requested_seen {
        warnings.add("ignored", Some(&shown(&params.target, base)));
    }
    if let Some(f) = &probe.walked {
        f();
    }

    let config = Config::default();
    let outcomes: Vec<FileOutcome> = candidates
        .par_iter()
        .map(|c| {
            let o = count_file(c, &root, &config, params, scratch.path(), probe, cancel);
            if let (FileOutcome::Counted { .. }, Some(f)) = (&o, &probe.counted) {
                f();
            }
            o
        })
        .collect();
    drop(scratch);

    let mut totals = Tally::default();
    let mut languages: BTreeMap<LanguageType, Tally> = BTreeMap::new();
    let mut files: Vec<(String, LanguageType, Tally)> = Vec::new();
    let mut generated: (u64, Vec<String>) = (0, Vec::new());
    for (c, o) in candidates.iter().zip(outcomes) {
        let (lang, code, comments, blanks) = match o {
            FileOutcome::Cancelled => return Outcome::Cancelled,
            FileOutcome::NotCode => {
                if params.is_file {
                    warnings.add("unrecognized", Some(&c.shown));
                }
                continue;
            }
            FileOutcome::Skipped(cause) => {
                warnings.add(cause, Some(&c.shown));
                continue;
            }
            FileOutcome::Counted {
                lang,
                code,
                comments,
                blanks,
                changed,
            } => {
                if changed {
                    warnings.add("changed_during_read", Some(&c.shown));
                }
                (lang, code, comments, blanks)
            }
        };
        let ok = totals.add(code, comments, blanks)
            & languages
                .entry(lang)
                .or_default()
                .add(code, comments, blanks);
        if !ok {
            warnings.add("overflow", None);
        }
        if let Some(dir) = generated_dir(&c.shown) {
            generated.0 += 1;
            if generated.1.len() < EXAMPLES && !generated.1.contains(&dir) {
                generated.1.push(dir);
            }
        }
        if params.detail == Detail::Files {
            let mut t = Tally::default();
            t.add(code, comments, blanks);
            files.push((c.shown.clone(), lang, t));
        }
    }
    if generated.0 > 0 {
        warnings.0.insert("generated_dirs", generated);
    }

    let (results, rows): (Vec<Value>, usize) = match params.detail {
        Detail::Summary => (Vec::new(), 0),
        Detail::Languages => {
            let mut rows: Vec<(LanguageType, Tally)> = languages.into_iter().collect();
            rows.sort_by(|a, b| {
                b.1.code
                    .cmp(&a.1.code)
                    .then_with(|| a.0.name().as_bytes().cmp(b.0.name().as_bytes()))
            });
            let shown = rows
                .iter()
                .take(params.top_n)
                .map(|(l, t)| t.row(None, *l, true, &mut warnings))
                .collect();
            (shown, rows.len())
        }
        Detail::Files => {
            files.sort_by(|a, b| {
                b.2.code
                    .cmp(&a.2.code)
                    .then_with(|| a.0.as_bytes().cmp(b.0.as_bytes()))
            });
            let shown = files
                .iter()
                .take(params.top_n)
                .map(|(p, l, t)| t.row(Some(p), *l, false, &mut warnings))
                .collect();
            (shown, files.len())
        }
    };
    let omitted = rows - results.len();
    let totals = totals.json(&mut warnings);
    Outcome::Done(serde_json::json!({
        "path": shown(&params.target, base),
        "detail": params.detail.as_str(),
        "language": params.language.map(|l| l.name()),
        "totals": totals,
        "results": results,
        "truncated": omitted > 0,
        "omitted": omitted,
        "partial": warnings.partial(),
        "warnings": warnings.json(),
    }))
}

/// The first usually generated directory a counted path goes through.
fn generated_dir(shown: &str) -> Option<String> {
    let path = Path::new(shown);
    let mut prefix = PathBuf::new();
    for c in path.parent()?.components() {
        prefix.push(c);
        if GENERATED_DIRS.iter().any(|d| c.as_os_str() == *d) {
            return Some(prefix.to_string_lossy().into_owned());
        }
    }
    None
}

/// The language a file's name gives, with no file access: `None` when the
/// extension is unknown (the file is not counted).
fn detect_by_name(absent: &Path, path: &Path) -> Option<Detect> {
    let name = path.file_name()?;
    // `absent` never exists: when the name says nothing, Tokei's shebang
    // probe on it finds no file.
    if let Some(l) = LanguageType::from_path(absent.join(name), &Config::default()) {
        return Some(Detect::Lang(l));
    }
    Path::new(name)
        .extension()
        .is_none()
        .then_some(Detect::Shebang)
}

/// Opens, checks, reads and counts one file.
fn count_file(
    c: &Candidate,
    root: &Path,
    config: &Config,
    params: &Params,
    scratch: &Path,
    probe: &CodeStatsProbe,
    cancel: &CancellationToken,
) -> FileOutcome {
    if cancel.is_cancelled() {
        return FileOutcome::Cancelled;
    }
    let (file, before) = match open_checked(c, root) {
        Ok(f) => f,
        Err(cause) => return FileOutcome::Skipped(cause),
    };
    let lang = match c.detect {
        Detect::Lang(l) => l,
        Detect::Shebang => match shebang_language(&file, scratch) {
            Some(l) => l,
            None => return FileOutcome::NotCode,
        },
    };
    if params.language.is_some_and(|want| want != lang) {
        return FileOutcome::NotCode;
    }
    if let Some(f) = &probe.opened {
        f(&c.open);
    }
    let mut text = Vec::new();
    if encoding_rs_io::DecodeReaderBytesBuilder::new()
        .build(&file)
        .read_to_end(&mut text)
        .is_err()
    {
        return FileOutcome::Skipped("unreadable");
    }
    let changed = match file.metadata() {
        Ok(after) => after.len() != before.len() || after.modified().ok() != before.modified().ok(),
        Err(_) => true,
    };
    drop(file);
    if cancel.is_cancelled() {
        return FileOutcome::Cancelled;
    }
    let stats = lang.parse_from_slice(&text, config);
    let (code, comments, blanks) = if lang == LanguageType::Jupyter {
        // Tokei reads an invalid notebook as an empty one: say so instead.
        let Ok(notebook) = serde_json::from_slice::<Notebook>(&text) else {
            return FileOutcome::Skipped("invalid_notebook");
        };
        // The root counters already add up every cell, and the entries of
        // the cell languages repeat them: only the blocks nested inside the
        // cells (code in a Markdown cell) come on top, taken from the cell
        // languages' entries alone (Tokei also copies them to the top).
        let mut sum = (stats.code, stats.comments, stats.blanks);
        let mut cell_languages = vec![LanguageType::Markdown, notebook.language()];
        cell_languages.dedup();
        for cells in cell_languages.iter().filter_map(|l| stats.blobs.get(l)) {
            for nested in cells.blobs.values() {
                let n = nested.summarise();
                sum.0 += n.code;
                sum.1 += n.comments;
                sum.2 += n.blanks;
            }
        }
        sum
    } else {
        let s = stats.summarise();
        (s.code, s.comments, s.blanks)
    };
    FileOutcome::Counted {
        lang,
        code: code as u64,
        comments: comments as u64,
        blanks: blanks as u64,
        changed,
    }
}

/// The shape Tokei requires of a notebook (its own `parse_jupyter`).
#[derive(Deserialize)]
#[allow(dead_code)]
struct Notebook {
    cells: Vec<NotebookCell>,
    metadata: NotebookMetadata,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct NotebookCell {
    cell_type: CellType,
    source: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum CellType {
    Markdown,
    Code,
}

#[derive(Deserialize)]
struct NotebookMetadata {
    kernelspec: Value,
    language_info: Value,
}

impl Notebook {
    /// The language of the code cells, chosen as Tokei chooses it.
    fn language(&self) -> LanguageType {
        self.metadata
            .kernelspec
            .get("language")
            .and_then(Value::as_str)
            .and_then(|v| v.parse::<LanguageType>().ok())
            .or_else(|| {
                self.metadata
                    .language_info
                    .get("file_extension")
                    .and_then(Value::as_str)
                    .and_then(LanguageType::from_file_extension)
            })
            .unwrap_or(LanguageType::Python)
    }
}

/// The language a shebang names, read from the validated descriptor: its
/// first bytes are copied to a private file for Tokei's probe.
fn shebang_language(file: &File, scratch: &Path) -> Option<LanguageType> {
    let mut head = [0u8; SHEBANG_BYTES];
    let len = read_head(file, &mut head)?;
    if !head[..len].starts_with(b"#!") {
        return None;
    }
    let mut copy = tempfile::NamedTempFile::new_in(scratch).ok()?;
    std::io::Write::write_all(&mut copy, &head[..len]).ok()?;
    LanguageType::from_shebang(copy.path())
}

#[cfg(unix)]
fn read_head(file: &File, buf: &mut [u8]) -> Option<usize> {
    use std::os::unix::fs::FileExt;
    let mut len = 0;
    while len < buf.len() {
        match file.read_at(&mut buf[len..], len as u64) {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(len)
}

#[cfg(not(unix))]
fn read_head(file: &File, buf: &mut [u8]) -> Option<usize> {
    use std::io::{Seek, SeekFrom};
    let mut f = file;
    let n = f.read(buf).ok()?;
    f.seek(SeekFrom::Start(0)).ok()?;
    Some(n)
}

#[cfg(unix)]
fn identity(meta: &Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity(_meta: &Metadata) -> (u64, u64) {
    static UNIQUE: AtomicU64 = AtomicU64::new(0);
    (u64::MAX, UNIQUE.fetch_add(1, Ordering::Relaxed))
}

/// Opens the file the walk validated, or says why it is not that file any
/// more: a final link is not followed, a FIFO swapped in does not block, and
/// what is open must be the same regular file, still inside the root.
fn open_checked(c: &Candidate, root: &Path) -> std::result::Result<(File, Metadata), &'static str> {
    let file = open_no_follow(&c.open).map_err(|e| match e.raw_os_error() {
        _ if e.kind() == std::io::ErrorKind::NotFound => "vanished",
        #[cfg(unix)]
        Some(libc::ELOOP) => "changed",
        _ => "unreadable",
    })?;
    let meta = file.metadata().map_err(|_| "unreadable")?;
    // The walk saw a regular file: anything else now is another entry.
    if !meta.is_file() {
        return Err("changed");
    }
    #[cfg(unix)]
    if identity(&meta) != (c.dev, c.ino) {
        return Err("changed");
    }
    if let Some(real) = fd_path(&file)
        && !real.starts_with(root)
    {
        // A hard link may be named by another of its paths: the one opened
        // must still resolve to itself.
        let still = std::fs::canonicalize(&c.open).is_ok_and(|p| p == c.open);
        if !still {
            return Err("changed");
        }
    }
    Ok((file, meta))
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

/// The path the kernel holds for an open file.
#[cfg(target_os = "macos")]
fn fd_path(file: &File) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes, NUL included.
    let r = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if r == -1 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
}

#[cfg(target_os = "linux")]
fn fd_path(file: &File) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn fd_path(_file: &File) -> Option<PathBuf> {
    None
}
