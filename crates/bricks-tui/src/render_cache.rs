//! Rendering of the live cells' text, kept between frames.
//!
//! The live area is drawn at every event and every tick (120 ms). An
//! answer's Markdown is parsed, and wrapped to the width, only when its
//! text, the width or the reasoning display changes; a tick or another
//! cell's event reuses what was made.
//!
//! While an answer streams, its text is cut at stable block boundaries
//! ([`markdown::last_stable_boundary`]): the blocks before the last one are
//! parsed and wrapped once, only the last (open) block is rendered again
//! when a fragment arrives. A document with a construct that can change
//! earlier blocks (a link reference definition, an HTML block) is rendered
//! whole at every change instead, and so is a single long block (an open
//! code fence, say) until it is followed by another. Once the answer is
//! closed it is rendered whole, exactly as the reference renderer does.
//!
//! A resize re-wraps the parsed lines without parsing again. Entries of
//! cells written to the scrollback are dropped; the cache never holds more
//! than the live cells.

use crate::markdown::{self, RichLine};
use crate::state::Cell;
use std::collections::HashMap;

/// Work done, for tests and diagnostics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RenderStats {
    /// Markdown parses.
    pub parses: u64,
    /// Bytes given to the parser, in total.
    pub parsed_bytes: u64,
    /// Wrapping passes (one per block or plain text wrapped).
    pub wraps: u64,
    /// Lines wrapped, in total.
    pub wrapped_lines: u64,
}

/// One block of an answer: its lines, parsed once, and wrapped at a width.
#[derive(Default)]
struct Block {
    /// Byte range in the text.
    end: usize,
    lines: Vec<RichLine>,
    wrapped: Option<(usize, Vec<RichLine>)>,
}

#[derive(Default)]
struct Entry {
    rendered: bool,
    /// (text length, open, reasoning shown): the revision rendered. Text
    /// only grows while a cell is live, so its length identifies it.
    rev: (usize, bool, bool),
    /// Stable blocks of an open answer (empty when rendered whole).
    stable: Vec<Block>,
    /// The rest: the open block, or the whole text.
    tail: Block,
    width: usize,
    /// The cell's wrapped lines, ready to draw.
    out: Vec<RichLine>,
}

#[derive(Default)]
pub struct RenderCache {
    entries: HashMap<usize, Entry>,
    pub stats: RenderStats,
}

impl RenderCache {
    /// The wrapped lines of the live cell `index` (Assistant or Thinking
    /// text), `None` for other cells (rendered by the caller).
    pub fn lines(
        &mut self,
        index: usize,
        cell: &Cell,
        show_thinking: bool,
        width: usize,
    ) -> Option<&[RichLine]> {
        let width = width.max(1);
        let (text, open, markdown_text) = match cell {
            Cell::Assistant { text, open } => (text, *open, true),
            Cell::Thinking { text, open } => (text, *open, false),
            _ => return None,
        };
        let rev = (text.len(), open, show_thinking);
        let stats = &mut self.stats;
        let e = self.entries.entry(index).or_default();
        if e.rendered && e.rev == rev && e.width == width {
            return Some(&self.entries[&index].out);
        }
        if !e.rendered || e.rev != rev {
            if markdown_text {
                update_markdown(e, text, open, stats);
            } else {
                let lines: Vec<RichLine> = crate::view::cell_rich_lines(cell, show_thinking);
                e.stable.clear();
                e.tail = Block {
                    end: text.len(),
                    lines,
                    wrapped: None,
                };
            }
            e.rev = rev;
            e.rendered = true;
        }
        e.width = width;
        let mut out = Vec::new();
        if markdown_text {
            // The blank line before an answer.
            out.push(RichLine::default());
        }
        for b in e.stable.iter_mut().chain(std::iter::once(&mut e.tail)) {
            if b.wrapped.as_ref().is_none_or(|(w, _)| *w != width) {
                stats.wraps += 1;
                stats.wrapped_lines += b.lines.len() as u64;
                b.wrapped = Some((width, markdown::wrap_rich_all(&b.lines, width)));
            }
            out.extend(b.wrapped.as_ref().unwrap().1.iter().cloned());
        }
        // As the whole document: no blank line at the end (the open block
        // may render nothing yet). The leading blank line stays.
        while markdown_text && out.len() > 1 && out.last().is_some_and(|l| l.line.spans.is_empty())
        {
            out.pop();
        }
        e.out = out;
        Some(&self.entries[&index].out)
    }

    /// Drop the entries of cells before `committed` (written to the
    /// scrollback) or at or after `len` (cells that no longer exist).
    pub fn retain_live(&mut self, committed: usize, len: usize) {
        self.entries.retain(|i, _| *i >= committed && *i < len);
    }

    /// Forget everything (session or project change, end).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn parse(text: &str, after_text: bool, last: bool, stats: &mut RenderStats) -> Vec<RichLine> {
    stats.parses += 1;
    stats.parsed_bytes += text.len() as u64;
    markdown::render_rich_part(text, after_text, last)
}

fn ends_with_text(blocks: &[Block]) -> bool {
    blocks
        .iter()
        .rev()
        .find_map(|b| b.lines.last())
        .is_some_and(|l| !l.line.spans.is_empty())
}

fn update_markdown(e: &mut Entry, text: &str, open: bool, stats: &mut RenderStats) {
    // Text only grows while the cell is live; anything else starts over.
    let consistent = e.stable.last().is_none_or(|b| b.end <= text.len());
    if !open || !consistent || markdown::needs_whole_render(text) {
        // Closed (the final rendering, as the reference renderer makes it),
        // or a document that must be rendered whole.
        e.stable.clear();
        if e.tail.end != text.len() || e.tail.lines.is_empty() || e.rev.1 != open {
            e.tail = Block {
                end: text.len(),
                lines: parse(text, false, true, stats),
                wrapped: None,
            };
        }
        return;
    }
    let mut from = e.stable.last().map(|b| b.end).unwrap_or(0);
    if let Some(b) = markdown::last_stable_boundary(text, from) {
        // The blocks between the last boundary and the new one are done.
        let after = ends_with_text(&e.stable);
        e.stable.push(Block {
            end: b,
            lines: parse(&text[from..b], after, false, stats),
            wrapped: None,
        });
        from = b;
    }
    let after = ends_with_text(&e.stable);
    e.tail = Block {
        end: text.len(),
        lines: parse(&text[from..], after, true, stats),
        wrapped: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[RichLine]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn reference(text: &str, width: usize) -> Vec<RichLine> {
        let mut v = vec![RichLine::default()];
        v.extend(markdown::wrap_rich_all(&markdown::render_rich(text), width));
        v
    }

    fn assistant(text: &str, open: bool) -> Cell {
        Cell::Assistant {
            text: text.into(),
            open,
        }
    }

    /// Documents mixing what streaming must not break.
    const DOCS: &[&str] = &[
        "# Title\n\nA paragraph with *emphasis*, `code` and a [link](https://example.com/a).\n\n## Next\n\nMore text that wraps over several lines when the terminal is narrow enough.\n",
        "Intro.\n\n```rust\nfn main() {\n\n    println!(\"hi\");\n}\n```\n\n# After the code\n\nDone.\n",
        "1. one\n2. two\n\n3. three after a blank\n\n   continued item\n\nText.\n\n- a\n  - nested b\n    - deeper c\n\nEnd.\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nAfter table.\n\n> quoted\n> more\n\nNot quoted.\n",
        "Unicode: 漢字 et emoji 👍🏽 dans une phrase assez longue pour être coupée.\n\n~~~\ncode ~~~ inside\n~~~\n\n**bold** end 🚀\n",
        "Text\n\n    indented code\n\n    still code\n\nBack to text.\n\n***\n\nAfter rule.\n",
    ];

    #[test]
    fn every_streamed_prefix_renders_as_the_reference() {
        for doc in DOCS {
            for width in [12usize, 40, 100] {
                let mut cache = RenderCache::default();
                let mut text = String::new();
                // Small, uneven fragments, cut inside Markdown markers.
                for (i, ch) in doc.chars().enumerate() {
                    text.push(ch);
                    if i % 3 != 0 {
                        continue;
                    }
                    let got = cache
                        .lines(0, &assistant(&text, true), false, width)
                        .unwrap()
                        .to_vec();
                    assert_eq!(
                        plain(&got),
                        plain(&reference(&text, width)),
                        "prefix {:?} at {width}",
                        text
                    );
                }
                let fin = cache
                    .lines(0, &assistant(doc, false), false, width)
                    .unwrap()
                    .to_vec();
                assert_eq!(
                    fin,
                    reference(doc, width),
                    "final rendering, styles included"
                );
            }
        }
    }

    #[test]
    fn ticks_and_other_cells_reparse_nothing() {
        let doc = DOCS[0].repeat(20);
        let mut cache = RenderCache::default();
        cache.lines(3, &assistant(&doc, true), false, 80);
        let before = cache.stats;
        for _ in 0..50 {
            // A tick, or an event of another cell: the same text.
            cache.lines(3, &assistant(&doc, true), false, 80);
        }
        assert_eq!(cache.stats, before);
    }

    #[test]
    fn streaming_reparses_only_the_open_block() {
        let doc = DOCS[0].repeat(40);
        let mut cache = RenderCache::default();
        let mut text = String::new();
        let mut whole_bytes = 0u64;
        for chunk in doc.as_bytes().chunks(7) {
            text.push_str(std::str::from_utf8(chunk).unwrap());
            whole_bytes += text.len() as u64;
            cache.lines(0, &assistant(&text, true), false, 80);
        }
        let s = cache.stats;
        eprintln!(
            "corpus {} B in {} fragments: parsed {} B in {} parses (whole prefix each time: {whole_bytes} B); wrapped {} lines",
            doc.len(),
            doc.len().div_ceil(7),
            s.parsed_bytes,
            s.parses,
            s.wrapped_lines
        );
        // Re-parsing the whole prefix at each fragment would cost
        // `whole_bytes`; the open block alone is a small part of it.
        assert!(
            s.parsed_bytes * 10 < whole_bytes,
            "parsed {} of {whole_bytes}",
            s.parsed_bytes
        );
        assert!(s.wrapped_lines < (doc.lines().count() as u64) * 10, "{s:?}");
    }

    #[test]
    fn a_late_reference_definition_falls_back_to_whole_renders() {
        let mut cache = RenderCache::default();
        let a = "See [the docs][d].\n\nMore text.\n\nAnd more.\n";
        cache.lines(0, &assistant(a, true), false, 80);
        let b = format!("{a}\n[d]: https://example.com/docs\n");
        let got = cache
            .lines(0, &assistant(&b, true), false, 80)
            .unwrap()
            .to_vec();
        assert_eq!(plain(&got), plain(&reference(&b, 80)));
        assert!(
            plain(&got)
                .iter()
                .any(|l| l.contains("https://example.com/docs"))
        );
    }

    #[test]
    fn a_resize_rewraps_without_parsing_and_thinking_follows_its_toggle() {
        let doc = DOCS[2].repeat(5);
        let mut cache = RenderCache::default();
        cache.lines(0, &assistant(&doc, false), false, 80);
        let parses = cache.stats.parses;
        for w in [30, 120, 30] {
            let got = cache
                .lines(0, &assistant(&doc, false), false, w)
                .unwrap()
                .to_vec();
            assert_eq!(got, reference(&doc, w));
        }
        assert_eq!(cache.stats.parses, parses, "resizing parses nothing");

        let thinking = Cell::Thinking {
            text: "line one\nline two".into(),
            open: true,
        };
        let hidden = cache.lines(1, &thinking, false, 80).unwrap().len();
        let shown = cache.lines(1, &thinking, true, 80).unwrap().len();
        assert_eq!((hidden, shown), (1, 3));
    }

    #[test]
    fn committed_cells_leave_the_cache() {
        let mut cache = RenderCache::default();
        for i in 0..5 {
            cache.lines(i, &assistant("text", false), false, 80);
        }
        assert_eq!(cache.len(), 5);
        cache.retain_live(3, 5);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }
}
