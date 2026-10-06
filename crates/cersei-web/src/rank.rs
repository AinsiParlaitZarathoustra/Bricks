//! Lexical ranking of passages with BM25 and selection under a budget.
//!
//! The index is built for one search (the passages of its pages) and thrown
//! away. BM25 is lexical: it rewards passages sharing the query's terms,
//! weighted by rarity; it does not understand paraphrases, and a zero score
//! means "no shared term", not "no information". Scores order passages
//! within one search only: they are not probabilities and are not comparable
//! across searches.
//!
//! The tokenizer is the same for queries and passages, French and English
//! alike: lower-case, accents folded, a short stop-word list, plural `s`
//! removed on longer words, and code identifiers kept whole *and* split
//! (`max_retries` → `max_retries`, `max`, `retries`; `HttpClient` →
//! `httpclient`, `http`, `client`; `rate-limit` → `rate-limit`, `rate`,
//! `limit`; `v2.1` stays `v2.1`).
//!
//! Selection takes the best passages within `budget_chars` and
//! `max_passages`, at most `per_source` per page on a first pass (diversity),
//! adds the preceding passage when a selected one starts mid-thought, and
//! drops near-duplicates — but never two statements that differ by a number,
//! version, date or negation.

use crate::chunk::Passage;
use crate::config::PassageConfig;
use bm25::{Embedder, EmbedderBuilder, Scorer, Tokenizer};
use std::collections::{BTreeSet, HashSet};

/// The tokenizer described in the module documentation.
#[derive(Debug, Clone, Copy, Default)]
pub struct LexTokenizer;

impl Tokenizer for LexTokenizer {
    fn tokenize(&self, input_text: &str) -> Vec<String> {
        tokenize(input_text)
    }
}

const STOP: &[&str] = &[
    // English
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "have", "in", "is", "it",
    "its", "of", "on", "or", "that", "the", "this", "to", "was", "were", "will", "with", "what",
    "which", "how", "do", "does", "can", // French
    "au", "aux", "ce", "ces", "cette", "dans", "de", "des", "du", "elle", "en", "est", "et", "il",
    "la", "le", "les", "leur", "lui", "ou", "par", "pour", "qu", "que", "qui", "se", "sur", "un",
    "une", "comment", "quel", "quelle", "quels", "quelles", "d", "l",
];

pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.')) {
        let raw = raw.trim_matches(|c| c == '.' || c == '-' || c == '_');
        if raw.is_empty() {
            continue;
        }
        let whole = norm(raw);
        let mut parts: Vec<String> = Vec::new();
        // camelCase / PascalCase parts (on the original case).
        let camel = camel_parts(raw);
        if camel.len() > 1 {
            parts.extend(camel.iter().map(|p| norm(p)));
        }
        if raw.contains(['_', '-']) || (raw.contains('.') && !is_version(raw)) {
            for p in raw.split(['_', '-', '.']) {
                if p.chars().count() >= 2 {
                    parts.push(norm(p));
                    let c = camel_parts(p);
                    if c.len() > 1 {
                        parts.extend(c.iter().map(|x| norm(x)));
                    }
                }
            }
        }
        let keep_whole = !STOP.contains(&whole.as_str()) && whole.chars().count() >= 2
            || whole.chars().any(|c| c.is_ascii_digit());
        if keep_whole {
            out.push(stem(&whole));
        }
        let mut seen = HashSet::new();
        for p in parts {
            if p != whole && !STOP.contains(&p.as_str()) && seen.insert(p.clone()) {
                out.push(stem(&p));
            }
        }
    }
    out
}

fn is_version(s: &str) -> bool {
    let s = s.trim_start_matches(['v', 'V']);
    !s.is_empty()
        && s.split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn camel_parts(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    for i in 0..chars.len() {
        let c = chars[i];
        let boundary = i > 0
            && c.is_uppercase()
            && (chars[i - 1].is_lowercase()
                || (chars.get(i + 1).is_some_and(|n| n.is_lowercase())
                    && chars[i - 1].is_uppercase()));
        if boundary && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        if c.is_alphanumeric() {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => out.push('a'),
            'ç' => out.push('c'),
            'è' | 'é' | 'ê' | 'ë' => out.push('e'),
            'ì' | 'í' | 'î' | 'ï' => out.push('i'),
            'ñ' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => out.push('o'),
            'ù' | 'ú' | 'û' | 'ü' => out.push('u'),
            'ý' | 'ÿ' => out.push('y'),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            'ß' => out.push_str("ss"),
            '’' => out.push('\''),
            c => out.push(c),
        }
    }
    out
}

/// Plural `s`/`x` of longer alphabetic words (both languages).
fn stem(t: &str) -> String {
    if t.chars().count() > 4 && t.chars().all(|c| c.is_alphabetic()) {
        if let Some(s) = t.strip_suffix('s') {
            if !s.ends_with('s') {
                return s.to_string();
            }
        }
    }
    t.to_string()
}

/// A passage of one of the pages of the search.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// Index of the page.
    pub source: usize,
    pub passage: &'a Passage,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Scored {
    pub source: usize,
    /// `Passage::index` within its page.
    pub passage: usize,
    pub score: f32,
    /// Distinct query terms present, over distinct query terms.
    pub coverage: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Relevance {
    /// At least one passage shares a good part of the query's terms.
    Found,
    /// Some terms match, but no passage covers a third of the query.
    Weak,
    /// No passage shares any query term.
    None,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Picked {
    pub scored: Scored,
    /// Added as the context preceding another pick (its passage index).
    pub context_for: Option<usize>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Selection {
    pub relevance: Relevance,
    /// In presentation order (context before the passage it precedes).
    pub picked: Vec<Picked>,
    pub considered: usize,
    /// Near-duplicates left out: `(source, passage)` and the kept one.
    pub duplicates: Vec<((usize, usize), (usize, usize))>,
    /// Good passages left out because the budget was spent.
    pub left_for_budget: usize,
}

/// Score every candidate (deterministic order: score, then page, then
/// position).
pub fn score(query: &str, candidates: &[Candidate<'_>]) -> Vec<Scored> {
    let corpus: Vec<&str> = candidates.iter().map(|c| c.passage.text.as_str()).collect();
    let embedder: Embedder<u32, LexTokenizer> =
        EmbedderBuilder::with_tokenizer_and_fit_to_corpus(LexTokenizer, &corpus).build();
    let mut scorer = Scorer::<usize, u32>::new();
    for (i, c) in candidates.iter().enumerate() {
        scorer.upsert(&i, embedder.embed(&c.passage.text));
    }
    let q_embed = embedder.embed(query);
    let q_terms: BTreeSet<String> = tokenize(query).into_iter().collect();
    let mut out: Vec<Scored> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let score = scorer.score(&i, &q_embed).unwrap_or(0.0);
            let terms: HashSet<String> = tokenize(&c.passage.text).into_iter().collect();
            let present = q_terms.iter().filter(|t| terms.contains(*t)).count();
            Scored {
                source: c.source,
                passage: c.passage.index,
                score: if score.is_finite() { score } else { 0.0 },
                coverage: if q_terms.is_empty() {
                    0.0
                } else {
                    present as f32 / q_terms.len() as f32
                },
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.source.cmp(&b.source))
            .then(a.passage.cmp(&b.passage))
    });
    out
}

/// Choose passages for `query` among the pages' passages.
pub fn select(query: &str, pages: &[Vec<Passage>], cfg: &PassageConfig) -> Selection {
    let candidates: Vec<Candidate<'_>> = pages
        .iter()
        .enumerate()
        .flat_map(|(source, ps)| ps.iter().map(move |passage| Candidate { source, passage }))
        .collect();
    let scored = score(query, &candidates);
    let best_cov = scored.iter().map(|s| s.coverage).fold(0.0f32, f32::max);
    let relevance = if scored.iter().all(|s| s.score <= 0.0) {
        Relevance::None
    } else if best_cov < 0.34 {
        Relevance::Weak
    } else {
        Relevance::Found
    };
    let mut sel = Selection {
        relevance,
        picked: Vec::new(),
        considered: candidates.len(),
        duplicates: Vec::new(),
        left_for_budget: 0,
    };
    if relevance == Relevance::None {
        return sel;
    }
    let text = |src: usize, idx: usize| pages[src].get(idx).map(|p| p.text.as_str()).unwrap_or("");
    let mut used = 0usize;
    let mut per_source = vec![0usize; pages.len()];
    let mut taken: HashSet<(usize, usize)> = HashSet::new();
    let pool: Vec<&Scored> = scored.iter().filter(|s| s.score > 0.0).collect();
    for pass in 0..2 {
        for s in &pool {
            if sel
                .picked
                .iter()
                .filter(|p| p.context_for.is_none())
                .count()
                >= cfg.max_passages
            {
                break;
            }
            if taken.contains(&(s.source, s.passage)) {
                continue;
            }
            if pass == 0 && per_source[s.source] >= cfg.per_source {
                continue;
            }
            let t = text(s.source, s.passage);
            if let Some(kept) = sel
                .picked
                .iter()
                .map(|p| (p.scored.source, p.scored.passage))
                .find(|(ks, kp)| near_duplicate(text(*ks, *kp), t))
            {
                sel.duplicates.push(((s.source, s.passage), kept));
                taken.insert((s.source, s.passage));
                continue;
            }
            let len = t.chars().count();
            if used + len > cfg.budget_chars {
                if pass == 1 {
                    sel.left_for_budget += 1;
                }
                continue;
            }
            // The passage before, when this one starts mid-thought.
            let mut context = None;
            if s.passage > 0 && starts_mid_thought(t) && !taken.contains(&(s.source, s.passage - 1))
            {
                let prev = &pages[s.source][s.passage - 1];
                let plen = prev.text.chars().count();
                if prev.section == pages[s.source][s.passage].section
                    && used + len + plen <= cfg.budget_chars
                {
                    context = Some(Picked {
                        scored: Scored {
                            source: s.source,
                            passage: s.passage - 1,
                            score: 0.0,
                            coverage: 0.0,
                        },
                        context_for: Some(s.passage),
                    });
                    used += plen;
                    taken.insert((s.source, s.passage - 1));
                }
            }
            if let Some(c) = context {
                sel.picked.push(c);
            }
            sel.picked.push(Picked {
                scored: (*s).clone(),
                context_for: None,
            });
            used += len;
            per_source[s.source] += 1;
            taken.insert((s.source, s.passage));
        }
    }
    sel
}

fn starts_mid_thought(t: &str) -> bool {
    let first = t.trim_start();
    if first.starts_with('#') || first.starts_with('|') || first.starts_with("```") {
        return false;
    }
    let lower_start = first.chars().next().is_some_and(|c| c.is_lowercase());
    let word: String = first
        .chars()
        .take_while(|c| c.is_alphabetic())
        .collect::<String>()
        .to_lowercase();
    lower_start
        || matches!(
            word.as_str(),
            "this"
                | "it"
                | "these"
                | "they"
                | "that"
                | "however"
                | "otherwise"
                | "then"
                | "ce"
                | "cela"
                | "ceci"
                | "il"
                | "elle"
                | "ils"
                | "elles"
                | "sinon"
                | "ensuite"
                | "cependant"
        )
}

/// Same content, give or take wording — and the same facts.
pub fn near_duplicate(a: &str, b: &str) -> bool {
    if facts(a) != facts(b) {
        return false;
    }
    let ta: HashSet<String> = tokenize(a).into_iter().collect();
    let tb: HashSet<String> = tokenize(b).into_iter().collect();
    if ta.is_empty() || tb.is_empty() {
        return a.trim() == b.trim();
    }
    let inter = ta.intersection(&tb).count() as f32;
    let union = ta.union(&tb).count() as f32;
    inter / union >= 0.85
}

/// Numbers, versions, dates and negations of a text: what must match for two
/// passages to count as the same statement.
fn facts(t: &str) -> (BTreeSet<String>, usize) {
    let lower = norm(t);
    let numbers: BTreeSet<String> = lower
        .split(|c: char| {
            !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '/' || c == ':')
        })
        .map(|w| w.trim_matches(|c| c == '.' || c == '-'))
        .filter(|w| w.chars().any(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .collect();
    let negations = lower
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| {
            matches!(
                *w,
                "not"
                    | "no"
                    | "never"
                    | "none"
                    | "cannot"
                    | "without"
                    | "don't"
                    | "doesn't"
                    | "isn't"
                    | "aren't"
                    | "won't"
                    | "can't"
                    | "ne"
                    | "n'"
                    | "pas"
                    | "jamais"
                    | "aucun"
                    | "aucune"
                    | "sans"
                    | "non"
                    | "plus"
            )
        })
        .count()
        + lower.matches("n't").count();
    (numbers, negations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::split;

    #[test]
    fn identifiers_and_languages_tokenize_consistently() {
        let t = tokenize("Configure max_retries in HttpClient (rate-limit, v2.1)");
        for w in [
            "configure",
            "max_retries",
            "max",
            "retrie",
            "httpclient",
            "http",
            "client",
            "rate-limit",
            "rate",
            "limit",
            "v2.1",
        ] {
            assert!(t.contains(&w.to_string()), "{w} missing from {t:?}");
        }
        let fr = tokenize("Les délais d'expiration de la requête");
        assert!(fr.contains(&"delai".to_string()), "{fr:?}");
        assert!(fr.contains(&"requete".to_string()), "{fr:?}");
        assert!(!fr.contains(&"les".to_string()));
        // Query and text meet on the same form.
        assert!(tokenize("requêtes").iter().any(|q| fr.contains(q)));
    }

    #[test]
    fn statements_differing_by_facts_are_not_duplicates() {
        let a = "The default timeout is 30 seconds since version 2.1 of the client.";
        assert!(near_duplicate(
            a,
            "The default timeout is 30 seconds since version 2.1 of the client!"
        ));
        assert!(!near_duplicate(
            a,
            "The default timeout is 60 seconds since version 2.1 of the client."
        ));
        assert!(!near_duplicate(
            a,
            "The default timeout is 30 seconds since version 2.2 of the client."
        ));
        assert!(!near_duplicate(
            "Retries are enabled for idempotent requests.",
            "Retries are not enabled for idempotent requests."
        ));
        assert!(!near_duplicate(
            "Le cache est activé par défaut.",
            "Le cache n'est pas activé par défaut."
        ));
    }

    fn cfg() -> PassageConfig {
        PassageConfig {
            min_chars: 40,
            max_chars: 300,
            max_passages: 3,
            budget_chars: 900,
            per_source: 1,
        }
    }

    #[test]
    fn selection_finds_the_api_passage_and_spreads_sources() {
        let p1 = split(
            "# Client\n\n## Install\n\nAdd the crate to Cargo.toml and build the project once.\n\n## Retries\n\nSet `max_retries` on the builder to retry idempotent requests; the default is 3.\n",
            40,
            300,
        );
        let p2 = split(
            "# FAQ\n\n## Retry behaviour\n\nThe client retries idempotent requests up to max_retries times with backoff.\n\n## Licence\n\nMIT.\n",
            40,
            300,
        );
        let sel = select(
            "how to configure max_retries",
            &[p1.clone(), p2.clone()],
            &cfg(),
        );
        assert_eq!(sel.relevance, Relevance::Found);
        let first = &sel.picked[0].scored;
        let text = if first.source == 0 {
            &p1[first.passage].text
        } else {
            &p2[first.passage].text
        };
        assert!(text.contains("max_retries"), "{text}");
        let sources: HashSet<usize> = sel.picked.iter().map(|p| p.scored.source).collect();
        assert_eq!(sources.len(), 2, "both pages contribute: {sel:?}");
    }

    #[test]
    fn no_shared_term_is_an_explicit_state() {
        let p = split(
            "# Cooking\n\nBoil the pasta in salted water for nine minutes.\n",
            10,
            300,
        );
        let sel = select("kubernetes ingress annotations", &[p], &cfg());
        assert_eq!(sel.relevance, Relevance::None);
        assert!(sel.picked.is_empty());
    }

    #[test]
    fn redundant_sources_are_deduplicated_but_not_contradictions() {
        let same = "## Timeout\n\nThe request timeout defaults to 30 seconds for every call made by the client.\n";
        let other = "## Timeout\n\nThe request timeout defaults to 60 seconds for every call made by the client.\n";
        let pages = vec![
            split(same, 10, 300),
            split(same, 10, 300),
            split(other, 10, 300),
        ];
        let sel = select("request timeout default", &pages, &cfg());
        let kept: Vec<usize> = sel.picked.iter().map(|p| p.scored.source).collect();
        assert_eq!(sel.duplicates.len(), 1, "{sel:?}");
        assert!(kept.contains(&2), "the 60 s statement is kept: {kept:?}");
    }

    #[test]
    fn a_small_budget_is_respected_and_reported() {
        let long = "# A\n\n".to_string() + &"timeout value explained here. ".repeat(9);
        let pages = vec![
            split(&long, 40, 300),
            split(&long.replace("A", "B"), 40, 300),
        ];
        let mut c = cfg();
        c.budget_chars = 300;
        c.per_source = 2;
        let sel = select("timeout value", &pages, &c);
        let total: usize = sel
            .picked
            .iter()
            .map(|p| {
                pages[p.scored.source][p.scored.passage]
                    .text
                    .chars()
                    .count()
            })
            .sum();
        assert!(total <= 300);
    }
}
