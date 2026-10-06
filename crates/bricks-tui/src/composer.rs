//! The prompt composer: a multi-line buffer with a cursor and a selection
//! that always sit on grapheme boundaries, prompt history, paste and typed
//! attachments.
//!
//! * Paste inserts text as is, newlines included: it never submits.
//! * Movement and deletion work by grapheme cluster (an accented letter
//!   written as two code points, an emoji with modifiers, a flag: one step).
//! * Display width uses `unicode-width` (wide CJK characters take two
//!   columns), so wrapped lines and the cursor position match the terminal.
//! * Attachments (files, folders, images) are blocks of their own, shown
//!   as chips above the text; they become `PromptBlock`s on submission.

use cersei_agent::control::{Prompt, PromptBlock};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attachment {
    File(String),
    Folder(String),
    Image(String),
}

impl Attachment {
    pub fn label(&self) -> String {
        match self {
            Attachment::File(p) => format!("file {p}"),
            Attachment::Folder(p) => format!("folder {p}/"),
            Attachment::Image(p) => format!("image {p}"),
        }
    }

    fn block(&self) -> PromptBlock {
        match self {
            Attachment::File(p) => PromptBlock::File { path: p.clone() },
            Attachment::Folder(p) => PromptBlock::Folder { path: p.clone() },
            Attachment::Image(p) => PromptBlock::Image { path: p.clone() },
        }
    }
}

#[derive(Debug, Default)]
pub struct Composer {
    text: String,
    /// Byte offset, always on a grapheme boundary.
    cursor: usize,
    /// Selection anchor (byte offset); the selection spans anchor..cursor.
    anchor: Option<usize>,
    attachments: Vec<Attachment>,
    history: Vec<String>,
    /// Position while browsing the history, and the draft it replaced.
    browsing: Option<(usize, String)>,
}

fn boundaries(s: &str) -> Vec<usize> {
    let mut b: Vec<usize> = s.grapheme_indices(true).map(|(i, _)| i).collect();
    b.push(s.len());
    b
}

impl Composer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
        self.anchor = None;
    }

    pub fn attach(&mut self, a: Attachment) {
        if !self.attachments.contains(&a) {
            self.attachments.push(a);
        }
    }

    pub fn remove_last_attachment(&mut self) -> Option<Attachment> {
        self.attachments.pop()
    }

    /// The selected range, ordered.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.cursor).then(|| (a.min(self.cursor), a.max(self.cursor)))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(a, b)| &self.text[a..b])
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((a, b)) = self.selection() {
            self.text.replace_range(a..b, "");
            self.cursor = a;
            self.anchor = None;
            true
        } else {
            self.anchor = None;
            false
        }
    }

    /// Insert text (typed or pasted) at the cursor, replacing the selection.
    pub fn insert_str(&mut self, s: &str) {
        self.browsing = None;
        self.delete_selection();
        // Terminals send CR for Enter inside pastes; keep one newline style.
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
        self.snap();
    }

    pub fn newline(&mut self) {
        self.insert_str("\n");
    }

    /// Keep the cursor on a grapheme boundary (inserting a combining mark
    /// merges with the previous character).
    fn snap(&mut self) {
        let b = boundaries(&self.text);
        if !b.contains(&self.cursor) {
            self.cursor = b
                .into_iter()
                .find(|&x| x >= self.cursor)
                .unwrap_or(self.text.len());
        }
    }

    fn prev_boundary(&self, at: usize) -> usize {
        boundaries(&self.text)
            .into_iter()
            .rev()
            .find(|&b| b < at)
            .unwrap_or(0)
    }

    fn next_boundary(&self, at: usize) -> usize {
        boundaries(&self.text)
            .into_iter()
            .find(|&b| b > at)
            .unwrap_or(self.text.len())
    }

    pub fn backspace(&mut self) {
        self.browsing = None;
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let p = self.prev_boundary(self.cursor);
        self.text.replace_range(p..self.cursor, "");
        self.cursor = p;
    }

    pub fn delete(&mut self) {
        self.browsing = None;
        if self.delete_selection() {
            return;
        }
        if self.cursor >= self.text.len() {
            return;
        }
        let n = self.next_boundary(self.cursor);
        self.text.replace_range(self.cursor..n, "");
    }

    fn moved(&mut self, to: usize, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = to;
    }

    pub fn left(&mut self, select: bool) {
        let to = self.prev_boundary(self.cursor);
        self.moved(to, select);
    }

    pub fn right(&mut self, select: bool) {
        let to = self.next_boundary(self.cursor);
        self.moved(to, select);
    }

    /// Previous word start.
    pub fn word_left(&mut self, select: bool) {
        let before = &self.text[..self.cursor];
        let to = before
            .unicode_word_indices()
            .map(|(i, _)| i)
            .rfind(|&i| i < self.cursor)
            .unwrap_or(0);
        self.moved(to, select);
    }

    /// Next word end.
    pub fn word_right(&mut self, select: bool) {
        let to = self.text[self.cursor..]
            .unicode_word_indices()
            .next()
            .map(|(i, w)| self.cursor + i + w.len())
            .unwrap_or(self.text.len());
        self.moved(to, select);
    }

    pub fn delete_word_left(&mut self) {
        if self.delete_selection() {
            return;
        }
        let end = self.cursor;
        self.word_left(false);
        self.text.replace_range(self.cursor..end, "");
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map(|i| at + i)
            .unwrap_or(self.text.len())
    }

    pub fn home(&mut self, select: bool) {
        let to = self.line_start(self.cursor);
        self.moved(to, select);
    }

    pub fn end(&mut self, select: bool) {
        let to = self.line_end(self.cursor);
        self.moved(to, select);
    }

    /// Display column of the cursor within its line.
    fn column(&self) -> usize {
        self.text[self.line_start(self.cursor)..self.cursor].width()
    }

    /// The byte offset at display column `col` of the line starting at
    /// `start` (never inside a grapheme).
    fn at_column(&self, start: usize, col: usize) -> usize {
        let end = self.line_end(start);
        let mut w = 0;
        for (i, g) in self.text[start..end].grapheme_indices(true) {
            let gw = g.width();
            if w + gw > col {
                return start + i;
            }
            w += gw;
        }
        end
    }

    pub fn on_first_line(&self) -> bool {
        !self.text[..self.cursor].contains('\n')
    }

    pub fn on_last_line(&self) -> bool {
        !self.text[self.cursor..].contains('\n')
    }

    pub fn up(&mut self, select: bool) {
        if self.on_first_line() {
            return;
        }
        let col = self.column();
        let prev_start = self.line_start(self.line_start(self.cursor) - 1);
        let to = self.at_column(prev_start, col);
        self.moved(to, select);
    }

    pub fn down(&mut self, select: bool) {
        if self.on_last_line() {
            return;
        }
        let col = self.column();
        let next_start = self.line_end(self.cursor) + 1;
        let to = self.at_column(next_start, col);
        self.moved(to, select);
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    /// Older prompt (when the cursor is on the first line).
    pub fn history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let pos = match &self.browsing {
            None => {
                self.browsing = Some((self.history.len() - 1, self.text.clone()));
                self.history.len() - 1
            }
            Some((0, _)) => return false,
            Some((p, d)) => {
                let (p, d) = (*p - 1, d.clone());
                self.browsing = Some((p, d));
                p
            }
        };
        self.text = self.history[pos].clone();
        self.cursor = self.text.len();
        self.anchor = None;
        true
    }

    /// Newer prompt, then back to the draft.
    pub fn history_next(&mut self) -> bool {
        let Some((p, draft)) = self.browsing.clone() else {
            return false;
        };
        if p + 1 < self.history.len() {
            self.browsing = Some((p + 1, draft));
            self.text = self.history[p + 1].clone();
        } else {
            self.browsing = None;
            self.text = draft;
        }
        self.cursor = self.text.len();
        self.anchor = None;
        true
    }

    /// The `@` mention being typed before the cursor: (start offset, query).
    /// A mention starts at the beginning or after whitespace; `@"a b` allows
    /// spaces until the closing quote.
    pub fn mention_query(&self) -> Option<(usize, String)> {
        let before = &self.text[..self.cursor];
        let at = before.rfind('@')?;
        if at > 0 && !before[..at].ends_with(char::is_whitespace) {
            return None;
        }
        let q = &before[at + 1..];
        if let Some(quoted) = q.strip_prefix('"') {
            if quoted.contains('"') {
                return None;
            }
            return Some((at, quoted.to_string()));
        }
        if q.contains(char::is_whitespace) {
            return None;
        }
        Some((at, q.to_string()))
    }

    /// Replace the mention being typed by an attachment.
    pub fn complete_mention(&mut self, a: Attachment) {
        if let Some((start, _)) = self.mention_query() {
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.anchor = None;
        }
        self.attach(a);
    }

    /// The `/command` being typed (the whole text starts with `/`).
    pub fn slash_query(&self) -> Option<&str> {
        let t = self.text.trim_start();
        let rest = t.strip_prefix('/')?;
        (!rest.contains(char::is_whitespace)).then_some(rest)
    }

    /// The prompt to submit; the composer is emptied and the text joins
    /// the history.
    pub fn take(&mut self) -> Prompt {
        let text = std::mem::take(&mut self.text);
        let mut blocks = Vec::new();
        if !text.trim().is_empty() {
            blocks.push(PromptBlock::Text { text: text.clone() });
            if self.history.last() != Some(&text) {
                self.history.push(text);
            }
        }
        blocks.extend(self.attachments.drain(..).map(|a| a.block()));
        self.cursor = 0;
        self.anchor = None;
        self.browsing = None;
        Prompt { blocks }
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.attachments.clear();
        self.cursor = 0;
        self.anchor = None;
        self.browsing = None;
    }

    /// The text wrapped to `width` columns (graphemes never split), and the
    /// cursor's (row, column) in it.
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(2);
        let mut rows = Vec::new();
        let mut cursor = (0, 0);
        let mut offset = 0;
        for line in self.text.split('\n') {
            let mut row = String::new();
            let mut w = 0;
            let mut placed = false;
            for (i, g) in line.grapheme_indices(true) {
                let gw = g.width();
                if w + gw > width {
                    rows.push(std::mem::take(&mut row));
                    w = 0;
                }
                if offset + i == self.cursor && !placed {
                    cursor = (rows.len(), w);
                    placed = true;
                }
                row.push_str(g);
                w += gw;
            }
            if offset + line.len() == self.cursor && !placed {
                if w >= width {
                    rows.push(std::mem::take(&mut row));
                    w = 0;
                }
                cursor = (rows.len(), w);
            }
            rows.push(row);
            offset += line.len() + 1;
        }
        (rows, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes_are_atomic_and_widths_are_displayed_widths() {
        let mut c = Composer::new();
        // "é" as e + combining acute, a family emoji, a flag, wide CJK.
        c.insert_str("e\u{301}👨‍👩‍👧🇫🇷漢");
        c.backspace();
        assert_eq!(c.text(), "e\u{301}👨‍👩‍👧🇫🇷");
        c.left(false);
        c.left(false);
        c.backspace();
        assert_eq!(c.text(), "👨‍👩‍👧🇫🇷", "the whole é, not half of it");
        c.set_text("漢字ab");
        let (rows, cur) = c.layout(3);
        assert_eq!(rows, vec!["漢", "字a", "b"]);
        assert_eq!(cur, (2, 1));
        // A combining mark typed after a letter merges with it.
        c.set_text("e");
        c.insert_str("\u{301}");
        c.left(false);
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn paste_keeps_newlines_and_selection_is_replaced() {
        let mut c = Composer::new();
        c.insert_str("début ");
        c.insert_str("ligne 1\r\nligne 2\rfin");
        assert_eq!(c.text(), "début ligne 1\nligne 2\nfin");
        c.home(false);
        c.end(true);
        assert_eq!(c.selected_text(), Some("fin"));
        c.insert_str("FIN");
        assert_eq!(c.text(), "début ligne 1\nligne 2\nFIN");
        c.up(false);
        c.up(false);
        assert!(c.on_first_line());
        c.word_right(false);
        c.delete_word_left();
        assert_eq!(c.text(), " ligne 1\nligne 2\nFIN");
    }

    #[test]
    fn history_mentions_and_slash_commands() {
        let mut c = Composer::new();
        c.insert_str("premier");
        let p = c.take();
        assert_eq!(
            p.blocks,
            vec![PromptBlock::Text {
                text: "premier".into()
            }]
        );
        c.insert_str("brouillon");
        assert!(c.history_prev());
        assert_eq!(c.text(), "premier");
        assert!(c.history_next());
        assert_eq!(c.text(), "brouillon", "back to the draft");

        c.set_text("regarde @src/ma");
        assert_eq!(c.mention_query(), Some((8, "src/ma".into())));
        c.set_text("mail@example.com");
        assert_eq!(c.mention_query(), None, "not after a word");
        c.set_text("voir @\"dossier avec esp");
        assert_eq!(c.mention_query().unwrap().1, "dossier avec esp");
        c.complete_mention(Attachment::Folder("dossier avec espaces".into()));
        assert_eq!(c.text(), "voir ");
        let p = c.take();
        assert_eq!(
            p.blocks[1],
            PromptBlock::Folder {
                path: "dossier avec espaces".into()
            }
        );

        c.set_text("/mod");
        assert_eq!(c.slash_query(), Some("mod"));
        c.set_text("/model test");
        assert_eq!(c.slash_query(), None);
    }
}
