//! cersei-web: search the web, read pages within bounds, and select citable
//! passages.
//!
//! The pipeline of a search ([`WebContext::research`]):
//!
//! 1. **search** — the configured provider, with a visible fallback cascade
//!    ([`search`]);
//! 2. **download** — the first pages, concurrently, bounded globally and per
//!    host, in time and in decoded bytes, under the network policy
//!    ([`fetch`], [`policy`]);
//! 3. **extract** — HTML/Markdown/text/JSON to Markdown on bounded blocking
//!    threads ([`extract`]); each document is kept in the session store
//!    ([`store`]);
//! 4. **split and rank** — structural passages, BM25 over the passages of
//!    this search only, selection under a character budget ([`chunk`],
//!    [`rank`]).
//!
//! Each stage is timed separately. Selecting passages loses information by
//! design: the full extracted documents stay readable (paged, in document
//! order) through [`WebContext::read`] and the store's files. Pages are
//! external, untrusted content: nothing here interprets their text as
//! instructions.

pub mod chunk;
pub mod config;
pub mod extract;
pub mod fetch;
pub mod policy;
pub mod rank;
pub mod search;
pub mod store;

pub use config::{LoadedWebConfig, ProviderKind, WebConfig};

use chunk::Passage;
use extract::{Extractor, Unreadable};
use fetch::{FetchError, Fetcher};
use rank::Selection;
use search::{SearchOutcome, Searcher};
use std::sync::Arc;
use std::time::Instant;
use store::{DocEntry, NewDoc, WebStore};

/// Everything a web call needs, shared across calls of a session.
pub struct WebContext {
    pub config: WebConfig,
    pub searcher: Searcher,
    pub fetcher: Arc<Fetcher>,
    pub extractor: Extractor,
    pub store: Option<Arc<WebStore>>,
}

impl WebContext {
    pub fn new(config: WebConfig, store: Option<Arc<WebStore>>) -> Result<Self, String> {
        config.validate()?;
        Ok(Self {
            searcher: Searcher::new(config.search.clone(), &config.fetch.user_agent)?,
            fetcher: Arc::new(Fetcher::new(config.fetch.clone())?),
            extractor: Extractor::new(config.extract.clone()),
            store,
            config,
        })
    }
}

/// Why a page gave no document.
#[derive(Debug, Clone, PartialEq, thiserror::Error, serde::Serialize)]
#[serde(tag = "stage", content = "detail", rename_all = "snake_case")]
pub enum PageError {
    #[error("download failed: {0}")]
    Fetch(#[serde(serialize_with = "display")] FetchError),
    #[error("{0}")]
    Unreadable(#[serde(serialize_with = "display")] Unreadable),
    #[error("could not be stored: {0}")]
    Store(String),
}

fn display<T: std::fmt::Display, S: serde::Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

/// A page read for a search.
#[derive(Debug, Clone)]
pub struct PageDoc {
    pub entry: DocEntry,
    pub markdown: String,
    pub passages: Vec<Passage>,
    /// Served from the session store, not downloaded again.
    pub cached: bool,
}

#[derive(Debug, Clone)]
pub struct PageResult {
    /// The search result it comes from (`S3`).
    pub source_id: String,
    pub url: String,
    pub outcome: Result<PageDoc, PageError>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Timings {
    pub search_ms: u64,
    pub fetch_ms: u64,
    pub extract_ms: u64,
    pub rank_ms: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Research {
    pub search: SearchOutcome,
    pub pages: Vec<PageResult>,
    /// `None` when no page was read.
    pub selection: Option<Selection>,
    pub timings: Timings,
}

/// What [`WebContext::research`] does.
#[derive(Debug, Clone, Copy)]
pub struct ResearchOptions {
    pub results: usize,
    /// Pages to read among the results (0: search only).
    pub read_pages: usize,
}

impl WebContext {
    /// Search, read the first pages and select passages for `query`.
    pub async fn research(&self, query: &str, opts: ResearchOptions) -> Research {
        let t0 = Instant::now();
        let search = self.searcher.search(query, opts.results).await;
        let search_ms = t0.elapsed().as_millis() as u64;
        let targets: Vec<(String, String)> = search
            .hits
            .iter()
            .take(opts.read_pages.min(self.config.fetch.max_pages))
            .map(|h| (h.source_id.clone(), h.url.clone()))
            .collect();
        let (pages, fetch_ms, extract_ms) = self.read_many(&targets).await;
        let t3 = Instant::now();
        let docs: Vec<Vec<Passage>> = pages
            .iter()
            .map(|p| {
                p.outcome
                    .as_ref()
                    .map(|d| d.passages.clone())
                    .unwrap_or_default()
            })
            .collect();
        let selection = (!targets.is_empty() && docs.iter().any(|d| !d.is_empty()))
            .then(|| rank::select(query, &docs, &self.config.passages));
        let rank_ms = t3.elapsed().as_millis() as u64;
        Research {
            search,
            pages,
            selection,
            timings: Timings {
                search_ms,
                fetch_ms,
                extract_ms,
                rank_ms,
                total_ms: t0.elapsed().as_millis() as u64,
            },
        }
    }

    /// Download (or take from the store) and extract several pages; results
    /// in the order given. Returns the network and extraction times.
    async fn read_many(&self, targets: &[(String, String)]) -> (Vec<PageResult>, u64, u64) {
        let mut cached: Vec<Option<PageDoc>> = Vec::new();
        let mut to_fetch = Vec::new();
        for (_, url) in targets {
            match self.stored_url(url) {
                Some(doc) => cached.push(Some(doc)),
                None => {
                    cached.push(None);
                    to_fetch.push(url.clone());
                }
            }
        }
        let t = Instant::now();
        let fetched = self.fetcher.fetch_many(&to_fetch).await;
        let fetch_ms = t.elapsed().as_millis() as u64;
        let t = Instant::now();
        let extractions = futures::future::join_all(fetched.into_iter().map(|r| async move {
            match r {
                Err(e) => Err(PageError::Fetch(e)),
                Ok(page) => self.extract_page(&page).await.map(|x| (page, x)),
            }
        }))
        .await;
        // Stored in the order of the results, whatever finished first, so
        // document ids follow the sources.
        let extractions: Vec<Result<PageDoc, PageError>> = extractions
            .into_iter()
            .map(|r| r.and_then(|(page, x)| self.store_page(&page, x)))
            .collect();
        let extract_ms = t.elapsed().as_millis() as u64;
        let mut fresh = extractions.into_iter();
        let pages = targets
            .iter()
            .zip(cached)
            .map(|((sid, url), c)| PageResult {
                source_id: sid.clone(),
                url: url.clone(),
                outcome: match c {
                    Some(doc) => Ok(doc),
                    None => fresh
                        .next()
                        .unwrap_or(Err(PageError::Store("missing".into()))),
                },
            })
            .collect();
        (pages, fetch_ms, extract_ms)
    }

    fn stored_url(&self, url: &str) -> Option<PageDoc> {
        let store = self.store.as_ref()?;
        let entry = store.find_url(url)?;
        let markdown = store.markdown(&entry).ok()?;
        let p = &self.config.passages;
        Some(PageDoc {
            passages: chunk::split(&markdown, p.min_chars, p.max_chars),
            entry,
            markdown,
            cached: true,
        })
    }

    async fn extract_page(
        &self,
        page: &fetch::FetchedPage,
    ) -> Result<extract::Extracted, PageError> {
        self.extractor
            .extract(
                page.final_url.clone(),
                page.content_type.clone(),
                page.body.clone(),
                page.truncated,
                page.declared_length,
            )
            .await
            .map_err(PageError::Unreadable)
    }

    fn store_page(
        &self,
        page: &fetch::FetchedPage,
        extracted: extract::Extracted,
    ) -> Result<PageDoc, PageError> {
        let mut notes = extracted.notes.clone();
        if let Some(i) = &page.interrupted {
            notes.push(format!("the transfer broke off: {i}"));
        }
        let strategy = extracted.strategy.to_string();
        let entry = match &self.store {
            Some(store) => store
                .put(NewDoc {
                    requested_url: page.requested.as_str(),
                    final_url: page.final_url.as_str(),
                    title: &extracted.title,
                    content_type: page.content_type.as_deref(),
                    raw: &page.body,
                    truncated: page.truncated,
                    markdown: &extracted.markdown,
                    strategy: &strategy,
                    notes: &notes,
                })
                .map_err(|e| PageError::Store(e.to_string()))?,
            None => DocEntry {
                id: "-".into(),
                requested_url: page.requested.to_string(),
                final_url: page.final_url.to_string(),
                title: extracted.title.clone(),
                content_type: page.content_type.clone(),
                fetched_at: 0,
                raw_file: String::new(),
                raw_bytes: page.body.len() as u64,
                truncated: page.truncated,
                markdown_file: String::new(),
                markdown_chars: extracted.markdown.chars().count(),
                strategy,
                notes,
            },
        };
        let p = &self.config.passages;
        Ok(PageDoc {
            passages: chunk::split(&extracted.markdown, p.min_chars, p.max_chars),
            entry,
            markdown: extracted.markdown,
            cached: false,
        })
    }

    /// One page as a document: from the store unless `refresh`, else
    /// downloaded and extracted (and stored).
    pub async fn read(&self, url: &str, refresh: bool) -> Result<PageDoc, PageError> {
        if !refresh {
            if let Some(doc) = self.stored_url(url) {
                return Ok(doc);
            }
        }
        let page = self.fetcher.fetch(url).await.map_err(PageError::Fetch)?;
        let extracted = self.extract_page(&page).await?;
        self.store_page(&page, extracted)
    }

    /// A stored document by id (`D3`).
    pub fn stored(&self, id: &str) -> Option<PageDoc> {
        let store = self.store.as_ref()?;
        let entry = store.get(id)?;
        let markdown = store.markdown(&entry).ok()?;
        let p = &self.config.passages;
        Some(PageDoc {
            passages: chunk::split(&markdown, p.min_chars, p.max_chars),
            entry,
            markdown,
            cached: true,
        })
    }
}

/// The largest char boundary `<= i` (stable-Rust friendly).
pub fn floor_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Byte offset of character `n` (or the end).
pub fn char_to_byte(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(b, _)| b).unwrap_or(s.len())
}
