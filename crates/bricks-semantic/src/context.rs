//! Context selection within a token budget.
//!
//! Every item costs what its rendering costs (path, relation, provenance,
//! signature, snippet): the budget counts all of it. Tokens are *estimated*
//! with the Context Manager's local heuristic (`cersei_types::tokens`); the
//! selection keeps the estimate's upper bound under the budget, a margin,
//! not a guarantee of the provider's count.
//!
//! Snippets are verbatim source with their original ranges. A scope larger
//! than the detail level allows becomes an explicit excerpt (signature,
//! a window around the match, the closing line) with the number of lines
//! left out; it is never returned whole beyond the limit.

use crate::query::{ContextPolicy, Detail};
use crate::rank::Candidate;
use crate::render;
use crate::result::*;
use crate::syntax::{self, Lang, ScopeClass, TreeCache};
use crate::view::Document;
use cersei_types::tokens::{estimate_text, ESTIMATION_METHOD};
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

pub struct ContextSettings {
    pub policy: ContextPolicy,
    pub detail: Detail,
    pub budget_tokens: u64,
    pub max_results: usize,
}

/// Limits per detail level: (scope lines kept whole, window around the
/// match in an excerpt, lines of context around a mention).
fn detail_limits(d: Detail) -> (u32, u32, u32) {
    match d {
        Detail::Compact => (12, 2, 0),
        Detail::Normal => (40, 5, 1),
        Detail::Deep => (120, 15, 3),
    }
}

#[derive(Default)]
pub struct Built {
    pub items: Vec<CodeItem>,
    pub budget: BudgetReport,
    pub omitted_budget: usize,
    pub omitted_max: usize,
    pub files_parsed: usize,
    pub tree_hits: usize,
}

pub fn source_range(doc: &Document, start: usize, end: usize) -> SourceRange {
    let m = doc.mapper();
    let start = m.floor_boundary(start);
    let end = m.floor_boundary(end.max(start));
    SourceRange {
        start_byte: start,
        end_byte: end,
        start: m
            .offset_to_line_col(start)
            .unwrap_or(crate::position::LineCol { line: 0, col: 0 }),
        end: m
            .offset_to_line_col(end)
            .unwrap_or(crate::position::LineCol { line: 0, col: 0 }),
    }
}

pub fn rel_path(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Verbatim whole lines `[a, b]` (0-based, inclusive).
fn lines_part(doc: &Document, a: u32, b: u32) -> Option<SnippetPart> {
    let m = doc.mapper();
    let s = m.line_range(a).ok()?.start;
    let e = m.line_range(b).ok()?.end;
    Some(SnippetPart {
        range: source_range(doc, s, e),
        text: doc.text[s..e].to_string(),
    })
}

/// A snippet of the scope `[s, e)` focused on `[fs, fe)`.
fn scope_snippet(
    doc: &Document,
    scope: (usize, usize),
    focus: (usize, usize),
    signature_end: Option<usize>,
    scope_name: &str,
    detail: Detail,
    note: Option<String>,
) -> Option<Snippet> {
    let m = doc.mapper();
    let (max_lines, window, _) = detail_limits(detail);
    let l0 = m.line_of(scope.0).ok()?;
    let l1 = m.line_of(scope.1.saturating_sub(1).max(scope.0)).ok()?;
    let total = l1 - l0 + 1;
    let scope_range = source_range(doc, scope.0, scope.1);
    if total <= max_lines {
        return Some(Snippet {
            kind: SnippetKind::Full,
            scope: scope_name.to_string(),
            scope_range,
            parts: vec![lines_part(doc, l0, l1)?],
            omitted_lines: 0,
            syntax_note: note,
        });
    }
    let sig_last = signature_end
        .and_then(|e| m.line_of(e.saturating_sub(1).max(scope.0)).ok())
        .unwrap_or(l0)
        .min(l0 + 2);
    let f = m.line_of(focus.0).ok()?.clamp(l0, l1);
    let mut spans: Vec<(u32, u32)> = vec![
        (l0, sig_last),
        (f.saturating_sub(window).max(l0), (f + window).min(l1)),
        (l1, l1),
    ];
    spans.sort();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let kept: u32 = merged.iter().map(|(a, b)| b - a + 1).sum();
    Some(Snippet {
        kind: SnippetKind::Excerpt,
        scope: scope_name.to_string(),
        scope_range,
        parts: merged
            .into_iter()
            .filter_map(|(a, b)| lines_part(doc, a, b))
            .collect(),
        omitted_lines: total - kept,
        syntax_note: note,
    })
}

fn lines_snippet(doc: &Document, at: usize, n: u32, note: Option<String>) -> Option<Snippet> {
    let m = doc.mapper();
    let l = m.line_of(at).ok()?;
    let a = l.saturating_sub(n);
    let b = (l + n).min(m.line_count() - 1);
    let part = lines_part(doc, a, b)?;
    Some(Snippet {
        kind: SnippetKind::Full,
        scope: "lines".into(),
        scope_range: part.range,
        parts: vec![part],
        omitted_lines: 0,
        syntax_note: note,
    })
}

struct Ctx<'a> {
    trees: &'a TreeCache,
    deadline: Instant,
    files_parsed: usize,
    tree_hits: usize,
}

impl Ctx<'_> {
    fn parse(&mut self, doc: &Document) -> Option<syntax::Parsed> {
        Lang::for_path(&doc.path)?;
        match self.trees.parse(doc, self.deadline) {
            Ok((p, hit)) => {
                if hit {
                    self.tree_hits += 1;
                } else {
                    self.files_parsed += 1;
                }
                Some(p)
            }
            Err(_) => None,
        }
    }
}

fn no_grammar_note(doc: &Document) -> String {
    let ext = doc.path.extension().and_then(|e| e.to_str()).unwrap_or("");
    format!("no grammar for `.{ext}`: line context, no syntax scope")
}

/// The snippet (and enclosing signature) of one candidate.
fn snippet_for(
    c: &Candidate,
    s: &ContextSettings,
    ctx: &mut Ctx,
) -> (Option<Snippet>, Option<String>) {
    let (_, _, around) = detail_limits(s.detail);
    let doc = &c.doc;
    let is_def = matches!(
        c.relation,
        Relation::Definition | Relation::Candidate | Relation::Declaration | Relation::Context
    );
    let want = match s.policy {
        ContextPolicy::None => return (None, None),
        ContextPolicy::Lines { n } => return (lines_snippet(doc, c.start, n.min(50), None), None),
        ContextPolicy::Function => Some(ScopeClass::Function),
        ContextPolicy::Type => Some(ScopeClass::Type),
        ContextPolicy::Block => Some(ScopeClass::Block),
        ContextPolicy::Auto => {
            if !is_def {
                // Mentions and references: the lines around, plus the
                // signature of the function they are in.
                let sig = if s.detail != Detail::Compact {
                    ctx.parse(doc).and_then(|p| {
                        let sc = syntax::enclosing(&p, c.start, c.end, Some(ScopeClass::Function))
                            .or_else(|| {
                                syntax::enclosing(&p, c.start, c.end, Some(ScopeClass::Type))
                            })?;
                        let t = doc.text.get(sc.start..sc.signature_end)?;
                        Some(syntax::truncate_chars(
                            &t.split_whitespace().collect::<Vec<_>>().join(" "),
                            160,
                        ))
                    })
                } else {
                    None
                };
                return (lines_snippet(doc, c.start, around, None), sig);
            }
            None
        }
    };
    // A definition carries its own node.
    if let (Some((a, b, kind)), None | Some(ScopeClass::Function) | Some(ScopeClass::Type)) =
        (&c.scope, want)
    {
        if want.is_none() || is_def {
            let sig_end = ctx
                .parse(doc)
                .and_then(|p| syntax::enclosing(&p, c.start, c.end.max(c.start + 1), None))
                .filter(|sc| sc.start == *a)
                .map(|sc| sc.signature_end);
            return (
                scope_snippet(
                    doc,
                    (*a, *b),
                    (c.start, c.end),
                    sig_end,
                    kind,
                    s.detail,
                    None,
                ),
                None,
            );
        }
    }
    match ctx.parse(doc) {
        Some(p) => match syntax::enclosing(&p, c.start, c.end, want) {
            Some(sc) => {
                let note = sc.has_errors.then(|| {
                    "syntax errors in this scope: boundaries may be approximate".to_string()
                });
                (
                    scope_snippet(
                        doc,
                        (sc.start, sc.end),
                        (c.start, c.end),
                        Some(sc.signature_end),
                        &sc.node_kind,
                        s.detail,
                        note,
                    ),
                    None,
                )
            }
            None => (
                lines_snippet(
                    doc,
                    c.start,
                    around.max(1),
                    Some("no enclosing scope of that kind: line context".into()),
                ),
                None,
            ),
        },
        None => (
            lines_snippet(doc, c.start, around.max(1), Some(no_grammar_note(doc))),
            None,
        ),
    }
}

/// Turn ranked candidates into items within the budget.
pub fn build(
    root: &Path,
    cands: Vec<Candidate>,
    trees: &TreeCache,
    s: &ContextSettings,
    deadline: Instant,
    header: &str,
) -> Built {
    let mut ctx = Ctx {
        trees,
        deadline,
        files_parsed: 0,
        tree_hits: 0,
    };
    let mut out = Built::default();
    let header_cost = estimate_text(header);
    let mut used = header_cost;
    let mut seen_scopes: HashSet<(std::path::PathBuf, usize, usize)> = HashSet::new();
    for c in cands {
        if out.items.len() >= s.max_results {
            out.omitted_max += 1;
            continue;
        }
        let (mut snippet, enclosing_sig) = snippet_for(&c, s, &mut ctx);
        if let Some(sn) = &snippet {
            let key = (
                c.doc.path.clone(),
                sn.scope_range.start_byte,
                sn.scope_range.end_byte,
            );
            // The same scope twice adds nothing.
            if sn.scope != "lines" && !seen_scopes.insert(key) {
                snippet = None;
            }
        }
        let range = source_range(&c.doc, c.start, c.end);
        let m = c.doc.mapper();
        let line_text = m
            .line_text(range.start.line)
            .map(|t| syntax::truncate_chars(t.trim_end(), 300))
            .unwrap_or_default();
        let path = rel_path(root, &c.doc.path);
        let mut item = CodeItem {
            id: item_id(&path, &range, &c.doc.revision),
            uri: cersei_lsp::path_to_uri(&c.doc.path),
            path,
            range,
            revision: c.doc.revision.clone(),
            symbol: c.symbol.clone(),
            relation: c.relation,
            certainty: c.certainty,
            freshness: c.freshness.clone(),
            score: c.score,
            line_text,
            signature: c.signature.clone().or(enclosing_sig),
            snippet,
            documentation: c.documentation.clone(),
            provenance: c.provenance.clone(),
        };
        let n = out.items.len() + 1;
        let cost = estimate_text(&render::render_item(n, &item));
        if used.upper + cost.upper <= s.budget_tokens {
            used += cost;
            out.items.push(item);
            continue;
        }
        out.budget.truncated = true;
        // Degrade: drop the snippet, keep the location.
        if item.snippet.is_some() {
            item.snippet = None;
            let cost = estimate_text(&render::render_item(n, &item));
            if used.upper + cost.upper <= s.budget_tokens {
                used += cost;
                out.items.push(item);
                continue;
            }
        }
        out.omitted_budget += 1;
    }
    out.budget.limit_tokens = s.budget_tokens;
    out.budget.used_tokens = used.tokens;
    out.budget.used_upper_tokens = used.upper;
    out.budget.method = ESTIMATION_METHOD.to_string();
    out.files_parsed = ctx.files_parsed;
    out.tree_hits = ctx.tree_hits;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{Backend, Certainty, Provenance};
    use crate::view::DocSource;
    use std::sync::Arc;
    use std::time::Duration;

    fn doc(p: &str, t: &str) -> Arc<Document> {
        Arc::new(Document::new(p.into(), Arc::from(t), DocSource::Disk, None))
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    fn cand(d: &Arc<Document>, at: &str, rel: Relation) -> Candidate {
        let s = d.text.find(at).unwrap();
        Candidate::new(
            d.clone(),
            s,
            s + at.len(),
            rel,
            Certainty::Textual,
            Provenance {
                backend: Backend::Lexical,
                method: "literal".into(),
            },
        )
    }

    fn settings(policy: ContextPolicy, detail: Detail, budget: u64) -> ContextSettings {
        ContextSettings {
            policy,
            detail,
            budget_tokens: budget,
            max_results: 50,
        }
    }

    #[test]
    fn large_function_is_excerpted_with_original_ranges() {
        let mut src = String::from("fn big(a: u32) -> u32 {\n");
        for i in 0..100 {
            src.push_str(&format!("    let v{i} = a + {i};\n"));
        }
        src.push_str("    TARGET\n}\n");
        let d = doc("/w/a.rs", &src);
        let trees = TreeCache::new(4);
        let b = build(
            Path::new("/w"),
            vec![cand(&d, "TARGET", Relation::TextMention)],
            &trees,
            &settings(ContextPolicy::Function, Detail::Normal, 10_000),
            far(),
            "",
        );
        let sn = b.items[0].snippet.as_ref().unwrap();
        assert_eq!(sn.kind, SnippetKind::Excerpt);
        assert!(sn.omitted_lines > 80);
        assert!(sn.parts[0].text.starts_with("fn big(a: u32) -> u32 {"));
        assert!(sn.parts.iter().any(|p| p.text.contains("TARGET")));
        assert!(sn.parts.last().unwrap().text.ends_with('}'));
        // Parts are verbatim at their ranges.
        for p in &sn.parts {
            assert_eq!(&src[p.range.start_byte..p.range.end_byte], p.text);
        }
    }

    #[test]
    fn unsupported_language_falls_back_to_lines() {
        let d = doc("/w/notes.md", "a\nb\nTARGET here\nc\n");
        let trees = TreeCache::new(4);
        let b = build(
            Path::new("/w"),
            vec![cand(&d, "TARGET", Relation::TextMention)],
            &trees,
            &settings(ContextPolicy::Function, Detail::Normal, 10_000),
            far(),
            "",
        );
        let sn = b.items[0].snippet.as_ref().unwrap();
        assert_eq!(sn.scope, "lines");
        assert!(sn.syntax_note.as_ref().unwrap().contains("no grammar"));
    }

    #[test]
    fn budget_counts_everything_and_reports_omissions() {
        let mut src = String::new();
        for i in 0..40 {
            src.push_str(&format!("fn f{i}() {{ let needle = {i}; }}\n"));
        }
        let d = doc("/w/a.rs", &src);
        let cands: Vec<Candidate> = (0..40)
            .map(|i| {
                let s = src.find(&format!("fn f{i}()")).unwrap() + 3;
                Candidate::new(
                    d.clone(),
                    s,
                    s + 2,
                    Relation::TextMention,
                    Certainty::Textual,
                    Provenance {
                        backend: Backend::Lexical,
                        method: "literal".into(),
                    },
                )
            })
            .collect();
        let trees = TreeCache::new(4);
        let b = build(
            Path::new("/w"),
            cands,
            &trees,
            &settings(ContextPolicy::Auto, Detail::Normal, 600),
            far(),
            "header",
        );
        assert!(b.budget.truncated);
        assert!(b.omitted_budget > 0);
        assert!(b.budget.used_upper_tokens <= 600);
        assert_eq!(b.items.len() + b.omitted_budget, 40);
        assert!(b.budget.method.contains("heuristic"));
        // Detail changes the amount of context.
        let compact = build(
            Path::new("/w"),
            vec![cand(&d, "needle = 5", Relation::TextMention)],
            &trees,
            &settings(ContextPolicy::Auto, Detail::Compact, 10_000),
            far(),
            "",
        );
        let deep = build(
            Path::new("/w"),
            vec![cand(&d, "needle = 5", Relation::TextMention)],
            &trees,
            &settings(ContextPolicy::Auto, Detail::Deep, 10_000),
            far(),
            "",
        );
        let lines = |b: &Built| {
            b.items[0].snippet.as_ref().unwrap().parts[0]
                .text
                .lines()
                .count()
        };
        assert!(lines(&compact) < lines(&deep));
        assert!(compact.items[0].signature.is_none());
        assert!(deep.items[0]
            .signature
            .as_deref()
            .unwrap()
            .starts_with("fn f5()"));
    }
}
