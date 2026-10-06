//! Keyed providers: Brave Search, Tavily and Exa, through their documented
//! APIs. HTTP statuses map to [`SearchError`] per provider documentation:
//!
//! | | auth | quota | rate limit |
//! |---|---|---|---|
//! | Brave | 401, 403 | 402 | 429 (`X-RateLimit-Reset`, seconds, first window) |
//! | Tavily | 401 | 432, 433 | 429 (`Retry-After`) |
//! | Exa | 401, 403 | 402 | 429 (`Retry-After`) |
//!
//! The key goes only in the provider's header, to the configured endpoint;
//! error messages quote at most a short excerpt of the body with the key
//! masked.

use super::{excerpt, RawHit, SearchError};
use crate::config::{ProviderConfig, ProviderKind, Secret};
use serde_json::{json, Value};

pub(crate) async fn search(
    client: &reqwest::Client,
    p: &ProviderConfig,
    key: &Secret,
    query: &str,
    count: usize,
) -> Result<Vec<RawHit>, SearchError> {
    let count = count.clamp(1, 20);
    let req = match p.kind {
        ProviderKind::Brave => {
            let mut q = vec![("q", query.to_string()), ("count", count.to_string())];
            if let Some(c) = &p.mode {
                q.push(("country", c.clone()));
            }
            client
                .get(p.endpoint.clone())
                .query(&q)
                .header("X-Subscription-Token", key.expose())
                .header(reqwest::header::ACCEPT, "application/json")
        }
        ProviderKind::Tavily => client
            .post(p.endpoint.clone())
            .bearer_auth(key.expose())
            .json(&json!({
                "query": query,
                "max_results": count,
                "search_depth": p.mode.as_deref().unwrap_or("basic"),
            })),
        ProviderKind::Exa => client
            .post(p.endpoint.clone())
            .header("x-api-key", key.expose())
            .json(&json!({
                "query": query,
                "numResults": count,
                "type": p.mode.as_deref().unwrap_or("auto"),
            })),
        ProviderKind::DuckDuckGo => unreachable!("not a keyed provider"),
    };
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() {
            SearchError::Timeout { ms: 0 }
        } else {
            SearchError::Network(e.without_url().to_string())
        }
    })?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = read_limited(resp, 4_000_000).await?;
    if !(200..300).contains(&status) {
        return Err(classify(p.kind, status, &headers, &body, key));
    }
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| SearchError::Malformed(format!("{} returned invalid JSON: {e}", p.kind)))?;
    parse(p.kind, &v)
}

async fn read_limited(mut resp: reqwest::Response, max: usize) -> Result<String, SearchError> {
    let mut buf = Vec::new();
    while let Some(c) = resp
        .chunk()
        .await
        .map_err(|e| SearchError::Network(e.without_url().to_string()))?
    {
        buf.extend_from_slice(&c);
        if buf.len() > max {
            return Err(SearchError::Malformed(format!(
                "response larger than {max} bytes"
            )));
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub fn classify(
    kind: ProviderKind,
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &str,
    key: &Secret,
) -> SearchError {
    let detail = excerpt(body, Some(key.expose()));
    let retry_after_ms = || {
        let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
        let secs = match kind {
            // Comma-separated, one value per window; the first is the
            // shortest window.
            ProviderKind::Brave => h("x-ratelimit-reset")
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse::<f64>().ok()),
            _ => h("retry-after").and_then(|v| v.trim().parse::<f64>().ok()),
        };
        secs.map(|s| (s.max(0.0) * 1000.0) as u64)
    };
    match (kind, status) {
        (_, 401) | (ProviderKind::Brave | ProviderKind::Exa, 403) => SearchError::Auth { status },
        (ProviderKind::Brave | ProviderKind::Exa, 402) | (ProviderKind::Tavily, 432 | 433) => {
            SearchError::Quota { status }
        }
        (_, 429) => SearchError::RateLimited {
            retry_after_ms: retry_after_ms(),
        },
        (_, 500..=599) => SearchError::Unavailable(format!("HTTP {status} from {kind}")),
        _ => SearchError::Http { status, detail },
    }
}

/// Results of a successful response.
pub fn parse(kind: ProviderKind, v: &Value) -> Result<Vec<RawHit>, SearchError> {
    let (list, snippet_key) = match kind {
        ProviderKind::Brave => (v.pointer("/web/results"), "description"),
        ProviderKind::Tavily => (v.get("results"), "content"),
        ProviderKind::Exa => (v.get("results"), "text"),
        ProviderKind::DuckDuckGo => unreachable!(),
    };
    let Some(list) = list else {
        // Brave omits `web` when there is nothing to return.
        if kind == ProviderKind::Brave && v.get("type").is_some() {
            return Ok(Vec::new());
        }
        return Err(SearchError::Malformed(format!(
            "{kind}: no result list in the response"
        )));
    };
    let list = list
        .as_array()
        .ok_or_else(|| SearchError::Malformed(format!("{kind}: results is not a list")))?;
    let mut out = Vec::new();
    for (i, r) in list.iter().enumerate() {
        let Some(url) = r.get("url").and_then(Value::as_str) else {
            continue;
        };
        let Ok(parsed) = url::Url::parse(url) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            continue;
        }
        let title = r.get("title").and_then(Value::as_str).unwrap_or(url);
        let mut snippet = r
            .get(snippet_key)
            .and_then(Value::as_str)
            .map(strip_tags)
            .unwrap_or_default();
        if kind == ProviderKind::Exa && snippet.is_empty() {
            snippet = r
                .get("highlights")
                .and_then(Value::as_array)
                .map(|h| {
                    h.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" … ")
                })
                .unwrap_or_default();
        }
        out.push(RawHit {
            title: strip_tags(title),
            url: url.to_string(),
            snippet: snippet.chars().take(500).collect(),
            provider: kind,
            rank: i + 1,
        });
    }
    Ok(out)
}

/// Brave highlights terms with `<strong>`; snippets are text.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn statuses_map_per_provider() {
        let k = Secret::new("k");
        let h = HeaderMap::new();
        assert_eq!(
            classify(ProviderKind::Tavily, 432, &h, "", &k),
            SearchError::Quota { status: 432 }
        );
        assert_eq!(
            classify(ProviderKind::Exa, 402, &h, "", &k),
            SearchError::Quota { status: 402 }
        );
        assert_eq!(
            classify(ProviderKind::Brave, 403, &h, "", &k),
            SearchError::Auth { status: 403 }
        );
        assert!(matches!(
            classify(ProviderKind::Exa, 503, &h, "", &k),
            SearchError::Unavailable(_)
        ));
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-reset", HeaderValue::from_static("1, 1419704"));
        assert_eq!(
            classify(ProviderKind::Brave, 429, &h, "", &k),
            SearchError::RateLimited {
                retry_after_ms: Some(1000)
            }
        );
    }

    #[test]
    fn brave_results_are_text() {
        let v = serde_json::json!({"type":"search","web":{"results":[
            {"title":"<strong>Tokio</strong> docs","url":"https://docs.rs/tokio","description":"An <strong>async</strong> runtime &amp; more"},
            {"title":"bad","url":"javascript:alert(1)"}
        ]}});
        let hits = parse(ProviderKind::Brave, &v).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Tokio docs");
        assert_eq!(hits[0].snippet, "An async runtime & more");
        assert!(
            parse(ProviderKind::Brave, &serde_json::json!({"type":"search"}))
                .unwrap()
                .is_empty()
        );
        assert!(parse(ProviderKind::Tavily, &serde_json::json!({"oops":1})).is_err());
    }
}
