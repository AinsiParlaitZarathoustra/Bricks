//! Incremental cleaning of terminal output.
//!
//! Bytes arrive in arbitrary chunks: an escape sequence or a UTF-8 character
//! may be split between two reads. [`TerminalCleaner`] keeps the state of a
//! small terminal parser across chunks and produces readable text:
//!
//! * CSI (`ESC [ … final`), OSC (`ESC ] … BEL|ST`), DCS/SOS/PM/APC strings
//!   and two-byte escapes are removed, as are their 8-bit C1 forms;
//! * a carriage return not followed by a line feed rewinds the line, so a
//!   progress bar leaves only its final state; backspace erases one character;
//! * line feeds and tabs are kept; other control characters are dropped;
//! * invalid UTF-8 becomes U+FFFD, never a panic or a lost line.
//!
//! The raw bytes are kept separately by the capture (see `capture.rs`): this
//! is the readable view only.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    /// OSC, DCS, SOS, PM, APC: a string ended by BEL (OSC) or ST.
    String {
        bel_ends: bool,
    },
    /// ESC seen inside a string: `\` completes ST.
    StringEscape {
        bel_ends: bool,
    },
}

#[derive(Debug)]
pub struct TerminalCleaner {
    state: State,
    /// Bytes of an incomplete UTF-8 character carried to the next chunk.
    pending: Vec<u8>,
    line: String,
    /// A `\r` was seen; the next character decides (CRLF or rewind).
    pending_cr: bool,
    /// Escape sequences removed so far.
    pub sequences_removed: u64,
    /// Longest line held (an endless line is emitted in cut pieces).
    max_line: usize,
}

/// Longest line the cleaner holds by default.
pub const DEFAULT_MAX_LINE: usize = 1024 * 1024;

impl Default for TerminalCleaner {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalCleaner {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            pending: Vec::new(),
            line: String::new(),
            pending_cr: false,
            sequences_removed: 0,
            max_line: DEFAULT_MAX_LINE,
        }
    }

    /// Hold at most `n` bytes of an unfinished line: beyond, the piece is
    /// emitted as a line ending with [`super::background::LINE_CUT`].
    pub fn with_max_line(mut self, n: usize) -> Self {
        self.max_line = n.max(64);
        self
    }

    /// Feed a chunk; returns the text of the lines completed by it (each with
    /// its `\n`). The current, unfinished line is held until more input or
    /// [`Self::finish`].
    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut out = String::new();
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(bytes);
        let mut i = 0;
        while i < buf.len() {
            let b = buf[i];
            // Decode one UTF-8 scalar (or an invalid byte).
            let (ch, len) = if b < 0x80 {
                (b as char, 1)
            } else {
                let need = match b {
                    0xC2..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    0xF0..=0xF4 => 4,
                    _ => 0,
                };
                if need == 0 {
                    ('\u{FFFD}', 1)
                } else if i + need > buf.len() {
                    // Incomplete at the end of the chunk: wait for the rest.
                    self.pending = buf[i..].to_vec();
                    break;
                } else {
                    match std::str::from_utf8(&buf[i..i + need]) {
                        Ok(s) => (s.chars().next().unwrap_or('\u{FFFD}'), need),
                        Err(_) => ('\u{FFFD}', 1),
                    }
                }
            };
            i += len;
            self.feed(ch, &mut out);
        }
        out
    }

    /// Flush what is left (an unfinished line, an incomplete character).
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        if !self.pending.is_empty() {
            self.pending.clear();
            self.feed('\u{FFFD}', &mut out);
        }
        self.pending_cr = false;
        out.push_str(&std::mem::take(&mut self.line));
        self.state = State::Ground;
        out
    }

    fn feed(&mut self, ch: char, out: &mut String) {
        match self.state {
            State::Ground => {}
            State::Escape => {
                self.state = match ch {
                    '[' => State::Csi,
                    ']' => State::String { bel_ends: true },
                    'P' | 'X' | '^' | '_' => State::String { bel_ends: false },
                    '\u{20}'..='\u{2F}' => State::EscapeIntermediate,
                    _ => {
                        self.sequences_removed += 1;
                        State::Ground
                    }
                };
                return;
            }
            State::EscapeIntermediate => {
                if !('\u{20}'..='\u{2F}').contains(&ch) {
                    self.sequences_removed += 1;
                    self.state = State::Ground;
                }
                return;
            }
            State::Csi => {
                if ('\u{40}'..='\u{7E}').contains(&ch) {
                    self.sequences_removed += 1;
                    self.state = State::Ground;
                } else if ch == '\u{1b}' {
                    // A malformed CSI interrupted by a new escape.
                    self.state = State::Escape;
                }
                return;
            }
            State::String { bel_ends } => {
                if ch == '\u{07}' && bel_ends || ch == '\u{9C}' {
                    self.sequences_removed += 1;
                    self.state = State::Ground;
                } else if ch == '\u{1b}' {
                    self.state = State::StringEscape { bel_ends };
                }
                return;
            }
            State::StringEscape { bel_ends } => {
                self.state = if ch == '\\' {
                    self.sequences_removed += 1;
                    State::Ground
                } else {
                    State::String { bel_ends }
                };
                return;
            }
        }

        // Ground state.
        if self.pending_cr {
            self.pending_cr = false;
            if ch != '\n' {
                // A lone CR rewinds the line: the progress state is replaced.
                self.line.clear();
            }
        }
        match ch {
            '\u{1b}' => self.state = State::Escape,
            '\u{9B}' => self.state = State::Csi,
            '\u{9D}' => self.state = State::String { bel_ends: true },
            '\u{90}' | '\u{98}' | '\u{9E}' | '\u{9F}' => {
                self.state = State::String { bel_ends: false }
            }
            '\n' => {
                out.push_str(&self.line);
                out.push('\n');
                self.line.clear();
            }
            '\r' => self.pending_cr = true,
            '\t' => self.line.push('\t'),
            '\u{08}' => {
                self.line.pop();
            }
            c if c.is_control() => {}
            c => {
                self.line.push(c);
                if self.line.len() >= self.max_line {
                    out.push_str(&self.line);
                    out.push_str(super::background::LINE_CUT);
                    out.push('\n');
                    self.line.clear();
                }
            }
        }
    }
}

/// Clean a complete text in one go.
pub fn clean(bytes: &[u8]) -> String {
    let mut c = TerminalCleaner::new();
    let mut s = c.push(bytes);
    s.push_str(&c.finish());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_cursor_moves_and_osc_titles_are_removed() {
        let raw = b"\x1b[1;31mred\x1b[0m \x1b]0;title\x07ok \x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\ \x1b(Bdone\x1b[2K\n";
        assert_eq!(clean(raw), "red ok link done\n");
    }

    #[test]
    fn sequences_and_characters_split_between_reads() {
        let raw = "\x1b[32mvert é 日本\x1b[0m\n".as_bytes();
        for cut in 0..raw.len() {
            let mut c = TerminalCleaner::new();
            let mut out = c.push(&raw[..cut]);
            out.push_str(&c.push(&raw[cut..]));
            out.push_str(&c.finish());
            assert_eq!(out, "vert é 日本\n", "cut at {cut}");
        }
    }

    #[test]
    fn progress_rewrites_keep_the_final_state_and_crlf_is_a_newline() {
        assert_eq!(
            clean(b"10%\r50%\r100%\ndone\r\nnext\n"),
            "100%\ndone\nnext\n"
        );
        // CR and LF split across reads.
        let mut c = TerminalCleaner::new();
        let mut out = c.push(b"line\r");
        out.push_str(&c.push(b"\nafter\n"));
        assert_eq!(out, "line\nafter\n");
    }

    #[test]
    fn tabs_kept_controls_dropped_backspace_erases() {
        assert_eq!(clean(b"a\tb\x07c\x00d\n"), "a\tbcd\n");
        assert_eq!(clean(b"abX\x08c\n"), "abc\n");
    }

    #[test]
    fn invalid_utf8_is_replaced_not_lost() {
        assert_eq!(clean(b"ok \xff\xfe end\n"), "ok \u{FFFD}\u{FFFD} end\n");
        // A truncated character at the very end.
        assert_eq!(clean(b"cut \xe6\x97"), "cut \u{FFFD}");
    }

    #[test]
    fn unfinished_line_is_held_until_finish() {
        let mut c = TerminalCleaner::new();
        assert_eq!(c.push(b"partial"), "");
        assert_eq!(c.push(b" line\nnext"), "partial line\n");
        assert_eq!(c.finish(), "next");
    }
}
