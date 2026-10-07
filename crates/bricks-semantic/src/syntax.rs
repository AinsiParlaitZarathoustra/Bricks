//! Tree-sitter: languages, a bounded tree cache with incremental reparse,
//! enclosing scopes and syntactic definitions.
//!
//! Parsers are owned per worker thread (`thread_local!`): a parser is never
//! shared. Trees are immutable once cached and shared as `Arc<Tree>`
//! (`Tree` is `Send + Sync`); an incremental reparse edits a *clone*.

use crate::view::Document;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tree_sitter::{InputEdit, Node, ParseOptions, Parser, Point, Tree};

/// Languages with a grammar. JavaScript files are parsed with the TSX
/// grammar of `tree-sitter-typescript`, a superset of JavaScript with JSX
/// (no separate JavaScript grammar is a dependency); `.ts` uses the
/// TypeScript grammar, whose `<T>expr` casts the TSX grammar rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
}

impl Lang {
    pub fn for_path(path: &Path) -> Option<Lang> {
        match path.extension()?.to_str()? {
            "rs" => Some(Lang::Rust),
            "ts" | "mts" | "cts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
            "py" | "pyi" => Some(Lang::Python),
            "go" => Some(Lang::Go),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::JavaScript => "javascript",
            Lang::Python => "python",
            Lang::Go => "go",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx | Lang::JavaScript => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }
}

thread_local! {
    static PARSERS: RefCell<HashMap<Lang, Parser>> = RefCell::new(HashMap::new());
}

/// Why a document has no tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseFailure {
    Unsupported,
    Deadline,
    Failed,
}

/// A parsed document version.
#[derive(Clone)]
pub struct Parsed {
    pub lang: Lang,
    pub tree: Arc<Tree>,
    pub text: Arc<str>,
    pub has_errors: bool,
}

struct Entry {
    hash: String,
    parsed: Parsed,
    used: u64,
}

/// Trees by path, at most one version per path (the latest parsed, kept
/// for incremental reparse), bounded by `capacity` entries (LRU).
pub struct TreeCache {
    entries: Mutex<HashMap<PathBuf, Entry>>,
    capacity: usize,
    clock: AtomicU64,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub incremental: AtomicU64,
}

impl TreeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity: capacity.max(1),
            clock: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            incremental: AtomicU64::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        self.entries.lock().clear();
    }

    /// Bytes of source held by cached trees (trees themselves are not
    /// measurable through the API; their size is of the same order).
    pub fn source_bytes(&self) -> u64 {
        self.entries
            .lock()
            .values()
            .map(|e| e.parsed.text.len() as u64)
            .sum()
    }

    /// The tree of `doc`, from the cache or parsed now (incrementally when
    /// an earlier version of the same path is cached). Returns whether it
    /// was a cache hit.
    pub fn parse(&self, doc: &Document, deadline: Instant) -> Result<(Parsed, bool), ParseFailure> {
        let lang = Lang::for_path(&doc.path).ok_or(ParseFailure::Unsupported)?;
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        let previous = {
            let mut entries = self.entries.lock();
            match entries.get_mut(&doc.path) {
                Some(e) if e.hash == doc.revision.hash && e.parsed.lang == lang => {
                    e.used = tick;
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    return Ok((e.parsed.clone(), true));
                }
                Some(e) if e.parsed.lang == lang => Some(e.parsed.clone()),
                _ => None,
            }
        };
        self.misses.fetch_add(1, Ordering::Relaxed);
        let old_tree = previous.as_ref().map(|p| {
            let mut t = (*p.tree).clone();
            t.edit(&edit_between(&p.text, &doc.text));
            t
        });
        if old_tree.is_some() {
            self.incremental.fetch_add(1, Ordering::Relaxed);
        }
        let tree = parse_text(lang, &doc.text, old_tree.as_ref(), deadline)?;
        let parsed = Parsed {
            lang,
            has_errors: tree.root_node().has_error(),
            tree: Arc::new(tree),
            text: Arc::clone(&doc.text),
        };
        let mut entries = self.entries.lock();
        if entries.len() >= self.capacity && !entries.contains_key(&doc.path) {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(p, _)| p.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            doc.path.clone(),
            Entry {
                hash: doc.revision.hash.clone(),
                parsed: parsed.clone(),
                used: tick,
            },
        );
        Ok((parsed, false))
    }
}

fn parse_text(
    lang: Lang,
    text: &str,
    old: Option<&Tree>,
    deadline: Instant,
) -> Result<Tree, ParseFailure> {
    PARSERS.with(|cell| {
        let mut parsers = cell.borrow_mut();
        let parser = match parsers.entry(lang) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let mut p = Parser::new();
                p.set_language(&lang.grammar())
                    .map_err(|_| ParseFailure::Failed)?;
                v.insert(p)
            }
        };
        let bytes = text.as_bytes();
        let len = bytes.len();
        let mut timed_out = false;
        let mut progress = |_: &tree_sitter::ParseState| {
            if Instant::now() >= deadline {
                timed_out = true;
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = ParseOptions::new().progress_callback(&mut progress);
        let tree = parser.parse_with_options(
            &mut |i, _| if i < len { &bytes[i..] } else { &[] },
            old,
            Some(options),
        );
        match tree {
            Some(t) => Ok(t),
            None => {
                // A cancelled parse leaves state behind: start clean next time.
                parser.reset();
                Err(if timed_out {
                    ParseFailure::Deadline
                } else {
                    ParseFailure::Failed
                })
            }
        }
    })
}

/// The edit turning `old` into `new` (common prefix and suffix kept).
fn edit_between(old: &str, new: &str) -> InputEdit {
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut prefix = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = ob.len().min(nb.len()) - prefix;
    let mut suffix = ob
        .iter()
        .rev()
        .zip(nb.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(ob.len() - suffix) || !new.is_char_boundary(nb.len() - suffix) {
        suffix -= 1;
    }
    let old_end = ob.len() - suffix;
    let new_end = nb.len() - suffix;
    InputEdit {
        start_byte: prefix,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: point_at(ob, prefix),
        old_end_position: point_at(ob, old_end),
        new_end_position: point_at(nb, new_end),
    }
}

fn point_at(bytes: &[u8], offset: usize) -> Point {
    let before = &bytes[..offset];
    let row = before.iter().filter(|&&b| b == b'\n').count();
    let col = offset
        - before
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
    Point { row, column: col }
}

// ─── Scopes and definitions ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeClass {
    Function,
    Type,
    Block,
}

/// Node kinds by class and language.
fn class_of(lang: Lang, kind: &str) -> Option<ScopeClass> {
    use ScopeClass::*;
    let c = match lang {
        Lang::Rust => match kind {
            "function_item" | "function_signature_item" | "closure_expression" => Function,
            "struct_item" | "enum_item" | "union_item" | "trait_item" | "impl_item"
            | "mod_item" | "type_item" | "macro_definition" => Type,
            "block" | "declaration_list" => Block,
            _ => return None,
        },
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => match kind {
            "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "function_expression"
            | "arrow_function"
            | "function_signature"
            | "method_signature" => Function,
            "class_declaration"
            | "abstract_class_declaration"
            | "class"
            | "interface_declaration"
            | "enum_declaration"
            | "type_alias_declaration"
            | "internal_module"
            | "module" => Type,
            "statement_block" | "class_body" => Block,
            _ => return None,
        },
        Lang::Python => match kind {
            "function_definition" | "lambda" => Function,
            "class_definition" => Type,
            "block" => Block,
            _ => return None,
        },
        Lang::Go => match kind {
            "function_declaration" | "method_declaration" | "func_literal" => Function,
            "type_spec" | "type_declaration" => Type,
            "block" => Block,
            _ => return None,
        },
    };
    Some(c)
}

/// The enclosing scope found for a range.
#[derive(Debug, Clone)]
pub struct Scope {
    pub class: ScopeClass,
    pub node_kind: String,
    pub start: usize,
    pub end: usize,
    pub name: Option<String>,
    /// Bytes of the signature (from the scope start to its body).
    pub signature_end: usize,
    pub has_errors: bool,
}

/// The innermost scope of class `want` (or, for `None`, any function or
/// type, falling back to a block) containing `[start, end)`.
pub fn enclosing(
    parsed: &Parsed,
    start: usize,
    end: usize,
    want: Option<ScopeClass>,
) -> Option<Scope> {
    let root = parsed.tree.root_node();
    let mut node = root.descendant_for_byte_range(start, end.max(start))?;
    let mut block: Option<Node> = None;
    loop {
        if let Some(class) = class_of(parsed.lang, node.kind()) {
            let ok = match want {
                Some(w) => class == w,
                None => class != ScopeClass::Block,
            };
            if ok {
                return Some(scope_of(parsed, node, class));
            }
            if class == ScopeClass::Block && block.is_none() {
                block = Some(node);
            }
        }
        match node.parent() {
            Some(p) => node = p,
            None => break,
        }
    }
    match (want, block) {
        (None, Some(b)) => Some(scope_of(parsed, b, ScopeClass::Block)),
        _ => None,
    }
}

fn scope_of(parsed: &Parsed, node: Node, class: ScopeClass) -> Scope {
    // Python decorators and TS/JS export wrappers belong to the definition.
    let mut outer = node;
    if let Some(p) = node.parent() {
        if matches!(p.kind(), "decorated_definition" | "export_statement") {
            outer = p;
        }
    }
    let body = node.child_by_field_name("body").or_else(|| {
        node.child_by_field_name("value")
            .filter(|_| node.kind() == "type_spec")
    });
    let text = parsed.text.as_bytes();
    let signature_end = match body {
        Some(b) if b.start_byte() > node.start_byte() => b.start_byte(),
        _ => {
            // No body: the first line.
            let from = node.start_byte();
            text[from..node.end_byte()]
                .iter()
                .position(|&b| b == b'\n')
                .map(|p| from + p)
                .unwrap_or(node.end_byte())
        }
    };
    Scope {
        class,
        node_kind: node.kind().to_string(),
        start: outer.start_byte(),
        end: node.end_byte(),
        name: name_of(parsed, node),
        signature_end,
        has_errors: node.has_error(),
    }
}

fn name_of(parsed: &Parsed, node: Node) -> Option<String> {
    let text = parsed.text.as_bytes();
    let field = match node.kind() {
        "impl_item" => node.child_by_field_name("type"),
        "arrow_function" | "function_expression" => {
            // `const name = () => ...`
            let p = node.parent()?;
            if p.kind() == "variable_declarator" {
                p.child_by_field_name("name")
            } else {
                None
            }
        }
        _ => node.child_by_field_name("name"),
    }?;
    field.utf8_text(text).ok().map(|s| s.to_string())
}

/// A definition the syntax tree shows.
#[derive(Debug, Clone)]
pub struct Definition {
    pub name: String,
    pub kind: String,
    pub node_kind: String,
    pub start: usize,
    pub end: usize,
    pub name_start: usize,
    pub name_end: usize,
    pub signature: String,
    /// Name of the enclosing type/impl/class, if any.
    pub container: Option<String>,
}

fn kind_label(lang: Lang, node_kind: &str, in_type: bool) -> &'static str {
    match (lang, node_kind) {
        (_, "function_item" | "function_signature_item") if in_type => "method",
        (_, "function_item" | "function_signature_item" | "function_declaration") => "function",
        (Lang::Python, "function_definition") if in_type => "method",
        (Lang::Python, "function_definition") => "function",
        (_, "method_definition" | "method_declaration" | "method_signature") => "method",
        (_, "generator_function_declaration" | "function_signature") => "function",
        (_, "arrow_function" | "function_expression") => "function",
        (_, "struct_item") => "struct",
        (_, "enum_item" | "enum_declaration") => "enum",
        (_, "union_item") => "union",
        (_, "trait_item") => "trait",
        (_, "impl_item") => "impl",
        (_, "mod_item" | "internal_module" | "module") => "module",
        (_, "type_item" | "type_alias_declaration") => "type",
        (_, "macro_definition") => "macro",
        (_, "class_declaration" | "abstract_class_declaration" | "class_definition") => "class",
        (_, "interface_declaration") => "interface",
        (_, "type_spec") => "type",
        (_, "const_item") => "const",
        (_, "static_item") => "static",
        _ => "symbol",
    }
}

fn is_definition(lang: Lang, node: Node) -> bool {
    match lang {
        Lang::Rust => matches!(
            node.kind(),
            "function_item"
                | "function_signature_item"
                | "struct_item"
                | "enum_item"
                | "union_item"
                | "trait_item"
                | "impl_item"
                | "mod_item"
                | "type_item"
                | "macro_definition"
                | "const_item"
                | "static_item"
        ),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => match node.kind() {
            "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "function_signature"
            | "method_signature"
            | "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "type_alias_declaration"
            | "internal_module" => true,
            "arrow_function" | "function_expression" => node
                .parent()
                .is_some_and(|p| p.kind() == "variable_declarator"),
            _ => false,
        },
        Lang::Python => matches!(node.kind(), "function_definition" | "class_definition"),
        Lang::Go => matches!(
            node.kind(),
            "function_declaration" | "method_declaration" | "type_spec"
        ),
    }
}

/// Every definition of a parsed document, in source order.
pub fn definitions(parsed: &Parsed) -> Vec<Definition> {
    let text = parsed.text.as_bytes();
    let mut out = Vec::new();
    // (node, container name, inside a type)
    let mut stack: Vec<(Node, Option<String>, bool)> = vec![(parsed.tree.root_node(), None, false)];
    while let Some((node, container, in_type)) = stack.pop() {
        let mut child_container = container.clone();
        let mut child_in_type = in_type;
        if is_definition(parsed.lang, node) {
            if let Some(name) = name_of(parsed, node) {
                let name_node = match node.kind() {
                    "impl_item" => node.child_by_field_name("type"),
                    "arrow_function" | "function_expression" => {
                        node.parent().and_then(|p| p.child_by_field_name("name"))
                    }
                    _ => node.child_by_field_name("name"),
                };
                let scope = scope_of(parsed, node, ScopeClass::Function);
                let sig = std::str::from_utf8(&text[node.start_byte()..scope.signature_end])
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let sig = truncate_chars(&sig, 240);
                let (ns, ne) = name_node
                    .map(|n| (n.start_byte(), n.end_byte()))
                    .unwrap_or((node.start_byte(), node.start_byte()));
                out.push(Definition {
                    kind: kind_label(parsed.lang, node.kind(), in_type).to_string(),
                    node_kind: node.kind().to_string(),
                    start: scope.start,
                    end: node.end_byte(),
                    name_start: ns,
                    name_end: ne,
                    signature: sig,
                    container: container.clone(),
                    name: name.clone(),
                });
                if matches!(class_of(parsed.lang, node.kind()), Some(ScopeClass::Type)) {
                    child_container = Some(name);
                    child_in_type = true;
                }
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for c in children.into_iter().rev() {
            stack.push((c, child_container.clone(), child_in_type));
        }
    }
    out
}

/// Whether `offset` lies inside an import or re-export (`use`, `import`,
/// `export { .. } from`), where a name refers to a definition elsewhere.
pub fn inside_import(parsed: &Parsed, offset: usize) -> bool {
    let Some(mut n) = parsed
        .tree
        .root_node()
        .descendant_for_byte_range(offset, offset)
    else {
        return false;
    };
    loop {
        match n.kind() {
            "use_declaration"
            | "import_statement"
            | "import_from_statement"
            | "import_declaration"
            | "import_spec" => return true,
            // `export function f` defines; `export { f } from` does not.
            "export_statement" => return n.child_by_field_name("declaration").is_none(),
            _ => {}
        }
        match n.parent() {
            Some(p) => n = p,
            None => return false,
        }
    }
}

/// The identifier at `offset`, if any (`[start, end)` and text).
pub fn identifier_at(parsed: &Parsed, offset: usize) -> Option<(usize, usize, String)> {
    let root = parsed.tree.root_node();
    let try_at = |o: usize| {
        let n = root.descendant_for_byte_range(o, o)?;
        let k = n.kind();
        if k.contains("identifier") || k == "type_identifier" || k == "field_identifier" {
            let t = n.utf8_text(parsed.text.as_bytes()).ok()?;
            Some((n.start_byte(), n.end_byte(), t.to_string()))
        } else {
            None
        }
    };
    try_at(offset).or_else(|| offset.checked_sub(1).and_then(try_at))
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::DocSource;
    use std::time::Duration;

    fn doc(name: &str, text: &str) -> Document {
        Document::new(PathBuf::from(name), Arc::from(text), DocSource::Disk, None)
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn grammars_by_extension() {
        assert_eq!(Lang::for_path(Path::new("a.ts")), Some(Lang::TypeScript));
        assert_eq!(Lang::for_path(Path::new("a.tsx")), Some(Lang::Tsx));
        assert_eq!(Lang::for_path(Path::new("a.jsx")), Some(Lang::JavaScript));
        assert_eq!(Lang::for_path(Path::new("a.md")), None);
        let cache = TreeCache::new(8);
        // JSX parses without errors with the TSX grammar...
        let jsx = doc("a.jsx", "const A = () => <div>{x}</div>;\n");
        assert!(!cache.parse(&jsx, far()).unwrap().0.has_errors);
        // ...and a `<T>` cast parses with the TypeScript grammar.
        let ts = doc("b.ts", "const n = <number>x;\n");
        assert!(!cache.parse(&ts, far()).unwrap().0.has_errors);
    }

    #[test]
    fn enclosing_function_type_and_signature() {
        let src = "struct S;\nimpl S {\n    /// doc\n    pub fn go(&self, x: u32) -> u32 {\n        let y = x + 1;\n        y\n    }\n}\n";
        let cache = TreeCache::new(8);
        let (p, _) = cache.parse(&doc("a.rs", src), far()).unwrap();
        let at = src.find("x + 1").unwrap();
        let f = enclosing(&p, at, at + 1, Some(ScopeClass::Function)).unwrap();
        assert_eq!(f.node_kind, "function_item");
        assert_eq!(f.name.as_deref(), Some("go"));
        assert_eq!(
            src[f.start..f.signature_end].trim(),
            "pub fn go(&self, x: u32) -> u32"
        );
        let t = enclosing(&p, at, at + 1, Some(ScopeClass::Type)).unwrap();
        assert_eq!(t.node_kind, "impl_item");
        assert_eq!(t.name.as_deref(), Some("S"));
        let defs = definitions(&p);
        let go = defs.iter().find(|d| d.name == "go").unwrap();
        assert_eq!(go.kind, "method");
        assert_eq!(go.container.as_deref(), Some("S"));
        assert_eq!(&src[go.name_start..go.name_end], "go");
    }

    #[test]
    fn definitions_per_language() {
        let cache = TreeCache::new(8);
        let cases = [
            ("a.py", "class K:\n    def m(self):\n        pass\n\ndef f():\n    return 1\n", vec!["K", "m", "f"]),
            ("a.go", "package p\n\ntype T struct{}\n\nfunc (t T) M() {}\n\nfunc F() {}\n", vec!["T", "M", "F"]),
            ("a.ts", "export class C { m(): void {} }\ninterface I {}\nconst g = () => 1;\nfunction h() {}\n", vec!["C", "m", "I", "g", "h"]),
            ("a.tsx", "export function View() { return <p/>; }\n", vec!["View"]),
        ];
        for (name, src, want) in cases {
            let (p, _) = cache.parse(&doc(name, src), far()).unwrap();
            let got: Vec<String> = definitions(&p).into_iter().map(|d| d.name).collect();
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn incomplete_code_still_gives_scopes() {
        let src = "fn ok() {}\nfn broken( {\n    let a = 1;\n";
        let cache = TreeCache::new(8);
        let (p, _) = cache.parse(&doc("a.rs", src), far()).unwrap();
        assert!(p.has_errors);
        let defs = definitions(&p);
        assert!(defs.iter().any(|d| d.name == "ok"));
    }

    #[test]
    fn cache_hit_and_incremental_reparse() {
        let cache = TreeCache::new(2);
        let v1 = doc("a.rs", "fn a() {}\nfn b() {}\n");
        assert!(!cache.parse(&v1, far()).unwrap().1);
        assert!(cache.parse(&v1, far()).unwrap().1);
        let v2 = doc("a.rs", "fn a() {}\nfn bé() { 1 }\n");
        let (p2, hit) = cache.parse(&v2, far()).unwrap();
        assert!(!hit);
        assert_eq!(cache.incremental.load(Ordering::Relaxed), 1);
        let names: Vec<String> = definitions(&p2).into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["a", "bé"]);
        // Same result as a fresh parse.
        let fresh = TreeCache::new(1);
        let (pf, _) = fresh.parse(&v2, far()).unwrap();
        assert_eq!(p2.tree.root_node().to_sexp(), pf.tree.root_node().to_sexp());
        // LRU bound.
        cache.parse(&doc("b.rs", "fn x() {}"), far()).unwrap();
        cache.parse(&doc("c.rs", "fn y() {}"), far()).unwrap();
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn unsupported_and_deadline() {
        let cache = TreeCache::new(2);
        assert_eq!(
            cache.parse(&doc("a.md", "# x"), far()).err(),
            Some(ParseFailure::Unsupported)
        );
        let big = "fn f() { let x = 1; }\n".repeat(20000);
        let r = cache.parse(&doc("big.rs", &big), Instant::now());
        assert_eq!(r.err(), Some(ParseFailure::Deadline));
        // The parser recovers after a cancelled parse.
        assert!(cache.parse(&doc("ok.rs", "fn ok() {}"), far()).is_ok());
    }

    #[test]
    fn imports_and_reexports() {
        let cache = TreeCache::new(4);
        let src = "pub use client::{a, parse};\nfn parse() {}\n";
        let (p, _) = cache.parse(&doc("a.rs", src), far()).unwrap();
        assert!(inside_import(&p, src.find("parse").unwrap()));
        assert!(!inside_import(&p, src.rfind("parse").unwrap()));
        let ts = "export { x } from './x';\nexport function y() {}\nimport z from 'z';\n";
        let (p, _) = cache.parse(&doc("a.ts", ts), far()).unwrap();
        assert!(inside_import(&p, ts.find('x').unwrap()));
        assert!(!inside_import(&p, ts.find("y()").unwrap()));
        assert!(inside_import(&p, ts.find('z').unwrap()));
    }

    #[test]
    fn identifier_lookup() {
        let src = "fn main() { helper(1); }\n";
        let cache = TreeCache::new(2);
        let (p, _) = cache.parse(&doc("a.rs", src), far()).unwrap();
        let at = src.find("helper").unwrap() + 2;
        assert_eq!(identifier_at(&p, at).unwrap().2, "helper");
    }
}
