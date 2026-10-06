//! Controlled tolerance for text replacement.
//!
//! Three stages, in order; the first that finds a *unique* match is applied:
//!
//! 1. **Exact.** `old` occurs verbatim. Several occurrences are an
//!    ambiguity unless `replace_all` is set.
//! 2. **Normalised**, line by line, with explicit rules only: line endings
//!    (LF/CRLF), trailing whitespace, and indentation shifted by one constant
//!    amount for the whole block (tabs or spaces), which preserves relative
//!    indentation (Python). Lines inside multi-line strings, template
//!    literals, raw strings and here-documents are compared exactly: their
//!    whitespace is content.
//! 3. **Relocation**, bounded: the same lines, allowing only blank lines to
//!    differ and runs of spaces to differ *outside* quoted text. It finds a
//!    block whose layout drifted; it never accepts a block whose code
//!    differs, however similar.
//!
//! The replacement is adapted to the block found on disk: its indentation is
//! shifted by the same amount, its line endings follow the file's. Bytes
//! outside the block are untouched.
//!
//! On failure, the best candidate regions are returned with their line
//! ranges, an excerpt and the reason, so the caller can say *why*: absent,
//! ambiguous, or present but differing in code. Nothing is guessed.

use similar::TextDiff;

/// Files with more lines than this are not searched beyond stage 2.
const RELOCATION_MAX_LINES: usize = 100_000;
/// Blocks with more lines than this are not relocated.
const RELOCATION_MAX_BLOCK: usize = 400;
/// Candidates reported on failure.
const MAX_CANDIDATES: usize = 3;

/// A region of the file shown when an edit is refused.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// 1-based, inclusive.
    pub start_line: usize,
    pub end_line: usize,
    pub similarity: f32,
    pub excerpt: String,
    pub reason: String,
}

/// How the match was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Exact,
    /// The rules that had to be applied.
    Normalized(Vec<String>),
    Relocated(Vec<String>),
}

impl Stage {
    pub fn describe(&self) -> String {
        match self {
            Stage::Exact => "exact match".into(),
            Stage::Normalized(r) => format!("normalised match ({})", r.join(", ")),
            Stage::Relocated(r) => format!("relocated block ({})", r.join(", ")),
        }
    }
}

/// A resolved edit.
#[derive(Debug, Clone, PartialEq)]
pub struct EditPlan {
    pub content: String,
    pub stage: Stage,
    pub replacements: usize,
    /// Replaced regions in the original, 1-based inclusive line ranges.
    pub ranges: Vec<(usize, usize)>,
}

/// Why an edit is refused. The content is never modified in these cases.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplaceError {
    /// No admissible match. `candidates` are the closest regions, each with
    /// the reason it was not used (empty when nothing resembles `old`).
    NotFound { candidates: Vec<Candidate> },
    /// Several regions match and `replace_all` is false.
    Ambiguous {
        count: usize,
        candidates: Vec<Candidate>,
    },
    /// `old` and `new` are identical.
    NoChange,
    /// `old` is empty but the content is not.
    EmptyOldString,
}

/// Perform a replacement and return the new content.
pub fn replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, ReplaceError> {
    plan_edit(content, old, new, replace_all, None).map(|p| p.content)
}

/// A model-facing explanation of a refused edit (no bytes were changed).
pub fn describe_failure(err: &ReplaceError, file: &str) -> String {
    let show = |cands: &[Candidate]| -> String {
        cands
            .iter()
            .map(|c| {
                format!(
                    "- lines {}–{}: {}\n{}",
                    c.start_line, c.end_line, c.reason, c.excerpt
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    match err {
        ReplaceError::NotFound { candidates } if candidates.is_empty() => format!(
            "old_string was not found in {file} (absent): no region resembles it. Re-read the file \
             and copy old_string from its current text."
        ),
        ReplaceError::NotFound { candidates } => format!(
            "old_string does not match {file} (differences are not admissible: only line endings, \
             trailing whitespace, a uniform indentation shift, blank lines and spacing outside \
             quoted text are tolerated). Closest regions:\n{}\nRead those lines and copy \
             old_string exactly before editing.",
            show(candidates)
        ),
        ReplaceError::Ambiguous { count, candidates } => format!(
            "old_string matches {count} regions of {file} (ambiguous). Add surrounding lines so \
             it identifies exactly one, or set replace_all=true to change all of them. Matches:\n{}",
            show(candidates)
        ),
        ReplaceError::NoChange => {
            "old_string and new_string are identical, so the edit would do nothing.".into()
        }
        ReplaceError::EmptyOldString => format!(
            "old_string is empty but {file} is not: an empty anchor cannot locate an edit."
        ),
    }
}

// ─── Line model ──────────────────────────────────────────────────────────────

struct Line<'a> {
    /// Without its terminator (and without a trailing `\r`).
    text: &'a str,
    start: usize,
    /// End of `text` (start of the terminator).
    end: usize,
    /// End including the terminator.
    full_end: usize,
}

fn split_lines(content: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = content.as_bytes();
    while start < content.len() {
        let nl = content[start..].find('\n').map(|p| start + p);
        let (end_nl, full_end) = match nl {
            Some(p) => (p, p + 1),
            None => (content.len(), content.len()),
        };
        let end = if end_nl > start && bytes[end_nl - 1] == b'\r' {
            end_nl - 1
        } else {
            end_nl
        };
        out.push(Line {
            text: &content[start..end],
            start,
            end,
            full_end,
        });
        start = full_end;
    }
    out
}

fn uses_crlf(content: &str) -> bool {
    let crlf = content.matches("\r\n").count();
    let lf = content.matches('\n').count();
    crlf * 2 > lf
}

/// Lines whose whitespace is content: inside (or opening/closing) multi-line
/// strings, template literals, raw strings and here-documents.
fn sensitive_lines(lines: &[Line], ext: Option<&str>) -> Vec<bool> {
    let shellish = matches!(
        ext,
        None | Some("sh" | "bash" | "zsh" | "ksh" | "rb" | "pl" | "pm" | "php")
    );
    let heredoc =
        regex::Regex::new(r#"<<(-|~)?\s*['"]?([A-Za-z_][A-Za-z0-9_]*)['"]?\s*$"#).expect("regex");
    let mut out = vec![false; lines.len()];
    // Active region: closing delimiter and whether it must be a whole line.
    let mut active: Option<(String, bool)> = None;
    for (i, line) in lines.iter().enumerate() {
        let text = line.text;
        if let Some((close, whole_line)) = active.clone() {
            out[i] = true;
            let closed = if whole_line {
                text.trim() == close
            } else {
                text.contains(close.as_str())
            };
            if closed {
                active = None;
            }
            continue;
        }
        // Openers on this line (the first one that stays open wins).
        for delim in ["\"\"\"", "'''", "`", "r#\"", "r##\""] {
            let count = text.matches(delim).count();
            let close = match delim {
                "r#\"" => "\"#",
                "r##\"" => "\"##",
                d => d,
            };
            let opens = if delim == close {
                count % 2 == 1
            } else {
                count > text.matches(close).count()
            };
            if opens {
                out[i] = true;
                active = Some((close.to_string(), false));
                break;
            }
        }
        if active.is_none() && (shellish || text.trim_end().ends_with(char::is_uppercase)) {
            if let Some(c) = heredoc.captures(text) {
                let word = c[2].to_string();
                if shellish || word.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_') {
                    out[i] = true;
                    active = Some((word, true));
                }
            }
        }
    }
    out
}

fn indent_of(s: &str) -> &str {
    &s[..s.len() - s.trim_start_matches([' ', '\t']).len()]
}

fn width(indent: &str, tab: usize) -> usize {
    indent
        .chars()
        .map(|c| if c == '\t' { tab } else { 1 })
        .sum()
}

/// Collapse runs of spaces/tabs outside quoted text; trim.
fn canonical(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut prev_space = false;
    // Only ASCII blanks are layout; other whitespace (U+3000…) is content.
    for c in line.trim_matches([' ', '\t']).chars() {
        if let Some(q) = quote {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if c == ' ' || c == '\t' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
            continue;
        }
        prev_space = false;
        if c == '"' || c == '\'' || c == '`' {
            quote = Some(c);
        }
        out.push(c);
    }
    out
}

/// Lines of `old`/`new` (CR stripped), without a final empty element.
fn text_lines(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    if v.len() > 1 && v.last() == Some(&"") {
        v.pop();
    }
    v
}

// ─── Planning ────────────────────────────────────────────────────────────────

/// Resolve an edit. `path_hint` (for its extension) tunes the detection of
/// here-documents.
pub fn plan_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    path_hint: Option<&str>,
) -> Result<EditPlan, ReplaceError> {
    if old == new {
        return Err(ReplaceError::NoChange);
    }
    if old.is_empty() {
        if content.is_empty() {
            return Ok(EditPlan {
                content: new.to_string(),
                stage: Stage::Exact,
                replacements: 1,
                ranges: vec![],
            });
        }
        return Err(ReplaceError::EmptyOldString);
    }

    let lines = split_lines(content);
    let line_of = |byte: usize| {
        lines
            .iter()
            .position(|l| byte < l.full_end)
            .unwrap_or(lines.len().saturating_sub(1))
            + 1
    };

    // ── 1. exact ──
    let positions: Vec<usize> = content.match_indices(old).map(|(i, _)| i).collect();
    if positions.len() == 1 || (replace_all && !positions.is_empty()) {
        let ranges = positions
            .iter()
            .map(|&p| (line_of(p), line_of(p + old.len().saturating_sub(1))))
            .collect();
        return Ok(EditPlan {
            content: if replace_all {
                content.replace(old, new)
            } else {
                content.replacen(old, new, 1)
            },
            stage: Stage::Exact,
            replacements: positions.len(),
            ranges,
        });
    }
    if positions.len() > 1 {
        let candidates = positions
            .iter()
            .take(MAX_CANDIDATES)
            .map(|&p| {
                let (a, b) = (line_of(p), line_of(p + old.len().saturating_sub(1)));
                Candidate {
                    start_line: a,
                    end_line: b,
                    similarity: 1.0,
                    excerpt: excerpt(&lines, a, b),
                    reason: "identical occurrence".into(),
                }
            })
            .collect();
        return Err(ReplaceError::Ambiguous {
            count: positions.len(),
            candidates,
        });
    }

    let ext = path_hint
        .and_then(|p| std::path::Path::new(p).extension())
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let sensitive = sensitive_lines(&lines, ext.as_deref());
    let crlf = uses_crlf(content);
    let old_lines = text_lines(old);
    let new_lines = text_lines(new);

    // ── 2. normalised ──
    let single_line = !old.contains('\n');
    if single_line {
        // A fragment of a line that differs only by surrounding whitespace.
        let core = old.trim();
        if !core.is_empty() && core != old {
            let hits: Vec<usize> = content.match_indices(core).map(|(i, _)| i).collect();
            let hits: Vec<usize> = hits
                .into_iter()
                .filter(|&p| !sensitive[line_of(p) - 1])
                .collect();
            if hits.len() == 1 || (replace_all && !hits.is_empty()) {
                let lead = &old[..old.len() - old.trim_start().len()];
                let trail = &old[old.trim_end().len()..];
                let new_core = new.strip_prefix(lead).unwrap_or(new);
                let new_core = new_core.strip_suffix(trail).unwrap_or(new_core);
                let ranges = hits.iter().map(|&p| (line_of(p), line_of(p))).collect();
                return Ok(EditPlan {
                    content: if replace_all {
                        content.replace(core, new_core)
                    } else {
                        content.replacen(core, new_core, 1)
                    },
                    stage: Stage::Normalized(vec!["surrounding whitespace".into()]),
                    replacements: hits.len(),
                    ranges,
                });
            }
            if hits.len() > 1 {
                return Err(ReplaceError::Ambiguous {
                    count: hits.len(),
                    candidates: hits
                        .iter()
                        .take(MAX_CANDIDATES)
                        .map(|&p| {
                            let l = line_of(p);
                            Candidate {
                                start_line: l,
                                end_line: l,
                                similarity: 1.0,
                                excerpt: excerpt(&lines, l, l),
                                reason: "same text, different surrounding whitespace".into(),
                            }
                        })
                        .collect(),
                });
            }
        }
    }

    let mut normalized: Vec<(usize, Shift)> = Vec::new();
    if old_lines.len() <= lines.len() && !old_lines.is_empty() {
        for i in 0..=(lines.len() - old_lines.len()) {
            if let Some(shift) =
                normalized_match(&lines[i..i + old_lines.len()], &sensitive[i..], &old_lines)
            {
                normalized.push((i, shift));
            }
        }
    }
    if normalized.len() == 1 || (replace_all && !normalized.is_empty()) {
        let mut rules = vec![];
        let shifts: Vec<&Shift> = normalized.iter().map(|(_, s)| s).collect();
        if content.contains('\r') {
            rules.push("line endings".to_string());
        }
        if shifts.iter().any(|s| s.trailing) {
            rules.push("trailing whitespace".to_string());
        }
        if shifts.iter().any(|s| s.delta != 0) {
            rules.push(format!(
                "indentation shifted by {} column(s)",
                shifts[0].delta
            ));
        }
        if shifts.iter().any(|s| s.mixed) {
            rules.push("tabs/spaces".to_string());
        }
        let blocks: Vec<(usize, usize, Shift)> = normalized
            .into_iter()
            .map(|(i, s)| (i, i + old_lines.len() - 1, s))
            .collect();
        return Ok(apply_blocks(
            content,
            &lines,
            &blocks,
            &new_lines,
            new,
            crlf,
            Stage::Normalized(rules),
        ));
    }
    if normalized.len() > 1 {
        return Err(ReplaceError::Ambiguous {
            count: normalized.len(),
            candidates: normalized
                .iter()
                .take(MAX_CANDIDATES)
                .map(|(i, _)| Candidate {
                    start_line: i + 1,
                    end_line: i + old_lines.len(),
                    similarity: 1.0,
                    excerpt: excerpt(&lines, i + 1, i + old_lines.len()),
                    reason: "same lines, different whitespace".into(),
                })
                .collect(),
        });
    }

    // ── 3. relocation ──
    let mut relocated = Vec::new();
    if lines.len() <= RELOCATION_MAX_LINES && old_lines.len() <= RELOCATION_MAX_BLOCK {
        relocated = relocate(&lines, &sensitive, &old_lines);
    }
    if relocated.len() == 1 || (replace_all && !relocated.is_empty()) {
        let blocks: Vec<(usize, usize, Shift)> = relocated;
        return Ok(apply_blocks(
            content,
            &lines,
            &blocks,
            &new_lines,
            new,
            crlf,
            Stage::Relocated(vec!["blank lines and spacing outside quoted text".into()]),
        ));
    }
    if relocated.len() > 1 {
        return Err(ReplaceError::Ambiguous {
            count: relocated.len(),
            candidates: relocated
                .iter()
                .take(MAX_CANDIDATES)
                .map(|(a, b, _)| Candidate {
                    start_line: a + 1,
                    end_line: b + 1,
                    similarity: 1.0,
                    excerpt: excerpt(&lines, a + 1, b + 1),
                    reason: "same code, different layout".into(),
                })
                .collect(),
        });
    }

    Err(ReplaceError::NotFound {
        candidates: closest(&lines, &sensitive, &old_lines),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shift {
    /// Columns added to the requested indentation (may be negative).
    delta: isize,
    tab: usize,
    trailing: bool,
    mixed: bool,
}

fn normalized_match(window: &[Line], sensitive: &[bool], old: &[&str]) -> Option<Shift> {
    'tabs: for tab in [4usize, 8, 2] {
        let mut delta: Option<isize> = None;
        let mut trailing = false;
        let mut mixed = false;
        for (j, (f, o)) in window.iter().zip(old).enumerate() {
            let ft = f.text;
            if sensitive[j] {
                if ft != *o {
                    return None;
                }
                continue;
            }
            if ft.trim().is_empty() && o.trim().is_empty() {
                continue;
            }
            let (fi, oi) = (indent_of(ft), indent_of(o));
            if ft[fi.len()..].trim_end() != o[oi.len()..].trim_end() {
                return None;
            }
            if ft[fi.len()..] != o[oi.len()..] {
                trailing = true;
            }
            if fi.contains('\t') != oi.contains('\t') {
                mixed = true;
            }
            let d = width(fi, tab) as isize - width(oi, tab) as isize;
            match delta {
                None => delta = Some(d),
                Some(x) if x == d => {}
                Some(_) => continue 'tabs,
            }
        }
        return Some(Shift {
            delta: delta.unwrap_or(0),
            tab,
            trailing,
            mixed,
        });
    }
    None
}

/// Stage 3: blocks equal to `old` after canonicalisation, blank lines aside.
fn relocate(lines: &[Line], sensitive: &[bool], old: &[&str]) -> Vec<(usize, usize, Shift)> {
    let wanted: Vec<(String, &str)> = old
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| (canonical(l), *l))
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for start in 0..lines.len() {
        if lines[start].text.trim_matches([' ', '\t']).is_empty()
            || canonical(lines[start].text) != wanted[0].0
        {
            continue;
        }
        let mut k = 0;
        let mut i = start;
        let mut ok = true;
        // Relative indentation is code (Python): one constant shift only.
        let delta = width(indent_of(lines[start].text), 4) as isize
            - width(indent_of(wanted[0].1), 4) as isize;
        while k < wanted.len() {
            if i >= lines.len() {
                ok = false;
                break;
            }
            let t = lines[i].text;
            if t.trim_matches([' ', '\t']).is_empty() {
                i += 1;
                continue;
            }
            let same_shift = width(indent_of(t), 4) as isize
                - width(indent_of(wanted[k].1), 4) as isize
                == delta;
            let equal = if sensitive[i] {
                t == wanted[k].1
            } else {
                same_shift && canonical(t) == wanted[k].0
            };
            if !equal {
                ok = false;
                break;
            }
            k += 1;
            i += 1;
        }
        if ok {
            let f = indent_of(lines[start].text);
            let o = indent_of(wanted[0].1);
            found.push((
                start,
                i - 1,
                Shift {
                    delta: width(f, 4) as isize - width(o, 4) as isize,
                    tab: 4,
                    trailing: false,
                    mixed: f.contains('\t') != o.contains('\t'),
                },
            ));
        }
    }
    found
}

/// Re-indent `new` by the block's shift, in the block's indentation style.
fn adapt(new_lines: &[&str], shift: &Shift, block_indent: &str) -> Vec<String> {
    if shift.delta == 0 && !shift.mixed {
        return new_lines.iter().map(|s| s.to_string()).collect();
    }
    let use_tabs = block_indent.contains('\t');
    new_lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                return String::new();
            }
            let ind = indent_of(line);
            let w = (width(ind, shift.tab) as isize + shift.delta).max(0) as usize;
            let indent = if use_tabs {
                format!(
                    "{}{}",
                    "\t".repeat(w / shift.tab),
                    " ".repeat(w % shift.tab)
                )
            } else {
                " ".repeat(w)
            };
            format!("{indent}{}", &line[ind.len()..])
        })
        .collect()
}

fn apply_blocks(
    content: &str,
    lines: &[Line],
    blocks: &[(usize, usize, Shift)],
    new_lines: &[&str],
    new_raw: &str,
    crlf: bool,
    stage: Stage,
) -> EditPlan {
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(content.len() + new_raw.len());
    let mut cursor = 0;
    for (a, b, shift) in blocks {
        let first = &lines[*a];
        let last = &lines[*b];
        let block_indent = indent_of(first.text);
        if new_raw.is_empty() {
            // Deleting whole lines removes their terminators too.
            out.push_str(&content[cursor..first.start]);
            cursor = last.full_end;
            continue;
        }
        out.push_str(&content[cursor..first.start]);
        out.push_str(&adapt(new_lines, shift, block_indent).join(eol));
        cursor = last.end;
    }
    out.push_str(&content[cursor..]);
    EditPlan {
        content: out,
        stage,
        replacements: blocks.len(),
        ranges: blocks.iter().map(|(a, b, _)| (a + 1, b + 1)).collect(),
    }
}

fn excerpt(lines: &[Line], a: usize, b: usize) -> String {
    let b = b.min(a + 5).min(lines.len());
    (a..=b)
        .filter_map(|n| {
            lines
                .get(n - 1)
                .map(|l| format!("{n:>6} | {}", crate::tool_primitives::fs::clip(l.text, 160)))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The regions closest to `old`, with why each was not used.
fn closest(lines: &[Line], sensitive: &[bool], old: &[&str]) -> Vec<Candidate> {
    if lines.is_empty() || old.is_empty() || lines.len() > RELOCATION_MAX_LINES {
        return Vec::new();
    }
    let n = old.len().min(lines.len());
    let target = old.join("\n");
    let first = old
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| canonical(l))
        .unwrap_or_default();
    // Anchors: lines resembling the first line of `old` (bounded work).
    let mut anchors: Vec<(usize, f32)> = lines
        .iter()
        .enumerate()
        .take(50_000)
        .map(|(i, l)| {
            (
                i,
                TextDiff::from_chars(canonical(l.text).as_str(), first.as_str()).ratio(),
            )
        })
        .filter(|(_, r)| *r >= 0.6)
        .collect();
    anchors.sort_by(|a, b| b.1.total_cmp(&a.1));
    anchors.truncate(20);
    let mut scored: Vec<Candidate> = anchors
        .into_iter()
        .map(|(i, _)| {
            let end = (i + n).min(lines.len());
            let window = lines[i..end].iter().map(|l| l.text).collect::<Vec<_>>().join("\n");
            let sim = TextDiff::from_chars(window.as_str(), target.as_str()).ratio();
            let in_string = sensitive[i..end].iter().any(|s| *s);
            Candidate {
                start_line: i + 1,
                end_line: end,
                similarity: sim,
                excerpt: excerpt(lines, i + 1, end),
                reason: if in_string {
                    format!(
                        "{:.0}% similar, but the differences include text inside a string or here-document",
                        sim * 100.0
                    )
                } else {
                    format!("{:.0}% similar, but the code itself differs (not an admissible variation)", sim * 100.0)
                },
            }
        })
        .filter(|c| c.similarity >= 0.5)
        .collect();
    scored.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    scored.dedup_by(|a, b| a.start_line == b.start_line);
    scored.truncate(MAX_CANDIDATES);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(content: &str, old: &str, new: &str) -> Result<EditPlan, ReplaceError> {
        plan_edit(content, old, new, false, None)
    }

    #[test]
    fn exact_first_and_ambiguity_policy() {
        assert_eq!(
            replace("hello world", "world", "earth", false).unwrap(),
            "hello earth"
        );
        match replace("a a a", "a", "b", false) {
            Err(ReplaceError::Ambiguous {
                count: 3,
                candidates,
            }) => assert_eq!(candidates.len(), 3),
            other => panic!("{other:?}"),
        }
        assert_eq!(replace("a a a", "a", "b", true).unwrap(), "b b b");
        assert_eq!(replace("x", "x", "x", false), Err(ReplaceError::NoChange));
        assert_eq!(replace("", "", "hi", false).unwrap(), "hi");
        assert_eq!(
            replace("x", "", "hi", false),
            Err(ReplaceError::EmptyOldString)
        );
    }

    #[test]
    fn crlf_files_keep_their_line_endings() {
        let content = "fn a() {\r\n    one();\r\n    two();\r\n}\r\n";
        let p = plan(content, "    one();\n    two();\n", "    three();\n").unwrap();
        assert_eq!(p.content, "fn a() {\r\n    three();\r\n}\r\n");
        assert!(matches!(p.stage, Stage::Normalized(_)));
    }

    #[test]
    fn trailing_whitespace_is_tolerated() {
        let content = "alpha   \nbeta\t\ngamma\n";
        let p = plan(content, "alpha\nbeta\n", "ALPHA\nBETA\n").unwrap();
        assert_eq!(p.content, "ALPHA\nBETA\ngamma\n");
    }

    #[test]
    fn indentation_is_shifted_and_the_replacement_follows_the_file() {
        let content =
            "class A:\n    def f(self):\n        if x:\n            return 1\n        return 2\n";
        // The model dropped one level of indentation everywhere.
        let old = "def f(self):\n    if x:\n        return 1\n    return 2\n";
        let new = "def f(self):\n    if x:\n        return 10\n    return 20\n";
        let p = plan(content, old, new).unwrap();
        assert_eq!(
            p.content,
            "class A:\n    def f(self):\n        if x:\n            return 10\n        return 20\n"
        );
        assert!(p.stage.describe().contains("indentation shifted by 4"));
    }

    #[test]
    fn relative_python_indentation_must_match() {
        // Same lines but a different nesting: not the same code.
        let content = "def f():\n    if x:\n        y()\n    z()\n";
        let old = "def f():\n    if x:\n        y()\n        z()\n";
        assert!(matches!(
            plan(content, old, "pass"),
            Err(ReplaceError::NotFound { .. })
        ));
    }

    #[test]
    fn tabs_versus_spaces() {
        let content = "func f() {\n\tif x {\n\t\ty()\n\t}\n}\n";
        let old = "    if x {\n        y()\n    }\n";
        let new = "    if x {\n        z()\n    }\n";
        let p = plan(content, old, new).unwrap();
        assert_eq!(p.content, "func f() {\n\tif x {\n\t\tz()\n\t}\n}\n");
    }

    #[test]
    fn whitespace_inside_strings_is_content() {
        // Single-line string: spacing inside quotes must match exactly.
        let content = "msg = \"a  b\"\n";
        assert!(matches!(
            plan(content, "msg = \"a b\"", "msg = 1"),
            Err(ReplaceError::NotFound { .. })
        ));
        // Outside quotes, spacing may differ.
        let p = plan("let   x  =  \"a  b\";\n", "let x = \"a  b\";", "let x = 0;").unwrap();
        assert_eq!(p.content, "let x = 0;\n");
        // Inside a here-document, even trailing spaces are content.
        let script = "cat <<EOF\nline  \nEOF\necho done\n";
        let r = plan_edit(script, "cat <<EOF\nline\nEOF\n", "x\n", false, Some("a.sh"));
        assert!(matches!(r, Err(ReplaceError::NotFound { .. })), "{r:?}");
        // Inside a Python triple-quoted string too.
        let py = "s = \"\"\"\n  two spaces\n\"\"\"\n";
        assert!(matches!(
            plan(py, "s = \"\"\"\ntwo spaces\n\"\"\"\n", "s = ''\n"),
            Err(ReplaceError::NotFound { .. })
        ));
    }

    #[test]
    fn a_block_with_extra_blank_lines_is_relocated() {
        let content = "fn a() {\n    one();\n\n    two();\n}\n";
        let old = "    one();\n    two();";
        let p = plan(content, old, "    both();").unwrap();
        assert_eq!(p.content, "fn a() {\n    both();\n}\n");
        assert!(matches!(p.stage, Stage::Relocated(_)));
    }

    #[test]
    fn similar_but_different_code_is_refused_with_candidates() {
        let content = "fn calc() {\n    let a = 1;\n    let b = 2;\n    a + b\n}\n";
        let old = "fn calc() {\n    let a = 1;\n    let b = 3;\n    a + b\n}";
        match plan(content, old, "fn calc() { 0 }") {
            Err(ReplaceError::NotFound { candidates }) => {
                assert_eq!(candidates[0].start_line, 1);
                assert!(candidates[0].similarity > 0.9);
                assert!(candidates[0].reason.contains("code itself differs"));
                assert!(candidates[0].excerpt.contains("let b = 2;"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ambiguous_normalized_matches_are_refused() {
        let content = "    x();\n  y\n        x();\n";
        match plan(content, "x(); ", "z();") {
            Err(ReplaceError::Ambiguous { count: 2, .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn nothing_similar_reports_no_candidate() {
        match plan("hello\n", "nonexistent", "x") {
            Err(ReplaceError::NotFound { candidates }) => assert!(candidates.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unicode_whitespace_is_not_indentation() {
        // U+3000 is content, not indentation: `b` is not `\u{3000}b`.
        let block = "a\n\u{3000}b\n";
        let p = plan(block, "a\nb\n", "x\ny\n").unwrap_err();
        assert!(matches!(p, ReplaceError::NotFound { .. }), "{p:?}");
    }

    #[test]
    fn deleting_a_block_removes_its_lines() {
        let content = "keep\n    drop1\n    drop2\nkeep2\n";
        let p = plan(content, "drop1\ndrop2\n", "").unwrap();
        assert_eq!(p.content, "keep\nkeep2\n");
    }
}
