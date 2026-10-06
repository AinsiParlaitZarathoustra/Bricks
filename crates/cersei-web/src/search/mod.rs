//! Web search over configurable providers with a visible fallback cascade.
//!
//! The configured provider is tried first, then `fallback` in order, within
//! `max_attempts` provider requests and the search `budget`. What triggers a
//! fallback is explicit ([`SearchError::allows_fallback`]); an empty result
//! list is an answer, not a failure, and does not move on. A rejected key is
//! reported as such and never retried. Every attempt — provider, outcome,
//! duration — is kept in [`SearchOutcome::attempts`], so a result obtained
//! from a fallback says so.

pub mod ddg;
pub mod pro;

use crate::config::{ProviderKind, SearchConfig};
use std::time::{Duration, Instant};
use url::Url;

/// One search result, uniform across providers.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SearchHit {
    /// Stable within one search: `S1`, `S2`, … in result order.
    pub source_id: String,
    pub title: String,
    /// Direct URL of the result (redirect wrappers removed).
    pub url: String,
    pub snippet: String,
    pub provider: ProviderKind,
    /// Position given by the provider (1-based), before de-duplication.
    pub rank: usize,
}

/// Why a provider gave no usable answer.
#[derive(Debug, Clone, PartialEq, thiserror::Error, serde::Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum SearchError {
    #[error("the API key was refused (HTTP {status})")]
    Auth { status: u16 },
    #[error("quota or plan limit reached (HTTP {status})")]
    Quota { status: u16 },
    #[error("rate limited (HTTP 429){}", retry_after_ms.map(|ms| format!(", retry after {ms} ms")).unwrap_or_default())]
    RateLimited { retry_after_ms: Option<u64> },
    #[error("an anti-bot challenge page was returned instead of results")]
    Challenge,
    #[error("the results page has an unexpected structure (parser out of date?)")]
    StructureChanged,
    #[error("request refused: HTTP {status}{}", if detail.is_empty() { String::new() } else { format!(" — {detail}") })]
    Http { status: u16, detail: String },
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("timed out after {ms} ms")]
    Timeout { ms: u64 },
    #[error("network error: {0}")]
    Network(String),
    #[error("malformed response: {0}")]
    Malformed(String),
}

impl SearchError {
    /// May the next provider of the cascade be tried?
    pub fn allows_fallback(&self) -> bool {
        // Everything except a request the next provider would reject the
        // same way is worth another provider; an invalid key of *this*
        // provider says nothing about the next one.
        !matches!(self, Self::Http { status, .. } if *status == 400)
    }

    /// May the same provider be asked again (once)?
    fn retry_after(&self, max_wait: Duration) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after_ms } => {
                let wait = Duration::from_millis(retry_after_ms.unwrap_or(500));
                (wait <= max_wait).then_some(wait)
            }
            _ => None,
        }
    }
}

/// One provider request.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Attempt {
    pub provider: ProviderKind,
    /// `Ok(n)`: n results (possibly 0).
    pub outcome: Result<usize, SearchError>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchOutcome {
    pub query: String,
    pub hits: Vec<SearchHit>,
    /// The provider whose answer is in `hits`.
    pub provider: Option<ProviderKind>,
    /// The configured first provider (to show a fallback happened).
    pub preferred: ProviderKind,
    pub attempts: Vec<Attempt>,
    pub elapsed_ms: u64,
}

impl SearchOutcome {
    pub fn used_fallback(&self) -> bool {
        self.provider.is_some_and(|p| p != self.preferred)
    }

    /// The last error when no provider answered.
    pub fn failure(&self) -> Option<&SearchError> {
        if self.provider.is_some() {
            return None;
        }
        self.attempts
            .iter()
            .rev()
            .find_map(|a| a.outcome.as_ref().err())
    }
}

/// Runs searches with one shared HTTP client.
pub struct Searcher {
    client: reqwest::Client,
    cfg: SearchConfig,
    user_agent: String,
}

impl Searcher {
    pub fn new(cfg: SearchConfig, user_agent: &str) -> Result<Self, String> {
        // Endpoints are fixed and validated at load; no redirect is followed
        // so a key is never sent anywhere else.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(user_agent.to_string())
            .connect_timeout(cfg.timeout)
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .build()
            .map_err(|e| format!("cannot build the search client: {e}"))?;
        Ok(Self {
            client,
            cfg,
            user_agent: user_agent.to_string(),
        })
    }

    pub fn config(&self) -> &SearchConfig {
        &self.cfg
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Search `query` for up to `count` results.
    pub async fn search(&self, query: &str, count: usize) -> SearchOutcome {
        let start = Instant::now();
        let deadline = start + self.cfg.budget;
        let mut attempts = Vec::new();
        let mut answer = None;
        'chain: for kind in self.cfg.chain() {
            let mut retried = false;
            loop {
                if attempts.len() >= self.cfg.max_attempts {
                    break 'chain;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break 'chain;
                }
                let t = Instant::now();
                let timeout = self.cfg.timeout.min(left);
                let result = match tokio::time::timeout(timeout, self.ask(kind, query, count)).await
                {
                    Ok(r) => r,
                    Err(_) => Err(SearchError::Timeout {
                        ms: timeout.as_millis() as u64,
                    }),
                };
                attempts.push(Attempt {
                    provider: kind,
                    outcome: result.as_ref().map(|h| h.len()).map_err(Clone::clone),
                    elapsed_ms: t.elapsed().as_millis() as u64,
                });
                match result {
                    Ok(hits) => {
                        answer = Some((kind, hits));
                        break 'chain;
                    }
                    Err(e) => {
                        if !retried {
                            if let Some(wait) = e.retry_after(self.cfg.max_retry_wait) {
                                if Instant::now() + wait < deadline {
                                    retried = true;
                                    tokio::time::sleep(wait).await;
                                    continue;
                                }
                            }
                        }
                        if !e.allows_fallback() {
                            break 'chain;
                        }
                        continue 'chain;
                    }
                }
            }
        }
        let (provider, hits) = match answer {
            Some((k, hits)) => (Some(k), finish(hits, count)),
            None => (None, Vec::new()),
        };
        SearchOutcome {
            query: query.to_string(),
            hits,
            provider,
            preferred: self.cfg.provider,
            attempts,
            elapsed_ms: start.elapsed().as_millis() as u64,
        }
    }

    async fn ask(
        &self,
        kind: ProviderKind,
        query: &str,
        count: usize,
    ) -> Result<Vec<RawHit>, SearchError> {
        let p = self.cfg.provider(kind);
        match kind {
            ProviderKind::DuckDuckGo => ddg::search(&self.client, p, query).await,
            _ => {
                let key = p.api_key.as_ref().ok_or_else(|| {
                    SearchError::Unavailable(format!(
                        "no key: environment variable {} is not set",
                        p.api_key_env.as_deref().unwrap_or("(none configured)")
                    ))
                })?;
                pro::search(&self.client, p, key, query, count).await
            }
        }
    }
}

/// A result before numbering and de-duplication.
#[derive(Debug, Clone, PartialEq)]
pub struct RawHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub provider: ProviderKind,
    pub rank: usize,
}

/// Drop duplicates (same normalised URL), keep provider order, number.
fn finish(hits: Vec<RawHit>, count: usize) -> Vec<SearchHit> {
    let mut seen = std::collections::HashSet::new();
    hits.into_iter()
        .filter(|h| seen.insert(dedup_key(&h.url)))
        .take(count)
        .enumerate()
        .map(|(i, h)| SearchHit {
            source_id: format!("S{}", i + 1),
            title: h.title,
            url: h.url,
            snippet: h.snippet,
            provider: h.provider,
            rank: h.rank,
        })
        .collect()
}

/// Comparison key of a URL: scheme and host case, default port, fragment
/// and `utm_*` tracking parameters ignored. Every other parameter (business
/// parameters, signatures) is kept, in order, and the URL itself is never
/// rewritten.
pub fn dedup_key(url: &str) -> String {
    let Ok(mut u) = Url::parse(url) else {
        return url.to_string();
    };
    u.set_fragment(None);
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !k.starts_with("utm_"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(kept);
    }
    let mut s = u.to_string();
    if u.path() == "/" && u.query().is_none() {
        s = s.trim_end_matches('/').to_string();
    }
    s
}

/// A short, single-line excerpt of a response body for an error message.
pub(crate) fn excerpt(body: &str, secret: Option<&str>) -> String {
    let mut s: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(k) = secret.filter(|k| !k.is_empty()) {
        s = s.replace(k, "***");
    }
    let mut out: String = s.chars().take(200).collect();
    if s.chars().count() > 200 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_ignores_tracking_but_keeps_signatures() {
        assert_eq!(
            dedup_key("HTTPS://Example.com/a?utm_source=x#frag"),
            dedup_key("https://example.com/a")
        );
        assert_ne!(
            dedup_key("https://cdn.example.com/f?sig=abc"),
            dedup_key("https://cdn.example.com/f?sig=def")
        );
        assert_ne!(
            dedup_key("https://example.com/item?id=1"),
            dedup_key("https://example.com/item?id=2")
        );
        assert_eq!(
            dedup_key("https://example.com/"),
            dedup_key("https://example.com")
        );
    }

    #[test]
    fn every_error_serializes() {
        for e in [
            SearchError::Auth { status: 401 },
            SearchError::Quota { status: 432 },
            SearchError::RateLimited {
                retry_after_ms: Some(5),
            },
            SearchError::Challenge,
            SearchError::StructureChanged,
            SearchError::Http {
                status: 500,
                detail: "x".into(),
            },
            SearchError::Unavailable("down".into()),
            SearchError::Timeout { ms: 1 },
            SearchError::Network("reset".into()),
            SearchError::Malformed("bad".into()),
        ] {
            let v = serde_json::to_value(&e).unwrap();
            assert!(v.get("kind").is_some(), "{v}");
        }
        let p = crate::PageError::Store("disk full".into());
        assert_eq!(serde_json::to_value(&p).unwrap()["detail"], "disk full");
        let p = crate::PageError::Fetch(crate::fetch::FetchError::Budget);
        assert!(serde_json::to_value(&p).unwrap()["detail"].is_string());
    }

    #[test]
    fn excerpts_hide_the_key_and_stay_short() {
        let e = excerpt("error: key sk-123 invalid\n\n", Some("sk-123"));
        assert_eq!(e, "error: key *** invalid");
        assert!(excerpt(&"x".repeat(500), None).chars().count() <= 201);
    }
}
