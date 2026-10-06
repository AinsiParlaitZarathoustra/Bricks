//! Bounded, concurrent page downloads.
//!
//! One shared `reqwest::Client` (connection pool; HTTP/2 when the server
//! negotiates it) with no automatic redirects, no proxy and a DNS resolver
//! that applies the [`NetworkPolicy`]. Each page:
//!
//! * waits for a global slot and a slot for its host (both bounded);
//! * then has `page_timeout` for connection, every redirect and the body;
//! * follows at most `max_redirects` redirects, each target checked again;
//! * is read as a stream of *decoded* bytes (after gzip/brotli/deflate) and
//!   cut at `max_page_bytes` whatever `Content-Length` says or omits; the
//!   page is then marked `truncated`, never presented as complete;
//! * draws from a byte budget shared by all pages of the call.
//!
//! [`Fetcher::fetch_many`] returns one result per URL in the order given,
//! whatever the order of completion. Dropping its future aborts the
//! downloads still running.

use crate::config::FetchConfig;
use crate::policy::{FilteringResolver, NetworkPolicy, PolicyError};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use url::Url;

/// A downloaded page (or the part of it that fit).
#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub requested: Url,
    pub final_url: Url,
    pub status: u16,
    pub content_type: Option<String>,
    /// `Content-Length` as announced (may be absent or wrong).
    pub declared_length: Option<u64>,
    /// `Content-Encoding` left in place: an encoding the client does not
    /// decode (gzip, brotli and deflate are decoded and leave no header), so
    /// `body` is still encoded.
    pub content_encoding: Option<String>,
    /// Decoded bytes, at most `max_page_bytes`.
    pub body: Vec<u8>,
    /// The page was longer than what was kept (size limit or byte budget).
    pub truncated: bool,
    /// The transfer broke off after `body` was received.
    pub interrupted: Option<String>,
    pub redirects: usize,
    /// From the start of the request to the end of the body.
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("refused by the network policy: {0}")]
    Policy(#[from] PolicyError),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("timed out after {0:?} (connection, redirects and body)")]
    Timeout(Duration),
    #[error("more than {0} redirects")]
    TooManyRedirects(usize),
    #[error("redirect without a valid Location header")]
    BadRedirect,
    #[error("HTTP {status}")]
    Http { status: u16 },
    #[error("network error: {0}")]
    Network(String),
    #[error("not started: the download budget of this call was exhausted")]
    Budget,
}

/// Downloads pages under a [`FetchConfig`].
pub struct Fetcher {
    client: reqwest::Client,
    policy: Arc<NetworkPolicy>,
    cfg: FetchConfig,
    global: Arc<Semaphore>,
    hosts: parking_lot::Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl Fetcher {
    pub fn new(cfg: FetchConfig) -> Result<Self, String> {
        let policy = Arc::new(NetworkPolicy::new(&cfg.allow_private));
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .dns_resolver(Arc::new(FilteringResolver::new(policy.clone())))
            .user_agent(cfg.user_agent.clone())
            .connect_timeout(cfg.page_timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .build()
            .map_err(|e| format!("cannot build the page client: {e}"))?;
        Ok(Self {
            client,
            global: Arc::new(Semaphore::new(cfg.concurrency)),
            hosts: parking_lot::Mutex::new(HashMap::new()),
            policy,
            cfg,
        })
    }

    pub fn config(&self) -> &FetchConfig {
        &self.cfg
    }

    pub fn policy(&self) -> &NetworkPolicy {
        &self.policy
    }

    fn host_slots(&self, url: &Url) -> Arc<Semaphore> {
        let key = format!(
            "{}:{}",
            url.host_str().unwrap_or_default().to_ascii_lowercase(),
            url.port_or_known_default().unwrap_or(0)
        );
        self.hosts
            .lock()
            .entry(key)
            .or_insert_with(|| Arc::new(Semaphore::new(self.cfg.per_host)))
            .clone()
    }

    /// Download one page.
    pub async fn fetch(&self, url: &str) -> Result<FetchedPage, FetchError> {
        let budget = AtomicUsize::new(self.cfg.max_total_bytes);
        let deadline = Instant::now() + self.cfg.budget;
        self.fetch_with(url, &budget, deadline).await
    }

    /// Download several pages concurrently; results in the order of `urls`.
    pub async fn fetch_many(
        self: &Arc<Self>,
        urls: &[String],
    ) -> Vec<Result<FetchedPage, FetchError>> {
        let budget = Arc::new(AtomicUsize::new(self.cfg.max_total_bytes));
        let deadline = Instant::now() + self.cfg.budget;
        let mut set = tokio::task::JoinSet::new();
        for (i, url) in urls.iter().enumerate() {
            let (this, url, budget) = (self.clone(), url.clone(), budget.clone());
            set.spawn(async move { (i, this.fetch_with(&url, &budget, deadline).await) });
        }
        let mut out: Vec<Option<Result<FetchedPage, FetchError>>> = vec![None; urls.len()];
        while let Some(joined) = set.join_next().await {
            if let Ok((i, r)) = joined {
                out[i] = Some(r);
            }
        }
        out.into_iter()
            .map(|r| r.unwrap_or_else(|| Err(FetchError::Network("download task failed".into()))))
            .collect()
    }

    async fn fetch_with(
        &self,
        url: &str,
        budget: &AtomicUsize,
        deadline: Instant,
    ) -> Result<FetchedPage, FetchError> {
        let requested = Url::parse(url).map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
        self.policy.check_url(&requested)?;
        // Waiting for slots counts against the call's budget, not the page's.
        let remaining = deadline.saturating_duration_since(Instant::now());
        let host = self.host_slots(&requested);
        let permits = tokio::time::timeout(remaining, async {
            let g = self.global.clone().acquire_owned().await;
            let h = host.acquire_owned().await;
            (g, h)
        })
        .await
        .map_err(|_| FetchError::Budget)?;
        let _permits = permits;
        if budget.load(Ordering::SeqCst) == 0 {
            return Err(FetchError::Budget);
        }
        let limit = self
            .cfg
            .page_timeout
            .min(deadline.saturating_duration_since(Instant::now()));
        if limit.is_zero() {
            return Err(FetchError::Budget);
        }
        let start = Instant::now();
        match tokio::time::timeout(limit, self.download(requested, budget, start)).await {
            Ok(r) => r,
            Err(_) => Err(FetchError::Timeout(limit)),
        }
    }

    async fn download(
        &self,
        requested: Url,
        budget: &AtomicUsize,
        start: Instant,
    ) -> Result<FetchedPage, FetchError> {
        let mut current = requested.clone();
        let mut redirects = 0;
        let mut response = loop {
            self.policy.check_url(&current)?;
            let resp = self
                .client
                .get(current.clone())
                .header(
                    reqwest::header::ACCEPT,
                    "text/html,application/xhtml+xml,text/markdown;q=0.9,text/plain;q=0.9,\
                     application/json;q=0.9,*/*;q=0.5",
                )
                .send()
                .await
                .map_err(network)?;
            if resp.status().is_redirection() {
                if redirects >= self.cfg.max_redirects {
                    return Err(FetchError::TooManyRedirects(self.cfg.max_redirects));
                }
                let next = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|loc| current.join(loc).ok())
                    .ok_or(FetchError::BadRedirect)?;
                redirects += 1;
                current = next;
                continue;
            }
            break resp;
        };
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(FetchError::Http { status });
        }
        let header = |name: reqwest::header::HeaderName| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header(reqwest::header::CONTENT_TYPE);
        let content_encoding = header(reqwest::header::CONTENT_ENCODING);
        // Announced, not trusted: with automatic decompression reqwest hides
        // the header, so it is read from the raw value when present.
        let declared_length = header(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.parse().ok())
            .or_else(|| response.content_length());
        let max = self.cfg.max_page_bytes;
        let mut body = Vec::new();
        let mut truncated = false;
        let mut interrupted = None;
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                // A body cut short (connection closed, wrong length): what
                // arrived is kept and marked incomplete.
                Err(e) if !body.is_empty() => {
                    interrupted = Some(network(e).to_string());
                    truncated = true;
                    break;
                }
                Err(e) => return Err(network(e)),
            };
            let room = max - body.len();
            let take = chunk.len().min(room);
            let granted = take_budget(budget, take);
            body.extend_from_slice(&chunk[..granted]);
            if granted < chunk.len() {
                truncated = true;
                break;
            }
        }
        Ok(FetchedPage {
            requested,
            final_url: current,
            status,
            content_type,
            declared_length,
            content_encoding,
            body,
            truncated,
            interrupted,
            redirects,
            elapsed: start.elapsed(),
        })
    }
}

/// Take up to `want` bytes from the shared budget; returns what was granted.
fn take_budget(budget: &AtomicUsize, want: usize) -> usize {
    let mut granted = 0;
    let _ = budget.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
        granted = want.min(left);
        Some(left - granted)
    });
    granted
}

fn network(e: reqwest::Error) -> FetchError {
    // The chain carries the policy refusal when the resolver produced it.
    let mut src: Option<&dyn std::error::Error> = Some(&e);
    while let Some(s) = src {
        if let Some(p) = s.downcast_ref::<PolicyError>() {
            return FetchError::Policy(p.clone());
        }
        src = s.source();
    }
    if e.is_timeout() {
        return FetchError::Network("timed out".into());
    }
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(&e);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    FetchError::Network(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_budget_is_never_overdrawn() {
        let b = AtomicUsize::new(10);
        assert_eq!(take_budget(&b, 4), 4);
        assert_eq!(take_budget(&b, 8), 6);
        assert_eq!(take_budget(&b, 1), 0);
    }
}
