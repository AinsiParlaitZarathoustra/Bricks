//! Aggregation and ranking.
//!
//! Merging: two findings are one item only when they have the same
//! document version, the same range, the same relation and the same symbol
//! name; their provenances are kept. Homonyms, different relations and
//! different versions stay separate.
//!
//! Ranking weights are heuristics, chosen per intent (table below), and
//! the score is only an order: it is not the probability that an item is
//! right.
//!
//! | relation      | text_search | find_symbol / definition | references | understand |
//! |---------------|-------------|--------------------------|------------|------------|
//! | definition    | 0.9         | 1.0                      | 0.6        | 1.0        |
//! | candidate     | 0.9         | 0.95                     | 0.5        | 0.9        |
//! | reference     | 0.8         | 0.3                      | 1.0        | 0.6        |
//! | text_mention  | 1.0         | 0.2                      | 0.4        | 0.3        |
//! | context       | 0.5         | 0.5                      | 0.5        | 1.5        |
//! | diagnostic    | 0.5         | 0.5                      | 0.5        | 0.8        |
//!
//! Bonuses: certainty (confirmed +0.30, syntactic +0.15; not for
//! `text_search`, where the exact string comes first), exact-case name
//! (+0.10), the requester's active file (+0.10) or its folder (+0.05).
//! Ties: path, then offset.

use crate::query::Intent;
use crate::result::{Certainty, Freshness, Provenance, Relation, SymbolRef};
use crate::view::Document;
use std::path::Path;
use std::sync::Arc;

/// A finding before context selection.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub doc: Arc<Document>,
    pub start: usize,
    pub end: usize,
    pub relation: Relation,
    pub certainty: Certainty,
    pub symbol: Option<SymbolRef>,
    pub signature: Option<String>,
    pub documentation: Option<String>,
    pub provenance: Vec<Provenance>,
    pub freshness: Freshness,
    /// Explicit scope for the context (a definition's node).
    pub scope: Option<(usize, usize, String)>,
    pub score: f32,
}

impl Candidate {
    pub fn new(
        doc: Arc<Document>,
        start: usize,
        end: usize,
        relation: Relation,
        certainty: Certainty,
        prov: Provenance,
    ) -> Self {
        Self {
            doc,
            start,
            end,
            relation,
            certainty,
            symbol: None,
            signature: None,
            documentation: None,
            provenance: vec![prov],
            freshness: Freshness::Current,
            scope: None,
            score: 0.0,
        }
    }
}

fn relation_weight(intent: Intent, r: Relation) -> f32 {
    use Relation::*;
    match intent {
        Intent::TextSearch | Intent::Auto => match r {
            TextMention => 1.0,
            Definition | Declaration | Candidate => 0.9,
            Reference => 0.8,
            _ => 0.5,
        },
        Intent::FindSymbol | Intent::Definition => match r {
            Definition => 1.0,
            Candidate => 0.95,
            Declaration => 0.9,
            Reference => 0.3,
            TextMention => 0.2,
            _ => 0.5,
        },
        Intent::References => match r {
            Reference => 1.0,
            Definition | Declaration => 0.6,
            Candidate => 0.5,
            TextMention => 0.4,
            _ => 0.5,
        },
        Intent::Understand | Intent::Diagnostics => match r {
            // The place asked about comes first.
            Context => 1.5,
            Definition | Declaration => 1.0,
            Candidate => 0.9,
            Diagnostic => 0.8,
            Reference => 0.6,
            TextMention => 0.3,
            Documentation => 0.7,
        },
    }
}

/// Score candidates for `intent` (heuristic order, see module docs).
pub fn score(
    intent: Intent,
    query_name: Option<&str>,
    active: Option<&Path>,
    cands: &mut [Candidate],
) {
    for c in cands.iter_mut() {
        let mut s = relation_weight(intent, c.relation);
        if !matches!(intent, Intent::TextSearch | Intent::Auto) {
            s += match c.certainty {
                Certainty::Confirmed => 0.30,
                Certainty::Syntactic => 0.15,
                Certainty::Textual => 0.0,
            };
        }
        if let (Some(q), Some(sym)) = (query_name, &c.symbol) {
            if sym.name == q {
                s += 0.10;
            }
        }
        if let Some(a) = active {
            if c.doc.path == a {
                s += 0.10;
            } else if c.doc.path.parent() == a.parent() {
                s += 0.05;
            }
        }
        c.score = s;
    }
}

/// Document, revision, range, relation, symbol name.
type MergeKey = (
    std::path::PathBuf,
    String,
    usize,
    usize,
    Relation,
    Option<String>,
);

/// Merge identical findings (see module docs) and sort by score.
pub fn merge_and_sort(cands: Vec<Candidate>) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::with_capacity(cands.len());
    let mut index: std::collections::HashMap<MergeKey, usize> = std::collections::HashMap::new();
    for c in cands {
        let key = (
            c.doc.path.clone(),
            c.doc.revision.hash.clone(),
            c.start,
            c.end,
            c.relation,
            c.symbol.as_ref().map(|s| s.name.clone()),
        );
        match index.get(&key) {
            Some(&i) => {
                let e = &mut out[i];
                for p in c.provenance {
                    if !e.provenance.contains(&p) {
                        e.provenance.push(p);
                    }
                }
                if c.certainty > e.certainty {
                    e.certainty = c.certainty;
                }
                e.score = e.score.max(c.score);
                if e.signature.is_none() {
                    e.signature = c.signature;
                }
                if e.documentation.is_none() {
                    e.documentation = c.documentation;
                }
                if e.scope.is_none() {
                    e.scope = c.scope;
                }
            }
            None => {
                index.insert(key, out.len());
                out.push(c);
            }
        }
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc.path.cmp(&b.doc.path))
            .then_with(|| a.start.cmp(&b.start))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::Backend;
    use crate::view::DocSource;

    fn doc(p: &str, t: &str) -> Arc<Document> {
        Arc::new(Document::new(p.into(), Arc::from(t), DocSource::Disk, None))
    }

    fn prov(b: Backend, m: &str) -> Provenance {
        Provenance {
            backend: b,
            method: m.into(),
        }
    }

    #[test]
    fn merges_same_finding_keeps_homonyms_and_relations_apart() {
        let d = doc("/w/a.rs", "fn x() {}\nfn x() {}\n");
        let mut a = Candidate::new(
            d.clone(),
            3,
            4,
            Relation::Definition,
            Certainty::Syntactic,
            prov(Backend::Syntax, "tree-sitter"),
        );
        a.symbol = Some(SymbolRef {
            name: "x".into(),
            kind: "function".into(),
        });
        let mut b = a.clone();
        b.certainty = Certainty::Confirmed;
        b.provenance = vec![prov(Backend::Lsp, "textDocument/definition")];
        // Homonym: same name, other range.
        let mut c = a.clone();
        c.start = 13;
        c.end = 14;
        // Same range, other relation.
        let mut m = a.clone();
        m.relation = Relation::TextMention;
        let out = merge_and_sort(vec![a, b, c, m]);
        assert_eq!(out.len(), 3);
        let merged = out
            .iter()
            .find(|c| c.start == 3 && c.relation == Relation::Definition)
            .unwrap();
        assert_eq!(merged.provenance.len(), 2);
        assert_eq!(merged.certainty, Certainty::Confirmed);
    }

    #[test]
    fn ranking_follows_intent() {
        let d = doc("/w/a.rs", "fn foo() {}\nfoo();\n");
        let def = Candidate::new(
            d.clone(),
            3,
            6,
            Relation::Definition,
            Certainty::Confirmed,
            prov(Backend::Lsp, "d"),
        );
        let r#ref = Candidate::new(
            d.clone(),
            12,
            15,
            Relation::Reference,
            Certainty::Confirmed,
            prov(Backend::Lsp, "r"),
        );
        let txt = Candidate::new(
            d,
            12,
            15,
            Relation::TextMention,
            Certainty::Textual,
            prov(Backend::Lexical, "t"),
        );
        let mut v = vec![def.clone(), r#ref.clone()];
        score(Intent::References, None, None, &mut v);
        assert_eq!(merge_and_sort(v)[0].relation, Relation::Reference);
        let mut v = vec![def.clone(), r#ref];
        score(Intent::Definition, None, None, &mut v);
        assert_eq!(merge_and_sort(v)[0].relation, Relation::Definition);
        let mut v = vec![txt, def];
        score(Intent::TextSearch, None, None, &mut v);
        // For a text search, the exact string comes first.
        assert_eq!(merge_and_sort(v)[0].relation, Relation::TextMention);
    }
}
