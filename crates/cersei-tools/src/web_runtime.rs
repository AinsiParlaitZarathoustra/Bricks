//! The [`cersei_web::WebContext`] the web tools use.
//!
//! The agent installs one per session in `ToolContext::extensions`
//! ([`WebRuntime`]): configuration from `bricks.toml` `[web]`, documents
//! stored with the session. Without it, the tools use defaults (DuckDuckGo,
//! default limits) and a store under the system temporary directory, one per
//! session id.

use crate::ToolContext;
use cersei_web::store::WebStore;
use cersei_web::{WebConfig, WebContext};
use std::sync::Arc;

/// Installed in `ToolContext::extensions` by the agent.
#[derive(Clone)]
pub struct WebRuntime(pub Arc<WebContext>);

static DEFAULTS: once_cell::sync::Lazy<dashmap::DashMap<String, Arc<WebContext>>> =
    once_cell::sync::Lazy::new(dashmap::DashMap::new);

/// The session's web context.
pub fn context(ctx: &ToolContext) -> Result<Arc<WebContext>, String> {
    if let Some(rt) = ctx.extensions.get::<WebRuntime>() {
        return Ok(rt.0.clone());
    }
    if let Some(c) = DEFAULTS.get(&ctx.session_id) {
        return Ok(c.clone());
    }
    let config = WebConfig::default();
    let dir = std::env::temp_dir()
        .join("bricks")
        .join("web")
        .join(sanitize(&ctx.session_id));
    let store = Arc::new(WebStore::open(dir, config.store.max_session_bytes));
    let c = Arc::new(WebContext::new(config, Some(store))?);
    DEFAULTS.insert(ctx.session_id.clone(), c.clone());
    Ok(c)
}

fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if out.is_empty() {
        "session".into()
    } else {
        out
    }
}

/// `1234567` → `1 234 567`.
pub(crate) fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}
