//! Local token estimation: the fallback used when no server measurement is
//! available. It is an *estimate*, never a count — only the `usage` a server
//! reports (or a configured counting endpoint) measures tokens.
//!
//! There is no universal tokenizer: every model family splits text
//! differently. The heuristic below counts character classes, which tracks
//! common BPE tokenizers reasonably on prose, code and JSON, and carries an
//! explicit upper bound so budget decisions can stay conservative.

use serde::{Deserialize, Serialize};

/// Identifies the heuristic in reports, so an estimate is never mistaken for
/// a measurement.
pub const ESTIMATION_METHOD: &str = "local character-class heuristic (v1)";

/// Relative margin applied to the central estimate to get the upper bound.
pub const UPPER_BOUND_FACTOR: f64 = 1.25;

/// A token estimate: a central value and a conservative upper bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenEstimate {
    pub tokens: u64,
    pub upper: u64,
}

impl TokenEstimate {
    pub const ZERO: TokenEstimate = TokenEstimate {
        tokens: 0,
        upper: 0,
    };

    /// An estimate with the default uncertainty around `tokens`.
    pub fn around(tokens: u64) -> Self {
        Self {
            tokens,
            upper: (tokens as f64 * UPPER_BOUND_FACTOR).ceil() as u64,
        }
    }

    /// Scale both values (used to apply a per-model calibration factor).
    pub fn scaled(self, factor: f64) -> Self {
        Self {
            tokens: (self.tokens as f64 * factor).round() as u64,
            upper: (self.upper as f64 * factor).ceil() as u64,
        }
    }
}

impl std::ops::Add for TokenEstimate {
    type Output = TokenEstimate;
    fn add(self, o: TokenEstimate) -> TokenEstimate {
        TokenEstimate {
            tokens: self.tokens + o.tokens,
            upper: self.upper + o.upper,
        }
    }
}

impl std::ops::AddAssign for TokenEstimate {
    fn add_assign(&mut self, o: TokenEstimate) {
        *self = *self + o;
    }
}

impl std::iter::Sum for TokenEstimate {
    fn sum<I: Iterator<Item = TokenEstimate>>(iter: I) -> Self {
        iter.fold(TokenEstimate::ZERO, |a, b| a + b)
    }
}

/// Estimate the tokens of a piece of text.
///
/// Weights per character class: ASCII letters and digits ≈ 4 per token,
/// ASCII punctuation ≈ 0.9 token each (code and JSON are punctuation-dense),
/// spaces ≈ 0.1, line breaks ≈ 0.5, and every non-ASCII character ≈ 1 token
/// (accented and CJK text tokenizes far worse than ASCII prose).
pub fn estimate_text(text: &str) -> TokenEstimate {
    if text.is_empty() {
        return TokenEstimate::ZERO;
    }
    let (mut alnum, mut punct, mut space, mut newline, mut other) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            alnum += 1;
        } else if ch == '\n' {
            newline += 1;
        } else if ch.is_ascii_whitespace() {
            space += 1;
        } else if ch.is_ascii() {
            punct += 1;
        } else {
            other += 1;
        }
    }
    let central = alnum as f64 / 4.0
        + punct as f64 * 0.9
        + space as f64 * 0.1
        + newline as f64 * 0.5
        + other as f64;
    TokenEstimate::around(central.ceil().max(1.0) as u64)
}

/// Estimate for a fixed per-item cost (an image, a framing overhead) whose
/// real cost is not derivable from local data. The upper bound is doubled.
pub fn fixed_cost(tokens: u64) -> TokenEstimate {
    TokenEstimate {
        tokens,
        upper: tokens * 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate_text(""), TokenEstimate::ZERO);
    }

    #[test]
    fn upper_bound_is_never_below_the_estimate() {
        for s in [
            "a",
            "hello world",
            "fn main() {}",
            "日本語のテキスト",
            "{\"a\":[1,2,3]}",
        ] {
            let e = estimate_text(s);
            assert!(e.tokens >= 1, "{s}");
            assert!(e.upper >= e.tokens, "{s}: {e:?}");
        }
    }

    #[test]
    fn prose_is_close_to_four_chars_per_token() {
        let prose =
            "The quick brown fox jumps over the lazy dog and keeps running far away. ".repeat(20);
        let e = estimate_text(&prose);
        let chars = prose.chars().count() as f64;
        let ratio = chars / e.tokens as f64;
        assert!((3.2..=5.0).contains(&ratio), "chars/token = {ratio}");
    }

    #[test]
    fn non_ascii_text_is_not_underestimated() {
        // CJK text is roughly one token per character with common tokenizers;
        // a bytes/4 rule would report ~0.75 token per character.
        let cjk = "上下文窗口管理".repeat(50);
        assert!(estimate_text(&cjk).tokens >= cjk.chars().count() as u64);
    }

    #[test]
    fn code_is_denser_than_prose() {
        let code = "fn add(a: i32, b: i32) -> i32 { a + b }\n".repeat(20);
        let prose = "the result of adding two numbers is returned to caller\n".repeat(20);
        let per_char = |s: &str| estimate_text(s).tokens as f64 / s.len() as f64;
        assert!(per_char(&code) > per_char(&prose));
    }
}
