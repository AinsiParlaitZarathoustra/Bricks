//! CodeSearch tool: hybrid BM25 + vector semantic search.
//!
//! Two modes:
//! 1. BM25 only (default): tantivy full-text search. No API calls.
//! 2. BM25 + Vector: BM25 candidates merged with HNSW vector k-NN search
//!    backed by the `cersei-embeddings` crate (Gemini / OpenAI / custom).
//!
//! ## Index lifecycle and freshness
//!
//! There is one index per canonical root and embedding configuration (the
//! provider's `model_id`, never a key). Every search first takes an
//! **inventory** of the sources: a walk with the workspace's ignore rules
//! (`.gitignore`, `.ignore`, hidden entries; the shared walker of the code
//! engine) and the tool's own exclusions, recording the path, size and
//! modification time of every indexable file. Its fingerprint is the
//! revision of the sources. An index is reused only for the same revision
//! and when no Bricks tool reported a change since its build began
//! ([`invalidate`], called by `code_scout::notify_workspace_changed` after a
//! call that may have written). So a creation, deletion, rename, external
//! edit or a change of ignore rules is seen at the next search, without a
//! watcher.
//!
//! Limit: an external edit that keeps both the size and the modification
//! time of a file (a coarse-timestamp filesystem, `touch -r`) is not seen by
//! the inventory; edits made through Bricks are, by notification.
//!
//! A build reads and chunks the files once: BM25, metadata and vectors all
//! come from that one snapshot of chunks, in its order. Walking, reading,
//! chunking and building run on the blocking pool (at most
//! [`MAX_CONCURRENT_BUILDS`] at once); vectors of unchanged chunks are
//! reused from the previous index of the same configuration. Concurrent
//! searches of one revision share one build. A build is published only if
//! nothing invalidated it meanwhile; a build whose callers all went away
//! stops at the next file (it cannot interrupt a system call already
//! blocked) and publishes nothing.

use crate::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use async_trait::async_trait;
use cersei_embeddings::{EmbeddingProvider, Metric, VectorIndex};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexReader, ReloadPolicy, TantivyDocument, doc};
use tokio::sync::watch;

// ─── Config ────────────────────────────────────────────────────────────────

const CHUNK_LINES: usize = 50;
const CHUNK_OVERLAP: usize = 10;
const BM25_CANDIDATES: usize = 20;
const VECTOR_CANDIDATES: usize = 20;
const DEFAULT_RESULTS: usize = 10;
const CHUNK_EMBED_CHARS: usize = 500;
/// Larger files are not indexed.
const MAX_FILE_BYTES: u64 = 500_000;
/// Directory names never indexed, in addition to the ignore rules.
const EXCLUDED_DIRS: &[&str] = &["node_modules", "target", "__pycache__", "venv"];
/// Indexes kept in memory (least recently used dropped first). Unit tests,
/// which run in parallel on one process-wide cache, keep more.
const MAX_CACHED_INDEXES: usize = if cfg!(test) { 256 } else { 4 };
/// Builds running at once, all workspaces together.
pub const MAX_CONCURRENT_BUILDS: usize = 2;
/// Attempts of one search when the sources change during its build.
const MAX_BUILD_ATTEMPTS: usize = 3;

const INDEXED_EXTENSIONS: &[&str] = &[
    "bash", "c", "cc", "cpp", "cs", "css", "go", "h", "hh", "hpp", "htm", "html", "java", "js",
    "json", "jsx", "kt", "lua", "md", "mjs", "proto", "py", "rb", "rs", "sass", "scss", "sh",
    "sql", "swift", "toml", "ts", "tsx", "txt", "xml", "yaml", "yml", "zsh", "cjs", "graphql",
    "gql", "jsonc", "ml", "mli", "f90", "f95", "cobol", "cbl", "ocaml",
];

// ─── Chunk metadata ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ChunkMeta {
    /// Relative to the indexed root (shown under the caller's spelling of
    /// it).
    path: String,
    start_line: usize,
    end_line: usize,
    content: String,
}

// ─── Inventory ─────────────────────────────────────────────────────────────

fn should_index(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| INDEXED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// The indexable files under a root, with what identifies their revision.
struct Inventory {
    /// (relative path, size, modification time in ns), sorted by path.
    files: Vec<(PathBuf, u64, u128)>,
    fingerprint: u64,
}

/// Walk `root` with the workspace's ignore rules (the walker shared with the
/// code engine and `Grep`: `.gitignore`/`.ignore` even outside a checkout,
/// hidden entries skipped; the root itself is always walked) and the tool's
/// exclusions. Blocking.
fn inventory(root: &Path) -> Result<Inventory, String> {
    let mut builder = bricks_semantic::lexical::walk_builder(
        root,
        &bricks_semantic::lexical::WalkFilters::default(),
    )?;
    builder
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_some_and(|t| t.is_dir())
                || !EXCLUDED_DIRS.contains(&e.file_name().to_str().unwrap_or(""))
        });
    let mut files = Vec::new();
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) || !should_index(entry.path()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_path_buf();
        files.push((rel, meta.len(), mtime));
    }
    files.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    files.hash(&mut h);
    Ok(Inventory {
        fingerprint: h.finish(),
        files,
    })
}

// ─── Indexing ──────────────────────────────────────────────────────────────

fn chunk_file(rel: &Path, content: &str) -> Vec<ChunkMeta> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }
    let path_str = rel.display().to_string();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let end = (start + CHUNK_LINES).min(lines.len());
        let chunk_content = lines[start..end].join("\n");
        if !chunk_content.trim().is_empty() {
            chunks.push(ChunkMeta {
                path: path_str.clone(),
                content: chunk_content,
                start_line: start + 1,
                end_line: end,
            });
        }
        if end >= lines.len() {
            break;
        }
        start += CHUNK_LINES - CHUNK_OVERLAP;
    }
    chunks
}

/// Read and chunk the inventoried files: the one collection of a build.
/// Checks `abort` between files. Blocking. `None` when aborted.
fn collect_chunks(
    root: &Path,
    files: &[(PathBuf, u64, u128)],
    abort: &AtomicBool,
) -> Option<(Vec<ChunkMeta>, usize)> {
    let mut chunks = Vec::new();
    let mut read = 0;
    for (rel, _, _) in files {
        if abort.load(Ordering::SeqCst) {
            #[cfg(test)]
            tests::hook(root, true);
            return None;
        }
        #[cfg(test)]
        tests::hook(root, false);
        // A file removed or unreadable since the inventory is left out; the
        // next inventory sees the difference.
        if let Ok(content) = std::fs::read_to_string(root.join(rel)) {
            read += 1;
            chunks.extend(chunk_file(rel, &content));
        }
    }
    Some((chunks, read))
}

fn build_bm25_index(
    chunks: &[ChunkMeta],
) -> Result<(Index, IndexReader, Field, Field, Field), String> {
    let mut schema_builder = Schema::builder();
    let path_field = schema_builder.add_text_field("path", STRING | STORED);
    let content_field = schema_builder.add_text_field("content", TEXT | STORED);
    let lines_field = schema_builder.add_text_field("lines", STRING | STORED);
    let schema = schema_builder.build();

    let index = Index::create_in_ram(schema);
    let mut writer = index
        .writer(50_000_000)
        .map_err(|e| format!("Writer error: {e}"))?;

    for chunk in chunks {
        writer
            .add_document(doc!(
                path_field => chunk.path.clone(),
                content_field => chunk.content.clone(),
                lines_field => format!("{}:{}", chunk.start_line, chunk.end_line),
            ))
            .map_err(|e| format!("Add doc error: {e}"))?;
    }

    writer.commit().map_err(|e| format!("Commit error: {e}"))?;

    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|e| format!("Reader error: {e}"))?;

    Ok((index, reader, path_field, content_field, lines_field))
}

/// A built index: BM25 and vectors over the same chunks, in their order.
struct CachedIndex {
    /// Fingerprint of the inventory it was built from.
    revision: u64,
    /// The slot's invalidation epoch when its build began.
    epoch: u64,
    // BM25
    bm25_index: Index,
    reader: IndexReader,
    path_field: Field,
    content_field: Field,
    lines_field: Field,
    // Vector (optional)
    vector_index: Option<VectorIndex>,
    chunks: Vec<ChunkMeta>, // chunk_id (vector key) → metadata
    /// Vectors by hash of the embedded text, reused by the next build.
    vectors: HashMap<u64, Vec<f32>>,
}

fn build_index(
    chunks: Vec<ChunkMeta>,
    embeddings: Option<Vec<Vec<f32>>>,
    vectors: HashMap<u64, Vec<f32>>,
    revision: u64,
    epoch: u64,
) -> Result<CachedIndex, String> {
    let file_count = chunks
        .iter()
        .map(|c| &c.path)
        .collect::<std::collections::HashSet<_>>()
        .len();
    tracing::info!(
        "CodeSearch: indexed {file_count} files, {} chunks",
        chunks.len()
    );

    let (bm25_index, reader, path_field, content_field, lines_field) = build_bm25_index(&chunks)?;

    let vector_index = match embeddings {
        Some(embs) if !embs.is_empty() && !embs[0].is_empty() => {
            match VectorIndex::from_vectors(&embs, Metric::Cosine) {
                Ok(idx) => Some(idx),
                Err(e) => {
                    tracing::warn!("Vector index failed, BM25 only: {e}");
                    None
                }
            }
        }
        _ => None,
    };

    Ok(CachedIndex {
        revision,
        epoch,
        bm25_index,
        reader,
        path_field,
        content_field,
        lines_field,
        vector_index,
        chunks,
        vectors,
    })
}

fn text_hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// The vectors of `texts`, in order, embedding only those not in
/// `known`. `None` (BM25 only) when the provider fails or answers with the
/// wrong number or size of vectors.
async fn embed_chunks(
    provider: &dyn EmbeddingProvider,
    texts: &[String],
    known: &HashMap<u64, Vec<f32>>,
) -> Option<(Vec<Vec<f32>>, HashMap<u64, Vec<f32>>)> {
    let hashes: Vec<u64> = texts.iter().map(|t| text_hash(t)).collect();
    let mut missing: Vec<String> = Vec::new();
    let mut missing_hashes: Vec<u64> = Vec::new();
    for (t, h) in texts.iter().zip(&hashes) {
        if !known.contains_key(h) && !missing_hashes.contains(h) {
            missing.push(t.clone());
            missing_hashes.push(*h);
        }
    }
    let mut map: HashMap<u64, Vec<f32>> = HashMap::new();
    if !missing.is_empty() {
        let embs = match provider.embed_batch(&missing).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Embedding failed, BM25 only: {e}");
                return None;
            }
        };
        let dims = provider.dimensions();
        if embs.len() != missing.len() || embs.iter().any(|v| v.len() != dims || v.is_empty()) {
            tracing::warn!(
                "Embedding answer does not match the request ({} vectors for {} texts, {} dimensions expected), BM25 only",
                embs.len(),
                missing.len(),
                dims
            );
            return None;
        }
        map.extend(missing_hashes.into_iter().zip(embs));
    }
    let mut out = Vec::with_capacity(texts.len());
    for h in &hashes {
        let v = map.get(h).or_else(|| known.get(h))?.clone();
        map.entry(*h).or_insert_with(|| v.clone());
        out.push(v);
    }
    Some((out, map))
}

// ─── Cache, sharing and invalidation ───────────────────────────────────────

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct IndexKey {
    /// Canonical root.
    root: PathBuf,
    /// Embedding configuration (`model_id`), empty for BM25 only.
    config: String,
}

#[derive(Clone)]
enum BuildError {
    /// The build stopped (its caller went away, or newer sources replaced
    /// it): try again.
    Abandoned,
    Failed(String),
}

type BuildOutcome = Result<Arc<CachedIndex>, BuildError>;

struct Pending {
    /// (revision, epoch) it builds.
    target: (u64, u64),
    id: u64,
    done: watch::Receiver<Option<BuildOutcome>>,
    abort: Arc<AtomicBool>,
}

#[derive(Default)]
struct Slot {
    ready: Option<Arc<CachedIndex>>,
    building: Option<Pending>,
    /// Incremented by every [`invalidate`] covering this root.
    epoch: u64,
    last_used: u64,
    /// Builds started (diagnostics).
    builds: u64,
    /// Files read by builds (diagnostics).
    reads: u64,
    /// Searches that waited for another one's build (diagnostics).
    joined: u64,
}

static SLOTS: Lazy<Mutex<HashMap<IndexKey, Slot>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static CLOCK: AtomicU64 = AtomicU64::new(1);
static BUILD_PERMITS: Lazy<tokio::sync::Semaphore> =
    Lazy::new(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_BUILDS));

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Files under `dir` may have changed: indexes of roots inside it, or
/// containing it, are no longer reused, and a build in progress for them is
/// not published.
pub fn invalidate(dir: &Path) {
    let dir = canonical(dir);
    let mut slots = SLOTS.lock().unwrap();
    for (key, slot) in slots.iter_mut() {
        if key.root.starts_with(&dir) || dir.starts_with(&key.root) {
            slot.epoch += 1;
        }
    }
}

/// Clears the build in progress when its future goes away unfinished: the
/// blocking work stops at its next check, waiters try again.
struct BuildGuard {
    key: IndexKey,
    id: u64,
    abort: Arc<AtomicBool>,
    tx: Option<watch::Sender<Option<BuildOutcome>>>,
}

impl BuildGuard {
    fn finish(&mut self, outcome: BuildOutcome) {
        let mut slots = SLOTS.lock().unwrap();
        if let Some(slot) = slots.get_mut(&self.key) {
            if slot.building.as_ref().is_some_and(|p| p.id == self.id) {
                slot.building = None;
            }
            // Published only if nothing invalidated or replaced it since
            // its build began.
            if let Ok(index) = &outcome
                && slot.epoch == index.epoch
                && !self.abort.load(Ordering::SeqCst)
            {
                slot.ready = Some(Arc::clone(index));
            }
        }
        drop(slots);
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Some(outcome));
        }
    }
}

impl Drop for BuildGuard {
    fn drop(&mut self) {
        if self.tx.is_some() {
            self.abort.store(true, Ordering::SeqCst);
            self.finish(Err(BuildError::Abandoned));
        }
    }
}

async fn run_build(
    mut guard: BuildGuard,
    root: PathBuf,
    inv: Inventory,
    epoch: u64,
    provider: Option<Arc<dyn EmbeddingProvider>>,
    previous: Option<Arc<CachedIndex>>,
) -> BuildOutcome {
    let abort = Arc::clone(&guard.abort);
    let _permit = BUILD_PERMITS.acquire().await.expect("never closed");
    if abort.load(Ordering::SeqCst) {
        guard.finish(Err(BuildError::Abandoned));
        return Err(BuildError::Abandoned);
    }
    let (r, a) = (root.clone(), Arc::clone(&abort));
    let files = inv.files;
    let collected = tokio::task::spawn_blocking(move || collect_chunks(&r, &files, &a))
        .await
        .map_err(|e| BuildError::Failed(format!("Index error: {e}")));
    let (chunks, read) = match collected {
        Ok(Some(c)) => c,
        Ok(None) => {
            guard.finish(Err(BuildError::Abandoned));
            return Err(BuildError::Abandoned);
        }
        Err(e) => {
            guard.finish(Err(e.clone()));
            return Err(e);
        }
    };
    if let Some(slot) = SLOTS.lock().unwrap().get_mut(&guard.key) {
        slot.reads += read as u64;
    }

    let (embeddings, vectors) = match &provider {
        Some(p) if !chunks.is_empty() && !abort.load(Ordering::SeqCst) => {
            let texts: Vec<String> = chunks
                .iter()
                .map(|c| c.content.chars().take(CHUNK_EMBED_CHARS).collect())
                .collect();
            let known = previous.as_ref().map(|i| &i.vectors);
            let empty = HashMap::new();
            match embed_chunks(p.as_ref(), &texts, known.unwrap_or(&empty)).await {
                Some((embs, map)) => (Some(embs), map),
                None => (None, HashMap::new()),
            }
        }
        _ => (None, HashMap::new()),
    };
    if abort.load(Ordering::SeqCst) {
        guard.finish(Err(BuildError::Abandoned));
        return Err(BuildError::Abandoned);
    }
    let revision = inv.fingerprint;
    let built = tokio::task::spawn_blocking(move || {
        build_index(chunks, embeddings, vectors, revision, epoch)
    })
    .await;
    let outcome = match built {
        Ok(Ok(index)) => Ok(Arc::new(index)),
        Ok(Err(e)) => Err(BuildError::Failed(format!("Index error: {e}"))),
        Err(e) => Err(BuildError::Failed(format!("Index error: {e}"))),
    };
    guard.finish(outcome.clone());
    outcome
}

/// The index of `root` for `provider`'s configuration, current for the
/// sources as inventoried now (see the module documentation).
async fn index_for(
    root: &Path,
    provider: Option<Arc<dyn EmbeddingProvider>>,
) -> Result<Arc<CachedIndex>, String> {
    let key = IndexKey {
        root: canonical(root),
        config: provider.as_ref().map(|p| p.model_id()).unwrap_or_default(),
    };
    for _ in 0..MAX_BUILD_ATTEMPTS {
        let walk_root = root.to_path_buf();
        let inv = tokio::task::spawn_blocking(move || inventory(&walk_root))
            .await
            .map_err(|e| format!("Index error: {e}"))??;
        enum Step {
            Use(Arc<CachedIndex>),
            Wait(watch::Receiver<Option<BuildOutcome>>),
            Build(BuildGuard, u64, Option<Arc<CachedIndex>>),
        }
        let step = {
            let mut slots = SLOTS.lock().unwrap();
            if !slots.contains_key(&key) && slots.len() >= MAX_CACHED_INDEXES {
                let oldest = slots
                    .iter()
                    .filter(|(_, s)| s.building.is_none())
                    .min_by_key(|(_, s)| s.last_used)
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest {
                    slots.remove(&k);
                }
            }
            let slot = slots.entry(key.clone()).or_default();
            slot.last_used = CLOCK.fetch_add(1, Ordering::SeqCst);
            let target = (inv.fingerprint, slot.epoch);
            if let Some(r) = slot
                .ready
                .as_ref()
                .filter(|r| (r.revision, r.epoch) == target)
            {
                Step::Use(Arc::clone(r))
            } else if let Some(p) = slot.building.as_ref().filter(|p| p.target == target) {
                slot.joined += 1;
                Step::Wait(p.done.clone())
            } else {
                // A build of older sources is replaced.
                if let Some(old) = slot.building.take() {
                    old.abort.store(true, Ordering::SeqCst);
                }
                let (tx, rx) = watch::channel(None);
                let abort = Arc::new(AtomicBool::new(false));
                let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
                slot.building = Some(Pending {
                    target,
                    id,
                    done: rx,
                    abort: Arc::clone(&abort),
                });
                slot.builds += 1;
                Step::Build(
                    BuildGuard {
                        key: key.clone(),
                        id,
                        abort,
                        tx: Some(tx),
                    },
                    slot.epoch,
                    slot.ready.clone(),
                )
            }
        };
        let outcome = match step {
            Step::Use(index) => return Ok(index),
            Step::Wait(mut rx) => loop {
                let current = rx.borrow().clone();
                if let Some(o) = current {
                    break o;
                }
                if rx.changed().await.is_err() {
                    break Err(BuildError::Abandoned);
                }
            },
            Step::Build(guard, epoch, previous) => {
                run_build(
                    guard,
                    root.to_path_buf(),
                    inv,
                    epoch,
                    provider.clone(),
                    previous,
                )
                .await
            }
        };
        match outcome {
            Ok(index) => return Ok(index),
            Err(BuildError::Failed(e)) => return Err(e),
            Err(BuildError::Abandoned) => continue,
        }
    }
    Err("Index error: the sources kept changing during the build; try again".into())
}

// ─── Search ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct SearchResult {
    path: String,
    content: String,
    start_line: usize,
    end_line: usize,
    bm25_score: f32,
    vector_score: f32,
    final_score: f32,
}

fn bm25_search(
    cached: &CachedIndex,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    let searcher = cached.reader.searcher();
    let qp = QueryParser::for_index(&cached.bm25_index, vec![cached.content_field]);
    let parsed = qp
        .parse_query(query)
        .map_err(|e| format!("Query parse: {e}"))?;
    let top = searcher
        .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
        .map_err(|e| format!("Search: {e}"))?;

    let mut results = Vec::new();
    for (score, addr) in top {
        let doc: TantivyDocument = searcher.doc(addr).map_err(|e| format!("Doc: {e}"))?;
        let path = doc
            .get_first(cached.path_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let content = doc
            .get_first(cached.content_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let lines = doc
            .get_first(cached.lines_field)
            .and_then(|v| v.as_str())
            .unwrap_or("0:0")
            .to_string();
        let (start, end) = lines
            .split_once(':')
            .map(|(s, e)| (s.parse().unwrap_or(0), e.parse().unwrap_or(0)))
            .unwrap_or((0, 0));
        results.push(SearchResult {
            path,
            content,
            start_line: start,
            end_line: end,
            bm25_score: score,
            vector_score: 0.0,
            final_score: score,
        });
    }
    Ok(results)
}

fn vector_search(
    cached: &CachedIndex,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    let vi = cached.vector_index.as_ref().ok_or("No vector index")?;
    let hits = vi
        .search(query_embedding, limit)
        .map_err(|e| format!("Vector search: {e}"))?;

    let mut results = Vec::new();
    for hit in hits {
        let key = hit.key as usize;
        if key < cached.chunks.len() {
            let chunk = &cached.chunks[key];
            results.push(SearchResult {
                path: chunk.path.clone(),
                content: chunk.content.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                bm25_score: 0.0,
                vector_score: hit.similarity,
                final_score: hit.similarity * 100.0,
            });
        }
    }
    Ok(results)
}

fn merge_results(
    bm25: Vec<SearchResult>,
    vector: Vec<SearchResult>,
    limit: usize,
) -> Vec<SearchResult> {
    let mut merged: HashMap<String, SearchResult> = HashMap::new();

    // Normalize BM25 scores to 0-1 range
    let max_bm25 = bm25
        .iter()
        .map(|r| r.bm25_score)
        .fold(0.0f32, f32::max)
        .max(1.0);

    for mut r in bm25 {
        let key = format!("{}:{}:{}", r.path, r.start_line, r.end_line);
        r.bm25_score /= max_bm25; // normalize to 0-1
        merged.insert(key, r);
    }

    for r in vector {
        let key = format!("{}:{}:{}", r.path, r.start_line, r.end_line);
        if let Some(existing) = merged.get_mut(&key) {
            existing.vector_score = r.vector_score;
        } else {
            merged.insert(key, r);
        }
    }

    // Blend: 60% BM25 + 40% vector
    let mut results: Vec<SearchResult> = merged
        .into_values()
        .map(|mut r| {
            r.final_score = r.bm25_score * 0.6 + r.vector_score * 0.4;
            r
        })
        .collect();

    results.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    results
}

// ─── Tool implementation ───────────────────────────────────────────────────

pub struct CodeSearchTool {
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
}

impl CodeSearchTool {
    /// BM25-only search. No network, no API key required.
    pub fn new() -> Self {
        Self {
            embedding_provider: None,
        }
    }

    /// Enable hybrid BM25 + vector search using the given embedding provider.
    ///
    /// Use [`cersei_embeddings::auto_from_model`] to construct a provider
    /// from an LLM model string, or build one explicitly with
    /// [`cersei_embeddings::GeminiEmbeddings`] / [`cersei_embeddings::OpenAiEmbeddings`].
    pub fn with_embeddings(provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            embedding_provider: Some(provider),
        }
    }
}

impl Default for CodeSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CodeSearchTool {
    fn name(&self) -> &str {
        "CodeSearch"
    }

    fn description(&self) -> &str {
        "Semantic code search across the codebase. Use natural language queries about behavior, \
         patterns, or concepts. Returns relevant code snippets with file paths and line numbers. \
         This is your DEFAULT tool for discovering code — use it before Grep when you need to \
         understand how something works rather than find an exact string."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::FileSystem
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language search query about code behavior, patterns, or concepts."
                },
                "path": { "type": "string", "description": "Directory to search in." },
                "limit": { "type": "integer", "description": "Max results (default: 10)." }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            query: String,
            path: Option<String>,
            limit: Option<usize>,
        }

        let input: Input = match crate::tool_feedback::parse_input(self, &input) {
            Ok(i) => i,
            Err(e) => return e,
        };

        // A relative path is relative to the working directory.
        let search_dir = input
            .path
            .map(|p| ctx.working_dir.join(p))
            .unwrap_or_else(|| ctx.working_dir.clone());
        let limit = input.limit.unwrap_or(DEFAULT_RESULTS);

        let cached = match index_for(&search_dir, self.embedding_provider.clone()).await {
            Ok(c) => c,
            Err(e) => return ToolResult::error(e),
        };
        let bm25_results = match bm25_search(&cached, &input.query, BM25_CANDIDATES) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("BM25 error: {e}")),
        };

        let results = match (&cached.vector_index, &self.embedding_provider) {
            (Some(_), Some(provider)) => match provider.embed(&input.query).await {
                Ok(query_emb) => {
                    let vec_results =
                        vector_search(&cached, &query_emb, VECTOR_CANDIDATES).unwrap_or_default();
                    merge_results(bm25_results, vec_results, limit)
                }
                Err(e) => {
                    tracing::warn!("Query embedding failed, BM25 only: {e}");
                    let mut r = bm25_results;
                    r.truncate(limit);
                    r
                }
            },
            _ => {
                let mut r = bm25_results;
                r.truncate(limit);
                r
            }
        };

        if results.is_empty() {
            return ToolResult::success(
                "No results found. Try different search terms or use Grep for exact patterns.",
            );
        }

        let mut output = String::new();
        for (i, r) in results.iter().enumerate() {
            output.push_str(&format!(
                "── Result {} ── {}:{}-{} (score: {:.2})\n{}\n\n",
                i + 1,
                search_dir.join(&r.path).display(),
                r.start_line,
                r.end_line,
                r.final_score,
                r.content
            ));
        }
        ToolResult::success(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::AllowAll;
    use crate::{CostTracker, Extensions};
    use std::sync::atomic::AtomicUsize;

    type Hook = Arc<dyn Fn(bool) + Send + Sync>;

    /// Per-root test hooks, called before each file a build reads
    /// (`false`) and when a build stops on its abort flag (`true`).
    static HOOKS: Lazy<Mutex<HashMap<PathBuf, Hook>>> = Lazy::new(|| Mutex::new(HashMap::new()));

    pub(super) fn hook(root: &Path, aborted: bool) {
        let h = HOOKS.lock().unwrap().get(root).cloned();
        if let Some(h) = h {
            h(aborted);
        }
    }

    fn set_hook(root: &Path, h: impl Fn(bool) + Send + Sync + 'static) {
        HOOKS
            .lock()
            .unwrap()
            .insert(root.to_path_buf(), Arc::new(h));
    }

    /// (builds, files read, searches that joined a build, published?)
    fn stats(root: &Path, config: &str) -> (u64, u64, u64, bool) {
        let key = IndexKey {
            root: canonical(root),
            config: config.into(),
        };
        let slots = SLOTS.lock().unwrap();
        slots
            .get(&key)
            .map(|s| (s.builds, s.reads, s.joined, s.ready.is_some()))
            .unwrap_or_default()
    }

    async fn search(dir: &Path, query: &str) -> String {
        let out = CodeSearchTool::new()
            .execute(serde_json::json!({ "query": query }), &ctx(dir))
            .await;
        assert!(!out.is_error, "{}", out.content);
        out.content
    }

    /// Counts the texts it embeds; fails or answers short when asked.
    struct Counting {
        inner: cersei_embeddings::HashingEmbeddings,
        embedded: AtomicUsize,
        fail: bool,
        short: bool,
    }

    impl Counting {
        fn new() -> Self {
            Self {
                inner: cersei_embeddings::HashingEmbeddings::new(64),
                embedded: AtomicUsize::new(0),
                fail: false,
                short: false,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for Counting {
        fn name(&self) -> &str {
            "counting"
        }
        fn dimensions(&self) -> usize {
            self.inner.dimensions()
        }
        async fn embed_batch(
            &self,
            texts: &[String],
        ) -> Result<Vec<Vec<f32>>, cersei_embeddings::EmbeddingError> {
            if self.fail {
                return Err(cersei_embeddings::EmbeddingError::Api("down".into()));
            }
            self.embedded.fetch_add(texts.len(), Ordering::SeqCst);
            let mut v = self.inner.embed_batch(texts).await?;
            if self.short {
                v.pop();
            }
            Ok(v)
        }
    }

    fn corpus(files: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..files {
            std::fs::write(
                dir.path().join(format!("f{i}.rs")),
                format!("fn function_number_{i}() {{ /* marker{i} */ }}\n"),
            )
            .unwrap();
        }
        dir
    }

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "code-search-test".into(),
            permissions: Arc::new(AllowAll),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    /// `path:start-end` of each `── Result n ── path:start-end (score: …)`
    /// header, the path relative to `root` (results name absolute paths).
    fn headers(out: &str, root: &Path) -> Vec<String> {
        let root = format!("{}/", root.display());
        out.lines()
            .filter(|l| l.starts_with("── Result"))
            .map(|l| {
                let h = l
                    .split(" ── ")
                    .nth(1)
                    .unwrap()
                    .split(" (score")
                    .next()
                    .unwrap();
                h.strip_prefix(&root).unwrap_or(h).to_string()
            })
            .collect()
    }

    /// BM25 paths and line ranges, the result limit, a query that does not
    /// parse, no match, and the hybrid mode — on the in-memory Tantivy
    /// index.
    #[tokio::test]
    async fn bm25_and_hybrid_search_keep_paths_lines_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        let mut long = String::new();
        for i in 1..=120 {
            if i == 95 {
                long.push_str("fn compute_invoice_total() { /* zebrafinch */ }\n");
            } else {
                long.push_str(&format!("let filler_{i} = {i};\n"));
            }
        }
        std::fs::write(src.join("billing.rs"), &long).unwrap();
        std::fs::write(src.join("notes.md"), "The zebrafinch sings at dawn.\n").unwrap();
        std::fs::write(src.join("image.bin"), "zebrafinch").unwrap();
        let c = ctx(dir.path());

        let tool = CodeSearchTool::new();
        let out = tool
            .execute(serde_json::json!({ "query": "zebrafinch" }), &c)
            .await;
        assert!(!out.is_error, "{}", out.content);
        let h = headers(&out.content, dir.path());
        // Chunks of 50 lines with a 10-line overlap: line 95 is in 81-120.
        assert!(h.iter().any(|x| x == "src/billing.rs:81-120"), "{h:?}");
        assert!(h.iter().any(|x| x == "src/notes.md:1-1"), "{h:?}");
        assert!(
            !h.iter().any(|x| x.contains("image.bin")),
            "not an indexed extension"
        );

        let out = tool
            .execute(serde_json::json!({ "query": "zebrafinch", "limit": 1 }), &c)
            .await;
        assert_eq!(
            headers(&out.content, dir.path()).len(),
            1,
            "{}",
            out.content
        );

        let out = tool
            .execute(serde_json::json!({ "query": "nosuchwordanywhere" }), &c)
            .await;
        assert!(!out.is_error && out.content.starts_with("No results found"));

        let out = tool
            .execute(serde_json::json!({ "query": "content:(" }), &c)
            .await;
        assert!(
            out.is_error && out.content.contains("Query parse"),
            "{}",
            out.content
        );

        // Hybrid: BM25 candidates merged with vector neighbours.
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("a.rs"), "fn zebrafinch_parser() {}\n").unwrap();
        std::fs::write(other.path().join("b.rs"), "fn unrelated() {}\n").unwrap();
        let hybrid = CodeSearchTool::with_embeddings(Arc::new(
            cersei_embeddings::HashingEmbeddings::new(64),
        ));
        let out = hybrid
            .execute(
                serde_json::json!({ "query": "zebrafinch_parser" }),
                &ctx(other.path()),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        let h = headers(&out.content, other.path());
        assert_eq!(h.first().map(String::as_str), Some("a.rs:1-1"), "{h:?}");
    }

    #[tokio::test]
    async fn one_collection_per_build_and_warm_reuse() {
        let dir = corpus(5);
        let out = search(dir.path(), "marker3").await;
        assert!(out.contains("f3.rs"), "{out}");
        assert_eq!(stats(dir.path(), ""), (1, 5, 0, true));
        // Warm: the inventory matches, nothing is read or built again.
        for _ in 0..3 {
            search(dir.path(), "marker1").await;
        }
        assert_eq!(stats(dir.path(), ""), (1, 5, 0, true));
    }

    #[tokio::test]
    async fn edits_additions_removals_renames_and_ignore_rules_are_seen() {
        let dir = corpus(3);
        let p = dir.path();
        assert!(search(p, "marker0").await.contains("f0.rs"));
        // External edit (another size).
        std::fs::write(p.join("f0.rs"), "fn renamed_thing() { /* kiwifruit */ }\n").unwrap();
        let out = search(p, "kiwifruit").await;
        assert!(out.contains("f0.rs"), "{out}");
        assert!(search(p, "marker0").await.starts_with("No results"));
        // Addition.
        std::fs::write(p.join("new.rs"), "fn added() { /* papaya */ }\n").unwrap();
        assert!(search(p, "papaya").await.contains("new.rs"));
        // Removal.
        std::fs::remove_file(p.join("f1.rs")).unwrap();
        assert!(search(p, "marker1").await.starts_with("No results"));
        // Rename.
        std::fs::rename(p.join("f2.rs"), p.join("moved.rs")).unwrap();
        let out = search(p, "marker2").await;
        assert!(out.contains("moved.rs") && !out.contains("f2.rs"), "{out}");
        // A new ignore rule (no `.git` needed), then its removal.
        std::fs::write(p.join(".gitignore"), "moved.rs\n").unwrap();
        assert!(search(p, "marker2").await.starts_with("No results"));
        std::fs::remove_file(p.join(".gitignore")).unwrap();
        assert!(search(p, "marker2").await.contains("moved.rs"));
        // Re-inclusion through `!`.
        std::fs::write(p.join(".ignore"), "*.rs\n!new.rs\n").unwrap();
        let out = search(p, "papaya OR marker2").await;
        assert!(out.contains("new.rs") && !out.contains("moved.rs"), "{out}");
    }

    #[tokio::test]
    async fn an_edit_keeping_size_and_time_needs_the_notification() {
        let dir = corpus(1);
        let p = dir.path();
        let file = p.join("f0.rs");
        assert!(search(p, "marker0").await.contains("f0.rs"));
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        // Same length, same modification time: invisible to the inventory.
        std::fs::write(&file, "fn function_number_0() { /* zzzzzzz */ }\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(before)
            .unwrap();
        assert!(
            search(p, "zzzzzzz").await.starts_with("No results"),
            "the documented limit of the inventory"
        );
        // A Bricks write notifies (the runner, after a call that ran a
        // writing tool): the index is not reused.
        crate::code_scout::notify_workspace_changed(&ctx(p));
        assert!(search(p, "zzzzzzz").await.contains("f0.rs"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_searches_share_one_build() {
        let dir = corpus(4);
        let p = dir.path().to_path_buf();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate = Mutex::new(Some(gate_rx));
        set_hook(&p, move |_| {
            if let Some(rx) = gate.lock().unwrap().take() {
                let _ = rx.recv();
            }
        });
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let p = p.clone();
                tokio::spawn(async move { search(&p, "marker2").await })
            })
            .collect();
        // Wait until the three others joined the first one's build.
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while stats(&p, "").2 < 3 {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("the searches join the build");
        gate_tx.send(()).unwrap();
        for t in tasks {
            assert!(t.await.unwrap().contains("f2.rs"));
        }
        let (builds, reads, joined, ready) = stats(&p, "");
        assert_eq!((builds, reads, joined, ready), (1, 4, 3, true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_invalidation_during_a_build_prevents_its_publication() {
        let dir = corpus(2);
        let p = dir.path().to_path_buf();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let state = Mutex::new(Some((entered_tx, gate_rx)));
        set_hook(&p, move |_| {
            if let Some((entered, gate)) = state.lock().unwrap().take() {
                let _ = entered.send(());
                let _ = gate.recv();
            }
        });
        let q = p.clone();
        let task = tokio::spawn(async move { search(&q, "marker1").await });
        tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
            .await
            .unwrap();
        invalidate(&p);
        gate_tx.send(()).unwrap();
        // The caller still gets its answer (it began before the change)…
        assert!(task.await.unwrap().contains("f1.rs"));
        // …but the index is not kept as current.
        assert_eq!(stats(&p, ""), (1, 2, 0, false));
        search(&p, "marker1").await;
        assert_eq!(stats(&p, "").0, 2);
    }

    #[tokio::test]
    async fn workspaces_and_configurations_do_not_mix() {
        let a = corpus(2);
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("other.rs"), "fn other() { /* marker0 */ }\n").unwrap();
        let out_a = search(a.path(), "marker0").await;
        let out_b = search(b.path(), "marker0").await;
        assert!(
            out_a.contains("f0.rs") && !out_a.contains("other.rs"),
            "{out_a}"
        );
        assert!(
            out_b.contains("other.rs") && !out_b.contains("f0.rs"),
            "{out_b}"
        );
        // The same root with embeddings is another index.
        let hybrid = CodeSearchTool::with_embeddings(Arc::new(Counting::new()));
        let out = hybrid
            .execute(serde_json::json!({ "query": "marker0" }), &ctx(a.path()))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(stats(a.path(), "").0, 1);
        assert_eq!(stats(a.path(), "counting:64").0, 1);
        // Two spellings of one root share its index.
        let dotted = a.path().join(".");
        search(&dotted, "marker1").await;
        assert_eq!(stats(a.path(), "").0, 1);
    }

    #[tokio::test]
    async fn vectors_match_their_chunks_and_unchanged_chunks_are_not_embedded_again() {
        let dir = corpus(6);
        let p = dir.path();
        let provider = Arc::new(Counting::new());
        let first = index_for(p, Some(provider.clone())).await.unwrap();
        assert_eq!(provider.embedded.load(Ordering::SeqCst), 6);
        let vi = first.vector_index.as_ref().expect("vectors");
        assert_eq!(vi.len(), first.chunks.len());
        for chunk in &first.chunks {
            let text: String = chunk.content.chars().take(CHUNK_EMBED_CHARS).collect();
            let q = provider.inner.embed_sync(&text);
            let hits = vector_search(&first, &q, 1).unwrap();
            assert_eq!(hits[0].content, chunk.content, "key → its own chunk");
        }
        // One file changes: only its chunk is embedded again.
        std::fs::write(p.join("f4.rs"), "fn changed_one() { /* lychee */ }\n").unwrap();
        let second = index_for(p, Some(provider.clone())).await.unwrap();
        assert_eq!(provider.embedded.load(Ordering::SeqCst), 7);
        assert_eq!(
            second.vector_index.as_ref().unwrap().len(),
            second.chunks.len()
        );
    }

    #[tokio::test]
    async fn an_embedding_failure_or_a_short_answer_keeps_bm25() {
        for (fail, short) in [(true, false), (false, true)] {
            let dir = corpus(3);
            let mut provider = Counting::new();
            provider.fail = fail;
            provider.short = short;
            let tool = CodeSearchTool::with_embeddings(Arc::new(provider));
            let out = tool
                .execute(serde_json::json!({ "query": "marker2" }), &ctx(dir.path()))
                .await;
            assert!(!out.is_error, "{}", out.content);
            assert!(out.content.contains("f2.rs"), "{}", out.content);
            let index = index_for(dir.path(), None).await.unwrap();
            assert!(index.vector_index.is_none());
        }
    }

    /// On a single-threaded runtime the build runs on the blocking pool: the
    /// runtime keeps serving other tasks, and a cancelled search stops its
    /// build at the next file without publishing it.
    #[test]
    fn a_single_thread_runtime_stays_responsive_and_cancellation_publishes_nothing() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = corpus(5);
            let p = dir.path().to_path_buf();
            let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
            let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
            let (stopped_tx, stopped_rx) = std::sync::mpsc::channel::<()>();
            let state = Mutex::new((Some((entered_tx, gate_rx)), Some(stopped_tx)));
            set_hook(&p, move |aborted| {
                let mut s = state.lock().unwrap();
                if aborted {
                    if let Some(tx) = s.1.take() {
                        let _ = tx.send(());
                    }
                } else if let Some((entered, gate)) = s.0.take() {
                    drop(s);
                    let _ = entered.send(());
                    let _ = gate.recv();
                }
            });
            let q = p.clone();
            let task = tokio::spawn(async move { search(&q, "marker1").await });
            // The build is blocked on its first file…
            tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
                .await
                .unwrap();
            // …and the runtime still handles an event.
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move { tx.send("event").unwrap() });
            assert_eq!(rx.await.unwrap(), "event");
            // Cancel the search, then let the build go on: it stops.
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            gate_tx.send(()).unwrap();
            tokio::task::spawn_blocking(move || {
                stopped_rx
                    .recv_timeout(std::time::Duration::from_secs(20))
                    .expect("the build stopped on its abort flag")
            })
            .await
            .unwrap();
            let (builds, reads, _, ready) = stats(&p, "");
            assert_eq!(builds, 1);
            assert!(!ready, "nothing published");
            assert_eq!(reads, 0, "stopped before finishing");
            // The next search builds normally.
            assert!(search(&p, "marker1").await.contains("f1.rs"));
            assert!(stats(&p, "").3);
        });
    }
}
