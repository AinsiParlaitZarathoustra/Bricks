//! cersei-web against local servers only: no public network, no billing.
//!
//! The test server is explicitly allowed by `web.fetch.allow_private`
//! (`127.0.0.1`), the exception a public configuration does not have.

use cersei_testkit::{Length, Reply, TestServer};
use cersei_web::config::{ProviderKind, Secret, WebConfig};
use cersei_web::extract::{self, DocKind, Source, Strategy, Unreadable};
use cersei_web::fetch::{FetchError, Fetcher};
use cersei_web::policy::PolicyError;
use cersei_web::rank::{self, Relevance};
use cersei_web::search::{SearchError, Searcher};
use cersei_web::store::WebStore;
use cersei_web::{chunk, PageError, ResearchOptions, WebContext};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

fn fixture(path: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn local_config() -> WebConfig {
    let mut c = WebConfig::default();
    c.fetch.allow_private = vec!["127.0.0.1".into()];
    c.fetch.page_timeout = Duration::from_millis(800);
    c.search.timeout = Duration::from_millis(800);
    c
}

fn endpoint(server: &TestServer, kind: ProviderKind, path: &str, c: &mut WebConfig) {
    let p = c
        .search
        .providers
        .iter_mut()
        .find(|p| p.kind == kind)
        .unwrap();
    p.endpoint = Url::parse(&server.url(path)).unwrap();
    if kind.needs_key() {
        p.api_key_env = Some(format!("{}_KEY", kind.name().to_uppercase()));
        p.api_key = Some(Secret::new(format!("secret-{}", kind.name())));
    }
}

// ─── Search ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn duckduckgo_results_empty_challenge_and_unknown_pages_are_told_apart() {
    let server = TestServer::start(|req| async move {
        let q = req.query("q").unwrap_or_default();
        match q.as_str() {
            "results" => Reply::html(
                fixture("ddg/results.html")
                    .replace("{{BASE_ENC}}", "https%3A%2F%2Fdocs.example.com")
                    .replace("{{BASE}}", "https://blog.example.fr"),
            ),
            "none" => Reply::html(fixture("ddg/no_results.html")),
            "challenge" => Reply::html(fixture("ddg/challenge.html")),
            "changed" => Reply::html(fixture("ddg/changed.html")),
            _ => Reply::text(403, "text/html", "Forbidden"),
        }
    })
    .await;
    let mut c = local_config();
    endpoint(&server, ProviderKind::DuckDuckGo, "/html/", &mut c);
    let s = Searcher::new(c.search.clone(), &c.fetch.user_agent).unwrap();

    let out = s.search("results", 10).await;
    assert_eq!(out.provider, Some(ProviderKind::DuckDuckGo));
    let urls: Vec<&str> = out.hits.iter().map(|h| h.url.as_str()).collect();
    assert_eq!(
        urls,
        vec![
            "https://docs.example.com/docs/retry.html",
            "https://blog.example.fr/blog/timeouts.html",
            "https://blog.example.fr/slow.html",
            "https://blog.example.fr/spec.pdf",
        ],
        "ad skipped, uddg decoded, tracking duplicate removed"
    );
    assert_eq!(out.hits[0].source_id, "S1");
    assert_eq!(out.hits[0].title, "Client API — Retry policy");
    assert!(out.hits[0].snippet.contains("idempotent"));
    // The provider's own positions are kept (the ad was 0, duplicate 3).
    assert_eq!(
        out.hits.iter().map(|h| h.rank).collect::<Vec<_>>(),
        vec![1, 2, 4, 5]
    );
    let ua = server.requests()[0]
        .header("user-agent")
        .unwrap()
        .to_string();
    assert!(ua.starts_with("Bricks/"), "{ua}");

    let none = s.search("none", 10).await;
    assert_eq!(none.provider, Some(ProviderKind::DuckDuckGo));
    assert!(none.hits.is_empty());
    assert!(matches!(none.attempts[0].outcome, Ok(0)));

    let ch = s.search("challenge", 10).await;
    assert_eq!(ch.provider, None);
    assert_eq!(ch.failure(), Some(&SearchError::Challenge));

    let changed = s.search("changed", 10).await;
    assert_eq!(changed.failure(), Some(&SearchError::StructureChanged));

    let refused = s.search("blocked", 10).await;
    assert!(matches!(
        refused.failure(),
        Some(SearchError::Http { status: 403, .. })
    ));
}

fn tavily_ok() -> Reply {
    Reply::json(
        200,
        r#"{"query":"q","results":[{"title":"T1","url":"https://t.example/1","content":"from tavily","score":0.9}]}"#,
    )
}

#[tokio::test]
async fn the_cascade_is_visible_bounded_and_never_hides_a_refused_key() {
    let exa_hits = Arc::new(AtomicUsize::new(0));
    let e2 = exa_hits.clone();
    let server = TestServer::start(move |req| {
        let e2 = e2.clone();
        async move {
            match req.path() {
                "/brave" => Reply::json(401, r#"{"error":"invalid token secret-brave"}"#),
                "/tavily" => Reply::json(432, r#"{"detail":"plan limit"}"#),
                "/exa" => {
                    e2.fetch_add(1, Ordering::SeqCst);
                    Reply::json(200, r#"{"results":[{"title":"E","url":"https://e.example/","text":"from exa"}]}"#)
                }
                _ => Reply::new(404),
            }
        }
    })
    .await;
    let mut c = local_config();
    for (k, p) in [
        (ProviderKind::Brave, "/brave"),
        (ProviderKind::Tavily, "/tavily"),
        (ProviderKind::Exa, "/exa"),
    ] {
        endpoint(&server, k, p, &mut c);
    }
    c.search.provider = ProviderKind::Brave;
    c.search.fallback = vec![ProviderKind::Tavily, ProviderKind::Exa];
    let s = Searcher::new(c.search.clone(), &c.fetch.user_agent).unwrap();
    let out = s.search("q", 5).await;
    assert_eq!(out.provider, Some(ProviderKind::Exa));
    assert!(out.used_fallback());
    let outcomes: Vec<_> = out
        .attempts
        .iter()
        .map(|a| (a.provider, a.outcome.clone()))
        .collect();
    assert_eq!(
        outcomes,
        vec![
            (ProviderKind::Brave, Err(SearchError::Auth { status: 401 })),
            (
                ProviderKind::Tavily,
                Err(SearchError::Quota { status: 432 })
            ),
            (ProviderKind::Exa, Ok(1)),
        ]
    );
    // Each key went to its own provider only, in its own header.
    let reqs = server.requests();
    let by = |p: &str| reqs.iter().find(|r| r.path() == p).unwrap();
    assert_eq!(
        by("/brave").header("x-subscription-token"),
        Some("secret-brave")
    );
    assert_eq!(
        by("/tavily").header("authorization"),
        Some("Bearer secret-tavily")
    );
    assert_eq!(by("/exa").header("x-api-key"), Some("secret-exa"));
    assert!(by("/exa").header("authorization").is_none());
    let dump = format!("{out:?} {}", serde_json::to_string(&out).unwrap());
    assert!(
        !dump.contains("secret-"),
        "no key in results or errors: {dump}"
    );

    // Bounded: two attempts allowed, the third provider is never asked.
    let mut c2 = c.clone();
    c2.search.max_attempts = 2;
    let before = exa_hits.load(Ordering::SeqCst);
    let out = Searcher::new(c2.search, &c.fetch.user_agent)
        .unwrap()
        .search("q", 5)
        .await;
    assert_eq!(out.attempts.len(), 2);
    assert_eq!(out.provider, None);
    assert_eq!(exa_hits.load(Ordering::SeqCst), before);

    // A refused key alone: reported, not retried, not replaced silently.
    let mut c3 = c.clone();
    c3.search.fallback.clear();
    let out = Searcher::new(c3.search, &c.fetch.user_agent)
        .unwrap()
        .search("q", 5)
        .await;
    assert_eq!(out.attempts.len(), 1);
    assert_eq!(out.failure(), Some(&SearchError::Auth { status: 401 }));
}

#[tokio::test]
async fn rate_limits_are_honoured_once_and_timeouts_fall_back() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let server = TestServer::start(move |req| {
        let calls = c2.clone();
        async move {
            match req.path() {
                "/tavily" => {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Reply::json(429, "{}").header("retry-after", "0.1")
                    } else {
                        tavily_ok()
                    }
                }
                "/brave" => Reply::json(200, "{}").delay(Duration::from_secs(3)),
                "/html/" => Reply::html(fixture("ddg/no_results.html")),
                _ => Reply::new(404),
            }
        }
    })
    .await;
    let mut c = local_config();
    endpoint(&server, ProviderKind::Tavily, "/tavily", &mut c);
    endpoint(&server, ProviderKind::Brave, "/brave", &mut c);
    endpoint(&server, ProviderKind::DuckDuckGo, "/html/", &mut c);
    c.search.provider = ProviderKind::Tavily;
    let out = Searcher::new(c.search.clone(), "t")
        .unwrap()
        .search("q", 5)
        .await;
    assert_eq!(out.provider, Some(ProviderKind::Tavily));
    assert_eq!(out.attempts.len(), 2);
    assert!(matches!(
        out.attempts[0].outcome,
        Err(SearchError::RateLimited {
            retry_after_ms: Some(100)
        })
    ));
    assert_eq!(out.hits[0].snippet, "from tavily");

    c.search.provider = ProviderKind::Brave;
    c.search.fallback = vec![ProviderKind::DuckDuckGo];
    c.search.timeout = Duration::from_millis(300);
    let out = Searcher::new(c.search, "t").unwrap().search("q", 5).await;
    assert!(matches!(
        out.attempts[0].outcome,
        Err(SearchError::Timeout { .. })
    ));
    assert_eq!(out.provider, Some(ProviderKind::DuckDuckGo));
    assert!(out.used_fallback());
}

// ─── Downloads ───────────────────────────────────────────────────────────────

fn page(i: usize) -> String {
    format!(
        "<html><head><title>P{i}</title></head><body><main><p>Page {i}.</p></main></body></html>"
    )
}

#[tokio::test]
async fn downloads_overlap_within_global_and_per_host_limits() {
    // Two servers (two hosts for the limiter) sharing one in-flight counter.
    let now = Arc::new(AtomicUsize::new(0));
    let max = Arc::new(AtomicUsize::new(0));
    let mk = |now: Arc<AtomicUsize>, max: Arc<AtomicUsize>| {
        move |req: cersei_testkit::Request| {
            let (now, max) = (now.clone(), max.clone());
            async move {
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(150)).await;
                now.fetch_sub(1, Ordering::SeqCst);
                Reply::html(page(req.target.len()))
            }
        }
    };
    let a = TestServer::start(mk(now.clone(), max.clone())).await;
    let b = TestServer::start(mk(now.clone(), max.clone())).await;
    let mut c = local_config();
    c.fetch.concurrency = 3;
    c.fetch.per_host = 2;
    let f = Arc::new(Fetcher::new(c.fetch).unwrap());
    let urls: Vec<String> = (0..4)
        .map(|i| a.url(&format!("/a{i}")))
        .chain((0..4).map(|i| b.url(&format!("/b{i}"))))
        .collect();
    let out = f.fetch_many(&urls).await;
    assert!(out.iter().all(|r| r.is_ok()));
    // Results come back in the order asked, whatever finished first.
    for (r, u) in out.iter().zip(&urls) {
        assert_eq!(r.as_ref().unwrap().requested.as_str(), u);
    }
    assert_eq!(a.max_in_flight(), 2, "per-host limit reached, not exceeded");
    assert_eq!(b.max_in_flight(), 2);
    assert_eq!(
        max.load(Ordering::SeqCst),
        3,
        "downloads overlapped up to the global limit"
    );
}

#[tokio::test]
async fn timeouts_redirects_and_failures_are_reported_per_url() {
    let server = TestServer::start(|req| async move {
        match req.path() {
            "/ok" => Reply::html(page(1)),
            "/slow" => Reply::html(page(2)).delay(Duration::from_secs(3)),
            "/slow-body" => Reply::new(200)
                .header("content-type", "text/html")
                .chunk(Duration::ZERO, b"<html><body>".to_vec())
                .chunk(Duration::from_secs(3), b"late</body></html>".to_vec())
                .length(Length::None),
            "/r1" => Reply::redirect(302, "/r2"),
            "/r2" => Reply::redirect(301, "ok"),
            "/loop" => Reply::redirect(302, "/loop"),
            "/to-metadata" => Reply::redirect(302, "http://169.254.169.254/latest/meta-data/"),
            "/missing" => Reply::new(404),
            _ => Reply::new(500),
        }
    })
    .await;
    let f = Arc::new(Fetcher::new(local_config().fetch).unwrap());
    let urls: Vec<String> = [
        "/ok",
        "/slow",
        "/r1",
        "/missing",
        "/loop",
        "/to-metadata",
        "/slow-body",
    ]
    .iter()
    .map(|p| server.url(p))
    .collect();
    let out = f.fetch_many(&urls).await;
    assert!(out[0].is_ok());
    assert!(matches!(out[1], Err(FetchError::Timeout(_))));
    let r = out[2].as_ref().unwrap();
    assert_eq!(r.redirects, 2);
    assert_eq!(r.final_url.path(), "/ok");
    assert_eq!(
        out[3].as_ref().unwrap_err(),
        &FetchError::Http { status: 404 }
    );
    assert_eq!(
        out[4].as_ref().unwrap_err(),
        &FetchError::TooManyRedirects(5)
    );
    assert!(
        matches!(&out[5], Err(FetchError::Policy(PolicyError::Private(h))) if h == "169.254.169.254"),
        "a redirect to a private address is refused: {:?}",
        out[5]
    );
    assert!(
        matches!(out[6], Err(FetchError::Timeout(_))),
        "the timeout covers the body"
    );
}

#[tokio::test]
async fn bodies_are_bounded_after_decompression_whatever_the_headers_say() {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&vec![b'a'; 5_000_000]).unwrap();
    let bomb = gz.finish().unwrap();
    assert!(bomb.len() < 50_000);
    let bomb = Arc::new(bomb);
    let server = TestServer::start(move |req| {
        let bomb = bomb.clone();
        async move {
            match req.path() {
                "/gzip" => Reply::new(200)
                    .header("content-type", "text/plain")
                    .header("content-encoding", "gzip")
                    .body((*bomb).clone()),
                "/no-length" => {
                    let mut r = Reply::new(200)
                        .header("content-type", "text/plain")
                        .length(Length::None);
                    for _ in 0..10 {
                        r = r.chunk(Duration::from_millis(5), vec![b'x'; 5000]);
                    }
                    r
                }
                "/short-length" => {
                    Reply::text(200, "text/plain", vec![b'y'; 1000]).length(Length::Wrong(10))
                }
                "/long-length" => Reply::text(200, "text/plain", vec![b'z'; 1000])
                    .length(Length::Wrong(1_000_000)),
                _ => Reply::new(404),
            }
        }
    })
    .await;
    let mut c = local_config();
    c.fetch.max_page_bytes = 200_000;
    let f = Fetcher::new(c.fetch).unwrap();
    let p = f.fetch(&server.url("/gzip")).await.unwrap();
    assert_eq!(p.body.len(), 200_000, "cut at the decoded limit");
    assert!(p.truncated);
    // Decoded by the client: the header is gone, the bytes are plain.
    assert_eq!(p.content_encoding, None);
    assert!(p.body.iter().all(|b| *b == b'a'));

    let p = f.fetch(&server.url("/no-length")).await.unwrap();
    assert_eq!(p.body.len(), 50_000);
    assert!(!p.truncated);

    // A length shorter than the body: only what was announced is read.
    let p = f.fetch(&server.url("/short-length")).await.unwrap();
    assert_eq!(p.body.len(), 10);
    // A length longer than the body: what arrived is kept, marked incomplete.
    let p = f.fetch(&server.url("/long-length")).await.unwrap();
    assert_eq!(p.body.len(), 1000);
    assert!(p.truncated && p.interrupted.is_some(), "{p:?}");
}

#[tokio::test]
async fn the_byte_budget_of_a_call_is_shared_and_private_hosts_need_an_exception() {
    let server =
        TestServer::start(|_| async { Reply::text(200, "text/plain", vec![b'q'; 150_000]) }).await;
    let mut c = local_config();
    c.fetch.max_page_bytes = 200_000;
    c.fetch.max_total_bytes = 300_000;
    let f = Arc::new(Fetcher::new(c.fetch.clone()).unwrap());
    let urls: Vec<String> = (0..3).map(|i| server.url(&format!("/{i}"))).collect();
    let out = f.fetch_many(&urls).await;
    let total: usize = out
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|p| p.body.len())
        .sum();
    assert!(total <= 300_000, "{total}");
    assert!(out
        .iter()
        .any(|r| matches!(r, Ok(p) if p.truncated) || matches!(r, Err(FetchError::Budget))));

    let mut public = c.fetch.clone();
    public.allow_private.clear();
    let f = Fetcher::new(public).unwrap();
    assert!(matches!(
        f.fetch(&server.url("/x")).await,
        Err(FetchError::Policy(PolicyError::Private(_)))
    ));
    assert!(matches!(
        f.fetch("http://localhost:1/").await,
        Err(FetchError::Policy(PolicyError::Private(_)))
    ));
}

#[tokio::test]
async fn cancelling_a_call_stops_downloads_not_yet_started() {
    let server = TestServer::start(|_| async {
        tokio::time::sleep(Duration::from_millis(400)).await;
        Reply::html(page(0))
    })
    .await;
    let mut c = local_config();
    c.fetch.per_host = 2;
    let f = Arc::new(Fetcher::new(c.fetch).unwrap());
    let urls: Vec<String> = (0..6).map(|i| server.url(&format!("/{i}"))).collect();
    let r = tokio::time::timeout(Duration::from_millis(150), f.fetch_many(&urls)).await;
    assert!(r.is_err(), "cancelled");
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(
        server.total_requests(),
        2,
        "only the two started downloads reached the server"
    );
}

// ─── Extraction ──────────────────────────────────────────────────────────────

fn extract_fixture(name: &str, url: &str, ct: &str) -> Result<extract::Extracted, Unreadable> {
    let body = std::fs::read(format!(
        "{}/tests/fixtures/pages/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let url = Url::parse(url).unwrap();
    extract::extract(
        &Source {
            url: &url,
            content_type: Some(ct),
            body: &body,
            truncated: false,
            declared_length: None,
        },
        &WebConfig::default().extract,
    )
}

#[test]
fn documentation_keeps_tables_code_warnings_and_resolved_links() {
    let e = extract_fixture(
        "retry.html",
        "https://docs.example.com/docs/retry.html",
        "text/html",
    )
    .unwrap();
    let md = &e.markdown;
    assert_eq!(e.kind, DocKind::Html);
    assert!(md.starts_with("# "), "{md}");
    assert!(
        md.contains("| `max_retries` | 3 | v2.1 |"),
        "table kept: {md}"
    );
    assert!(
        md.contains("```") && md.contains(".max_retries(5)"),
        "code kept: {md}"
    );
    assert!(
        md.contains("do not retry `POST /payments`"),
        "warning kept: {md}"
    );
    assert!(
        md.contains("(https://docs.example.com/docs/timeouts.html)"),
        "relative link resolved: {md}"
    );
    assert!(
        md.contains("(https://docs.example.com/errors.html#transient)"),
        "{md}"
    );
    for noise in [
        "Pricing",
        "Privacy",
        "window.analytics",
        "font-family",
        "Install",
    ] {
        assert!(!md.contains(noise), "{noise} left in: {md}");
    }
    println!("retry.html strategy: {:?} notes: {:?}", e.strategy, e.notes);
}

#[test]
fn articles_and_navigation_heavy_pages_keep_their_substance() {
    let e = extract_fixture(
        "timeouts_fr.html",
        "https://blog.example.fr/blog/timeouts.html",
        "text/html; charset=utf-8",
    )
    .unwrap();
    assert!(
        e.markdown.contains("30 secondes depuis la version 2.1"),
        "{}",
        e.markdown
    );
    assert!(e.markdown.contains("délai total"));
    assert!(!e.markdown.contains("Accueil"));
    assert!(
        e.title.starts_with("Comprendre les délais d'expiration"),
        "{}",
        e.title
    );

    let e = extract_fixture(
        "nav_heavy.html",
        "https://api.example.com/docs/status",
        "text/html",
    )
    .unwrap();
    assert!(e.markdown.contains("Retry-After"), "{}", e.markdown);
    assert!(
        !e.markdown.contains("Investors") && !e.markdown.contains("Accessibility"),
        "{}",
        e.markdown
    );
}

#[test]
fn unicode_json_shells_and_empty_pages() {
    let e = extract_fixture("unicode.html", "https://x.example/g", "text/html").unwrap();
    for s in ["タイムアウト", "таймаут", "مهلة", "« »", "⏱️", "’"] {
        assert!(e.markdown.contains(s), "{s} lost: {}", e.markdown);
    }
    let j = extract_fixture(
        "data.json",
        "https://x.example/data.json",
        "application/json",
    )
    .unwrap();
    assert_eq!(j.kind, DocKind::Json);
    assert_eq!(j.strategy, Strategy::AsServed);
    assert!(j.markdown.contains("\"max_retries\":3"));
    assert!(matches!(
        extract_fixture("shell.html", "https://x.example/app", "text/html"),
        Err(Unreadable::NeedsJavascript { .. })
    ));
    assert!(matches!(
        extract_fixture("empty.html", "https://x.example/e", "text/html"),
        Err(Unreadable::Empty)
    ));
}

// ─── Ranking on the corpus ──────────────────────────────────────────────────

fn corpus() -> Vec<(String, Vec<chunk::Passage>)> {
    let p = WebConfig::default().passages;
    [
        ("retry.html", "https://docs.example.com/docs/retry.html"),
        (
            "timeouts_fr.html",
            "https://blog.example.fr/blog/timeouts.html",
        ),
        ("nav_heavy.html", "https://api.example.com/docs/status"),
    ]
    .iter()
    .map(|(f, u)| {
        let e = extract_fixture(f, u, "text/html").unwrap();
        let ps = chunk::split(&e.markdown, p.min_chars, p.max_chars);
        (e.markdown, ps)
    })
    .collect()
}

#[test]
fn expected_passages_are_found_in_french_english_and_for_api_names() {
    let docs = corpus();
    let pages: Vec<Vec<chunk::Passage>> = docs.iter().map(|(_, p)| p.clone()).collect();
    let cfg = WebConfig::default().passages;
    for (query, source, needle) in [
        ("max_retries default value", 0, "max_retries"),
        ("délai total par défaut", 1, "30 secondes"),
        ("Retry-After header 429", 2, "Retry-After"),
        ("POST /payments idempotent retry", 0, "POST /payments"),
    ] {
        let sel = rank::select(query, &pages, &cfg);
        assert_eq!(sel.relevance, Relevance::Found, "{query}");
        let first = sel.picked.iter().find(|p| p.context_for.is_none()).unwrap();
        assert_eq!(first.scored.source, source, "{query}: {sel:?}");
        let passage = &pages[source][first.scored.passage];
        assert!(passage.text.contains(needle), "{query}: {}", passage.text);
        // The reference points at the same text in the extracted document.
        let md = &docs[source].0;
        let start = cersei_web::char_to_byte(md, passage.char_start);
        let end = cersei_web::char_to_byte(md, passage.char_end);
        assert_eq!(&md[start..end], passage.text);
    }
    let none = rank::select("kubernetes ingress annotations", &pages, &cfg);
    assert_eq!(none.relevance, Relevance::None);
}

// ─── Whole pipeline ─────────────────────────────────────────────────────────

async fn site() -> TestServer {
    TestServer::start(|req| async move {
        let base = format!("http://{}", req.header("host").unwrap_or_default());
        match req.path() {
            "/html/" => Reply::html(
                fixture("ddg/results.html")
                    .replace(
                        "{{BASE_ENC}}",
                        &base.replace(':', "%3A").replace('/', "%2F"),
                    )
                    .replace("{{BASE}}", &base),
            ),
            "/docs/retry.html" => Reply::html(fixture("pages/retry.html")),
            "/blog/timeouts.html" => Reply::html(fixture("pages/timeouts_fr.html")),
            "/slow.html" => Reply::html(page(9)).delay(Duration::from_secs(3)),
            "/spec.pdf" => Reply::text(200, "application/pdf", b"%PDF-1.7\n%binary".to_vec()),
            _ => Reply::new(404),
        }
    })
    .await
}

#[tokio::test]
async fn research_reads_pages_selects_traceable_passages_and_survives_a_restore() {
    let server = site().await;
    let dir = tempfile::tempdir().unwrap();
    let mut c = local_config();
    endpoint(&server, ProviderKind::DuckDuckGo, "/html/", &mut c);
    let store = Arc::new(WebStore::open(
        dir.path().join("web"),
        c.store.max_session_bytes,
    ));
    let ctx = WebContext::new(c.clone(), Some(store)).unwrap();
    let r = ctx
        .research(
            "max_retries retry policy",
            ResearchOptions {
                results: 8,
                read_pages: 4,
            },
        )
        .await;
    assert_eq!(r.search.provider, Some(ProviderKind::DuckDuckGo));
    assert_eq!(r.pages.len(), 4);
    assert!(r.pages[0].outcome.is_ok() && r.pages[1].outcome.is_ok());
    assert!(matches!(
        &r.pages[2].outcome,
        Err(PageError::Fetch(FetchError::Timeout(_)))
    ));
    assert!(matches!(
        &r.pages[3].outcome,
        Err(PageError::Unreadable(Unreadable::Binary { mime, .. })) if mime == "application/pdf"
    ));
    let sel = r.selection.as_ref().unwrap();
    assert_eq!(sel.relevance, Relevance::Found);
    let first = sel.picked.iter().find(|p| p.context_for.is_none()).unwrap();
    let doc = r.pages[first.scored.source].outcome.as_ref().unwrap();
    let passage = &doc.passages[first.scored.passage];
    assert!(passage.text.contains("max_retries"));
    assert_eq!(r.pages[first.scored.source].source_id, "S1");
    assert!(!passage.section.is_empty());
    // Timings are measured per stage.
    assert!(
        r.timings.fetch_ms >= 700,
        "the slow page held the fetch stage: {:?}",
        r.timings
    );

    // Both readable pages are stored; the raw file is what was received.
    let entry = &doc.entry;
    assert_eq!(entry.id, "D1");
    let saved_raw = std::fs::read(dir.path().join("web").join(&entry.raw_file)).unwrap();
    assert_eq!(saved_raw, fixture("pages/retry.html").into_bytes());
    assert!(!entry.truncated);

    // A restored session reads the document back without downloading it.
    let downloads = server
        .requests()
        .iter()
        .filter(|q| q.path() == "/docs/retry.html")
        .count();
    let restored = WebContext::new(
        c,
        Some(Arc::new(WebStore::open(dir.path().join("web"), 64 << 20))),
    )
    .unwrap();
    let again = restored
        .read(&server.url("/docs/retry.html"), false)
        .await
        .unwrap();
    assert!(again.cached);
    assert_eq!(again.markdown, doc.markdown);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|q| q.path() == "/docs/retry.html")
            .count(),
        downloads
    );
    assert!(restored.stored("D2").is_some());
}
