//! Downloaded bytes → Markdown.
//!
//! * **Decoding**: the `charset` of `Content-Type`, else a BOM, else (HTML) a
//!   `<meta charset>` in the first bytes, else UTF-8 — any label
//!   `encoding_rs` knows. An unknown label is reported as an unsupported
//!   encoding, not guessed.
//! * **Kinds**: HTML/XHTML, Markdown, plain text and JSON are read; PDF and
//!   other binary content are reported with their type and the size actually
//!   received (or announced, said as such), never shown.
//! * **HTML** is extracted two ways: Readability (`dom_smoothie`, good on
//!   articles) and a conservative pass over `main`/`article`/`[role=main]`
//!   (or `body`) with scripts, styles and navigation chrome removed — better
//!   on documentation and reference pages, whose tables and code Readability
//!   may judge peripheral. The richer of the two is kept and the choice,
//!   with both sizes, is reported. Both serialise through the same DOM
//!   library's Markdown writer; links and images are resolved against the
//!   final URL.
//! * Pages that are only a JavaScript shell are reported as such; a very
//!   short extraction is flagged as possibly incomplete.
//!
//! Parsing runs on blocking threads, at most `concurrency` at once, on input
//! already bounded by the download limit and an element-count cap — the
//! bound that guarantees it ends: a started blocking task is not interrupted
//! by an async timeout or an abort.

use crate::config::ExtractConfig;
use std::sync::Arc;
use tokio::sync::Semaphore;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    Html,
    Markdown,
    Text,
    Json,
}

/// How the Markdown was obtained.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "method")]
pub enum Strategy {
    /// Readability's main content.
    Readability,
    /// The named container (`main`, `article`, `[role=main]`, `body`…).
    Conservative { root: String },
    /// Served as Markdown, text or JSON: taken as is.
    AsServed,
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Readability => f.write_str("Readability (main content)"),
            Self::Conservative { root } => write!(f, "conservative extraction of <{root}>"),
            Self::AsServed => f.write_str("as served"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Extracted {
    pub title: String,
    pub markdown: String,
    pub kind: DocKind,
    pub strategy: Strategy,
    pub charset: String,
    /// Findings worth showing: losses, short extraction, decoding fallback.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Unreadable {
    /// PDF, image, archive… `bytes` received (maybe truncated).
    Binary {
        mime: String,
        bytes: usize,
        truncated: bool,
        declared: Option<u64>,
    },
    /// A charset label nobody knows.
    UnsupportedEncoding { label: String },
    /// The page needs JavaScript to show its content.
    NeedsJavascript { title: String, chars: usize },
    /// Nothing readable was found.
    Empty,
    /// More elements than `max_elements`: not parsed.
    TooLarge { elements: usize, limit: usize },
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Binary {
                mime,
                bytes,
                truncated,
                declared,
            } => {
                write!(f, "unsupported content type {mime}; ")?;
                if *truncated {
                    write!(f, "{bytes} bytes received before the size limit")?;
                } else {
                    write!(f, "{bytes} bytes received")?;
                }
                if let Some(d) = declared {
                    write!(f, " (announced length: {d} bytes)")?;
                }
                Ok(())
            }
            Self::UnsupportedEncoding { label } => {
                write!(f, "unsupported text encoding `{label}`")
            }
            Self::NeedsJavascript { title, chars } => write!(
                f,
                "the page \"{title}\" needs JavaScript to render its content; only {chars} \
                 characters were readable without it"
            ),
            Self::Empty => f.write_str("no readable content was found"),
            Self::TooLarge { elements, limit } => write!(
                f,
                "the page has about {elements} elements (limit {limit}); it was not parsed"
            ),
        }
    }
}

/// The input of an extraction.
#[derive(Debug, Clone)]
pub struct Source<'a> {
    pub url: &'a Url,
    pub content_type: Option<&'a str>,
    pub body: &'a [u8],
    pub truncated: bool,
    pub declared_length: Option<u64>,
}

/// Runs extractions on blocking threads, bounded.
pub struct Extractor {
    cfg: ExtractConfig,
    slots: Arc<Semaphore>,
}

impl Extractor {
    pub fn new(cfg: ExtractConfig) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(cfg.concurrency)),
            cfg,
        }
    }

    pub fn config(&self) -> &ExtractConfig {
        &self.cfg
    }

    pub async fn extract(
        &self,
        url: Url,
        content_type: Option<String>,
        body: Vec<u8>,
        truncated: bool,
        declared_length: Option<u64>,
    ) -> Result<Extracted, Unreadable> {
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore open");
        let cfg = self.cfg.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            extract(
                &Source {
                    url: &url,
                    content_type: content_type.as_deref(),
                    body: &body,
                    truncated,
                    declared_length,
                },
                &cfg,
            )
        })
        .await
        .unwrap_or(Err(Unreadable::Empty))
    }
}

/// Extract synchronously (the caller bounds the input).
pub fn extract(src: &Source<'_>, cfg: &ExtractConfig) -> Result<Extracted, Unreadable> {
    let mime = src
        .content_type
        .and_then(|ct| ct.split(';').next())
        .map(|m| m.trim().to_ascii_lowercase());
    let kind = match classify(mime.as_deref(), src.body) {
        Ok(k) => k,
        Err(mime) => {
            return Err(Unreadable::Binary {
                mime,
                bytes: src.body.len(),
                truncated: src.truncated,
                declared: src.declared_length,
            })
        }
    };
    let (text, charset, mut notes) = decode(src.body, src.content_type, kind == DocKind::Html)?;
    if src.truncated {
        notes.push(
            "the download stopped at the size limit: this is the beginning of the page, not \
             the whole page"
                .into(),
        );
    }
    let mut out = match kind {
        DocKind::Html => html(&text, src.url, cfg, notes)?,
        DocKind::Json => {
            if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                notes.push(if src.truncated {
                    "the JSON document is incomplete (truncated download)".into()
                } else {
                    "the content is not valid JSON; shown as text".into()
                });
            }
            plain(text, DocKind::Json, src.url, notes)
        }
        k => plain(text, k, src.url, notes),
    };
    out.charset = charset;
    if out.markdown.trim().is_empty() {
        return Err(Unreadable::Empty);
    }
    Ok(out)
}

/// `Ok(kind)` for readable content, `Err(mime)` for binary content.
fn classify(mime: Option<&str>, body: &[u8]) -> Result<DocKind, String> {
    match mime {
        Some("text/html" | "application/xhtml+xml") => return Ok(DocKind::Html),
        Some("text/markdown" | "text/x-markdown") => return Ok(DocKind::Markdown),
        Some(m) if m == "application/json" || m.ends_with("+json") || m == "text/json" => {
            return Ok(DocKind::Json)
        }
        Some(m) if m.starts_with("text/") => return Ok(DocKind::Text),
        Some(m)
            if m.starts_with("image/")
                || m.starts_with("audio/")
                || m.starts_with("video/")
                || m.starts_with("font/")
                || m == "application/pdf"
                || m == "application/zip"
                || m == "application/gzip" =>
        {
            return Err(m.to_string())
        }
        _ => {}
    }
    // Unknown or generic type: sniff.
    let head = &body[..body.len().min(1024)];
    let magic: &[(&[u8], &str)] = &[
        (b"%PDF-", "application/pdf"),
        (b"\x89PNG", "image/png"),
        (b"GIF8", "image/gif"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"PK\x03\x04", "application/zip"),
        (b"\x1f\x8b", "application/gzip"),
    ];
    for (m, t) in magic {
        if head.starts_with(m) {
            return Err(t.to_string());
        }
    }
    let utf16 = head.starts_with(&[0xff, 0xfe]) || head.starts_with(&[0xfe, 0xff]);
    if !utf16 && head.contains(&0) {
        return Err(mime.unwrap_or("application/octet-stream").to_string());
    }
    let lower = String::from_utf8_lossy(head).to_ascii_lowercase();
    if lower.contains("<html") || lower.contains("<!doctype html") {
        return Ok(DocKind::Html);
    }
    let t = lower.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        return Ok(DocKind::Json);
    }
    match mime {
        None | Some("application/octet-stream") if !lower.is_empty() => Ok(DocKind::Text),
        Some(m) if m.starts_with("application/") && !m.contains("xml") => Err(m.to_string()),
        _ => Ok(DocKind::Text),
    }
}

fn decode(
    body: &[u8],
    content_type: Option<&str>,
    html: bool,
) -> Result<(String, String, Vec<String>), Unreadable> {
    let mut notes = Vec::new();
    let declared = content_type.and_then(charset_param).or_else(|| {
        if html {
            meta_charset(&body[..body.len().min(4096)])
        } else {
            None
        }
    });
    let (enc, bom_len) = match encoding_rs::Encoding::for_bom(body) {
        Some((e, n)) => (Some(e), n),
        None => (None, 0),
    };
    let encoding = match (enc, &declared) {
        (Some(e), _) => e,
        (None, Some(label)) => {
            encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(|| {
                Unreadable::UnsupportedEncoding {
                    label: label.clone(),
                }
            })?
        }
        (None, None) => encoding_rs::UTF_8,
    };
    let (text, _, had_errors) = encoding.decode(&body[bom_len..]);
    let mut text = text.into_owned();
    let mut name = encoding.name().to_string();
    if had_errors {
        if declared.is_none() && encoding == encoding_rs::UTF_8 {
            // Undeclared and not UTF-8: the common legacy web encoding.
            let (t, _, _) = encoding_rs::WINDOWS_1252.decode(body);
            text = t.into_owned();
            name = "windows-1252".into();
            notes.push(
                "no charset was declared and the bytes are not UTF-8; decoded as windows-1252"
                    .into(),
            );
        } else {
            notes.push(format!(
                "some bytes are invalid in the declared encoding ({name}) and were replaced"
            ));
        }
    }
    Ok((text, name, notes))
}

fn charset_param(ct: &str) -> Option<String> {
    ct.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k.trim().eq_ignore_ascii_case("charset"))
            .then(|| v.trim().trim_matches('"').trim_matches('\'').to_string())
    })
}

fn meta_charset(head: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(head).to_ascii_lowercase();
    let i = s.find("charset=")?;
    let rest = &s[i + 8..];
    let v: String = rest
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
        .collect();
    (!v.is_empty()).then_some(v)
}

fn plain(text: String, kind: DocKind, url: &Url, notes: Vec<String>) -> Extracted {
    let title = match kind {
        DocKind::Markdown => text
            .lines()
            .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
            .unwrap_or_else(|| last_segment(url)),
        _ => last_segment(url),
    };
    Extracted {
        title,
        markdown: text.replace("\r\n", "\n"),
        kind,
        strategy: Strategy::AsServed,
        charset: String::new(),
        notes,
    }
}

fn last_segment(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut s| s.next_back().map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| url.host_str().unwrap_or_default().to_string())
}

/// Tags removed before the conservative extraction.
const NOISE: &str = "script, style, noscript, template, svg, canvas, iframe, object, embed, \
                     form, button, input, select, nav, aside, footer, [role=navigation], \
                     [role=banner], [role=contentinfo], [aria-hidden=true], .sidebar, \
                     .navbar, .breadcrumb, .breadcrumbs, .toc, #toc, .cookie, .cookies";

fn html(
    text: &str,
    url: &Url,
    cfg: &ExtractConfig,
    mut notes: Vec<String>,
) -> Result<Extracted, Unreadable> {
    let elements = text.matches('<').count();
    if elements > cfg.max_elements * 2 {
        return Err(Unreadable::TooLarge {
            elements: elements / 2,
            limit: cfg.max_elements,
        });
    }
    let readability = {
        let rcfg = dom_smoothie::Config {
            text_mode: dom_smoothie::TextMode::Markdown,
            max_elements_to_parse: cfg.max_elements,
            ..Default::default()
        };
        dom_smoothie::Readability::new(text, Some(url.as_str()), Some(rcfg))
            .ok()
            .and_then(|mut r| r.parse().ok())
            .map(|a| (a.title.to_string(), tidy(&a.text_content)))
    };
    let (root, conservative, page_title) = conservative(text, url);
    let r_len = readability
        .as_ref()
        .map(|(_, m)| m.chars().count())
        .unwrap_or(0);
    let c_len = conservative.chars().count();
    let r_blocks = readability.as_ref().map(|(_, m)| blocks(m)).unwrap_or(0);
    let c_blocks = blocks(&conservative);
    // Readability is kept when it keeps the substance; the conservative
    // extraction when Readability failed, is very short, or dropped code
    // blocks and tables the page's main container has.
    let use_conservative = readability.is_none()
        || r_len < cfg.min_reliable_chars.min(c_len)
        || (root != "body" && c_blocks > r_blocks)
        || (root != "body" && r_len * 2 < c_len);
    let (title, markdown, strategy) = if use_conservative {
        if readability.is_some() {
            notes.push(format!(
                "Readability kept {r_len} characters; the <{root}> element was used instead \
                 ({c_len} characters{})",
                if c_blocks > r_blocks {
                    format!(", {c_blocks} code blocks/tables against {r_blocks}")
                } else {
                    String::new()
                }
            ));
        }
        let t = readability
            .as_ref()
            .map(|(t, _)| t.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or(page_title);
        (t, conservative, Strategy::Conservative { root })
    } else {
        let (t, m) = readability.expect("checked");
        if c_len > r_len * 3 / 2 {
            notes.push(format!(
                "Readability kept {r_len} of the {c_len} characters of <{root}>: navigation \
                 and peripheral sections were left out"
            ));
        }
        (t, m, Strategy::Readability)
    };
    let chars = markdown.chars().count();
    if chars < 200 && looks_like_js_shell(text) {
        return Err(Unreadable::NeedsJavascript { title, chars });
    }
    if chars == 0 {
        return Err(Unreadable::Empty);
    }
    if chars < cfg.min_reliable_chars {
        notes.push(format!(
            "short extraction ({chars} characters): the page may hold more than this"
        ));
    }
    let markdown = if markdown.trim_start().starts_with("# ") || title.is_empty() {
        markdown
    } else {
        format!("# {title}\n\n{markdown}")
    };
    Ok(Extracted {
        title,
        markdown,
        kind: DocKind::Html,
        strategy,
        charset: String::new(),
        notes,
    })
}

/// The main container as Markdown: `(root name, markdown, <title>)`.
fn conservative(text: &str, url: &Url) -> (String, String, String) {
    let doc = dom_query::Document::from(text);
    let page_title = doc
        .select("title")
        .text()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    doc.select(NOISE).remove();
    // A header inside the main container usually holds the page heading:
    // only top-level page headers are chrome.
    doc.select("body > header").remove();
    for (attr, sel) in [("href", "a[href]"), ("src", "img[src]")] {
        for node in doc.select(sel).iter() {
            if let Some(v) = node.attr(attr) {
                if let Ok(abs) = url.join(v.trim()) {
                    node.set_attr(attr, abs.as_str());
                }
            }
        }
    }
    for root in [
        "main",
        "[role=main]",
        "article",
        "#content",
        ".content",
        "body",
    ] {
        let sel = doc.select(root);
        if sel.length() == 0 {
            continue;
        }
        // Several articles (a listing): take the parent body instead.
        if root == "article" && sel.length() > 1 {
            continue;
        }
        let Some(node) = sel.nodes().first() else {
            continue;
        };
        let md = tidy(&node.md(None));
        if !md.trim().is_empty() {
            return (root.trim_matches(['[', ']']).to_string(), md, page_title);
        }
    }
    ("body".into(), String::new(), page_title)
}

/// Code fences and tables.
fn blocks(md: &str) -> usize {
    let mut n = 0;
    let mut in_table = false;
    for line in md.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            n += 1; // counted twice per block; compared like for like
        }
        let row = t.starts_with('|');
        if row && !in_table {
            n += 2;
        }
        in_table = row;
    }
    n
}

fn looks_like_js_shell(html: &str) -> bool {
    let l = html.to_ascii_lowercase();
    let scripts = l.matches("<script").count();
    let mounts = [
        "id=\"root\"",
        "id=\"app\"",
        "id=\"__next\"",
        "id=\"___gatsby\"",
        "ng-app",
    ]
    .iter()
    .any(|m| l.contains(m));
    let noscript_js = l.split("<noscript").skip(1).any(|s| {
        s.split("</noscript>")
            .next()
            .unwrap_or("")
            .contains("javascript")
    });
    noscript_js || (mounts && scripts > 0) || scripts >= 5
}

/// Remove defensive backslash escapes that only hurt reading and searching
/// (`v2\.1`), outside code; collapse runs of blank lines. Escapes that carry
/// meaning (`\*`, `\_`, `\|`, `\[`, a line-leading `\#`, `1\.`, `\-`) stay.
pub fn tidy(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut fence = false;
    let mut blank = 0;
    for line in md.lines() {
        let t = line.trim_end();
        if t.trim_start().starts_with("```") {
            fence = !fence;
        }
        if t.is_empty() && !fence {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        if fence {
            out.push_str(t);
        } else {
            out.push_str(&unescape_line(t));
        }
        out.push('\n');
    }
    out.trim().to_string() + "\n"
}

fn unescape_line(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    let first_text = chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            in_code = !in_code;
        }
        if c == '\\' && !in_code && i + 1 < chars.len() {
            let n = chars[i + 1];
            let leading = i == first_text;
            let after_digits =
                i > 0 && chars[first_text..i].iter().all(|d| d.is_ascii_digit()) && n == '.';
            let harmless = matches!(
                n,
                '.' | '!'
                    | '('
                    | ')'
                    | ':'
                    | ';'
                    | ','
                    | '?'
                    | '='
                    | '/'
                    | '&'
                    | '%'
                    | '@'
                    | '\''
                    | '"'
            ) || (matches!(n, '-' | '+' | '#' | '>') && !leading);
            if harmless && !after_digits && !(leading && n == '.') {
                out.push(n);
                i += 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ExtractConfig {
        crate::config::WebConfig::default().extract
    }

    fn run(ct: &str, body: &[u8]) -> Result<Extracted, Unreadable> {
        let url = Url::parse("https://docs.example.com/api/v2/guide.html").unwrap();
        extract(
            &Source {
                url: &url,
                content_type: Some(ct),
                body,
                truncated: false,
                declared_length: None,
            },
            &cfg(),
        )
    }

    #[test]
    fn escapes_that_only_hurt_reading_are_removed() {
        assert_eq!(tidy("Since v2\\.1 \\(beta\\)"), "Since v2.1 (beta)\n");
        assert_eq!(tidy("`a\\.b` and a\\-b"), "`a\\.b` and a-b\n");
        assert_eq!(
            tidy("1\\. not a list\n\\- item"),
            "1\\. not a list\n\\- item\n"
        );
        assert_eq!(tidy("\\*not bold\\* a\\|b"), "\\*not bold\\* a\\|b\n");
        assert_eq!(tidy("```\nx\\.y\n```"), "```\nx\\.y\n```\n");
        assert_eq!(tidy("a\n\n\n\nb"), "a\n\nb\n");
    }

    #[test]
    fn binary_and_pdf_are_reported_with_their_size() {
        match run("application/pdf", b"%PDF-1.7 ....") {
            Err(Unreadable::Binary { mime, bytes, .. }) => {
                assert_eq!(mime, "application/pdf");
                assert_eq!(bytes, 13);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            run("application/octet-stream", b"\x89PNG\r\n"),
            Err(Unreadable::Binary { .. })
        ));
    }

    #[test]
    fn encodings_are_decoded_or_reported() {
        let latin1 = b"<html><head><title>Caf\xe9</title></head><body><main><p>Un caf\xe9 cr\xe8me, s'il vous pla\xeet. Une phrase assez longue pour \xeatre lue.</p></main></body></html>";
        let e = run("text/html; charset=ISO-8859-1", latin1).unwrap();
        assert!(e.markdown.contains("café crème"), "{}", e.markdown);
        assert_eq!(e.charset, "windows-1252");
        assert!(matches!(
            run("text/plain; charset=x-klingon", b"abc"),
            Err(Unreadable::UnsupportedEncoding { .. })
        ));
        let bom = b"\xef\xbb\xbfhello \xc3\xa9t\xc3\xa9";
        assert_eq!(run("text/plain", bom).unwrap().markdown, "hello été");
    }

    #[test]
    fn javascript_shells_are_not_read_as_pages() {
        let html = br#"<html><head><title>App</title><script src="/a.js"></script></head>
            <body><div id="root"></div><noscript>You need to enable JavaScript to run this app.</noscript></body></html>"#;
        assert!(matches!(
            run("text/html", html),
            Err(Unreadable::NeedsJavascript { .. })
        ));
    }
}
