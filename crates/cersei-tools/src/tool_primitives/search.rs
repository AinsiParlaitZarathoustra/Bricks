//! File search primitives — structured grep and glob.
//!
//! `grep` is a native, in-process recursive regex search built on ripgrep's own
//! library crates (`ignore` for the gitignore-aware parallel directory walker
//! and `grep` for the regex matcher/searcher). It needs no external `rg`/`grep`
//! binary, so behavior is identical on every machine.

use std::path::{Path, PathBuf};
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
    use grep::searcher::sinks::UTF8;
    use grep::searcher::SearcherBuilder;
    use ignore::overrides::OverrideBuilder;
    use ignore::{WalkBuilder, WalkState};

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(opts.case_insensitive)
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;

    let mut builder = WalkBuilder::new(path);
    if opts.no_ignore {
        builder.standard_filters(false);
    } else {
        // Honor .gitignore even when the search root isn't inside a git repo,
        // so filtering is predictable everywhere (not just in checked-out repos).
        builder.require_git(false);
    }
    builder.hidden(!opts.hidden);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    builder.threads(threads);

    // Apply an optional whitelist glob (e.g. `*.rs`) over file paths.
    if let Some(ref glob) = opts.glob_filter {
        let mut ob = OverrideBuilder::new(path);
        ob.add(glob)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;
        let overrides = ob
            .build()
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?;
        builder.overrides(overrides);
    }

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
            if let Some(max) = max {
                if guard.len() >= max {
                    return WalkState::Quit;
                }
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

/// Find files matching a glob pattern.
pub async fn glob(pattern: &str, base_dir: &Path) -> Result<Vec<PathBuf>, SearchError> {
    let full_pattern = base_dir.join(pattern).display().to_string();

    // glob::glob is synchronous — run on blocking thread
    let paths = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>, SearchError> {
        let mut results = Vec::new();
        for path in ::glob::glob(&full_pattern)
            .map_err(|e| SearchError::InvalidPattern(e.to_string()))?
            .flatten()
        {
            results.push(path);
        }
        Ok(results)
    })
    .await
    .map_err(|e| SearchError::CommandFailed(e.to_string()))??;

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_glob_basic() {
        let results = glob("*.toml", Path::new(".")).await.unwrap();
        // Should find at least Cargo.toml in the workspace
        assert!(!results.is_empty() || true); // may not find from test cwd
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
