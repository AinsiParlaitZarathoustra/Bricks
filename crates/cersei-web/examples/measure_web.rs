//! Measure the web pipeline on the local corpus: downloaded HTML → extracted
//! Markdown → selected passages.
//!
//! ```text
//! cargo run --release -p cersei-web --example measure_web
//! ```
//!
//! For each page and query: sizes and estimated tokens at each stage
//! (method: `cersei_types::tokens::ESTIMATION_METHOD`), the time of each
//! stage (median of 7 runs), allocations of the extraction, and whether the
//! reference passage for the query was kept. Pages are also enlarged
//! (sections repeated ×10 and ×50) to see how cost grows with size.
//!
//! Everything is local: the "download" stage is a loopback HTTP server and
//! says nothing about latency on the Internet; it is reported to show the
//! pipeline's own overhead only.

use cersei_testkit::{Reply, TestServer};
use cersei_web::config::WebConfig;
use cersei_web::extract::{self, Source};
use cersei_web::fetch::Fetcher;
use cersei_web::{chunk, rank};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(l.size() as u64, Ordering::Relaxed);
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn tokens(s: &str) -> u64 {
    cersei_types::tokens::estimate_text(s).tokens
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn ms(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64() * 1000.0)
}

/// Repeat the main content `n` times (a longer page of the same shape).
fn enlarge(html: &str, n: usize) -> String {
    let (Some(a), Some(b)) = (html.find("<main>"), html.find("</main>")) else {
        return html.to_string();
    };
    let inner = &html[a + 6..b];
    let mut body = String::new();
    for i in 0..n {
        body.push_str(&inner.replace("<h1>", &format!("<h1>Part {i}: ")));
    }
    format!("{}<main>{body}</main>{}", &html[..a], &html[b + 7..])
}

fn main() {
    let dir = format!("{}/tests/fixtures/pages", env!("CARGO_MANIFEST_DIR"));
    let read = |f: &str| std::fs::read_to_string(format!("{dir}/{f}")).unwrap();
    let cfg = WebConfig::default();
    let retry = read("retry.html");
    let cases: Vec<(String, String, &str, &str, &str)> = vec![
        (
            "retry.html".into(),
            retry.clone(),
            "https://docs.example.com/docs/retry.html",
            "max_retries default value",
            "| `max_retries` | 3 | v2.1 |",
        ),
        (
            "retry.html".into(),
            retry.clone(),
            "https://docs.example.com/docs/retry.html",
            "POST /payments idempotent",
            "do not retry `POST /payments`",
        ),
        (
            "timeouts_fr.html".into(),
            read("timeouts_fr.html"),
            "https://blog.example.fr/t",
            "délai total par défaut",
            "30 secondes depuis la version 2.1",
        ),
        (
            "nav_heavy.html".into(),
            read("nav_heavy.html"),
            "https://api.example.com/s",
            "Retry-After 429",
            "Retry-After header in seconds",
        ),
        (
            "retry.html ×10".into(),
            enlarge(&retry, 10),
            "https://docs.example.com/docs/retry.html",
            "backoff_ms initial delay",
            "Initial delay, doubled at each attempt",
        ),
        (
            "retry.html ×50".into(),
            enlarge(&retry, 50),
            "https://docs.example.com/docs/retry.html",
            "backoff_ms initial delay",
            "Initial delay, doubled at each attempt",
        ),
    ];
    println!(
        "Token estimate: {}",
        cersei_types::tokens::ESTIMATION_METHOD
    );
    println!();
    println!("| page | query | HTML bytes / tok | Markdown chars / tok | passages chars / tok | kept vs HTML | extract ms | split ms | rank ms | extract allocs (MB) | reference kept |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for (name, html, url, query, needle) in &cases {
        let u = url::Url::parse(url).unwrap();
        let src = Source {
            url: &u,
            content_type: Some("text/html; charset=utf-8"),
            body: html.as_bytes(),
            truncated: false,
            declared_length: None,
        };
        let mut te = Vec::new();
        let mut ts = Vec::new();
        let mut tr = Vec::new();
        let mut allocs = (0, 0);
        let mut result = None;
        for i in 0..7 {
            let (a0, b0) = (
                ALLOCS.load(Ordering::Relaxed),
                BYTES.load(Ordering::Relaxed),
            );
            let t = Instant::now();
            let e = extract::extract(&src, &cfg.extract).unwrap();
            te.push(t.elapsed());
            if i == 0 {
                allocs = (
                    ALLOCS.load(Ordering::Relaxed) - a0,
                    BYTES.load(Ordering::Relaxed) - b0,
                );
            }
            let t = Instant::now();
            let ps = chunk::split(&e.markdown, cfg.passages.min_chars, cfg.passages.max_chars);
            ts.push(t.elapsed());
            let t = Instant::now();
            let sel = rank::select(query, std::slice::from_ref(&ps), &cfg.passages);
            tr.push(t.elapsed());
            result = Some((e, ps, sel));
        }
        let (e, ps, sel) = result.unwrap();
        let picked: String = sel
            .picked
            .iter()
            .map(|p| ps[p.scored.passage].text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let kept = picked.contains(needle);
        let pct = 100.0 * (1.0 - tokens(&picked) as f64 / tokens(html).max(1) as f64);
        println!(
            "| {name} | {query} | {} / {} | {} / {} | {} / {} | −{pct:.1} % | {} | {} | {} | {} allocs, {:.1} | {} |",
            html.len(),
            tokens(html),
            e.markdown.chars().count(),
            tokens(&e.markdown),
            picked.chars().count(),
            tokens(&picked),
            ms(median(te)),
            ms(median(ts)),
            ms(median(tr)),
            allocs.0,
            allocs.1 as f64 / 1e6,
            if kept { "yes" } else { "**no**" }
        );
    }

    // The download stage over loopback (pipeline overhead, not Internet
    // latency): five pages, concurrency 5, two per host, 50 ms server delay.
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let page = Arc::new(retry.clone());
        let server = TestServer::start(move |_| {
            let page = page.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Reply::html((*page).clone())
            }
        })
        .await;
        let mut c = cfg.fetch.clone();
        c.allow_private = vec!["127.0.0.1".into()];
        let f = Arc::new(Fetcher::new(c).unwrap());
        let urls: Vec<String> = (0..5).map(|i| server.url(&format!("/{i}"))).collect();
        let t = Instant::now();
        let out = f.fetch_many(&urls).await;
        let elapsed = t.elapsed();
        println!();
        println!(
            "Download stage (loopback, 5 pages × 50 ms server delay, 2 per host): {} ms total, {} ok, max in flight on the host: {}",
            ms(elapsed),
            out.iter().filter(|r| r.is_ok()).count(),
            server.max_in_flight()
        );
    });
}
