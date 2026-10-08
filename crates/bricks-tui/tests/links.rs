//! Web links in the terminal, checked on the bytes the backend writes: a
//! small emulator replays them (cursor moves, styles, OSC 8) and tells, cell
//! by cell, what is shown and which address a click would open.

use bricks_tui::links::{self, Placed};
use bricks_tui::markdown::{self, RichLine};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A terminal grid: each cell holds its grapheme and the link it carries.
struct Screen {
    w: usize,
    cells: Vec<Vec<(String, Option<String>)>>,
    x: usize,
    y: usize,
    link: Option<String>,
}

impl Screen {
    fn new(w: usize, h: usize) -> Self {
        Screen {
            w,
            cells: vec![vec![(" ".to_string(), None); w]; h],
            x: 0,
            y: 0,
            link: None,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        let s = String::from_utf8(bytes.to_vec()).expect("UTF-8 output");
        let mut rest = s.as_str();
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix("\x1b]8;") {
                // OSC 8 ; params ; uri ST
                let end = r.find("\x1b\\").expect("OSC 8 terminated by ST");
                let body = &r[..end];
                let (_params, uri) = body.split_once(';').expect("params;uri");
                assert!(
                    !uri.contains('\x1b') && !uri.contains('\x07'),
                    "control in uri"
                );
                self.link = (!uri.is_empty()).then(|| uri.to_string());
                rest = &r[end + 2..];
            } else if let Some(r) = rest.strip_prefix("\x1b[") {
                let end = r.find(|c: char| c.is_ascii_alphabetic()).unwrap();
                let (args, cmd) = (&r[..end], &r[end..end + 1]);
                if cmd == "H" {
                    let mut it = args.split(';').map(|n| n.parse::<usize>().unwrap_or(1));
                    self.y = it.next().unwrap_or(1) - 1;
                    self.x = it.next().unwrap_or(1) - 1;
                }
                rest = &r[end + 1..];
            } else if rest.starts_with('\x1b') {
                panic!("unexpected escape: {:?}", &rest[..rest.len().min(12)]);
            } else {
                let g = rest.graphemes(true).next().unwrap();
                let w = g.width().max(1);
                if self.x < self.w {
                    self.cells[self.y][self.x] = (g.to_string(), self.link.clone());
                    for k in 1..w {
                        if self.x + k < self.w {
                            self.cells[self.y][self.x + k] = (String::new(), self.link.clone());
                        }
                    }
                }
                self.x += w;
                rest = &rest[g.len()..];
            }
        }
    }

    fn text(&self, y: usize) -> String {
        self.cells[y].iter().map(|c| c.0.as_str()).collect()
    }

    /// The address under column `x` of row `y`.
    fn link_at(&self, x: usize, y: usize) -> Option<&str> {
        self.cells[y][x].1.as_deref()
    }

    /// `(start column, text, url)` of each linked stretch of row `y`.
    fn links(&self, y: usize) -> Vec<(usize, String, String)> {
        let mut out: Vec<(usize, String, String)> = Vec::new();
        let mut col = 0;
        for c in &self.cells[y] {
            if let Some(u) = &c.1 {
                match out.last_mut() {
                    Some(last) if last.2 == *u && last.0 + last.1.width() == col => {
                        last.1.push_str(&c.0)
                    }
                    _ => out.push((col, c.0.clone(), u.clone())),
                }
            }
            col += c.0.width().max(usize::from(!c.0.is_empty()));
        }
        out
    }
}

fn rich(md: &str, width: usize) -> Vec<RichLine> {
    markdown::wrap_rich_all(&markdown::render_rich(md), width)
}

/// The buffer of `lines`, with links applied when `on`.
fn buffer(lines: &[RichLine], w: u16, h: u16, on: bool) -> (Buffer, Vec<Placed>) {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    let placed = links::draw_rich(&mut buf, area, lines);
    if on {
        links::apply(&mut buf, &placed);
    }
    (buf, placed)
}

/// Bytes written to go from `prev` to `next` (the interface's redraw).
fn diff_bytes(prev: &Buffer, next: &Buffer) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut b = CrosstermBackend::new(&mut out);
        b.draw(prev.diff(next).into_iter()).unwrap();
        Backend::flush(&mut b).unwrap();
    }
    out
}

/// Bytes written when every cell is printed (lines sent to the scrollback).
fn all_bytes(buf: &Buffer) -> Vec<u8> {
    let w = buf.area.width as usize;
    let mut out = Vec::new();
    {
        let mut b = CrosstermBackend::new(&mut out);
        b.draw(
            buf.content
                .iter()
                .enumerate()
                .map(|(i, c)| ((i % w) as u16, (i / w) as u16, c)),
        )
        .unwrap();
        Backend::flush(&mut b).unwrap();
    }
    out
}

fn shown(buf_bytes: &[u8], w: usize, h: usize) -> Screen {
    let mut s = Screen::new(w, h);
    s.feed(buf_bytes);
    s
}

#[test]
fn neighbours_with_the_same_text_open_their_own_address() {
    let lines = rich("[doc](https://a.example/x) [doc](https://b.example/y)", 80);
    let (buf, _) = buffer(&lines, 80, 2, true);
    let empty = Buffer::empty(buf.area);
    for bytes in [diff_bytes(&empty, &buf), all_bytes(&buf)] {
        let s = shown(&bytes, 80, 2);
        assert_eq!(
            s.text(0).trim_end(),
            "doc <https://a.example/x> doc <https://b.example/y>"
        );
        // Each link (its text and its written address) opens its own.
        let l = s.links(0);
        assert_eq!(
            l,
            vec![
                (
                    0,
                    "doc <https://a.example/x>".into(),
                    "https://a.example/x".into()
                ),
                (
                    26,
                    "doc <https://b.example/y>".into(),
                    "https://b.example/y".into()
                ),
            ]
        );
        // The space between them is no link.
        let gap = "doc <https://a.example/x>".width();
        assert_eq!(s.link_at(gap, 0), None);
    }
}

#[test]
fn styles_unicode_and_wide_characters_stay_in_place() {
    let md = "Voir [l'**été** 漢字 🎉 café](https://ex.com/é) fin";
    let lines = rich(md, 80);
    let (plain, _) = buffer(&lines, 80, 1, false);
    let (on, _) = buffer(&lines, 80, 1, true);
    let empty = Buffer::empty(on.area);
    let a = shown(&diff_bytes(&empty, &plain), 80, 1);
    let b = shown(&diff_bytes(&empty, &on), 80, 1);
    assert_eq!(a.text(0), b.text(0), "same columns with and without links");
    assert!(b
        .text(0)
        .starts_with("Voir l'été 漢字 🎉 café <https://ex.com/é> fin"));
    let urls: Vec<String> = b.links(0).into_iter().map(|l| l.2).collect();
    assert!(
        urls.iter().all(|u| u == "https://ex.com/%C3%A9"),
        "{urls:?}"
    );
    // Every visible character of the link carries it, nothing else does.
    let start = "Voir ".width();
    let end = "Voir l'été 漢字 🎉 café <https://ex.com/é>".width();
    for x in 0..80 {
        let inside = (start..end).contains(&x);
        assert_eq!(
            b.link_at(x, 0).is_some(),
            inside,
            "column {x}: {:?}",
            b.cells[0][x]
        );
    }
}

#[test]
fn a_long_link_wraps_and_every_piece_opens_it() {
    let md = "[un lien dont le texte est bien plus long que la ligne](https://ex.com/long)";
    let lines = rich(md, 20);
    assert!(lines.len() >= 3);
    let (buf, _) = buffer(&lines, 20, lines.len() as u16, true);
    let s = shown(&all_bytes(&buf), 20, lines.len());
    let mut text = String::new();
    for y in 0..lines.len() {
        for (_, t, u) in s.links(y) {
            assert_eq!(u, "https://ex.com/long");
            text.push_str(&t);
        }
    }
    assert!(
        text.replace(' ', "")
            .contains("unliendontletexteestbienpluslongquelaligne"),
        "{text}"
    );
}

#[test]
fn a_link_replaced_by_text_leaves_no_link_behind() {
    let (with, _) = buffer(&rich("[lien](https://ex.com/a) et suite", 40), 40, 1, true);
    let (without, _) = buffer(&rich("texte ordinaire sans lien du tout", 40), 40, 1, true);
    let empty = Buffer::empty(with.area);
    let mut s = Screen::new(40, 1);
    s.feed(&diff_bytes(&empty, &with));
    assert!(!s.links(0).is_empty());
    s.feed(&diff_bytes(&with, &without));
    assert_eq!(s.text(0).trim_end(), "texte ordinaire sans lien du tout");
    assert!(s.links(0).is_empty(), "{:?}", s.links(0));
    // And back: the link returns where it is drawn.
    s.feed(&diff_bytes(&without, &with));
    assert_eq!(s.links(0)[0].2, "https://ex.com/a");
    // A shorter frame (scrolled, resized): nothing linked past the text.
    let (short, _) = buffer(&rich("x", 40), 40, 1, true);
    s.feed(&diff_bytes(&with, &short));
    assert!(s.links(0).is_empty());
}

#[test]
fn off_or_unsafe_means_text_only() {
    let md = "[a](https://ex.com) [b](file:///etc/passwd) [c](javascript:alert(1)) [d](./x.md)";
    let lines = rich(md, 80);
    // Off: no OSC 8 at all, the addresses written.
    let (off, _) = buffer(&lines, 80, 1, false);
    let bytes = diff_bytes(&Buffer::empty(off.area), &off);
    assert!(!String::from_utf8_lossy(&bytes).contains("\x1b]8"));
    // On: only the web address is active.
    let (on, _) = buffer(&lines, 80, 1, true);
    let s = shown(&diff_bytes(&Buffer::empty(on.area), &on), 80, 1);
    let urls: Vec<String> = s.links(0).into_iter().map(|l| l.2).collect();
    assert!(
        !urls.is_empty() && urls.iter().all(|u| u == "https://ex.com/"),
        "{urls:?}"
    );
    assert!(
        s.text(0).contains("<file:///etc/passwd>"),
        "shown, not active"
    );
    // A destination with control characters: never active, shown neutralised.
    let lines = rich("[x](<https://ex.com/\u{1b}]8;;https://evil\u{7}>)", 80);
    let (on, _) = buffer(&lines, 80, 1, true);
    let out = String::from_utf8(diff_bytes(&Buffer::empty(on.area), &on)).unwrap();
    assert!(
        !out.contains("evil\u{7}") && !out.contains("\u{1b}]8;;https://evil"),
        "{out:?}"
    );
}

#[test]
fn restored_and_live_answers_render_the_same() {
    use bricks_tui::state::App;
    use cersei_types::{Message, Role};
    let text = "Voir [la doc](https://ex.com/doc).";
    let mut app = App::new();
    app.load_history(&[
        Message::user("q"),
        Message {
            role: Role::Assistant,
            ..Message::assistant(text)
        },
    ]);
    let restored = app
        .cells
        .iter()
        .flat_map(|c| bricks_tui::view::cell_rich_lines(c, false))
        .filter(|l| l.links.iter().any(Option::is_some))
        .count();
    assert_eq!(restored, 1);
    let live = markdown::render_rich(text);
    assert_eq!(
        live.iter()
            .filter(|l| l.links.iter().any(Option::is_some))
            .count(),
        1
    );
}
