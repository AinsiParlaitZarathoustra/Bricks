//! Structural skeletons of source files, extracted with Tree-sitter.
//!
//! A skeleton is an *exploration view*: imports, attributes and decorators,
//! signatures with their types and generics, type declarations with their
//! fields and variants, and the comments that document them stay; function
//! bodies (and very long initialisers) are replaced by a marker that names
//! the original line range. Every kept line carries its original line number,
//! so the agent can read exactly the body it needs. It is not source code
//! and must not be compiled or used as a patch base.
//!
//! The extraction is conservative. A body that contains a parse error or a
//! missing node is kept in full, top-level error regions are never hidden,
//! and when too much of the file is uncertain no skeleton is produced at all.
//! Nothing is removed only because it was not recognised: only nodes known to
//! be function bodies or initialisers are hidden.

use std::collections::BTreeSet;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkeletonLanguage {
    Rust,
    Python,
    TypeScript,
    /// TSX grammar; also used for JavaScript (`.js`, `.jsx`, `.mjs`, `.cjs`).
    Tsx,
    Go,
}

impl SkeletonLanguage {
    pub fn from_path(path: &str) -> Option<Self> {
        let ext = std::path::Path::new(path)
            .extension()?
            .to_str()?
            .to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => SkeletonLanguage::Rust,
            "py" | "pyi" => SkeletonLanguage::Python,
            "ts" | "mts" | "cts" => SkeletonLanguage::TypeScript,
            "tsx" | "js" | "jsx" | "mjs" | "cjs" => SkeletonLanguage::Tsx,
            "go" => SkeletonLanguage::Go,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            SkeletonLanguage::Rust => "Rust",
            SkeletonLanguage::Python => "Python",
            SkeletonLanguage::TypeScript => "TypeScript",
            SkeletonLanguage::Tsx => "JavaScript/TSX",
            SkeletonLanguage::Go => "Go",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            SkeletonLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
            SkeletonLanguage::Python => tree_sitter_python::LANGUAGE.into(),
            SkeletonLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            SkeletonLanguage::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            SkeletonLanguage::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }

    /// Nodes whose `body` is hidden.
    fn is_function(self, kind: &str) -> bool {
        match self {
            SkeletonLanguage::Rust => kind == "function_item",
            SkeletonLanguage::Python => kind == "function_definition",
            SkeletonLanguage::TypeScript | SkeletonLanguage::Tsx => matches!(
                kind,
                "function_declaration"
                    | "generator_function_declaration"
                    | "method_definition"
                    | "function_expression"
                    | "function"
                    | "generator_function"
                    | "arrow_function"
            ),
            SkeletonLanguage::Go => matches!(
                kind,
                "function_declaration" | "method_declaration" | "func_literal"
            ),
        }
    }

    /// Declarations whose header stays visible even inside a hidden body.
    fn is_declaration(self, kind: &str) -> bool {
        match self {
            SkeletonLanguage::Rust => matches!(
                kind,
                "function_item"
                    | "function_signature_item"
                    | "struct_item"
                    | "enum_item"
                    | "union_item"
                    | "impl_item"
                    | "trait_item"
                    | "mod_item"
                    | "type_item"
                    | "const_item"
                    | "static_item"
                    | "macro_definition"
            ),
            SkeletonLanguage::Python => {
                matches!(
                    kind,
                    "function_definition" | "class_definition" | "decorated_definition"
                )
            }
            SkeletonLanguage::TypeScript | SkeletonLanguage::Tsx => matches!(
                kind,
                "function_declaration"
                    | "generator_function_declaration"
                    | "class_declaration"
                    | "abstract_class_declaration"
                    | "method_definition"
                    | "interface_declaration"
                    | "type_alias_declaration"
                    | "enum_declaration"
                    | "internal_module"
            ),
            SkeletonLanguage::Go => matches!(
                kind,
                "function_declaration" | "method_declaration" | "type_declaration"
            ),
        }
    }

    fn is_attachment(self, kind: &str) -> bool {
        matches!(
            kind,
            "comment" | "line_comment" | "block_comment" | "attribute_item" | "decorator"
        )
    }

    fn marker(self, indent: &str, n: usize, from: usize, to: usize) -> String {
        let what = if n == 1 { "line" } else { "lines" };
        match self {
            SkeletonLanguage::Python => {
                format!("{indent}...  # ⋯ {n} {what} omitted (L{from}–L{to})")
            }
            _ => format!("{indent}// ⋯ {n} {what} omitted (L{from}–L{to})"),
        }
    }
}

/// A skeleton and what it leaves out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skeleton {
    /// The view, one `NN | <line>` per kept line (the `Read` format).
    pub text: String,
    pub language: SkeletonLanguage,
    pub total_lines: usize,
    pub shown_lines: usize,
    /// Hidden ranges, as original 1-based inclusive line numbers.
    pub omitted: Vec<(usize, usize)>,
    /// Regions kept in full because the parse was uncertain there.
    pub uncertain_regions: usize,
}

/// Why no skeleton was produced. The caller then keeps the content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoSkeleton {
    ParseFailed,
    Unreliable {
        error_lines: usize,
        total_lines: usize,
    },
    NothingToOmit,
}

impl std::fmt::Display for NoSkeleton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoSkeleton::ParseFailed => write!(f, "the parser produced no tree"),
            NoSkeleton::Unreliable {
                error_lines,
                total_lines,
            } => write!(
                f,
                "{error_lines} of {total_lines} lines did not parse; a skeleton would be unreliable"
            ),
            NoSkeleton::NothingToOmit => write!(f, "too little to omit for a skeleton to help"),
        }
    }
}

/// Minimum fraction of lines a skeleton must hide to be worth showing.
const MIN_SAVING: f64 = 0.15;
/// Initialisers longer than this are hidden (module and type level only).
const LONG_INITIALISER_LINES: usize = 8;

/// Build the skeleton of `source`. `first_line` is the number of its first
/// line (1 for a whole file).
pub fn skeleton(
    source: &str,
    lang: SkeletonLanguage,
    first_line: usize,
) -> Result<Skeleton, NoSkeleton> {
    let mut parser = Parser::new();
    parser
        .set_language(&lang.grammar())
        .map_err(|_| NoSkeleton::ParseFailed)?;
    let tree = parser.parse(source, None).ok_or(NoSkeleton::ParseFailed)?;
    let root = tree.root_node();
    let lines: Vec<&str> = source.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Err(NoSkeleton::NothingToOmit);
    }

    let mut nodes = Vec::new();
    collect(root, &mut nodes);

    // Uncertain regions: ERROR and MISSING nodes.
    let uncertain: Vec<(usize, usize)> = nodes
        .iter()
        .filter(|n| n.is_error() || n.is_missing())
        .map(|n| (n.start_position().row, n.end_position().row))
        .collect();
    let error_lines: BTreeSet<usize> = uncertain.iter().flat_map(|&(a, b)| a..=b).collect();
    if root.is_error() || error_lines.len() * 10 > total * 3 {
        return Err(NoSkeleton::Unreliable {
            error_lines: error_lines.len(),
            total_lines: total,
        });
    }
    let overlaps_error = |a: usize, b: usize| uncertain.iter().any(|&(s, e)| s <= b && e >= a);

    let mut hidden = vec![false; total];
    let mut kept_uncertain = 0usize;
    for node in &nodes {
        let range = if lang.is_function(node.kind()) {
            body_range(*node, lang)
        } else {
            initialiser_range(*node, lang)
        };
        let Some((a, b)) = range else { continue };
        if overlaps_error(a, b) {
            kept_uncertain += 1;
            continue;
        }
        for h in hidden.iter_mut().take(b + 1).skip(a) {
            *h = true;
        }
    }

    // Declarations nested in hidden bodies keep their header (and the
    // comments and attributes attached to them).
    for node in &nodes {
        if !lang.is_declaration(node.kind()) {
            continue;
        }
        let start = attached_start(*node, lang);
        // `@decorator def f(): …` wraps the definition that has the body.
        let def = if node.kind() == "decorated_definition" {
            node.child_by_field_name("definition").unwrap_or(*node)
        } else {
            *node
        };
        let body_node = def.child_by_field_name("body");
        // The header ends where the body starts: on the `{` line, or on the
        // line before a Python block.
        let header_end = match body_node {
            Some(body) => match lang {
                SkeletonLanguage::Python => body
                    .prev_sibling()
                    .map(|p| p.end_position().row)
                    .unwrap_or(body.start_position().row),
                _ => body.start_position().row,
            },
            None => def.end_position().row,
        };
        if !hidden[start.min(total - 1)] && !hidden[header_end.min(total - 1)] {
            continue;
        }
        for h in hidden
            .iter_mut()
            .take(header_end.min(total - 1) + 1)
            .skip(start)
        {
            *h = false;
        }
        // The closing line of a braced body.
        if let Some(body) = body_node {
            let close = body.end_position().row;
            if lang != SkeletonLanguage::Python && close < total && close > header_end {
                hidden[close] = false;
            }
        }
    }
    // Uncertain regions are always visible.
    for &(a, b) in &uncertain {
        for h in hidden.iter_mut().take(b.min(total - 1) + 1).skip(a) {
            *h = false;
        }
    }

    let hidden_count = hidden.iter().filter(|h| **h).count();
    if (hidden_count as f64) < total as f64 * MIN_SAVING {
        return Err(NoSkeleton::NothingToOmit);
    }

    let mut out = String::with_capacity(source.len() / 2);
    let mut omitted = Vec::new();
    // Same layout as the `Read` tool: `  12 | text`, aligned.
    let w = (first_line + total).to_string().len();
    let mut row = 0;
    while row < total {
        if hidden[row] {
            let start = row;
            while row < total && hidden[row] {
                row += 1;
            }
            let indent: String = lines[start]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            let (from, to) = (first_line + start, first_line + row - 1);
            omitted.push((from, to));
            out.push_str(&format!("{:>w$} | ", ""));
            out.push_str(&lang.marker(&indent, row - start, from, to));
            out.push('\n');
            continue;
        }
        out.push_str(&format!("{:>w$} | {}\n", first_line + row, lines[row]));
        row += 1;
    }

    Ok(Skeleton {
        text: out,
        language: lang,
        total_lines: total,
        shown_lines: total - hidden_count,
        omitted,
        uncertain_regions: uncertain.len() + kept_uncertain,
    })
}

fn collect<'t>(root: Node<'t>, out: &mut Vec<Node<'t>>) {
    let mut cursor = root.walk();
    loop {
        out.push(cursor.node());
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// Lines of a function body to hide (0-based, inclusive).
fn body_range(node: Node, lang: SkeletonLanguage) -> Option<(usize, usize)> {
    let body = node.child_by_field_name("body")?;
    match lang {
        SkeletonLanguage::Python => {
            if body.kind() != "block" {
                return None;
            }
            let mut start = body.start_position().row;
            if start <= node.start_position().row {
                return None; // `def f(): return 1`
            }
            // Keep the docstring: it documents the contract.
            if let Some(first) = body.named_child(0) {
                if first.kind() == "expression_statement"
                    && first.named_child(0).is_some_and(|s| s.kind() == "string")
                {
                    start = first.end_position().row + 1;
                }
            }
            let end = body.end_position().row;
            (end >= start && end - start >= 1).then_some((start, end))
        }
        _ => {
            if !matches!(body.kind(), "block" | "statement_block") {
                return None; // an arrow function with an expression body
            }
            let open = body.start_position().row;
            let close = body.end_position().row;
            (close >= open + 3).then(|| (open + 1, close - 1))
        }
    }
}

/// Long initialiser values at module or type level (tables, configuration
/// objects), excluding functions, which are handled as functions.
fn initialiser_range(node: Node, lang: SkeletonLanguage) -> Option<(usize, usize)> {
    let value = match (lang, node.kind()) {
        (SkeletonLanguage::Rust, "const_item" | "static_item") => {
            node.child_by_field_name("value")?
        }
        (SkeletonLanguage::TypeScript | SkeletonLanguage::Tsx, "variable_declarator") => {
            // Module level only: `const X = …` or `export const X = …`.
            let grand = node.parent()?.parent()?;
            if !matches!(grand.kind(), "program" | "export_statement") {
                return None;
            }
            node.child_by_field_name("value")?
        }
        (SkeletonLanguage::Python, "assignment") => {
            let stmt = node.parent()?;
            let container = stmt.parent()?;
            let top = container.kind() == "module"
                || (container.kind() == "block"
                    && container
                        .parent()
                        .is_some_and(|c| c.kind() == "class_definition"));
            if stmt.kind() != "expression_statement" || !top {
                return None;
            }
            node.child_by_field_name("right")?
        }
        _ => return None,
    };
    if matches!(
        value.kind(),
        "arrow_function"
            | "function_expression"
            | "function"
            | "class"
            | "lambda"
            | "closure_expression"
    ) {
        return None;
    }
    let a = value.start_position().row;
    let b = value.end_position().row;
    (b > a + LONG_INITIALISER_LINES).then(|| (a + 1, b - 1))
}

/// First line of a declaration, including the comments, attributes and
/// decorators directly attached above it.
fn attached_start(node: Node, lang: SkeletonLanguage) -> usize {
    let mut start = node.start_position().row;
    let mut prev = node.prev_sibling();
    while let Some(p) = prev {
        if !lang.is_attachment(p.kind()) || p.end_position().row + 1 < start {
            break;
        }
        start = p.start_position().row;
        prev = p.prev_sibling();
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(src: &str, lang: SkeletonLanguage) -> Skeleton {
        skeleton(src, lang, 1).unwrap_or_else(|e| panic!("no skeleton: {e}"))
    }

    fn body(n: usize, indent: &str, stmt: &str) -> String {
        (0..n)
            .map(|i| format!("{indent}{}\n", stmt.replace("{i}", &i.to_string())))
            .collect()
    }

    #[test]
    fn rust_keeps_contracts_and_hides_bodies() {
        let src = format!(
            "use std::collections::HashMap;\n\n/// Adds things.\n#[inline]\npub fn add<T: Into<i64>>(a: T, b: T) -> i64 {{\n{}}}\n\npub struct Point {{\n    pub x: f64,\n    pub y: f64,\n}}\n\npub enum Shape {{\n    Circle(f64),\n    Square {{ side: f64 }},\n}}\n\nimpl Point {{\n    /// Distance.\n    pub fn dist(&self) -> f64 {{\n{}    }}\n}}\n",
            body(6, "    ", "let s{i} = \"}{\"; // braces in a string"),
            body(5, "        ", "let v{i} = self.x * {i}.0;"),
        );
        let s = view(&src, SkeletonLanguage::Rust);
        for kept in [
            "use std::collections::HashMap;",
            "/// Adds things.",
            "#[inline]",
            "pub fn add<T: Into<i64>>(a: T, b: T) -> i64 {",
            "pub x: f64,",
            "Circle(f64),",
            "Square { side: f64 },",
            "/// Distance.",
            "pub fn dist(&self) -> f64 {",
        ] {
            assert!(s.text.contains(kept), "missing {kept:?}:\n{}", s.text);
        }
        assert!(!s.text.contains("let s0"), "{}", s.text);
        assert!(!s.text.contains("let v0"), "{}", s.text);
        assert!(
            s.text.contains("// ⋯ 6 lines omitted (L6–L11)"),
            "{}",
            s.text
        );
        // Line numbers are the original ones.
        assert!(s.text.contains(" 5 | pub fn add"), "{}", s.text);
    }

    #[test]
    fn nested_functions_keep_their_signature() {
        let src = format!(
            "fn outer() -> u8 {{\n{}    fn inner(x: u8) -> u8 {{\n{}    }}\n{}    inner(1)\n}}\n",
            body(3, "    ", "let a{i} = {i};"),
            body(4, "        ", "let b{i} = x + {i};"),
            body(3, "    ", "let c{i} = {i};"),
        );
        let s = view(&src, SkeletonLanguage::Rust);
        assert!(s.text.contains("fn outer() -> u8 {"));
        assert!(s.text.contains("fn inner(x: u8) -> u8 {"), "{}", s.text);
        assert!(!s.text.contains("let b0"));
        assert!(!s.text.contains("let a0"));
    }

    #[test]
    fn python_keeps_decorators_docstrings_and_nesting() {
        let src = format!(
            "import os\nfrom typing import Generic, TypeVar\n\nT = TypeVar(\"T\")\n\n@dataclass\nclass Box(Generic[T]):\n    \"\"\"A box.\"\"\"\n    value: T\n\n    @property\n    def size(self) -> int:\n        \"\"\"Contract: never negative.\"\"\"\n{}\n\ndef outer(x: int) -> int:\n{}    def inner(y: int) -> int:\n{}    return inner(x)\n",
            body(5, "        ", "n{i} = {i} * 2"),
            body(3, "    ", "a{i} = {i}"),
            body(4, "        ", "b{i} = y + {i}"),
        );
        let s = view(&src, SkeletonLanguage::Python);
        for kept in [
            "import os",
            "@dataclass",
            "class Box(Generic[T]):",
            "\"\"\"A box.\"\"\"",
            "value: T",
            "@property",
            "def size(self) -> int:",
            "\"\"\"Contract: never negative.\"\"\"",
            "def outer(x: int) -> int:",
            "def inner(y: int) -> int:",
        ] {
            assert!(s.text.contains(kept), "missing {kept:?}:\n{}", s.text);
        }
        assert!(!s.text.contains("n0 = 0"), "{}", s.text);
        assert!(!s.text.contains("b0 = y"));
        assert!(s.text.contains("...  # ⋯"), "{}", s.text);
    }

    #[test]
    fn typescript_keeps_types_and_signatures() {
        let src = format!(
            "import {{ a }} from './a';\n\nexport interface User<T> {{\n  id: string;\n  data: T;\n}}\n\nexport type Id = string | number;\n\nexport class Store<T> {{\n  @observable items: T[] = [];\n\n  async load(id: Id): Promise<User<T>> {{\n{}  }}\n}}\n\nexport const handler = async (req: Request): Promise<Response> => {{\n{}}};\n\nexport function plain(s: string): string {{\n  const t = '}}{{';\n{}}}\n",
            body(5, "    ", "const x{i} = await fetch(`/u/${{id}}`);"),
            body(5, "  ", "const y{i} = req.url;"),
            body(4, "  ", "const z{i} = s + t;"),
        );
        let s = view(&src, SkeletonLanguage::TypeScript);
        for kept in [
            "import { a } from './a';",
            "export interface User<T> {",
            "data: T;",
            "export type Id = string | number;",
            "export class Store<T> {",
            "@observable items: T[] = [];",
            "async load(id: Id): Promise<User<T>> {",
            "export const handler = async (req: Request): Promise<Response> => {",
            "export function plain(s: string): string {",
        ] {
            assert!(s.text.contains(kept), "missing {kept:?}:\n{}", s.text);
        }
        assert!(!s.text.contains("const x0"));
        assert!(!s.text.contains("const y0"));
    }

    #[test]
    fn javascript_uses_the_tsx_grammar() {
        let src = format!(
            "/** Renders a list. */\nexport function List({{ items }}) {{\n{}  return <ul>{{items.map(i => <li>{{i}}</li>)}}</ul>;\n}}\n",
            body(5, "  ", "const k{i} = items.length + {i};"),
        );
        let s = view(&src, SkeletonLanguage::from_path("a.jsx").unwrap());
        assert!(s.text.contains("/** Renders a list. */"));
        assert!(s.text.contains("export function List({ items }) {"));
        assert!(!s.text.contains("const k0"));
    }

    #[test]
    fn go_keeps_types_and_signatures() {
        let src = format!(
            "package calc\n\n// Adder adds.\ntype Adder struct {{\n\tBase int\n}}\n\n// Add returns the sum.\nfunc (a *Adder) Add(x int) int {{\n{}\treturn a.Base + x\n}}\n",
            body(5, "\t", "y{i} := x * {i}"),
        );
        let s = view(&src, SkeletonLanguage::Go);
        assert!(s.text.contains("// Add returns the sum."));
        assert!(s.text.contains("func (a *Adder) Add(x int) int {"));
        assert!(s.text.contains("Base int"));
        assert!(!s.text.contains("y0 :="));
    }

    #[test]
    fn a_body_with_a_syntax_error_is_kept_whole() {
        let src = format!(
            "fn good() {{\n{}}}\n\nfn broken() {{\n    let x = ;\n{}}}\n",
            body(6, "    ", "let a{i} = {i};"),
            body(6, "    ", "let b{i} = {i};"),
        );
        let s = view(&src, SkeletonLanguage::Rust);
        assert!(!s.text.contains("let a0"), "good body hidden");
        assert!(
            s.text.contains("let x = ;"),
            "error region kept:\n{}",
            s.text
        );
        assert!(
            s.text.contains("let b0"),
            "broken body kept whole:\n{}",
            s.text
        );
        assert!(s.uncertain_regions >= 1);
    }

    #[test]
    fn mostly_broken_code_gets_no_skeleton() {
        let src = "fn ( {{ ]] let\n".repeat(30);
        assert!(matches!(
            skeleton(&src, SkeletonLanguage::Rust, 1),
            Err(NoSkeleton::Unreliable { .. })
        ));
    }

    #[test]
    fn short_files_are_not_worth_a_skeleton() {
        assert_eq!(
            skeleton("fn a() -> u8 { 1 }\n", SkeletonLanguage::Rust, 1),
            Err(NoSkeleton::NothingToOmit)
        );
    }

    #[test]
    fn unknown_extensions_have_no_language() {
        assert_eq!(SkeletonLanguage::from_path("notes.txt"), None);
        assert_eq!(SkeletonLanguage::from_path("Makefile"), None);
        assert_eq!(SkeletonLanguage::from_path("x.rb"), None);
    }

    #[test]
    fn unicode_lines_are_never_split() {
        let src = format!(
            "/// Résumé : calcule la moyenne — 平均\nfn moyenne(v: &[f64]) -> f64 {{\n{}}}\n",
            body(6, "    ", "let é{i} = \"données {i} ✓\";"),
        );
        let s = view(&src, SkeletonLanguage::Rust);
        assert!(s.text.contains("/// Résumé : calcule la moyenne — 平均"));
        assert!(std::str::from_utf8(s.text.as_bytes()).is_ok());
    }
}
