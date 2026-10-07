//! A small Rust workspace and a scripted language server that resolves
//! `module::name` to `module.rs` (enough to tell homonyms apart).

#![allow(dead_code)]

use bricks_semantic::lsp::Launcher;
use bricks_semantic::{SemanticConfig, SemanticEngine};
use cersei_lsp::mock::{self, MockConfig, MockDocs, MockHandle};
use cersei_lsp::{LspClient, LspResult, LspServerConfig};
use futures::future::BoxFuture;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub fn workspace() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let w = |p: &str, t: &str| {
        let f = d.path().join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    };
    w("Cargo.toml", "[package]\nname = \"demo\"\n");
    w(
        "src/alpha.rs",
        "/// Alpha runner.\npub fn run() -> u32 {\n    1\n}\n",
    );
    w(
        "src/beta.rs",
        "/// Beta runner.\npub fn run() -> u32 {\n    2\n}\n",
    );
    w(
        "src/main.rs",
        "mod alpha;\nmod beta;\n\nfn main() {\n    let a = alpha::run();\n    let b = beta::run();\n    println!(\"{a}{b}\");\n}\n",
    );
    w(
        "src/uni.rs",
        "// é😀 accents\npub fn ünï() {}\nfn caller() { ünï(); }\r\n",
    );
    w("README.md", "run the demo: cargo run\n");
    d
}

pub fn canonical(d: &tempfile::TempDir) -> PathBuf {
    std::fs::canonicalize(d.path()).unwrap()
}

fn text_of(uri: &str, docs: &MockDocs) -> Option<String> {
    let uri = cersei_lsp::normalize_uri(uri);
    docs.get(&uri)
        .map(|(_, t)| t.clone())
        .or_else(|| std::fs::read_to_string(cersei_lsp::uri_to_path(&uri)).ok())
}

/// Byte offset of an LSP position (`utf16` or bytes).
fn offset(text: &str, line: u64, ch: u64, utf16: bool) -> usize {
    let mut start = 0;
    for _ in 0..line {
        start += text[start..]
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or(text.len() - start);
    }
    let mut units = 0u64;
    for (i, c) in text[start..].char_indices() {
        if units >= ch {
            return start + i;
        }
        units += if utf16 {
            c.len_utf16() as u64
        } else {
            c.len_utf8() as u64
        };
    }
    text.len()
}

fn position(text: &str, off: usize, utf16: bool) -> Value {
    let before = &text[..off];
    let line = before.matches('\n').count();
    let ls = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let ch: usize = text[ls..off]
        .chars()
        .map(|c| if utf16 { c.len_utf16() } else { c.len_utf8() })
        .sum();
    json!({"line": line, "character": ch})
}

fn word_at(text: &str, off: usize) -> (usize, usize) {
    let is_w = |c: char| c.is_alphanumeric() || c == '_';
    let s = text[..off]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_w(*c))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(off);
    let e = text[off..]
        .char_indices()
        .find(|(_, c)| !is_w(*c))
        .map(|(i, _)| off + i)
        .unwrap_or(text.len());
    (s, e)
}

fn loc(uri: &str, text: &str, s: usize, e: usize, utf16: bool) -> Value {
    json!({"uri": uri, "range": {"start": position(text, s, utf16), "end": position(text, e, utf16)}})
}

/// The scripted server's answers.
pub fn handler(root: PathBuf, utf16: bool) -> mock::Handler {
    Arc::new(move |method: &str, params: &Value, docs: &MockDocs| {
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let text = text_of(&uri, docs).unwrap_or_default();
        let p = &params["position"];
        let off = offset(
            &text,
            p["line"].as_u64().unwrap_or(0),
            p["character"].as_u64().unwrap_or(0),
            utf16,
        );
        let (s, e) = word_at(&text, off);
        let word = text[s..e].to_string();
        match method {
            "textDocument/definition" => {
                // `module::word` → module.rs; else this file.
                let target_uri = if text[..s].ends_with("::") {
                    let q = &text[..s - 2];
                    let (qs, _) = word_at(q, q.len());
                    cersei_lsp::path_to_uri(&root.join("src").join(format!("{}.rs", &q[qs..])))
                } else {
                    uri.clone()
                };
                let t = text_of(&target_uri, docs).unwrap_or_default();
                match t.find(&format!("fn {word}")) {
                    Some(i) => Ok(json!([loc(
                        &target_uri,
                        &t,
                        i + 3,
                        i + 3 + word.len(),
                        utf16
                    )])),
                    None => Ok(Value::Null),
                }
            }
            "textDocument/references" => {
                let mut out = Vec::new();
                let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("src"))
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .collect();
                files.sort();
                // References of `run` from `alpha::run` are the `alpha::run` uses.
                let qual = if text[..s].ends_with("::") {
                    let q = &text[..s - 2];
                    let (qs, _) = word_at(q, q.len());
                    Some(q[qs..].to_string())
                } else {
                    None
                };
                for f in files {
                    let u = cersei_lsp::path_to_uri(&f);
                    let t = text_of(&u, docs).unwrap_or_default();
                    for (i, _) in t.match_indices(&word) {
                        let is_def = t[..i].ends_with("fn ");
                        let q_ok = match &qual {
                            Some(q) => t[..i].ends_with(&format!("{q}::")),
                            None => true,
                        };
                        if !is_def && q_ok {
                            out.push(loc(&u, &t, i, i + word.len(), utf16));
                        }
                    }
                }
                Ok(Value::Array(out))
            }
            "workspace/symbol" => {
                let q = params["query"].as_str().unwrap_or("").to_string();
                let mut out = Vec::new();
                let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("src"))
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .collect();
                files.sort();
                for f in files {
                    let u = cersei_lsp::path_to_uri(&f);
                    let t = text_of(&u, docs).unwrap_or_default();
                    if let Some(i) = t.find(&format!("fn {q}(")) {
                        // The whole item's line, like SymbolInformation.
                        let ls = t[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
                        let le = t[i..].find('\n').map(|p| i + p).unwrap_or(t.len());
                        out.push(
                            json!({"name": q, "kind": 12, "location": loc(&u, &t, ls, le, utf16)}),
                        );
                    }
                }
                Ok(Value::Array(out))
            }
            "textDocument/documentSymbol" => {
                let out: Vec<Value> = text
                    .match_indices("fn ")
                    .map(|(i, _)| {
                        let (s, e) = word_at(&text, i + 3);
                        json!({"name": &text[s..e], "kind": 12,
                               "range": loc(&uri, &text, i, e, utf16)["range"],
                               "selectionRange": loc(&uri, &text, s, e, utf16)["range"]})
                    })
                    .collect();
                Ok(Value::Array(out))
            }
            "textDocument/hover" => {
                Ok(json!({"contents": {"kind": "markdown", "value": format!("fn {word}()")}}))
            }
            _ => Err((-32601, format!("unsupported {method}"))),
        }
    })
}

pub fn caps(utf16: bool) -> Value {
    json!({
        "positionEncoding": if utf16 { "utf-16" } else { "utf-8" },
        "textDocumentSync": {"openClose": true, "change": 1},
        "definitionProvider": true,
        "referencesProvider": true,
        "hoverProvider": true,
        "workspaceSymbolProvider": true,
        "documentSymbolProvider": true,
    })
}

/// Launches scripted servers, counting launches and keeping handles.
pub struct MockLauncher {
    pub config: Mutex<MockConfig>,
    pub launches: AtomicUsize,
    pub handles: Mutex<Vec<MockHandle>>,
    pub launch_delay: Duration,
    pub installed: bool,
}

impl MockLauncher {
    pub fn new(config: MockConfig) -> Arc<Self> {
        Arc::new(Self {
            config: Mutex::new(config),
            launches: AtomicUsize::new(0),
            handles: Mutex::new(Vec::new()),
            launch_delay: Duration::from_millis(0),
            installed: true,
        })
    }

    pub fn last(&self) -> MockHandle {
        self.handles
            .lock()
            .last()
            .cloned()
            .expect("a server was launched")
    }
}

impl Launcher for MockLauncher {
    fn check(&self, config: &LspServerConfig) -> Result<(), String> {
        if self.installed {
            Ok(())
        } else {
            Err(format!("`{}` is not installed", config.command))
        }
    }

    fn launch(
        &self,
        config: LspServerConfig,
        root: PathBuf,
        _timeout: Duration,
    ) -> BoxFuture<'static, LspResult<Arc<LspClient>>> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let cfg = self.config.lock().clone();
        let (handle, reader, writer) = mock::spawn(cfg);
        self.handles.lock().push(handle);
        let delay = self.launch_delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            let client = Arc::new(LspClient::new(config));
            client.connect(reader, writer, &root).await;
            client.initialize().await?;
            Ok(client)
        })
    }
}

pub fn engine_with(
    root: &Path,
    launcher: Arc<MockLauncher>,
    mut cfg: SemanticConfig,
) -> Arc<SemanticEngine> {
    // The scripted server never reports progress: no settle delay.
    cfg.lsp.startup_settle_ms = 0;
    SemanticEngine::with_launcher(root, cfg, launcher)
}

pub fn no_lsp() -> SemanticConfig {
    let mut c = SemanticConfig::default();
    c.lsp.enabled = false;
    c
}
