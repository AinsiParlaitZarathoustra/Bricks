//! Hand-written lexer for the AgentTemplate language.
//!
//! Produces a flat `Vec<Token>` with spans. Comments (`# ...`) are discarded.
//! Consecutive newlines collapse to a single `Newline` token (statement sep).

use crate::error::{ParseError, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Var(String), // $name (the `$` is stripped)
    Str(String),
    Num(f64),
    Dot,
    Comma,
    Colon,
    Eq,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
}

impl Lexer {
    fn new(src: &str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn span_here(&self, len: u32) -> Span {
        Span::new(self.line, self.col, len)
    }

    fn tokenize(mut self) -> Result<Vec<Token>, ParseError> {
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => {
                    out.push(Token {
                        tok: Tok::Eof,
                        span: self.span_here(0),
                    });
                    break;
                }
                Some(c) if c == ' ' || c == '\t' || c == '\r' => {
                    self.bump();
                }
                Some('#') => {
                    // comment to end of line
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some('\n') => {
                    let span = self.span_here(1);
                    self.bump();
                    // collapse runs of newlines (and intervening blanks)
                    if !matches!(out.last().map(|t| &t.tok), Some(Tok::Newline) | None) {
                        out.push(Token {
                            tok: Tok::Newline,
                            span,
                        });
                    }
                }
                Some('"') | Some('\'') => out.push(self.lex_string()?),
                Some('$') => out.push(self.lex_var()?),
                Some(c) if c.is_ascii_digit() => out.push(self.lex_number()?),
                Some('-') if self.peek2().map(|c| c.is_ascii_digit()).unwrap_or(false) => {
                    out.push(self.lex_number()?)
                }
                Some(c) if c.is_ascii_alphabetic() || c == '_' => out.push(self.lex_ident()),
                Some(c) => {
                    let span = self.span_here(1);
                    let single = match c {
                        '.' => Tok::Dot,
                        ',' => Tok::Comma,
                        ':' => Tok::Colon,
                        '=' => Tok::Eq,
                        '(' => Tok::LParen,
                        ')' => Tok::RParen,
                        '[' => Tok::LBracket,
                        ']' => Tok::RBracket,
                        other => {
                            return Err(ParseError {
                                message: format!("unexpected character '{other}'"),
                                span,
                            })
                        }
                    };
                    self.bump();
                    out.push(Token { tok: single, span });
                }
            }
        }
        Ok(out)
    }

    fn lex_string(&mut self) -> Result<Token, ParseError> {
        let start = self.span_here(0);
        let quote = self.bump().unwrap(); // ' or "
        let mut s = String::new();
        loop {
            match self.bump() {
                None => {
                    return Err(ParseError {
                        message: "unterminated string literal".into(),
                        span: start,
                    })
                }
                Some('\\') => match self.bump() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('\'') => s.push('\''),
                    Some(other) => {
                        s.push('\\');
                        s.push(other);
                    }
                    None => {
                        return Err(ParseError {
                            message: "unterminated escape in string".into(),
                            span: start,
                        })
                    }
                },
                Some(c) if c == quote => break,
                Some(c) => s.push(c),
            }
        }
        let len = s.chars().count() as u32 + 2;
        Ok(Token {
            tok: Tok::Str(s),
            span: Span::new(start.line, start.col, len),
        })
    }

    fn lex_var(&mut self) -> Result<Token, ParseError> {
        let start = self.span_here(0);
        self.bump(); // $
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                name.push(c);
                self.bump();
            } else {
                break;
            }
        }
        if name.is_empty() {
            return Err(ParseError {
                message: "expected identifier after '$'".into(),
                span: start,
            });
        }
        let len = name.chars().count() as u32 + 1;
        Ok(Token {
            tok: Tok::Var(name),
            span: Span::new(start.line, start.col, len),
        })
    }

    fn lex_number(&mut self) -> Result<Token, ParseError> {
        let start = self.span_here(0);
        let mut raw = String::new();
        if self.peek() == Some('-') {
            raw.push('-');
            self.bump();
        }
        let mut seen_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                raw.push(c);
                self.bump();
            } else if c == '.'
                && !seen_dot
                && self.peek2().map(|d| d.is_ascii_digit()).unwrap_or(false)
            {
                seen_dot = true;
                raw.push(c);
                self.bump();
            } else {
                break;
            }
        }
        let n: f64 = raw.parse().map_err(|_| ParseError {
            message: format!("invalid number literal '{raw}'"),
            span: start,
        })?;
        Ok(Token {
            tok: Tok::Num(n),
            span: Span::new(start.line, start.col, raw.chars().count() as u32),
        })
    }

    fn lex_ident(&mut self) -> Token {
        let start = self.span_here(0);
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                name.push(c);
                self.bump();
            } else {
                break;
            }
        }
        let len = name.chars().count() as u32;
        Token {
            tok: Tok::Ident(name),
            span: Span::new(start.line, start.col, len),
        }
    }
}

/// Tokenize source into a token stream terminated by `Tok::Eof`.
pub fn lex(src: &str) -> Result<Vec<Token>, ParseError> {
    Lexer::new(src).tokenize()
}
