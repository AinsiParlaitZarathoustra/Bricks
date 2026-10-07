//! Lexical search on ripgrep's libraries (`ignore` walker, `grep` matcher
//! and searcher). No `rg` binary is needed.
//!
//! Files and buffers are searched the same way: the document text (disk or
//! buffer) is loaded once into the query snapshot and searched as a slice,
//! so a buffer hides the disk version of its file and every match offset
//! refers to the exact text the rest of the response uses.

use crate::query::MatchMode;
use crate::view::{Document, Snapshot, Unreadable};
use grep::matcher::Matcher;
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Which files a walk visits.
#[derive(Debug, Clone, Default)]
pub struct WalkFilters {
    /// Whitelist globs (`*.rs`); empty: every file.
    pub include: Vec<String>,
    /// Gitignore-style globs to exclude.
    pub exclude: Vec<String>,
    /// Extensions without the dot.
    pub extensions: Vec<String>,
    /// Ignore `.gitignore`/`.ignore`/hidden rules.
    pub no_ignore: bool,
    /// Include hidden files.
    pub hidden: bool,
}

/// The walker shared by the engine and the `Grep` tool: gitignore rules
/// apply even outside a git checkout, hidden files are skipped by default.
pub fn walk_builder(root: &Path, filters: &WalkFilters) -> Result<WalkBuilder, String> {
    let mut builder = WalkBuilder::new(root);
    if filters.no_ignore {
        builder.standard_filters(false);
    } else {
        builder.require_git(false);
    }
    builder.hidden(!filters.hidden);
    let mut globs: Vec<String> = filters.include.clone();
    globs.extend(
        filters
            .extensions
            .iter()
            .map(|e| format!("*.{}", e.trim_start_matches('.'))),
    );
    if !globs.is_empty() || !filters.exclude.is_empty() {
        let mut ob = OverrideBuilder::new(root);
        for g in &globs {
            ob.add(g).map_err(|e| e.to_string())?;
        }
        for g in &filters.exclude {
            ob.add(&format!("!{g}")).map_err(|e| e.to_string())?;
        }
        builder.overrides(ob.build().map_err(|e| e.to_string())?);
    }
    Ok(builder)
}

/// Build the matcher: literal or regex, line-oriented.
pub fn build_matcher(
    pattern: &str,
    mode: MatchMode,
    case_insensitive: bool,
) -> Result<RegexMatcher, String> {
    RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .fixed_strings(mode == MatchMode::Literal)
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .map_err(|e| e.to_string())
}

/// Files of a walk, in a deterministic order.
#[derive(Debug, Default)]
pub struct Listing {
    pub files: Vec<PathBuf>,
    /// The file limit stopped the walk.
    pub truncated: bool,
    pub deadline_hit: bool,
}

/// List the files under `roots` (sorted walk, so truncation is
/// deterministic too), plus the view's overlay paths under the roots that
/// pass the same filters.
pub fn list_files(
    roots: &[PathBuf],
    filters: &WalkFilters,
    overlay_paths: &[PathBuf],
    max_files: usize,
    deadline: Instant,
) -> Result<Listing, String> {
    let mut listing = Listing::default();
    let mut seen = std::collections::BTreeSet::new();
    'roots: for root in roots {
        if root.is_file() {
            seen.insert(root.clone());
            continue;
        }
        let mut builder = walk_builder(root, filters)?;
        builder.sort_by_file_name(|a, b| a.cmp(b));
        for entry in builder.build() {
            if Instant::now() >= deadline {
                listing.deadline_hit = true;
                break 'roots;
            }
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            if seen.len() >= max_files {
                listing.truncated = true;
                break 'roots;
            }
            seen.insert(entry.into_path());
        }
    }
    // Buffers for files not on disk (yet) are searchable too.
    if !overlay_paths.is_empty() {
        for root in roots {
            for p in overlay_paths {
                if p.starts_with(root) && passes_filters(root, p, filters) {
                    seen.insert(p.clone());
                }
            }
        }
    }
    listing.files = seen.into_iter().collect();
    Ok(listing)
}

fn passes_filters(root: &Path, path: &Path, filters: &WalkFilters) -> bool {
    if !filters.extensions.is_empty() {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !filters
            .extensions
            .iter()
            .any(|e| e.trim_start_matches('.') == ext)
        {
            return false;
        }
    }
    if filters.include.is_empty() && filters.exclude.is_empty() {
        return true;
    }
    let mut ob = OverrideBuilder::new(root);
    for g in &filters.include {
        let _ = ob.add(g);
    }
    for g in &filters.exclude {
        let _ = ob.add(&format!("!{g}"));
    }
    match ob.build() {
        Ok(o) => !o.matched(path, false).is_ignore(),
        Err(_) => true,
    }
}

/// One match in one document.
#[derive(Debug, Clone)]
pub struct LexicalMatch {
    pub doc: Arc<Document>,
    /// Byte span of the match.
    pub start: usize,
    pub end: usize,
    /// Byte span of the matched line (without terminator).
    pub line_start: usize,
    pub line_end: usize,
}

/// Outcome of a lexical search.
#[derive(Debug, Default)]
pub struct LexicalOutcome {
    pub matches: Vec<LexicalMatch>,
    pub files_listed: usize,
    pub files_searched: usize,
    pub bytes_searched: u64,
    pub skipped: BTreeMap<String, Vec<String>>,
    pub file_limit_hit: bool,
    pub match_limit_hit: bool,
    pub deadline_hit: bool,
    pub cancelled: bool,
}

struct SpanSink<'a> {
    matcher: &'a RegexMatcher,
    text: &'a str,
    out: Vec<(usize, usize, usize, usize)>,
    remaining: usize,
}

impl Sink for SpanSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let block_start = m.absolute_byte_offset() as usize;
        // A sink match may span several lines only in multi-line mode, which
        // is off: it is one line, terminator included.
        let bytes = m.bytes();
        let mut line_len = bytes.len();
        while line_len > 0 && matches!(bytes[line_len - 1], b'\n' | b'\r') {
            line_len -= 1;
        }
        let line = &bytes[..line_len];
        let (ls, le) = (block_start, block_start + line_len);
        let mut found = false;
        let _ = self.matcher.find_iter(line, |span| {
            if self.remaining == 0 {
                return false;
            }
            let (s, e) = (ls + span.start(), ls + span.end());
            // Never split a UTF-8 sequence (a regex can match bytes).
            if self.text.is_char_boundary(s) && self.text.is_char_boundary(e) {
                self.out.push((s, e, ls, le));
                self.remaining -= 1;
                found = true;
            }
            true
        });
        if !found && self.remaining > 0 {
            // The line matched as a whole (e.g. an empty-width regex).
            self.out.push((ls, ls, ls, le));
            self.remaining -= 1;
        }
        Ok(self.remaining > 0)
    }
}

/// Search one document.
pub fn search_document(
    matcher: &RegexMatcher,
    doc: &Arc<Document>,
    max: usize,
) -> Vec<LexicalMatch> {
    let mut searcher = SearcherBuilder::new()
        .line_number(false)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build();
    let mut sink = SpanSink {
        matcher,
        text: &doc.text,
        out: Vec::new(),
        remaining: max,
    };
    let _ = searcher.search_slice(matcher, doc.text.as_bytes(), &mut sink);
    sink.out
        .into_iter()
        .map(|(start, end, line_start, line_end)| LexicalMatch {
            doc: Arc::clone(doc),
            start,
            end,
            line_start,
            line_end,
        })
        .collect()
}

/// What searching one file gave.
enum FileOutcome {
    Matches(Vec<LexicalMatch>, u64),
    NoMatch(u64),
    Skipped(String),
    Gone,
    NotRun,
}

fn search_file(
    snapshot: &Snapshot,
    matcher: &RegexMatcher,
    path: &Path,
    max: usize,
) -> FileOutcome {
    // A document this query already read, or a buffer: search that text.
    if let Some(known) = snapshot.known(path) {
        return match known {
            Ok(doc) => {
                let n = doc.text.len() as u64;
                FileOutcome::Matches(search_document(matcher, &doc, max), n)
            }
            Err(Unreadable::Deleted) | Err(Unreadable::NotFound) => FileOutcome::Gone,
            Err(why) => FileOutcome::Skipped(why.label_kind()),
        };
    }
    // A disk file: search the bytes first; only a matching file becomes a
    // document (validated, hashed, kept in the snapshot), so the text the
    // matches refer to is the text that was searched.
    let bytes = match snapshot.read_bytes(path) {
        Ok(b) => b,
        Err(Unreadable::Deleted) | Err(Unreadable::NotFound) => return FileOutcome::Gone,
        Err(why) => return FileOutcome::Skipped(why.label_kind()),
    };
    let n = bytes.len() as u64;
    if !matches!(matcher.find(&bytes), Ok(Some(_))) {
        return FileOutcome::NoMatch(n);
    }
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return FileOutcome::Skipped(Unreadable::NotUtf8.label_kind()),
    };
    let doc = snapshot.adopt(path, text);
    FileOutcome::Matches(search_document(matcher, &doc, max), n)
}

/// Search the listed files of a snapshot within limits. Files are searched
/// in parallel; results are taken in listing order, so the same snapshot
/// gives the same matches and the same truncation.
#[allow(clippy::too_many_arguments)]
pub fn search(
    snapshot: &Snapshot,
    matcher: &RegexMatcher,
    roots: &[PathBuf],
    filters: &WalkFilters,
    max_files: usize,
    max_matches: usize,
    deadline: Instant,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<LexicalOutcome, String> {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let listing = list_files(
        roots,
        filters,
        &snapshot.view.overlay_paths(),
        max_files,
        deadline,
    )?;
    let mut out = LexicalOutcome {
        files_listed: listing.files.len(),
        file_limit_hit: listing.truncated,
        deadline_hit: listing.deadline_hit,
        ..Default::default()
    };
    let files = &listing.files;
    let next = AtomicUsize::new(0);
    let stopped_by_time = AtomicBool::new(false);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    let mut slots: Vec<FileOutcome> = (0..files.len()).map(|_| FileOutcome::NotRun).collect();
    let per_thread: Vec<Vec<(usize, FileOutcome)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        if cancel.is_cancelled() {
                            break;
                        }
                        if Instant::now() >= deadline {
                            stopped_by_time.store(true, Ordering::Relaxed);
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= files.len() {
                            break;
                        }
                        local.push((i, search_file(snapshot, matcher, &files[i], max_matches)));
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    for (i, o) in per_thread.into_iter().flatten() {
        slots[i] = o;
    }
    for (path, outcome) in files.iter().zip(slots) {
        match outcome {
            FileOutcome::NotRun => {
                if cancel.is_cancelled() {
                    out.cancelled = true;
                } else if stopped_by_time.load(Ordering::Relaxed) {
                    out.deadline_hit = true;
                }
                break;
            }
            FileOutcome::Gone => continue,
            FileOutcome::Skipped(kind) => {
                out.skipped
                    .entry(kind)
                    .or_default()
                    .push(path.display().to_string());
            }
            FileOutcome::NoMatch(n) => {
                out.files_searched += 1;
                out.bytes_searched += n;
            }
            FileOutcome::Matches(found, n) => {
                out.files_searched += 1;
                out.bytes_searched += n;
                let room = max_matches - out.matches.len();
                if found.len() >= room {
                    out.matches.extend(found.into_iter().take(room));
                    out.match_limit_hit = true;
                    break;
                }
                out.matches.extend(found);
            }
        }
    }
    Ok(out)
}

impl Unreadable {
    /// Grouping key for skipped files.
    pub fn label_kind(&self) -> String {
        match self {
            Unreadable::TooLarge { limit, .. } => format!("larger than {limit} bytes"),
            other => other.label(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{ViewHandle, ViewKind};
    use std::collections::HashMap;
    use std::time::Duration;

    fn ws() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::create_dir_all(d.path().join("target")).unwrap();
        std::fs::write(d.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::write(d.path().join("src/a.rs"), "fn alpha() {}\n// alpha.beta\n").unwrap();
        std::fs::write(d.path().join("src/b.py"), "def alpha():\n    pass\n").unwrap();
        std::fs::write(d.path().join("target/gen.rs"), "fn alpha() {}\n").unwrap();
        d
    }

    fn run(
        d: &Path,
        view: ViewHandle,
        pat: &str,
        mode: MatchMode,
        filters: WalkFilters,
    ) -> LexicalOutcome {
        let snap = Snapshot::new(view, 1 << 20);
        let m = build_matcher(pat, mode, false).unwrap();
        search(
            &snap,
            &m,
            &[d.to_path_buf()],
            &filters,
            1000,
            1000,
            Instant::now() + Duration::from_secs(5),
            &Default::default(),
        )
        .unwrap()
    }

    fn shared() -> ViewHandle {
        ViewHandle::new(ViewKind::Shared, HashMap::new())
    }

    #[test]
    fn literal_regex_positions_and_ignores() {
        let d = ws();
        let out = run(
            d.path(),
            shared(),
            "alpha.beta",
            MatchMode::Literal,
            Default::default(),
        );
        assert_eq!(out.matches.len(), 1);
        let m = &out.matches[0];
        assert_eq!(&m.doc.text[m.start..m.end], "alpha.beta");
        assert_eq!(&m.doc.text[m.line_start..m.line_end], "// alpha.beta");

        // Regex: `.` matches any char; gitignored target/ is skipped.
        let out = run(
            d.path(),
            shared(),
            r"fn alph.\(",
            MatchMode::Regex,
            Default::default(),
        );
        assert_eq!(out.matches.len(), 1);
        assert!(out.matches[0].doc.path.ends_with("src/a.rs"));
        assert_eq!(out.matches[0].start, 0);
        assert_eq!(out.matches[0].end, 9);

        // Extension filter and exclusion.
        let f = WalkFilters {
            extensions: vec!["py".into()],
            ..Default::default()
        };
        let out = run(d.path(), shared(), "alpha", MatchMode::Literal, f);
        assert_eq!(out.matches.len(), 1);
        assert!(out.matches[0].doc.path.ends_with("b.py"));
        let f = WalkFilters {
            exclude: vec!["*.py".into()],
            ..Default::default()
        };
        let out = run(d.path(), shared(), "alpha", MatchMode::Literal, f);
        assert!(out.matches.iter().all(|m| !m.doc.path.ends_with("b.py")));
    }

    #[test]
    fn buffer_replaces_disk_version() {
        let d = ws();
        let view = ViewHandle::new(ViewKind::Private, HashMap::new());
        let a = d.path().join("src/a.rs");
        view.set_buffer(&a, "fn gamma() {}\n", 2).unwrap();
        view.set_buffer(d.path().join("src/new.rs"), "fn alpha_new() {}\n", 1)
            .unwrap();
        let out = run(
            d.path(),
            view,
            "alpha",
            MatchMode::Literal,
            Default::default(),
        );
        let files: Vec<_> = out
            .matches
            .iter()
            .map(|m| {
                m.doc
                    .path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert!(!files.contains(&"a.rs".to_string()), "old disk text hidden");
        assert!(
            files.contains(&"new.rs".to_string()),
            "unsaved new file searched"
        );
    }

    #[test]
    fn limits_are_reported_and_deterministic() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..20 {
            std::fs::write(d.path().join(format!("f{i:02}.txt")), "x x x\n").unwrap();
        }
        let snap = Snapshot::new(shared(), 1 << 20);
        let m = build_matcher("x", MatchMode::Literal, false).unwrap();
        let go = |max_files, max_matches| {
            search(
                &snap,
                &m,
                &[d.path().to_path_buf()],
                &Default::default(),
                max_files,
                max_matches,
                Instant::now() + Duration::from_secs(5),
                &Default::default(),
            )
            .unwrap()
        };
        let a = go(5, 1000);
        assert!(a.file_limit_hit);
        assert_eq!(a.files_listed, 5);
        assert_eq!(a.matches.len(), 15);
        let b = go(100, 7);
        assert!(b.match_limit_hit);
        assert_eq!(b.matches.len(), 7);
        let c = go(100, 7);
        let key = |o: &LexicalOutcome| {
            o.matches
                .iter()
                .map(|m| (m.doc.path.clone(), m.start))
                .collect::<Vec<_>>()
        };
        assert_eq!(key(&b), key(&c));
        // Deadline already passed.
        let late = search(
            &snap,
            &m,
            &[d.path().to_path_buf()],
            &Default::default(),
            100,
            100,
            Instant::now(),
            &Default::default(),
        )
        .unwrap();
        assert!(late.deadline_hit);
    }

    #[test]
    fn unicode_and_crlf_offsets() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("u.txt"), "é😀 cible\r\nautre cible\r\n").unwrap();
        let out = run(
            d.path(),
            shared(),
            "cible",
            MatchMode::Literal,
            Default::default(),
        );
        assert_eq!(out.matches.len(), 2);
        let m = &out.matches[0];
        assert_eq!(m.start, "é😀 ".len());
        assert_eq!(&m.doc.text[m.line_start..m.line_end], "é😀 cible");
        let m = &out.matches[1];
        assert_eq!(&m.doc.text[m.line_start..m.line_end], "autre cible");
    }
}
