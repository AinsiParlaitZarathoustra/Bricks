//! Live checks against the public Internet. Not part of the ordinary test
//! run: they are `#[ignore]`d and also require `BRICKS_LIVE_WEB=1`.
//!
//! ```text
//! BRICKS_LIVE_WEB=1 cargo test -p cersei-web --test live_web -- --ignored --nocapture
//! ```
//!
//! Keyed providers are tried only when their key variable is set
//! (`BRAVE_SEARCH_API_KEY`, `TAVILY_API_KEY`, `EXA_API_KEY`); no fake key is
//! ever sent to a real API. A DuckDuckGo refusal or challenge is printed,
//! not treated as a bug: the HTML interface is not a contractual API.

use cersei_web::config::{ProviderKind, Secret, WebConfig};
use cersei_web::{ResearchOptions, WebContext};

fn live() -> bool {
    std::env::var("BRICKS_LIVE_WEB").as_deref() == Ok("1")
}

#[tokio::test]
#[ignore = "public network; set BRICKS_LIVE_WEB=1"]
async fn duckduckgo_search_and_read() {
    if !live() {
        return;
    }
    let ctx = WebContext::new(WebConfig::default(), None).unwrap();
    let r = ctx
        .research(
            "tokio JoinSet documentation",
            ResearchOptions {
                results: 5,
                read_pages: 3,
            },
        )
        .await;
    println!("attempts: {:?}", r.search.attempts);
    println!("timings: {:?}", r.timings);
    for p in &r.pages {
        match &p.outcome {
            Ok(d) => println!(
                "{} {} — {} chars, {}",
                p.source_id, d.entry.final_url, d.entry.markdown_chars, d.entry.strategy
            ),
            Err(e) => println!("{} {} — {e}", p.source_id, p.url),
        }
    }
    if let Some(e) = r.search.failure() {
        println!("DuckDuckGo did not answer usefully: {e}");
    }
}

#[tokio::test]
#[ignore = "public network and a real key; set BRICKS_LIVE_WEB=1 and a key variable"]
async fn keyed_providers_when_configured() {
    if !live() {
        return;
    }
    for (kind, var) in [
        (ProviderKind::Brave, "BRAVE_SEARCH_API_KEY"),
        (ProviderKind::Tavily, "TAVILY_API_KEY"),
        (ProviderKind::Exa, "EXA_API_KEY"),
    ] {
        let Ok(key) = std::env::var(var) else {
            println!("{kind}: {var} not set, skipped");
            continue;
        };
        let mut c = WebConfig::default();
        c.search.provider = kind;
        let p = c
            .search
            .providers
            .iter_mut()
            .find(|p| p.kind == kind)
            .unwrap();
        p.api_key_env = Some(var.into());
        p.api_key = Some(Secret::new(key));
        let ctx = WebContext::new(c, None).unwrap();
        let r = ctx
            .research(
                "rust reqwest streaming body limit",
                ResearchOptions {
                    results: 3,
                    read_pages: 0,
                },
            )
            .await;
        println!(
            "{kind}: {:?} — {} results",
            r.search.attempts,
            r.search.hits.len()
        );
    }
}
