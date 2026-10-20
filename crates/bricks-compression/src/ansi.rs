//! ANSI escape handling.
//!
//! Credits: adapted from rtk (Rust Token Killer) — `rtk/src/core/utils.rs`.
//! MIT © Patrick Szymkowiak. See LICENSE.

use once_cell::sync::Lazy;
use regex::Regex;

static ANSI_RE: Lazy<Regex> = Lazy::new(|| {
    // Covers CSI sequences (color/style) and common OSC/ESC codes.
    Regex::new(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").unwrap()
});

pub fn strip_ansi(text: &str) -> String {
    ANSI_RE.replace_all(text, "").into_owned()
}

/// Clean one log line the way a terminal would display it: a carriage return
/// rewinds the line (progress bars), so only the text after the last `\r`
/// remains (or the last non-empty segment); ANSI escapes and other control
/// characters except tabs are removed.
pub fn clean_line(line: &str) -> String {
    let visible = if line.contains('\r') {
        line.rsplit('\r')
            .find(|seg| !strip_ansi(seg).trim().is_empty())
            .unwrap_or("")
    } else {
        line
    };
    let stripped = strip_ansi(visible);
    if stripped.chars().any(|c| c.is_control() && c != '\t') {
        stripped
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect()
    } else {
        stripped
    }
}

/// Cut `s` to at most `max_chars` characters, never inside a UTF-8 sequence,
/// saying how much was removed.
pub fn cut_chars(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let kept: String = s.chars().take(max_chars).collect();
    format!("{kept}… [+{} chars]", count - max_chars)
}

/// Unicode-safe char truncation — keeps at most `max_len` chars, appends `...`
/// when the input is longer.
pub fn truncate(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else if max_len < 3 {
        "...".to_string()
    } else {
        format!("{}...", s.chars().take(max_len - 3).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_basic() {
        assert_eq!(strip_ansi("\x1b[31mError\x1b[0m"), "Error");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\x1b[1m\x1b[32mOK\x1b[0m\x1b[0m"), "OK");
    }

    #[test]
    fn progress_rewrites_keep_the_final_state() {
        assert_eq!(
            clean_line("Downloading 10%\rDownloading 55%\rDone ✓"),
            "Done ✓"
        );
        assert_eq!(clean_line("50%\r100%\r"), "100%");
        assert_eq!(clean_line("\x1b[32mok\x1b[0m\tnext\x07"), "ok\tnext");
    }

    #[test]
    fn cut_chars_respects_utf8() {
        assert_eq!(cut_chars("données", 3), "don… [+4 chars]");
        assert_eq!(cut_chars("abc", 5), "abc");
    }

    #[test]
    fn truncate_unicode() {
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("hi", 10), "hi");
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("hello world", 3), "...");
        // Thai, multi-byte
        assert_eq!(truncate("สวัสดีครับ", 5).chars().count(), 5);
    }
}
