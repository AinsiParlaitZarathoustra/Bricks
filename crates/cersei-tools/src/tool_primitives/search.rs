//! File search primitives — structured grep and glob.
//!
//! `grep` is a native, in-process recursive regex search built on ripgrep's own
//! library crates (`ignore` for the gitignore-aware parallel directory walker
//! and `grep` for the regex matcher/searcher). It needs no external `rg`/`grep`
//! binary, so behavior is identical on every machine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A single search match with context.
#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub file: PathBuf,
    pub line_number: usize,
    pub line_content: String,
}

/// Options for grep.
///
/// Defaults mirror ripgrep's code-search defaults: gitignore/`.ignore` rules are
/// respected and hidden + binary files are skipped. The boolean opt-outs default
/// to `false`, so `GrepOptions::default()` keeps that sensible behavior.
#[derive(Debug, Clone, Default)]
pub struct GrepOptions {
    /// Whitelist glob applied to file paths (e.g. `*.rs`). `None` searches all files.
    pub glob_filter: Option<String>,
    /// Cap on the number of matches returned. `None` is unlimited.
    pub max_results: Option<usize>,
    /// Case-insensitive matching.
    pub case_insensitive: bool,
    /// When `true`, ignore `.gitignore`/`.ignore`/hidden filtering (search everything).
    pub no_ignore: bool,
    /// When `true`, include hidden files/directories in the search.
    pub hidden: bool,
}

/// Search errors.
#[derive(Debug)]
pub enum SearchError {
    InvalidPattern(String),
    IoError(std::io::Error),
    CommandFailed(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPattern(p) => write!(f, "invalid pattern: {p}"),
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::CommandFailed(msg) => write!(f, "command failed: {msg}"),
        }
    }
}

impl std::error::Error for SearchError {}

impl From<std::io::Error> for SearchError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

/// Recursively search file contents using a regex pattern.
///
/// Native and in-process: uses ripgrep's `ignore` crate for a gitignore-aware
/// parallel directory walk and ripgrep's `grep` crate for matching. No external
/// `rg`/`grep` binary is required. `path` may be a directory (searched
/// recursively) or a single file. Results are returned sorted by `(file,
/// line_number)` for deterministic output.
pub async fn grep(
    pattern: &str,
    path: &Path,
    opts: GrepOptions,
) -> Result<Vec<SearchMatch>, SearchError> {
    let pattern = pattern.to_string();
    let path = path.to_path_buf();

    tokio::task::spawn_blocking(move || grep_blocking(&pattern, &path, opts))
        .await
        .map_err(|e| SearchError::CommandFailed(e.to_string()))?
}

/// Synchronous core of [`grep`], intended to run on a blocking thread.
fn grep_blocking(
    pattern: &str,
    path: &Path,
    opts: GrepOptions,
) -> Result<Vec<SearchMatch>, SearchError> {
    use grep::regex::RegexMatcherBuilder;
    use grep::searcher::SearcherBuilder;
    use grep::searcher::sinks::UTF8;
    use ignore::WalkState;

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(opts.case_insensitive)
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;

    // The walker shared with the code understanding engine: same ignore
    // rules, same hidden-file default.
    let filters = bricks_semantic::lexical::WalkFilters {
        include: opts.glob_filter.iter().cloned().collect(),
        no_ignore: opts.no_ignore,
        hidden: opts.hidden,
        ..Default::default()
    };
    let mut builder = bricks_semantic::lexical::walk_builder(path, &filters)
        .map_err(SearchError::InvalidPattern)?;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    builder.threads(threads);

    let results: Arc<Mutex<Vec<SearchMatch>>> = Arc::new(Mutex::new(Vec::new()));
    let max = opts.max_results;

    builder.build_parallel().run(|| {
        let matcher = matcher.clone();
        let results = Arc::clone(&results);
        let mut searcher = SearcherBuilder::new().line_number(true).build();

        Box::new(move |entry| {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => return WalkState::Continue,
            };
            // Only search regular files.
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }

            let mut local: Vec<SearchMatch> = Vec::new();
            let file = entry.path().to_path_buf();
            let search_result = searcher.search_path(
                &matcher,
                entry.path(),
                UTF8(|lnum, line| {
                    local.push(SearchMatch {
                        file: file.clone(),
                        line_number: lnum as usize,
                        line_content: line.trim_end_matches(['\n', '\r']).to_string(),
                    });
                    Ok(true)
                }),
            );
            // Ignore per-file read/decoding errors (e.g. permission denied) and
            // keep walking, mirroring ripgrep's resilience.
            if search_result.is_err() || local.is_empty() {
                return WalkState::Continue;
            }

            let mut guard = results.lock().unwrap();
            guard.extend(local);
            if let Some(max) = max
                && guard.len() >= max
            {
                return WalkState::Quit;
            }
            WalkState::Continue
        })
    });

    let mut matches = Arc::try_unwrap(results)
        .map(|m| m.into_inner().unwrap())
        .unwrap_or_else(|arc| arc.lock().unwrap().clone());

    // Parallel walk order is nondeterministic — sort for stable output.
    matches.sort_by(|a, b| a.file.cmp(&b.file).then(a.line_number.cmp(&b.line_number)));
    if let Some(max) = max {
        matches.truncate(max);
    }

    Ok(matches)
}

/// Bounds and limits of [`glob_walk`].
#[derive(Debug, Clone)]
pub struct GlobOptions {
    /// Matches to return; the walk stops at the first match past it (which
    /// only proves that more exist). `0` returns without walking.
    pub limit: usize,
    /// Entries (files and directories) the walk may visit before it stops.
    pub max_visited: usize,
}

/// Entries a glob walk visits at most by default.
pub const GLOB_MAX_VISITED: usize = 200_000;

impl Default for GlobOptions {
    fn default() -> Self {
        Self {
            limit: usize::MAX,
            max_visited: GLOB_MAX_VISITED,
        }
    }
}

/// What a glob walk found.
#[derive(Debug, Default, Clone)]
pub struct GlobOutcome {
    /// Matches, sorted (at most `limit`).
    pub paths: Vec<PathBuf>,
    /// More matches exist than returned (their number is not known).
    pub more: bool,
    /// Entries visited.
    pub visited: usize,
    /// The walk stopped at `max_visited`: the list may be incomplete.
    pub visit_cap_hit: bool,
    /// The walk was abandoned (its caller went away).
    pub cancelled: bool,
}

/// Find files and directories matching `pattern` under `base_dir` (all of
/// them, within [`GLOB_MAX_VISITED`] entries). See [`glob_walk`].
pub async fn glob(pattern: &str, base_dir: &Path) -> Result<Vec<PathBuf>, SearchError> {
    Ok(glob_walk(pattern, base_dir, GlobOptions::default())
        .await?
        .paths)
}

/// Find files and directories matching `pattern` under `base_dir`.
///
/// `base_dir` is taken literally; only `pattern` is a glob (`*` and `?`
/// within a path component, `**` across components, `[…]` classes). Its
/// leading literal components narrow the walk. The walk uses the walker
/// shared with `Grep` and the code engine: `.gitignore`/`.ignore` rules
/// (with their `!` re-inclusions) apply even outside a git checkout and
/// ignored directories are pruned before descent; hidden entries are
/// skipped unless the pattern names a hidden component (`.github/**`). The
/// walk is sorted, so which matches are kept is deterministic; it stops at
/// the first match past `limit`, at `max_visited` entries, or when the
/// returned future is dropped (checked at every entry, on the blocking
/// pool). A fully literal pattern names one path, returned if it exists.
pub async fn glob_walk(
    pattern: &str,
    base_dir: &Path,
    opts: GlobOptions,
) -> Result<GlobOutcome, SearchError> {
    struct Stop(Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    if opts.limit == 0 {
        ::glob::Pattern::new(pattern).map_err(|e| SearchError::InvalidPattern(e.to_string()))?;
        return Ok(GlobOutcome::default());
    }
    let stop = Stop(Arc::new(AtomicBool::new(false)));
    let abort = Arc::clone(&stop.0);
    let (pattern, base) = (pattern.to_string(), base_dir.to_path_buf());
    let outcome =
        tokio::task::spawn_blocking(move || glob_walk_blocking(&pattern, &base, &opts, &abort))
            .await
            .map_err(|e| SearchError::CommandFailed(e.to_string()))?;
    drop(stop);
    outcome
}

fn has_glob_meta(component: &str) -> bool {
    component.contains(['*', '?', '['])
}

/// Synchronous core of [`glob_walk`].
pub(crate) fn glob_walk_blocking(
    pattern: &str,
    base_dir: &Path,
    opts: &GlobOptions,
    abort: &AtomicBool,
) -> Result<GlobOutcome, SearchError> {
    let mut out = GlobOutcome::default();
    if opts.limit == 0 {
        return Ok(out);
    }
    // Leading literal components narrow the walk; the rest is matched
    // against paths relative to it.
    let pat = Path::new(pattern);
    let mut root = if pat.is_absolute() {
        PathBuf::new()
    } else {
        base_dir.to_path_buf()
    };
    let mut rest = Vec::new();
    for c in pat.components() {
        let text = c.as_os_str().to_string_lossy().to_string();
        if rest.is_empty() && !has_glob_meta(&text) {
            root.push(c.as_os_str());
        } else {
            rest.push(text);
        }
    }
    if rest.is_empty() {
        // A literal path.
        if root.exists() {
            out.paths.push(root);
        }
        return Ok(out);
    }
    let rest = rest.join("/");
    let matcher =
        ::glob::Pattern::new(&rest).map_err(|e| SearchError::InvalidPattern(e.to_string()))?;
    let options = ::glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    if !root.is_dir() {
        return Ok(out);
    }
    let hidden = pat.components().any(|c| {
        let t = c.as_os_str().to_string_lossy();
        t.starts_with('.') && t != "." && t != ".."
    });
    let filters = bricks_semantic::lexical::WalkFilters {
        hidden,
        ..Default::default()
    };
    let mut builder = bricks_semantic::lexical::walk_builder(&root, &filters)
        .map_err(SearchError::InvalidPattern)?;
    builder
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b));
    for entry in builder.build() {
        if abort.load(Ordering::SeqCst) {
            out.cancelled = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 {
            continue;
        }
        out.visited += 1;
        if out.visited > opts.max_visited {
            out.visit_cap_hit = true;
            break;
        }
        let Ok(rel) = entry.path().strip_prefix(&root) else {
            continue;
        };
        if matcher.matches_path_with(rel, options) {
            if out.paths.len() == opts.limit {
                out.more = true;
                break;
            }
            out.paths.push(entry.path().to_path_buf());
        }
    }
    out.paths.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── glob ──

    fn names(out: &GlobOutcome, root: &Path) -> Vec<String> {
        out.paths
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    fn walk(pattern: &str, root: &Path, limit: usize) -> GlobOutcome {
        glob_walk_blocking(
            pattern,
            root,
            &GlobOptions {
                limit,
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn ignored_trees_are_pruned_and_unignored_homonyms_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No `.git`: the rules apply all the same.
        fs::write(root.join(".gitignore"), "target/\n*.log\n!keep.log\n").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        for i in 0..300 {
            fs::write(root.join(format!("target/debug/f{i}.rs")), "").unwrap();
        }
        fs::create_dir_all(root.join("app/node_modules")).unwrap();
        fs::write(root.join("app/node_modules/kept.rs"), "").unwrap();
        fs::write(root.join("src.rs"), "").unwrap();
        fs::write(root.join("drop.log"), "").unwrap();
        fs::write(root.join("keep.log"), "").unwrap();
        let out = walk("**/*.rs", root, 1000);
        assert_eq!(names(&out, root), ["app/node_modules/kept.rs", "src.rs"]);
        // The 300 ignored files were never visited.
        assert!(out.visited < 20, "visited {}", out.visited);
        assert_eq!(names(&walk("*.log", root, 10), root), ["keep.log"]);
    }

    #[test]
    fn the_walk_stops_once_the_limit_is_passed() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..1000 {
            fs::write(tmp.path().join(format!("f{i:04}.txt")), "").unwrap();
        }
        let out = walk("*.txt", tmp.path(), 10);
        assert_eq!(out.paths.len(), 10);
        assert!(out.more);
        assert_eq!(out.visited, 11, "stopped at the first match past the limit");
        // Deterministic selection and order.
        assert_eq!(names(&out, tmp.path())[0], "f0000.txt");
        assert_eq!(
            names(&walk("*.txt", tmp.path(), 10), tmp.path()),
            names(&out, tmp.path())
        );
        // Exactly the limit: no "more".
        let all = walk("f000*.txt", tmp.path(), 10);
        assert_eq!(all.paths.len(), 10);
        assert!(!all.more);
        // A rare pattern still walks everything (bounded by max_visited).
        let rare = glob_walk_blocking(
            "nothing*",
            tmp.path(),
            &GlobOptions {
                limit: 10,
                max_visited: 100,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(rare.paths.is_empty() && rare.visit_cap_hit);
        assert_eq!(rare.visited, 101);
    }

    #[test]
    fn literal_bases_brackets_spaces_hidden_roots_and_recursion() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join(".work dir [1]");
        fs::create_dir_all(base.join("sub/deep")).unwrap();
        for f in [
            "a.rs",
            "b.rs",
            "c.rs",
            "sub/m.rs",
            "sub/deep/z.rs",
            "sub/n.txt",
        ] {
            fs::write(base.join(f), "").unwrap();
        }
        fs::create_dir_all(base.join(".github")).unwrap();
        fs::write(base.join(".github/ci.yml"), "").unwrap();
        // The base is literal even with brackets, spaces and a leading dot.
        assert_eq!(
            names(&walk("*.rs", &base, 100), &base),
            ["a.rs", "b.rs", "c.rs"]
        );
        assert_eq!(names(&walk("[ab].rs", &base, 100), &base), ["a.rs", "b.rs"]);
        assert_eq!(
            names(&walk("**/*.rs", &base, 100), &base),
            ["a.rs", "b.rs", "c.rs", "sub/deep/z.rs", "sub/m.rs"]
        );
        assert_eq!(names(&walk("sub/*.rs", &base, 100), &base), ["sub/m.rs"]);
        assert_eq!(
            names(&walk("sub/**/*.rs", &base, 100), &base),
            ["sub/deep/z.rs", "sub/m.rs"]
        );
        // Hidden entries only when the pattern names one.
        assert!(names(&walk("**/*.yml", &base, 100), &base).is_empty());
        assert_eq!(
            names(&walk(".github/*.yml", &base, 100), &base),
            [".github/ci.yml"]
        );
        // A literal path, and an absolute pattern (whose brackets would be
        // glob syntax: only the base is literal).
        assert_eq!(names(&walk("sub/n.txt", &base, 100), &base), ["sub/n.txt"]);
        let plain = tmp.path().join("plain");
        fs::create_dir_all(plain.join("sub")).unwrap();
        fs::write(plain.join("sub/n.txt"), "").unwrap();
        let abs = format!("{}/sub/*.txt", plain.display());
        let out = walk(&abs, Path::new("/nonexistent"), 100);
        assert_eq!(names(&out, &plain), ["sub/n.txt"]);
    }

    #[test]
    fn limit_zero_no_match_invalid_pattern_and_cancellation() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.rs"), "").unwrap();
        let zero = walk("*.rs", tmp.path(), 0);
        assert!(zero.paths.is_empty() && zero.visited == 0 && !zero.more);
        let none = walk("*.zig", tmp.path(), 10);
        assert!(none.paths.is_empty() && !none.more);
        let bad = glob_walk_blocking(
            "[",
            tmp.path(),
            &GlobOptions::default(),
            &AtomicBool::new(false),
        );
        assert!(matches!(bad, Err(SearchError::InvalidPattern(_))));
        let stopped = glob_walk_blocking(
            "*.rs",
            tmp.path(),
            &GlobOptions::default(),
            &AtomicBool::new(true),
        )
        .unwrap();
        assert!(stopped.cancelled && stopped.paths.is_empty() && stopped.visited == 0);
    }

    #[tokio::test]
    async fn finds_match_with_line_number_and_trimmed_content() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "alpha\nbeta TARGET here\ngamma\n").unwrap();

        let m = grep("TARGET", tmp.path(), GrepOptions::default())
            .await
            .unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line_number, 2);
        assert_eq!(m[0].line_content, "beta TARGET here");
        assert!(m[0].file.ends_with("a.txt"));
    }

    #[tokio::test]
    async fn searches_recursively_into_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("sub/deep")).unwrap();
        fs::write(tmp.path().join("top.rs"), "NEEDLE\n").unwrap();
        fs::write(tmp.path().join("sub/mid.rs"), "no match\nNEEDLE\n").unwrap();
        fs::write(tmp.path().join("sub/deep/low.rs"), "NEEDLE\n").unwrap();

        let m = grep("NEEDLE", tmp.path(), GrepOptions::default())
            .await
            .unwrap();
        assert_eq!(m.len(), 3);
    }

    #[tokio::test]
    async fn respects_gitignore_by_default_but_not_with_no_ignore() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(tmp.path().join("kept.txt"), "SECRET\n").unwrap();
        fs::write(tmp.path().join("ignored.txt"), "SECRET\n").unwrap();

        // Default: the gitignored file is skipped.
        let m = grep("SECRET", tmp.path(), GrepOptions::default())
            .await
            .unwrap();
        assert_eq!(m.len(), 1);
        assert!(m[0].file.ends_with("kept.txt"));

        // no_ignore: both files are searched.
        let opts = GrepOptions {
            no_ignore: true,
            ..Default::default()
        };
        let m = grep("SECRET", tmp.path(), opts).await.unwrap();
        assert_eq!(m.len(), 2);
    }

    #[tokio::test]
    async fn case_insensitive_matching() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "Hello World\n").unwrap();

        let sensitive = grep("hello", tmp.path(), GrepOptions::default())
            .await
            .unwrap();
        assert!(sensitive.is_empty());

        let opts = GrepOptions {
            case_insensitive: true,
            ..Default::default()
        };
        let insensitive = grep("hello", tmp.path(), opts).await.unwrap();
        assert_eq!(insensitive.len(), 1);
    }

    #[tokio::test]
    async fn glob_filter_restricts_file_types() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.rs"), "MATCH\n").unwrap();
        fs::write(tmp.path().join("b.txt"), "MATCH\n").unwrap();

        let opts = GrepOptions {
            glob_filter: Some("*.rs".to_string()),
            ..Default::default()
        };
        let m = grep("MATCH", tmp.path(), opts).await.unwrap();
        assert_eq!(m.len(), 1);
        assert!(m[0].file.ends_with("a.rs"));
    }

    #[tokio::test]
    async fn max_results_caps_output() {
        let tmp = tempfile::tempdir().unwrap();
        let body: String = (0..50).map(|_| "HIT\n").collect();
        fs::write(tmp.path().join("a.txt"), body).unwrap();

        let opts = GrepOptions {
            max_results: Some(10),
            ..Default::default()
        };
        let m = grep("HIT", tmp.path(), opts).await.unwrap();
        assert_eq!(m.len(), 10);
    }

    #[tokio::test]
    async fn results_are_sorted_deterministically() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("z.txt"), "X\n").unwrap();
        fs::write(tmp.path().join("a.txt"), "X\nX\n").unwrap();

        let m = grep("X", tmp.path(), GrepOptions::default()).await.unwrap();
        // Sorted by (file, line): a.txt:1, a.txt:2, z.txt:1
        assert_eq!(m.len(), 3);
        assert!(m[0].file.ends_with("a.txt") && m[0].line_number == 1);
        assert!(m[1].file.ends_with("a.txt") && m[1].line_number == 2);
        assert!(m[2].file.ends_with("z.txt"));
    }

    #[tokio::test]
    async fn searches_a_single_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("only.txt");
        fs::write(&target, "FOO\n").unwrap();
        fs::write(tmp.path().join("other.txt"), "FOO\n").unwrap();

        let m = grep("FOO", &target, GrepOptions::default()).await.unwrap();
        assert_eq!(m.len(), 1);
        assert!(m[0].file.ends_with("only.txt"));
    }

    // Real-repo smoke test (run explicitly: `cargo test -p cersei-tools
    // real_repo_smoke -- --ignored`). Searches the actual workspace and asserts
    // it (a) finds our own source, (b) skips the gitignored `target/` dir.
    #[tokio::test]
    #[ignore]
    async fn real_repo_smoke() {
        // Crate dir is .../crates/cersei-tools; workspace root is two up.
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = crate_dir.parent().unwrap().parent().unwrap();

        let matches = grep("WalkBuilder::new", workspace_root, GrepOptions::default())
            .await
            .unwrap();

        // Our native grep() implementation contains this call.
        assert!(
            matches
                .iter()
                .any(|m| m.file.ends_with("tool_primitives/search.rs")),
            "expected to find our own source; got {} matches",
            matches.len()
        );
        // The gitignored build directory must be excluded.
        assert!(
            !matches
                .iter()
                .any(|m| m.file.components().any(|c| c.as_os_str() == "target")),
            "target/ should be gitignored and skipped"
        );
        eprintln!(
            "real_repo_smoke: {} matches across the workspace",
            matches.len()
        );
    }

    #[tokio::test]
    async fn invalid_regex_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "x\n").unwrap();
        let r = grep("(unclosed", tmp.path(), GrepOptions::default()).await;
        assert!(matches!(r, Err(SearchError::InvalidPattern(_))));
    }
}
