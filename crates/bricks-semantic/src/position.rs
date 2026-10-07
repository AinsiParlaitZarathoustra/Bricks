//! Exact position conversions for one document version.
//!
//! Conventions used throughout the engine:
//!
//! * the canonical position is a **byte offset** into the document's UTF-8
//!   text; ranges are **half-open** `[start, end)`;
//! * [`LineCol`] is **0-based**, its column counted in **bytes** from the
//!   line start; lines end at `\n`, `\r\n` or a lone `\r` (the LSP rule);
//! * Tree-sitter points count rows at `\n` only, so they are converted
//!   separately ([`PositionMapper::to_point`]);
//! * LSP positions count columns in the encoding negotiated with the server
//!   (UTF-16 unless the server chose otherwise).
//!
//! 1-based `line:col` is a display convention, applied at the frontier only.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Column unit of an LSP position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PositionEncoding {
    Utf8,
    /// The LSP default when nothing else is negotiated.
    #[default]
    Utf16,
    Utf32,
}

impl PositionEncoding {
    /// Parse an LSP `PositionEncodingKind`.
    pub fn from_lsp(kind: &str) -> Option<Self> {
        match kind {
            "utf-8" => Some(Self::Utf8),
            "utf-16" => Some(Self::Utf16),
            "utf-32" => Some(Self::Utf32),
            _ => None,
        }
    }

    pub fn as_lsp(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
            Self::Utf32 => "utf-32",
        }
    }

    fn units(self, ch: char) -> u32 {
        match self {
            Self::Utf8 => ch.len_utf8() as u32,
            Self::Utf16 => ch.len_utf16() as u32,
            Self::Utf32 => 1,
        }
    }
}

/// 0-based line and byte column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// A position that does not exist in this document version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionError {
    OffsetOutOfRange {
        offset: usize,
        len: usize,
    },
    NotCharBoundary {
        offset: usize,
    },
    LineOutOfRange {
        line: u32,
        lines: u32,
    },
    ColumnOutOfRange {
        line: u32,
        col: u32,
    },
    /// The column falls inside a multi-unit character (e.g. between the two
    /// halves of a UTF-16 surrogate pair).
    InsideCharacter {
        line: u32,
        col: u32,
    },
}

impl std::fmt::Display for PositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OffsetOutOfRange { offset, len } => {
                write!(
                    f,
                    "offset {offset} is past the end of the document ({len} bytes)"
                )
            }
            Self::NotCharBoundary { offset } => {
                write!(f, "offset {offset} is inside a UTF-8 sequence")
            }
            Self::LineOutOfRange { line, lines } => {
                write!(f, "line {line} does not exist ({lines} lines)")
            }
            Self::ColumnOutOfRange { line, col } => {
                write!(f, "column {col} is past the end of line {line}")
            }
            Self::InsideCharacter { line, col } => {
                write!(f, "column {col} of line {line} is inside a character")
            }
        }
    }
}

impl std::error::Error for PositionError {}

/// Converts between offsets, lines/columns, Tree-sitter points and LSP
/// positions for one exact text.
#[derive(Debug, Clone)]
pub struct PositionMapper {
    text: Arc<str>,
    /// Byte offset of each line start (LSP line rule).
    line_starts: Vec<usize>,
}

impl PositionMapper {
    pub fn new(text: Arc<str>) -> Self {
        let bytes = text.as_bytes();
        let mut line_starts = vec![0];
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => line_starts.push(i + 1),
                b'\r' => {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    line_starts.push(i + 1);
                }
                _ => {}
            }
            i += 1;
        }
        Self { text, line_starts }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Number of lines (an empty document has one empty line; a trailing
    /// line break opens a last, empty line).
    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    fn check_offset(&self, offset: usize) -> Result<(), PositionError> {
        if offset > self.text.len() {
            return Err(PositionError::OffsetOutOfRange {
                offset,
                len: self.text.len(),
            });
        }
        if !self.text.is_char_boundary(offset) {
            return Err(PositionError::NotCharBoundary { offset });
        }
        Ok(())
    }

    /// Byte range of a line's content, without its terminator.
    pub fn line_range(&self, line: u32) -> Result<std::ops::Range<usize>, PositionError> {
        let idx = line as usize;
        let start = *self
            .line_starts
            .get(idx)
            .ok_or(PositionError::LineOutOfRange {
                line,
                lines: self.line_count(),
            })?;
        let mut end = self
            .line_starts
            .get(idx + 1)
            .copied()
            .unwrap_or(self.text.len());
        let bytes = self.text.as_bytes();
        if end > start && bytes[end - 1] == b'\n' {
            end -= 1;
        }
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        Ok(start..end)
    }

    /// A line's content, without its terminator.
    pub fn line_text(&self, line: u32) -> Result<&str, PositionError> {
        let r = self.line_range(line)?;
        Ok(&self.text[r])
    }

    /// The line containing `offset` (an offset on a terminator belongs to
    /// the line it ends).
    pub fn line_of(&self, offset: usize) -> Result<u32, PositionError> {
        self.check_offset(offset)?;
        Ok(self.line_starts.partition_point(|&s| s <= offset) as u32 - 1)
    }

    pub fn offset_to_line_col(&self, offset: usize) -> Result<LineCol, PositionError> {
        let line = self.line_of(offset)?;
        let start = self.line_starts[line as usize];
        Ok(LineCol {
            line,
            col: (offset - start) as u32,
        })
    }

    pub fn line_col_to_offset(&self, lc: LineCol) -> Result<usize, PositionError> {
        let range = self.line_range(lc.line)?;
        let offset = range.start + lc.col as usize;
        if offset > range.end {
            return Err(PositionError::ColumnOutOfRange {
                line: lc.line,
                col: lc.col,
            });
        }
        self.check_offset(offset)?;
        Ok(offset)
    }

    /// Tree-sitter point (rows at `\n`, byte columns).
    pub fn to_point(&self, offset: usize) -> Result<tree_sitter::Point, PositionError> {
        self.check_offset(offset)?;
        let before = &self.text.as_bytes()[..offset];
        let row = before.iter().filter(|&&b| b == b'\n').count();
        let line_start = before
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        Ok(tree_sitter::Point {
            row,
            column: offset - line_start,
        })
    }

    /// Offset of a Tree-sitter point.
    pub fn from_point(&self, point: tree_sitter::Point) -> Result<usize, PositionError> {
        let bytes = self.text.as_bytes();
        let mut start = 0usize;
        for _ in 0..point.row {
            match bytes[start..].iter().position(|&b| b == b'\n') {
                Some(p) => start += p + 1,
                None => {
                    return Err(PositionError::LineOutOfRange {
                        line: point.row as u32,
                        lines: bytes.iter().filter(|&&b| b == b'\n').count() as u32 + 1,
                    })
                }
            }
        }
        let offset = start + point.column;
        self.check_offset(offset)?;
        Ok(offset)
    }

    /// LSP position of `offset`, columns in `enc` units.
    pub fn to_lsp(
        &self,
        offset: usize,
        enc: PositionEncoding,
    ) -> Result<cersei_lsp::Position, PositionError> {
        let lc = self.offset_to_line_col(offset)?;
        let start = self.line_starts[lc.line as usize];
        let character = self.text[start..offset].chars().map(|c| enc.units(c)).sum();
        Ok(cersei_lsp::Position {
            line: lc.line,
            character,
        })
    }

    /// Offset of an LSP position. Per the specification, a column past the
    /// end of its line means the end of the line; a line past the last one
    /// and a column inside a character are errors.
    pub fn from_lsp(
        &self,
        pos: &cersei_lsp::Position,
        enc: PositionEncoding,
    ) -> Result<usize, PositionError> {
        let range = self.line_range(pos.line)?;
        let mut units = 0u32;
        for (i, ch) in self.text[range.clone()].char_indices() {
            if units == pos.character {
                return Ok(range.start + i);
            }
            units += enc.units(ch);
            if units > pos.character {
                return Err(PositionError::InsideCharacter {
                    line: pos.line,
                    col: pos.character,
                });
            }
        }
        Ok(range.end)
    }

    /// The largest char boundary `<= offset` (never cuts a UTF-8 sequence).
    pub fn floor_boundary(&self, offset: usize) -> usize {
        let mut o = offset.min(self.text.len());
        while !self.text.is_char_boundary(o) {
            o -= 1;
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> PositionMapper {
        PositionMapper::new(Arc::from(s))
    }

    #[test]
    fn empty_document() {
        let pm = m("");
        assert_eq!(pm.line_count(), 1);
        assert_eq!(
            pm.offset_to_line_col(0).unwrap(),
            LineCol { line: 0, col: 0 }
        );
        assert_eq!(pm.to_lsp(0, PositionEncoding::Utf16).unwrap().character, 0);
        assert!(pm.offset_to_line_col(1).is_err());
        assert_eq!(
            pm.from_lsp(
                &cersei_lsp::Position {
                    line: 0,
                    character: 5
                },
                PositionEncoding::Utf16
            )
            .unwrap(),
            0
        );
        assert!(pm
            .from_lsp(
                &cersei_lsp::Position {
                    line: 1,
                    character: 0
                },
                PositionEncoding::Utf16
            )
            .is_err());
    }

    #[test]
    fn crlf_and_lone_cr() {
        let pm = m("a\r\nbc\rd\n");
        assert_eq!(pm.line_count(), 4);
        assert_eq!(pm.line_text(0).unwrap(), "a");
        assert_eq!(pm.line_text(1).unwrap(), "bc");
        assert_eq!(pm.line_text(2).unwrap(), "d");
        assert_eq!(pm.line_text(3).unwrap(), "");
        // `b` is byte 3.
        assert_eq!(
            pm.offset_to_line_col(3).unwrap(),
            LineCol { line: 1, col: 0 }
        );
        // The `\r` of the CRLF belongs to line 0.
        assert_eq!(pm.line_of(1).unwrap(), 0);
        // Tree-sitter does not break at a lone `\r`: `d` is on row 1.
        assert_eq!(
            pm.to_point(6).unwrap(),
            tree_sitter::Point { row: 1, column: 3 }
        );
        assert_eq!(
            pm.from_point(tree_sitter::Point { row: 1, column: 3 })
                .unwrap(),
            6
        );
        // End of document.
        assert_eq!(
            pm.offset_to_line_col(8).unwrap(),
            LineCol { line: 3, col: 0 }
        );
    }

    #[test]
    fn unicode_columns_per_encoding() {
        // é: 2 bytes / 1 UTF-16 unit; 😀: 4 bytes / 2 UTF-16 units.
        let text = "é😀x";
        let pm = m(text);
        let x = text.find('x').unwrap();
        assert_eq!(x, 6);
        assert_eq!(pm.to_lsp(x, PositionEncoding::Utf8).unwrap().character, 6);
        assert_eq!(pm.to_lsp(x, PositionEncoding::Utf16).unwrap().character, 3);
        assert_eq!(pm.to_lsp(x, PositionEncoding::Utf32).unwrap().character, 2);
        for enc in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf16,
            PositionEncoding::Utf32,
        ] {
            let p = pm.to_lsp(x, enc).unwrap();
            assert_eq!(pm.from_lsp(&p, enc).unwrap(), x, "{enc:?}");
        }
        // Inside the surrogate pair of 😀.
        assert_eq!(
            pm.from_lsp(
                &cersei_lsp::Position {
                    line: 0,
                    character: 2
                },
                PositionEncoding::Utf16
            ),
            Err(PositionError::InsideCharacter { line: 0, col: 2 })
        );
        // Inside a UTF-8 sequence.
        assert_eq!(
            pm.offset_to_line_col(1),
            Err(PositionError::NotCharBoundary { offset: 1 })
        );
        assert_eq!(pm.floor_boundary(4), 2);
        // Past the end of the line: end of line (LSP rule).
        assert_eq!(
            pm.from_lsp(
                &cersei_lsp::Position {
                    line: 0,
                    character: 99
                },
                PositionEncoding::Utf16
            )
            .unwrap(),
            text.len()
        );
    }

    #[test]
    fn line_col_round_trip() {
        let pm = m("fn a() {}\n  let ü = 1;\n");
        for off in [0, 3, 10, 12, 14, 24] {
            if !pm.text().is_char_boundary(off) {
                continue;
            }
            let lc = pm.offset_to_line_col(off).unwrap();
            assert_eq!(pm.line_col_to_offset(lc).unwrap(), off);
        }
        assert!(pm.line_col_to_offset(LineCol { line: 0, col: 50 }).is_err());
        assert!(pm.line_col_to_offset(LineCol { line: 9, col: 0 }).is_err());
    }
}
