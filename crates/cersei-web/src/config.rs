//! `[web]` settings of `bricks.toml`: search providers, fetch limits, passage
//! selection and session storage.
//!
//! Without a `[web]` section the defaults apply: DuckDuckGo HTML search (no
//! key), five pages, global concurrency 5, two downloads per host, 4 s per
//! page, 1 500 000 decoded bytes per page. A provider is used only when the
//! file names it; an API key is read only from the environment variable the
//! file names for it (`api_key_env`) — the presence of some variable never
//! selects a provider. Keys are kept in [`Secret`], which never prints.

use serde::Deserialize;
use std::fmt;
use std::time::Duration;
use url::Url;

/// A search backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[serde(alias = "ddg")]
    DuckDuckGo,
    Brave,
    Tavily,
    Exa,
}

impl ProviderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::Brave => "brave",
            Self::Tavily => "tavily",
            Self::Exa => "exa",
        }
    }

    pub fn needs_key(self) -> bool {
        !matches!(self, Self::DuckDuckGo)
    }

    fn default_endpoint(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "https://html.duckduckgo.com/html/",
            Self::Brave => "https://api.search.brave.com/res/v1/web/search",
            Self::Tavily => "https://api.tavily.com/search",
            Self::Exa => "https://api.exa.ai/search",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An API key. `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one header that carries it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// One provider's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub endpoint: Url,
    /// Name of the environment variable holding the key.
    pub api_key_env: Option<String>,
    /// Resolved at load; `None` when the variable is unset or empty.
    pub api_key: Option<Secret>,
    /// Provider-specific option: Tavily `search_depth`, Exa `type`, Brave
    /// `country`. Passed through as given.
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchConfig {
    /// Tried first.
    pub provider: ProviderKind,
    /// Tried in order after `provider` fails in a way that allows it.
    pub fallback: Vec<ProviderKind>,
    /// Provider requests in one search, retries included.
    pub max_attempts: usize,
    /// One provider request.
    pub timeout: Duration,
    /// Whole search, fallbacks included.
    pub budget: Duration,
    /// Longest wait honoured for a `Retry-After` / rate-limit reset.
    pub max_retry_wait: Duration,
    pub providers: Vec<ProviderConfig>,
}

impl SearchConfig {
    pub fn provider(&self, kind: ProviderKind) -> &ProviderConfig {
        self.providers
            .iter()
            .find(|p| p.kind == kind)
            .expect("every provider has a configuration")
    }

    /// `provider` then `fallback`, without repeats.
    pub fn chain(&self) -> Vec<ProviderKind> {
        let mut out = vec![self.provider];
        for k in &self.fallback {
            if !out.contains(k) {
                out.push(*k);
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FetchConfig {
    /// Pages read after a search.
    pub max_pages: usize,
    pub concurrency: usize,
    pub per_host: usize,
    /// Connection, redirects and body of one page.
    pub page_timeout: Duration,
    /// Decoded (decompressed) bytes kept per page.
    pub max_page_bytes: usize,
    /// Decoded bytes of all pages of one call.
    pub max_total_bytes: usize,
    pub max_redirects: usize,
    /// All downloads of one call.
    pub budget: Duration,
    /// Hosts allowed even though they are private or local (`host` or
    /// `host:port`). Empty by default: such addresses are refused.
    pub allow_private: Vec<String>,
    pub user_agent: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractConfig {
    /// Element count above which an HTML page is not parsed.
    pub max_elements: usize,
    /// Pages parsed at the same time (blocking threads).
    pub concurrency: usize,
    /// Below this many characters an extraction is not trusted by itself.
    pub min_reliable_chars: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PassageConfig {
    pub min_chars: usize,
    pub max_chars: usize,
    pub max_passages: usize,
    /// Characters of passages in one result.
    pub budget_chars: usize,
    pub per_source: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoreConfig {
    /// Bytes of documents kept for one session (raw and extracted).
    pub max_session_bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebConfig {
    pub search: SearchConfig,
    pub fetch: FetchConfig,
    pub extract: ExtractConfig,
    pub passages: PassageConfig,
    pub store: StoreConfig,
}

pub const DEFAULT_USER_AGENT: &str = concat!(
    "Bricks/",
    env!("CARGO_PKG_VERSION"),
    " (cersei-web; +https://github.com/pacifio/cersei)"
);

impl Default for WebConfig {
    fn default() -> Self {
        let providers = [
            ProviderKind::DuckDuckGo,
            ProviderKind::Brave,
            ProviderKind::Tavily,
            ProviderKind::Exa,
        ]
        .into_iter()
        .map(|kind| ProviderConfig {
            kind,
            endpoint: Url::parse(kind.default_endpoint()).expect("valid default endpoint"),
            api_key_env: None,
            api_key: None,
            mode: None,
        })
        .collect();
        Self {
            search: SearchConfig {
                provider: ProviderKind::DuckDuckGo,
                fallback: Vec::new(),
                max_attempts: 3,
                timeout: Duration::from_secs(5),
                budget: Duration::from_secs(12),
                max_retry_wait: Duration::from_secs(2),
                providers,
            },
            fetch: FetchConfig {
                max_pages: 5,
                concurrency: 5,
                per_host: 2,
                page_timeout: Duration::from_secs(4),
                max_page_bytes: 1_500_000,
                max_total_bytes: 8_000_000,
                max_redirects: 5,
                budget: Duration::from_secs(12),
                allow_private: Vec::new(),
                user_agent: DEFAULT_USER_AGENT.to_string(),
            },
            extract: ExtractConfig {
                max_elements: 60_000,
                concurrency: 2,
                min_reliable_chars: 400,
            },
            passages: PassageConfig {
                min_chars: 500,
                max_chars: 1500,
                max_passages: 5,
                budget_chars: 6000,
                per_source: 2,
            },
            store: StoreConfig {
                max_session_bytes: 64 * 1024 * 1024,
            },
        }
    }
}

// ─── bricks.toml ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct FileRoot {
    web: Option<FileWeb>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileWeb {
    search: Option<FileSearch>,
    fetch: Option<FileFetch>,
    extract: Option<FileExtract>,
    passages: Option<FilePassages>,
    store: Option<FileStore>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileSearch {
    provider: Option<ProviderKind>,
    fallback: Option<Vec<ProviderKind>>,
    max_attempts: Option<usize>,
    timeout_ms: Option<u64>,
    budget_ms: Option<u64>,
    max_retry_wait_ms: Option<u64>,
    duckduckgo: Option<FileProvider>,
    brave: Option<FileProvider>,
    tavily: Option<FileProvider>,
    exa: Option<FileProvider>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileProvider {
    endpoint: Option<String>,
    api_key_env: Option<String>,
    mode: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileFetch {
    max_pages: Option<usize>,
    concurrency: Option<usize>,
    per_host: Option<usize>,
    page_timeout_ms: Option<u64>,
    max_page_bytes: Option<usize>,
    max_total_bytes: Option<usize>,
    max_redirects: Option<usize>,
    budget_ms: Option<u64>,
    allow_private: Option<Vec<String>>,
    user_agent: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileExtract {
    max_elements: Option<usize>,
    concurrency: Option<usize>,
    min_reliable_chars: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FilePassages {
    min_chars: Option<usize>,
    max_chars: Option<usize>,
    max_passages: Option<usize>,
    budget_chars: Option<usize>,
    per_source: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileStore {
    max_session_bytes: Option<u64>,
}

/// A loaded configuration and what was noticed while loading it.
#[derive(Debug, Clone)]
pub struct LoadedWebConfig {
    pub config: WebConfig,
    /// Non-fatal findings (a key variable that is not set…). Never contains a
    /// key value.
    pub diagnostics: Vec<String>,
}

impl WebConfig {
    /// Read the `[web]` section of a `bricks.toml` text, resolving keys from
    /// the process environment.
    pub fn from_bricks_toml(text: &str) -> Result<LoadedWebConfig, String> {
        Self::from_bricks_toml_with_env(text, |name| std::env::var(name).ok())
    }

    /// Same, with an explicit environment (tests).
    pub fn from_bricks_toml_with_env(
        text: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<LoadedWebConfig, String> {
        // Other sections of the file are not ours to judge: only `[web]` is
        // read, strictly.
        let root = web_only(text)?;
        let mut c = Self::default();
        let mut diagnostics = Vec::new();
        let Some(web) = root.web else {
            return Ok(LoadedWebConfig {
                config: c,
                diagnostics,
            });
        };
        if let Some(s) = web.search {
            if let Some(p) = s.provider {
                c.search.provider = p;
            }
            if let Some(f) = s.fallback {
                c.search.fallback = f;
            }
            set(&mut c.search.max_attempts, s.max_attempts);
            set_ms(&mut c.search.timeout, s.timeout_ms);
            set_ms(&mut c.search.budget, s.budget_ms);
            set_ms(&mut c.search.max_retry_wait, s.max_retry_wait_ms);
            for (kind, fp) in [
                (ProviderKind::DuckDuckGo, s.duckduckgo),
                (ProviderKind::Brave, s.brave),
                (ProviderKind::Tavily, s.tavily),
                (ProviderKind::Exa, s.exa),
            ] {
                let Some(fp) = fp else { continue };
                let pc = c
                    .search
                    .providers
                    .iter_mut()
                    .find(|p| p.kind == kind)
                    .expect("all providers present");
                if let Some(e) = fp.endpoint {
                    pc.endpoint = Url::parse(&e)
                        .map_err(|err| format!("web.search.{kind}.endpoint: {err}"))?;
                }
                pc.api_key_env = fp.api_key_env;
                pc.mode = fp.mode;
            }
        }
        if let Some(f) = web.fetch {
            set(&mut c.fetch.max_pages, f.max_pages);
            set(&mut c.fetch.concurrency, f.concurrency);
            set(&mut c.fetch.per_host, f.per_host);
            set_ms(&mut c.fetch.page_timeout, f.page_timeout_ms);
            set(&mut c.fetch.max_page_bytes, f.max_page_bytes);
            set(&mut c.fetch.max_total_bytes, f.max_total_bytes);
            set(&mut c.fetch.max_redirects, f.max_redirects);
            set_ms(&mut c.fetch.budget, f.budget_ms);
            if let Some(a) = f.allow_private {
                c.fetch.allow_private = a;
            }
            if let Some(ua) = f.user_agent {
                c.fetch.user_agent = ua;
            }
        }
        if let Some(e) = web.extract {
            set(&mut c.extract.max_elements, e.max_elements);
            set(&mut c.extract.concurrency, e.concurrency);
            set(&mut c.extract.min_reliable_chars, e.min_reliable_chars);
        }
        if let Some(p) = web.passages {
            set(&mut c.passages.min_chars, p.min_chars);
            set(&mut c.passages.max_chars, p.max_chars);
            set(&mut c.passages.max_passages, p.max_passages);
            set(&mut c.passages.budget_chars, p.budget_chars);
            set(&mut c.passages.per_source, p.per_source);
        }
        if let Some(s) = web.store {
            if let Some(v) = s.max_session_bytes {
                c.store.max_session_bytes = v;
            }
        }
        c.validate()?;
        c.resolve_keys(&env, &mut diagnostics);
        Ok(LoadedWebConfig {
            config: c,
            diagnostics,
        })
    }

    /// Check bounds and endpoints. Keys are not required here (see
    /// [`Self::resolve_keys`]).
    pub fn validate(&self) -> Result<(), String> {
        let s = &self.search;
        positive("web.search.max_attempts", s.max_attempts, 10)?;
        duration("web.search.timeout_ms", s.timeout, 60_000)?;
        duration("web.search.budget_ms", s.budget, 120_000)?;
        if s.max_retry_wait > s.budget {
            return Err("web.search.max_retry_wait_ms must not exceed budget_ms".into());
        }
        for (i, k) in s.fallback.iter().enumerate() {
            if s.fallback[..i].contains(k) {
                return Err(format!("web.search.fallback lists {k} twice"));
            }
        }
        for p in &s.providers {
            check_endpoint(p)?;
            if let Some(var) = &p.api_key_env {
                if var.is_empty()
                    || !var
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    return Err(format!(
                        "web.search.{}.api_key_env must be an environment variable name \
                         (A-Z, 0-9, _), not a key",
                        p.kind
                    ));
                }
            }
        }
        for k in s.chain() {
            if k.needs_key() && s.provider(k).api_key_env.is_none() {
                return Err(format!(
                    "web.search: {k} is used but web.search.{k}.api_key_env is not set"
                ));
            }
        }
        let f = &self.fetch;
        positive("web.fetch.max_pages", f.max_pages, 20)?;
        positive("web.fetch.concurrency", f.concurrency, 32)?;
        positive("web.fetch.per_host", f.per_host, f.concurrency)?;
        duration("web.fetch.page_timeout_ms", f.page_timeout, 60_000)?;
        positive("web.fetch.max_page_bytes", f.max_page_bytes, 50_000_000)?;
        positive("web.fetch.max_total_bytes", f.max_total_bytes, 200_000_000)?;
        if f.max_total_bytes < f.max_page_bytes {
            return Err("web.fetch.max_total_bytes must be at least max_page_bytes".into());
        }
        if f.max_redirects > 20 {
            return Err("web.fetch.max_redirects must be at most 20".into());
        }
        duration("web.fetch.budget_ms", f.budget, 300_000)?;
        if f.user_agent.trim().is_empty() || f.user_agent.chars().any(|c| c.is_control()) {
            return Err("web.fetch.user_agent must be a non-empty printable string".into());
        }
        let e = &self.extract;
        positive("web.extract.max_elements", e.max_elements, 2_000_000)?;
        positive("web.extract.concurrency", e.concurrency, 16)?;
        let p = &self.passages;
        positive("web.passages.min_chars", p.min_chars, 10_000)?;
        positive("web.passages.max_chars", p.max_chars, 20_000)?;
        if p.min_chars >= p.max_chars {
            return Err("web.passages.min_chars must be below max_chars".into());
        }
        positive("web.passages.max_passages", p.max_passages, 20)?;
        positive("web.passages.per_source", p.per_source, 20)?;
        if p.budget_chars < p.max_chars {
            return Err("web.passages.budget_chars must be at least max_chars".into());
        }
        if self.store.max_session_bytes < f.max_page_bytes as u64 * 2 {
            return Err("web.store.max_session_bytes must hold at least two pages".into());
        }
        Ok(())
    }

    /// Read the key of every provider that names a variable. A missing key is
    /// a diagnostic (the provider is then reported unavailable when tried),
    /// never a silent switch to another provider.
    pub fn resolve_keys(
        &mut self,
        env: &dyn Fn(&str) -> Option<String>,
        diagnostics: &mut Vec<String>,
    ) {
        let chain = self.search.chain();
        for p in &mut self.search.providers {
            let Some(var) = &p.api_key_env else { continue };
            p.api_key = env(var).filter(|v| !v.trim().is_empty()).map(Secret::new);
            if p.api_key.is_none() && chain.contains(&p.kind) {
                diagnostics.push(format!(
                    "web.search.{}: environment variable {var} is not set; {} will be reported \
                     unavailable",
                    p.kind, p.kind
                ));
            }
        }
    }
}

fn web_only(text: &str) -> Result<FileRoot, String> {
    let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    match value.get("web") {
        None => Ok(FileRoot::default()),
        Some(w) => {
            let web: FileWeb = w
                .clone()
                .try_into()
                .map_err(|e: toml::de::Error| format!("[web]: {}", e.to_string().trim()))?;
            Ok(FileRoot { web: Some(web) })
        }
    }
}

fn set<T>(slot: &mut T, v: Option<T>) {
    if let Some(v) = v {
        *slot = v;
    }
}

fn set_ms(slot: &mut Duration, v: Option<u64>) {
    if let Some(ms) = v {
        *slot = Duration::from_millis(ms);
    }
}

fn positive(name: &str, v: usize, max: usize) -> Result<(), String> {
    if v == 0 || v > max {
        return Err(format!("{name} must be between 1 and {max} (got {v})"));
    }
    Ok(())
}

fn duration(name: &str, d: Duration, max_ms: u128) -> Result<(), String> {
    let ms = d.as_millis();
    if ms < 100 || ms > max_ms {
        return Err(format!(
            "{name} must be between 100 and {max_ms} ms (got {ms})"
        ));
    }
    Ok(())
}

/// HTTPS, or plain HTTP to a loopback address (local proxies, tests).
fn check_endpoint(p: &ProviderConfig) -> Result<(), String> {
    let u = &p.endpoint;
    let loopback = matches!(u.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(u.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        || u.host_str() == Some("localhost");
    match u.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => {
            return Err(format!(
                "web.search.{}.endpoint must use https (plain http only to a loopback address)",
                p.kind
            ))
        }
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(format!(
            "web.search.{}.endpoint must not carry credentials",
            p.kind
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_without_a_section() {
        let l = WebConfig::from_bricks_toml_with_env("[context]\nx = 1\n", env_none).unwrap();
        assert_eq!(l.config, WebConfig::default());
        assert_eq!(l.config.search.chain(), vec![ProviderKind::DuckDuckGo]);
        let f = &l.config.fetch;
        assert_eq!(
            (f.max_pages, f.concurrency, f.per_host, f.max_page_bytes),
            (5, 5, 2, 1_500_000)
        );
        assert_eq!(f.page_timeout, Duration::from_secs(4));
    }

    #[test]
    fn a_provider_with_fallback_and_its_key() {
        let toml = r#"
[web.search]
provider = "brave"
fallback = ["tavily", "duckduckgo"]
[web.search.brave]
api_key_env = "BRAVE_KEY"
[web.search.tavily]
api_key_env = "TAVILY_KEY"
mode = "advanced"
"#;
        let l = WebConfig::from_bricks_toml_with_env(toml, |v| {
            (v == "BRAVE_KEY").then(|| "sk-very-secret".to_string())
        })
        .unwrap();
        let s = &l.config.search;
        assert_eq!(
            s.chain(),
            vec![
                ProviderKind::Brave,
                ProviderKind::Tavily,
                ProviderKind::DuckDuckGo
            ]
        );
        assert!(s.provider(ProviderKind::Brave).api_key.is_some());
        assert!(s.provider(ProviderKind::Tavily).api_key.is_none());
        assert_eq!(l.diagnostics.len(), 1);
        assert!(l.diagnostics[0].contains("TAVILY_KEY"));
        // The key is never printed.
        let dump = format!("{:?} {:?}", l.config, l.diagnostics);
        assert!(!dump.contains("sk-very-secret"), "{dump}");
    }

    #[test]
    fn a_key_in_the_file_is_refused() {
        let toml =
            "[web.search]\nprovider = \"exa\"\n[web.search.exa]\napi_key_env = \"sk-123abc\"\n";
        let e = WebConfig::from_bricks_toml_with_env(toml, env_none).unwrap_err();
        assert!(e.contains("environment variable name"), "{e}");
        assert!(!e.contains("sk-123abc"));
    }

    #[test]
    fn invalid_values_are_reported() {
        for (toml, needle) in [
            ("[web.search]\nprovider = \"brave\"\n", "api_key_env is not set"),
            ("[web.fetch]\nper_host = 9\n", "per_host"),
            ("[web.fetch]\nmax_page_bytes = 0\n", "max_page_bytes"),
            ("[web.passages]\nmin_chars = 2000\n", "min_chars"),
            (
                "[web.search.brave]\nendpoint = \"http://search.example.com/\"\napi_key_env = \"K\"\n",
                "https",
            ),
            ("[web.search]\nfallback = [\"exa\", \"exa\"]\n", "twice"),
            ("[web.fetch]\nbogus = 1\n", "unknown field"),
        ] {
            let e = WebConfig::from_bricks_toml_with_env(toml, env_none).unwrap_err();
            assert!(e.contains(needle), "{toml}: {e}");
        }
    }

    #[test]
    fn loopback_http_endpoints_are_allowed_for_local_servers() {
        let toml = "[web.search]\nprovider = \"tavily\"\n[web.search.tavily]\napi_key_env = \"T\"\nendpoint = \"http://127.0.0.1:9/search\"\n";
        assert!(WebConfig::from_bricks_toml_with_env(toml, env_none).is_ok());
    }
}
