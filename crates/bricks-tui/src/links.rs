//! Clickable web links (OSC 8) in the terminal.
//!
//! A Markdown link keeps its destination next to its text through parsing,
//! styling and wrapping ([`crate::markdown::RichLine`]). When the text is
//! drawn, [`draw_rich`] lets Ratatui place every span and records the exact
//! columns of the link spans; [`apply`] then turns each run of link cells
//! (same destination, same style) into one cell holding
//! `OSC 8 ; id ; url ST` + the visible graphemes + `OSC 8 ; ; ST`, with a
//! forced width of the run, the run's other cells left empty. Columns are
//! measured on the visible graphemes before any escape sequence exists.
//!
//! * Both drawing paths stay right: the cell diff skips the run's other
//!   cells (forced width), and a path that prints every cell prints
//!   nothing for them. A cell that is no longer a link differs from the
//!   previous one, so it is redrawn — without the link.
//! * Only `http` and `https` addresses with a host are ever activated, as
//!   their serialised, printable-ASCII form: nothing in a destination can
//!   close the sequence or start another one. Other destinations are shown
//!   as text, never activated.
//! * Nothing here is stored: events and the conversation keep the Markdown.
//! * Rendering never opens a browser or fetches anything; the terminal opens
//!   a link on its own gesture (⌘-click in iTerm2).

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::Style;
use std::num::NonZeroU16;
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;

use crate::markdown::RichLine;

/// `--hyperlinks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HyperlinkMode {
    #[default]
    Auto,
    Always,
    Never,
}

/// Longest address activated.
const MAX_URL: usize = 2048;

/// The address to activate for a Markdown destination: an absolute `http`
/// or `https` URL with a host, serialised (percent-encoded) and made only of
/// printable ASCII. `None` for anything else (relative or local paths,
/// `file:`, `javascript:`, invalid or oversized addresses).
pub fn web_target(dest: &str) -> Option<Arc<str>> {
    let dest = dest.trim();
    if dest.is_empty() || dest.len() > MAX_URL || dest.chars().any(char::is_control) {
        return None;
    }
    let u = url::Url::parse(dest).ok()?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none_or(str::is_empty) {
        return None;
    }
    let s = u.as_str();
    (s.len() <= MAX_URL && s.bytes().all(|b| (0x21..=0x7e).contains(&b))).then(|| Arc::from(s))
}

/// Text shown for a destination: control characters (an escape, a bell...)
/// replaced, so nothing reaches the terminal as a command.
pub fn visible(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

/// Whether links are activated, from the mode, whether the output is a
/// terminal, and the environment.
///
/// `auto` recognises: iTerm2, WezTerm, Ghostty, the VS Code terminal
/// (`TERM_PROGRAM`), kitty, Windows Terminal, VTE ≥ 0.50 (GNOME Terminal…),
/// Konsole. Inside tmux or screen, `auto` stays off: the multiplexer's own
/// support (and its configuration) cannot be seen from here. `TERM` alone
/// proves nothing (`xterm-256color` is claimed by many terminals). Apple's
/// Terminal is not recognised.
pub fn enabled(
    mode: HyperlinkMode,
    is_terminal: bool,
    env: impl Fn(&str) -> Option<String>,
) -> bool {
    if !is_terminal {
        return false;
    }
    match mode {
        HyperlinkMode::Never => false,
        HyperlinkMode::Always => true,
        HyperlinkMode::Auto => {
            if env("TMUX").is_some() || env("STY").is_some() {
                return false;
            }
            let program = env("TERM_PROGRAM").unwrap_or_default();
            if matches!(
                program.as_str(),
                "iTerm.app" | "WezTerm" | "ghostty" | "vscode"
            ) {
                return true;
            }
            if env("KITTY_WINDOW_ID").is_some() || env("TERM").as_deref() == Some("xterm-kitty") {
                return true;
            }
            if env("WT_SESSION").is_some() || env("KONSOLE_VERSION").is_some() {
                return true;
            }
            env("VTE_VERSION")
                .and_then(|v| v.trim().parse::<u32>().ok())
                .is_some_and(|v| v >= 5000)
        }
    }
}

/// A link span as drawn: row, columns `[x0, x1)`, destination.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub y: u16,
    pub x0: u16,
    pub x1: u16,
    pub url: Arc<str>,
}

/// Draw rich lines from the top of `area` (one row each, no wrapping: they
/// are wrapped already), with Ratatui's own span layout, and return where
/// the link spans landed.
pub fn draw_rich(buf: &mut Buffer, area: Rect, lines: &[RichLine]) -> Vec<Placed> {
    let mut placed = Vec::new();
    for (row, rl) in lines.iter().enumerate().take(area.height as usize) {
        let y = area.y + row as u16;
        let mut x = area.x;
        for (i, span) in rl.line.spans.iter().enumerate() {
            if x >= area.right() {
                break;
            }
            let (nx, _) = buf.set_span(x, y, span, area.right() - x);
            if let Some(Some(url)) = rl.links.get(i) {
                if nx > x {
                    placed.push(Placed {
                        y,
                        x0: x,
                        x1: nx,
                        url: Arc::clone(url),
                    });
                }
            }
            x = nx;
        }
    }
    placed
}

fn link_id(url: &str) -> String {
    // FNV-1a: stable across runs, so every piece of one link (wrapped over
    // several rows) is one link for the terminal.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("b{h:016x}")
}

/// Turn placed link spans into OSC 8 cells (see the module docs).
pub fn apply(buf: &mut Buffer, placed: &[Placed]) {
    // Adjacent spans of one destination on one row form one stretch; it is
    // cut wherever the style changes (a cell prints with one style).
    let mut i = 0;
    while i < placed.len() {
        let p = &placed[i];
        let mut x1 = p.x1;
        let mut j = i + 1;
        while j < placed.len() && placed[j].y == p.y && placed[j].x0 == x1 && placed[j].url == p.url
        {
            x1 = placed[j].x1;
            j += 1;
        }
        let mut x = p.x0;
        while x < x1 {
            let style = cell_style(buf, x, p.y);
            let start = x;
            let mut text = String::new();
            while x < x1 && cell_style(buf, x, p.y) == style {
                let sym = buf[(x, p.y)].symbol().to_string();
                let w = cell_width(&sym).max(1);
                text.push_str(&sym);
                x = (x + w).min(x1);
            }
            let width = x - start;
            if let Some(w) = NonZeroU16::new(width) {
                let open = format!("\x1b]8;id={};{}\x1b\\", link_id(&p.url), p.url);
                let first = &mut buf[(start, p.y)];
                first.set_symbol(&format!("{open}{text}\x1b]8;;\x1b\\"));
                first.set_diff_option(CellDiffOption::ForcedWidth(w));
                for cx in start + 1..x {
                    let c = &mut buf[(cx, p.y)];
                    c.set_symbol("");
                    c.set_diff_option(CellDiffOption::Skip);
                }
            }
        }
        i = j;
    }
}

/// Columns of a grapheme, as Ratatui counts them when it lays text out
/// (its own function is private; kept identical).
fn cell_width(s: &str) -> u16 {
    if s.len() == 1 {
        return 1;
    }
    let marks = s
        .chars()
        .filter(|c| matches!(*c, '\u{FF9E}' | '\u{FF9F}'))
        .count() as u16;
    (UnicodeWidthStr::width(s) as u16).saturating_add(marks)
}

fn cell_style(buf: &Buffer, x: u16, y: u16) -> Style {
    buf[(x, y)].style()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_addresses_are_activated() {
        assert_eq!(
            web_target("https://example.com/a b?q=é").as_deref(),
            Some("https://example.com/a%20b?q=%C3%A9")
        );
        assert!(web_target("http://example.com").is_some());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "./README.md",
            "/tmp/x",
            "mailto:a@b.c",
            "https://",
            "https://exa\u{1b}]8;;evil\u{7}mple.com",
            "https://example.com/\u{7}",
            "ftp://example.com",
        ] {
            assert_eq!(web_target(bad), None, "{bad:?}");
        }
        assert_eq!(
            web_target(&format!("https://e.com/{}", "a".repeat(3000))),
            None
        );
        assert_eq!(visible("a\u{1b}]8;;x\u{7}b"), "a\u{FFFD}]8;;x\u{FFFD}b");
    }

    #[test]
    fn detection_is_explicit_about_what_it_knows() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let iterm = env(&[("TERM_PROGRAM", "iTerm.app"), ("TERM", "xterm-256color")]);
        assert!(enabled(HyperlinkMode::Auto, true, iterm));
        assert!(
            !enabled(HyperlinkMode::Auto, false, iterm),
            "not a terminal"
        );
        assert!(!enabled(HyperlinkMode::Never, true, iterm));
        let plain = env(&[("TERM", "xterm-256color")]);
        assert!(
            !enabled(HyperlinkMode::Auto, true, plain),
            "TERM proves nothing"
        );
        assert!(enabled(HyperlinkMode::Always, true, plain));
        assert!(!enabled(HyperlinkMode::Always, false, plain), "redirected");
        let tmux = env(&[("TERM_PROGRAM", "iTerm.app"), ("TMUX", "/tmp/t,1,0")]);
        assert!(!enabled(HyperlinkMode::Auto, true, tmux));
        assert!(!enabled(
            HyperlinkMode::Auto,
            true,
            env(&[("TERM_PROGRAM", "Apple_Terminal")])
        ));
        assert!(enabled(
            HyperlinkMode::Auto,
            true,
            env(&[("VTE_VERSION", "7600")])
        ));
        assert!(!enabled(
            HyperlinkMode::Auto,
            true,
            env(&[("VTE_VERSION", "4600")])
        ));
    }
}
