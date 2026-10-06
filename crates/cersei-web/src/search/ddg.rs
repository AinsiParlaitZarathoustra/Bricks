//! DuckDuckGo's HTML interface (`html.duckduckgo.com/html/`).
//!
//! This is a web page without JavaScript, not a contractual API: its markup
//! can change and the service can refuse automated requests. The parser
//! therefore tells apart four situations — results, an explicit "no
//! results", an anti-bot challenge (even with HTTP 200), and a page it does
//! not recognise — instead of reading the last three as an empty search.
//! Requests carry the configured, identified User-Agent; no challenge is
//! answered or worked around.

use super::{RawHit, SearchError};
use crate::config::{ProviderConfig, ProviderKind};
use dom_query::Document;
use url::Url;

pub(crate) async fn search(
    client: &reqwest::Client,
    p: &ProviderConfig,
    query: &str,
) -> Result<Vec<RawHit>, SearchError> {
    let resp = client
        .get(p.endpoint.clone())
        .query(&[("q", query)])
        .header(reqwest::header::ACCEPT, "text/html")
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                SearchError::Timeout { ms: 0 }
            } else {
                SearchError::Network(e.to_string())
            }
        })?;
    let status = resp.status().as_u16();
    let body = read_limited(resp, 2_000_000).await?;
    parse(status, &body, &p.endpoint)
}

async fn read_limited(mut resp: reqwest::Response, max: usize) -> Result<String, SearchError> {
    let mut buf = Vec::new();
    while let Some(c) = resp
        .chunk()
        .await
        .map_err(|e| SearchError::Network(e.to_string()))?
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

/// Classify and parse a results page.
pub fn parse(status: u16, body: &str, endpoint: &Url) -> Result<Vec<RawHit>, SearchError> {
    let lower = body.to_ascii_lowercase();
    if is_challenge(&lower) {
        return Err(SearchError::Challenge);
    }
    match status {
        200 => {}
        403 | 429 => {
            return Err(SearchError::Http {
                status,
                detail: "the service refused the request".into(),
            })
        }
        s => {
            return Err(SearchError::Http {
                status: s,
                detail: super::excerpt(body, None),
            })
        }
    }
    let doc = Document::from(body);
    let mut hits = Vec::new();
    let mut rank = 0;
    for node in doc.select("div.result, div.web-result").iter() {
        let class = node
            .attr("class")
            .map(|c| c.to_string())
            .unwrap_or_default();
        if class.contains("result--ad") || class.contains("result--no-result") {
            continue;
        }
        let link = node.select("a.result__a");
        let Some(href) = link.attr("href") else {
            continue;
        };
        rank += 1;
        let Some(url) = result_url(&href, endpoint) else {
            continue;
        };
        let title = clean(&link.text());
        let snippet = clean(&node.select(".result__snippet").text());
        hits.push(RawHit {
            title: if title.is_empty() { url.clone() } else { title },
            url,
            snippet,
            provider: ProviderKind::DuckDuckGo,
            rank,
        });
    }
    if !hits.is_empty() {
        return Ok(hits);
    }
    let no_results = doc.select(".no-results, .result--no-result").length() > 0
        || lower.contains("no results.")
        || lower.contains("no more results");
    if no_results {
        return Ok(Vec::new());
    }
    Err(SearchError::StructureChanged)
}

fn is_challenge(lower: &str) -> bool {
    [
        "anomaly-modal",
        "challenge-form",
        "/anomaly.js",
        "bots use duckduckgo too",
        "please complete the following challenge",
        "select all squares containing",
    ]
    .iter()
    .any(|m| lower.contains(m))
}

/// The direct URL of a result link. `uddg` is decoded only on DuckDuckGo's
/// own redirect path (`/l/`); other DuckDuckGo links are internal and
/// dropped; relative links are resolved against the endpoint.
pub fn result_url(href: &str, endpoint: &Url) -> Option<String> {
    let u = endpoint.join(href.trim()).ok()?;
    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    let is_ddg = host == "duckduckgo.com" || host.ends_with(".duckduckgo.com");
    let target = if is_ddg {
        if !u.path().starts_with("/l/") {
            return None;
        }
        let v = u.query_pairs().find(|(k, _)| k == "uddg")?.1.into_owned();
        Url::parse(&v).ok()?
    } else {
        u
    };
    matches!(target.scheme(), "http" | "https")
        .then(|| target.host().is_some())
        .filter(|ok| *ok)
        .map(|_| target.to_string())
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep() -> Url {
        Url::parse("https://html.duckduckgo.com/html/").unwrap()
    }

    #[test]
    fn uddg_is_decoded_only_on_the_redirect_path() {
        let e = ep();
        assert_eq!(
            result_url(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fdocs.rs%2Ftokio%3Fa%3D1%26b%3D2&rut=abc",
                &e
            )
            .as_deref(),
            Some("https://docs.rs/tokio?a=1&b=2")
        );
        assert_eq!(
            result_url("https://example.org/page?uddg=x", &e).as_deref(),
            Some("https://example.org/page?uddg=x")
        );
        assert_eq!(result_url("/html/?q=next", &e), None);
        assert_eq!(
            result_url("//duckduckgo.com/l/?uddg=javascript%3Aalert(1)", &e),
            None
        );
        assert_eq!(result_url("//duckduckgo.com/y.js?ad=1", &e), None);
    }
}
