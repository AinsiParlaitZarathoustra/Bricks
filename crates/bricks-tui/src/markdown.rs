//! Markdown → terminal lines, and wrapping by display width.
//!
//! Headings, emphasis, inline code, fenced code blocks (kept verbatim,
//! never re-wrapped into prose), lists, quotes and links (shown with their
//! target). No syntax highlighting: code is shown in one distinct style.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn code_style() -> Style {
    Style::default().fg(Color::Cyan)
}

/// Render Markdown to logical lines (not wrapped).
pub fn render(text: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut styles: Vec<Style> = vec![Style::default()];
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut in_code = false;
    let mut quote = 0usize;
    let mut link: Option<String> = None;

    let flush = |cur: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>, quote: usize| {
        if cur.is_empty() {
            return;
        }
        let mut spans = Vec::new();
        if quote > 0 {
            spans.push(Span::styled(
                "│ ".repeat(quote),
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.append(cur);
        lines.push(Line::from(spans));
    };
    let blank = |lines: &mut Vec<Line<'static>>| {
        if lines.last().is_some_and(|l| !l.spans.is_empty()) {
            lines.push(Line::default());
        }
    };

    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(text, opts) {
        let style = *styles.last().unwrap_or(&Style::default());
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush(&mut cur, &mut lines, quote);
                    blank(&mut lines);
                    let s = Style::default().add_modifier(Modifier::BOLD);
                    styles.push(if level == HeadingLevel::H1 {
                        s.add_modifier(Modifier::UNDERLINED)
                    } else {
                        s
                    });
                }
                Tag::Paragraph => {}
                Tag::Emphasis => styles.push(style.add_modifier(Modifier::ITALIC)),
                Tag::Strong => styles.push(style.add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => styles.push(style.add_modifier(Modifier::CROSSED_OUT)),
                Tag::BlockQuote(_) => {
                    flush(&mut cur, &mut lines, quote);
                    quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    flush(&mut cur, &mut lines, quote);
                    in_code = true;
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    lines.push(Line::from(Span::styled(
                        format!("┌─ {lang}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                Tag::List(start) => {
                    flush(&mut cur, &mut lines, quote);
                    list_stack.push(start);
                }
                Tag::Item => {
                    flush(&mut cur, &mut lines, quote);
                    let depth = list_stack.len().saturating_sub(1);
                    let marker = match list_stack.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}. ");
                            *n += 1;
                            m
                        }
                        _ => "• ".to_string(),
                    };
                    cur.push(Span::raw(format!("{}{marker}", "  ".repeat(depth))));
                }
                Tag::Link { dest_url, .. } => {
                    link = Some(dest_url.to_string());
                    styles.push(style.add_modifier(Modifier::UNDERLINED));
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    styles.pop();
                    flush(&mut cur, &mut lines, quote);
                }
                TagEnd::Paragraph => {
                    flush(&mut cur, &mut lines, quote);
                    if list_stack.is_empty() {
                        lines.push(Line::default());
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    styles.pop();
                }
                TagEnd::BlockQuote(_) => {
                    flush(&mut cur, &mut lines, quote);
                    quote = quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    in_code = false;
                    lines.push(Line::from(Span::styled(
                        "└─",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                TagEnd::List(_) => {
                    flush(&mut cur, &mut lines, quote);
                    list_stack.pop();
                    if list_stack.is_empty() {
                        lines.push(Line::default());
                    }
                }
                TagEnd::Item => flush(&mut cur, &mut lines, quote),
                TagEnd::Link => {
                    styles.pop();
                    if let Some(url) = link.take() {
                        cur.push(Span::styled(
                            format!(" <{url}>"),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if in_code {
                    for l in t.lines() {
                        lines.push(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                            Span::styled(l.to_string(), code_style()),
                        ]));
                    }
                } else {
                    cur.push(Span::styled(t.to_string(), style));
                }
            }
            Event::Code(t) => cur.push(Span::styled(t.to_string(), code_style())),
            Event::SoftBreak => cur.push(Span::styled(" ", style)),
            Event::HardBreak => flush(&mut cur, &mut lines, quote),
            Event::Rule => {
                flush(&mut cur, &mut lines, quote);
                lines.push(Line::from(Span::styled(
                    "───",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Event::TaskListMarker(done) => cur.push(Span::raw(if done { "[x] " } else { "[ ] " })),
            Event::Html(t) | Event::InlineHtml(t) => cur.push(Span::styled(t.to_string(), style)),
            _ => {}
        }
    }
    flush(&mut cur, &mut lines, quote);
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
    lines
}

/// Wrap one line to `width` display columns: at spaces when possible,
/// never inside a grapheme; styles are kept.
pub fn wrap(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_w = 0usize;
    for span in &line.spans {
        let style = span.style;
        // Split into words with their trailing spaces.
        for word in span.content.split_word_bounds() {
            let w = word.width();
            if row_w + w > width && row_w > 0 {
                out.push(Line::from(std::mem::take(&mut row)));
                row_w = 0;
                if word.trim().is_empty() {
                    continue;
                }
            }
            if w > width {
                // A word longer than the line: cut by grapheme.
                let mut piece = String::new();
                for g in word.graphemes(true) {
                    let gw = g.width();
                    if row_w + gw > width && row_w > 0 {
                        row.push(Span::styled(std::mem::take(&mut piece), style));
                        out.push(Line::from(std::mem::take(&mut row)));
                        row_w = 0;
                    }
                    piece.push_str(g);
                    row_w += gw;
                }
                row.push(Span::styled(piece, style));
            } else {
                row.push(Span::styled(word.to_string(), style));
                row_w += w;
            }
        }
    }
    out.push(Line::from(row));
    out
}

pub fn wrap_all(lines: &[Line<'static>], width: usize) -> Vec<Line<'static>> {
    lines.iter().flat_map(|l| wrap(l, width)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn markdown_keeps_code_verbatim_and_marks_structure() {
        let md = "# Titre\n\nUn *mot* et `code`.\n\n- un\n- deux\n\n```rust\nfn  main() {}\n```\n\n[lien](https://x.y)";
        let p = plain(&render(md));
        assert!(p.contains(&"Titre".to_string()), "{p:?}");
        assert!(p.contains(&"Un mot et code.".to_string()), "{p:?}");
        assert!(
            p.contains(&"• un".to_string()) && p.contains(&"• deux".to_string()),
            "{p:?}"
        );
        assert!(
            p.contains(&"│ fn  main() {}".to_string()),
            "spacing kept: {p:?}"
        );
        assert!(p.iter().any(|l| l.contains("lien <https://x.y>")), "{p:?}");
    }

    #[test]
    fn wrapping_counts_display_columns() {
        let l = Line::from("漢字 漢字 abc défg");
        let w = plain(&wrap(&l, 6));
        assert!(
            w.iter().all(|r| UnicodeWidthStr::width(r.as_str()) <= 6),
            "{w:?}"
        );
        assert_eq!(w.concat().replace(' ', ""), "漢字漢字abcdéfg");
        let long = plain(&wrap(&Line::from("aaaaaaaaaa"), 4));
        assert_eq!(long, vec!["aaaa", "aaaa", "aa"]);
    }
}
