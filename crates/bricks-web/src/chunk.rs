//! Structural splitting of extracted Markdown into passages.
//!
//! The Markdown is read as blocks — headings, fenced code, tables, and
//! paragraphs/lists separated by blank lines — and blocks are packed into
//! passages of `min_chars`..`max_chars` Unicode characters without crossing
//! a heading once the passage is big enough. Each passage records the
//! heading path it sits under and its exact range in the document (bytes
//! and characters); its text is that slice of the document, verbatim.
//!
//! A block larger than `max_chars` is never cut silently: prose is split at
//! sentence ends, code blocks and tables at line ends, and every piece says
//! which part of which block it is (`fragment`). This is structure, not
//! understanding: a passage boundary can still separate related statements.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Heading,
    Prose,
    Code,
    Table,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Passage {
    /// Position in the document (0-based).
    pub index: usize,
    /// Headings above the passage, outermost first.
    pub section: Vec<String>,
    pub byte_start: usize,
    pub byte_end: usize,
    pub char_start: usize,
    pub char_end: usize,
    /// The document's text over that range.
    pub text: String,
    /// Set when the passage is a piece of one oversized block.
    pub fragment: Option<String>,
    /// For a piece of a table after the first: its header row.
    pub table_header: Option<String>,
}

#[derive(Debug, Clone)]
struct Block {
    kind: BlockKind,
    start: usize,
    end: usize,
    section: Vec<String>,
}

/// Split `md` into passages.
pub fn split(md: &str, min_chars: usize, max_chars: usize) -> Vec<Passage> {
    let blocks = blocks(md);
    let chars = CharIndex::new(md);
    let mut out: Vec<Passage> = Vec::new();
    let mut cur: Option<(usize, usize, Vec<String>)> = None; // byte range + section
    let flush = |cur: &mut Option<(usize, usize, Vec<String>)>, out: &mut Vec<Passage>| {
        if let Some((s, e, sec)) = cur.take() {
            let text = md[s..e].trim_end();
            if !text.trim().is_empty() {
                let e = s + text.len();
                out.push(Passage {
                    index: out.len(),
                    section: sec,
                    byte_start: s,
                    byte_end: e,
                    char_start: chars.at(s),
                    char_end: chars.at(e),
                    text: text.to_string(),
                    fragment: None,
                    table_header: None,
                });
            }
        }
    };
    for b in &blocks {
        let len = chars.at(b.end) - chars.at(b.start);
        if b.kind == BlockKind::Heading {
            // A heading starts a new passage unless the current one is
            // still too small to stand alone.
            let small = cur
                .as_ref()
                .is_some_and(|(s, e, _)| chars.at(*e) - chars.at(*s) < min_chars / 2);
            if !small {
                flush(&mut cur, &mut out);
            }
        }
        if len > max_chars {
            flush(&mut cur, &mut out);
            for p in pieces(md, b, max_chars) {
                let (s, e, fragment, header) = p;
                out.push(Passage {
                    index: out.len(),
                    section: b.section.clone(),
                    byte_start: s,
                    byte_end: e,
                    char_start: chars.at(s),
                    char_end: chars.at(e),
                    text: md[s..e].to_string(),
                    fragment: Some(fragment),
                    table_header: header,
                });
            }
            continue;
        }
        match &mut cur {
            Some((s, e, _)) if chars.at(b.end) - chars.at(*s) <= max_chars => *e = b.end,
            Some(_) => {
                flush(&mut cur, &mut out);
                cur = Some((b.start, b.end, b.section.clone()));
            }
            None => cur = Some((b.start, b.end, b.section.clone())),
        }
    }
    flush(&mut cur, &mut out);
    // A trailing passage far below the minimum joins its predecessor when
    // they share a section and the result still fits.
    if out.len() >= 2 {
        let n = out.len();
        let (a, b) = (&out[n - 2], &out[n - 1]);
        if b.char_end - b.char_start < min_chars / 2
            && b.fragment.is_none()
            && a.fragment.is_none()
            && a.section == b.section
            && b.char_end - a.char_start <= max_chars
        {
            let merged = Passage {
                index: a.index,
                section: a.section.clone(),
                byte_start: a.byte_start,
                byte_end: b.byte_end,
                char_start: a.char_start,
                char_end: b.char_end,
                text: md[a.byte_start..b.byte_end].to_string(),
                fragment: None,
                table_header: None,
            };
            out.truncate(n - 2);
            out.push(merged);
        }
    }
    out
}

/// Blocks with their heading path.
fn blocks(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let lines: Vec<(usize, &str)> = line_spans(md);
    let mut i = 0;
    while i < lines.len() {
        let (start, line) = lines[i];
        let t = line.trim_start();
        if t.is_empty() {
            i += 1;
            continue;
        }
        let section = |stack: &Vec<(usize, String)>| stack.iter().map(|(_, h)| h.clone()).collect();
        if let Some(level) = heading_level(t) {
            let title = t[level..].trim().trim_end_matches('#').trim().to_string();
            stack.retain(|(l, _)| *l < level);
            stack.push((level, title));
            let end = start + line.len();
            out.push(Block {
                kind: BlockKind::Heading,
                start,
                end,
                section: section(&stack),
            });
            i += 1;
            continue;
        }
        let (kind, mut j) = if t.starts_with("```") || t.starts_with("~~~") {
            let fence = &t[..3];
            let mut j = i + 1;
            while j < lines.len() && !lines[j].1.trim_start().starts_with(fence) {
                j += 1;
            }
            (BlockKind::Code, (j + 1).min(lines.len()))
        } else if t.starts_with('|') {
            let mut j = i + 1;
            while j < lines.len() && lines[j].1.trim_start().starts_with('|') {
                j += 1;
            }
            (BlockKind::Table, j)
        } else {
            let mut j = i + 1;
            while j < lines.len() {
                let n = lines[j].1.trim_start();
                if n.is_empty()
                    || heading_level(n).is_some()
                    || n.starts_with("```")
                    || n.starts_with("~~~")
                    || n.starts_with('|')
                {
                    break;
                }
                j += 1;
            }
            (BlockKind::Prose, j)
        };
        j = j.max(i + 1);
        let (last_start, last) = lines[j - 1];
        out.push(Block {
            kind,
            start,
            end: last_start + last.len(),
            section: section(&stack),
        });
        i = j;
    }
    out
}

fn heading_level(t: &str) -> Option<usize> {
    let n = t.chars().take_while(|c| *c == '#').count();
    ((1..=6).contains(&n) && t[n..].starts_with(' ')).then_some(n)
}

/// `(byte offset, line without its newline)`.
fn line_spans(md: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut pos = 0;
    for l in md.split_inclusive('\n') {
        out.push((pos, l.trim_end_matches('\n').trim_end_matches('\r')));
        pos += l.len();
    }
    out
}

/// Pieces of an oversized block: `(start, end, fragment label, table header)`.
fn pieces(md: &str, b: &Block, max_chars: usize) -> Vec<(usize, usize, String, Option<String>)> {
    let text = &md[b.start..b.end];
    // Cut points (byte offsets within `text`) where a piece may end.
    let cuts: Vec<usize> = match b.kind {
        BlockKind::Prose => sentence_ends(text),
        _ => text.match_indices('\n').map(|(i, _)| i + 1).collect(),
    };
    let header =
        (b.kind == BlockKind::Table).then(|| text.lines().take(2).collect::<Vec<_>>().join("\n"));
    let mut spans = Vec::new();
    let mut s = 0;
    let mut chars_since = 0usize;
    let mut last_cut = None;
    let mut prev = 0;
    for &c in cuts.iter().chain(std::iter::once(&text.len())) {
        let add = text[prev..c].chars().count();
        if chars_since + add > max_chars && last_cut.is_some() {
            let e = last_cut.take().unwrap();
            spans.push((s, e));
            s = e;
            chars_since = text[s..c].chars().count();
        } else {
            chars_since += add;
        }
        last_cut = Some(c);
        prev = c;
    }
    if s < text.len() {
        spans.push((s, text.len()));
    }
    // A single cut-free run longer than the limit is split at character
    // boundaries as a last resort, and said so.
    let mut final_spans = Vec::new();
    for (a, z) in spans {
        if text[a..z].chars().count() <= max_chars {
            final_spans.push((a, z));
            continue;
        }
        let mut start = a;
        let mut count = 0;
        for (i, _) in text[a..z].char_indices() {
            if count == max_chars {
                final_spans.push((start, a + i));
                start = a + i;
                count = 0;
            }
            count += 1;
        }
        final_spans.push((start, z));
    }
    let what = match b.kind {
        BlockKind::Code => "code block",
        BlockKind::Table => "table",
        _ => "paragraph",
    };
    let n = final_spans.len();
    final_spans
        .into_iter()
        .enumerate()
        .map(|(k, (a, z))| {
            let first_line = md[..b.start + a].matches('\n').count() + 1;
            let last_line = md[..b.start + z]
                .trim_end_matches('\n')
                .matches('\n')
                .count()
                + 1;
            (
                b.start + a,
                b.start + z,
                format!(
                    "part {}/{} of a {what} (document lines {first_line}–{last_line})",
                    k + 1,
                    n
                ),
                if k > 0 { header.clone() } else { None },
            )
        })
        .collect()
}

/// Byte offsets just after sentence ends (`. `, `! `, `? `, newline).
fn sentence_ends(text: &str) -> Vec<usize> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len() {
        let end = match b[i] {
            b'\n' => true,
            b'.' | b'!' | b'?' => b.get(i + 1).is_some_and(|n| *n == b' ' || *n == b'\n'),
            _ => false,
        };
        if end {
            out.push(i + 1);
        }
    }
    out
}

/// Byte offset → character offset, for one document.
struct CharIndex<'a> {
    /// Character count at each multiple of 64 bytes.
    marks: Vec<usize>,
    text: &'a str,
}

impl<'a> CharIndex<'a> {
    fn new(text: &'a str) -> Self {
        let mut marks = Vec::with_capacity(text.len() / 64 + 1);
        let mut count = 0;
        let bytes = text.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if i.is_multiple_of(64) {
                marks.push(count);
            }
            if (*b & 0xC0) != 0x80 {
                count += 1;
            }
        }
        marks.push(count);
        Self { marks, text }
    }

    fn at(&self, byte: usize) -> usize {
        let bytes = self.text.as_bytes();
        let base = byte / 64;
        let mut count = self.marks[base.min(self.marks.len() - 1)];
        for b in &bytes[base * 64..byte.min(bytes.len())] {
            if (*b & 0xC0) != 0x80 {
                count += 1;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passages_are_verbatim_slices_with_sections() {
        let md = "# Guide\n\nIntro paragraph.\n\n## Install\n\nRun the installer. Then check.\n\n## Use\n\nCall `run()`.\n";
        let ps = split(md, 20, 60);
        for p in &ps {
            assert_eq!(&md[p.byte_start..p.byte_end], p.text);
            assert_eq!(md[..p.byte_start].chars().count(), p.char_start);
        }
        let install = ps.iter().find(|p| p.text.contains("installer")).unwrap();
        assert_eq!(
            install.section,
            vec!["Guide".to_string(), "Install".to_string()]
        );
        assert!(install.text.starts_with("## Install"));
    }

    #[test]
    fn oversized_code_is_split_at_lines_and_labelled() {
        let code: String = (0..40).map(|i| format!("let x{i} = {i};\n")).collect();
        let md = format!("# Ex\n\nSome text here.\n\n```rust\n{code}```\n\nAfter.\n");
        let ps = split(&md, 20, 120);
        let parts: Vec<_> = ps.iter().filter(|p| p.fragment.is_some()).collect();
        assert!(parts.len() >= 3, "{ps:#?}");
        for p in &parts {
            assert!(p.text.chars().count() <= 120);
            assert!(
                p.text.ends_with('\n') || p.text.ends_with("```"),
                "{:?}",
                p.text
            );
            assert!(p.fragment.as_ref().unwrap().contains("code block"));
        }
        // Together the pieces are the whole block.
        let joined: String = parts.iter().map(|p| p.text.as_str()).collect();
        assert!(joined.starts_with("```rust\n") && joined.ends_with("```"));
    }

    #[test]
    fn table_pieces_carry_the_header() {
        let rows: String = (0..30).map(|i| format!("| opt{i} | {i} |\n")).collect();
        let md = format!("| Option | Default |\n| --- | --- |\n{rows}");
        let ps = split(&md, 20, 150);
        assert!(ps.len() > 1);
        assert!(ps[0].table_header.is_none());
        assert_eq!(
            ps[1].table_header.as_deref(),
            Some("| Option | Default |\n| --- | --- |")
        );
    }

    #[test]
    fn unicode_offsets_are_characters() {
        let md = "# Été\n\nCafé crème — naïve façade. 日本語のテキスト。\n";
        let ps = split(md, 5, 200);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].char_end, md.trim_end().chars().count());
    }
}
