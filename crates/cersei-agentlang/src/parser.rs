//! Recursive-descent parser for the AgentTemplate language.
//!
//! Grammar (informally):
//! ```text
//! program    = { NEWLINE } { statement ( NEWLINE | EOF ) } ;
//! statement  = assignment | expr ;
//! assignment = VAR "=" expr ;
//! expr       = chain ;
//! chain      = primary { "." IDENT "(" [args] ")" } ;   (* chain tails *)
//! primary    = call | VAR | literal | "(" expr ")" ;
//! call       = IDENT { "." IDENT } "(" [args] ")" ;      (* namespaced head *)
//! args       = arg { "," arg } [ "," ] ;
//! arg        = IDENT ":" expr  |  expr ;
//! literal    = STR | NUM | "true" | "false" | "null" | "[" [expr {"," expr}] "]" ;
//! ```

use crate::ast::*;
use crate::error::{ParseError, Span};
use crate::lexer::{Tok, Token, lex};

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        self.toks
            .get(self.pos + n)
            .map(|t| &t.tok)
            .unwrap_or(&Tok::Eof)
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if !matches!(t.tok, Tok::Eof) {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, expected: &Tok) -> Result<Token, ParseError> {
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(expected) {
            Ok(self.bump())
        } else {
            Err(ParseError {
                message: format!("expected {:?}, found {:?}", expected, self.peek()),
                span: self.span(),
            })
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.bump();
        }
    }

    fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Tok::Eof) {
            stmts.push(self.parse_stmt()?);
            // A statement must be terminated by a newline or EOF.
            match self.peek() {
                Tok::Newline => self.skip_newlines(),
                Tok::Eof => break,
                other => {
                    return Err(ParseError {
                        message: format!("expected end of statement, found {other:?}"),
                        span: self.span(),
                    });
                }
            }
        }
        Ok(Program { stmts })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        // assignment: VAR "=" expr  (one-token lookahead on '=')
        if let Tok::Var(name) = self.peek()
            && matches!(self.peek_at(1), Tok::Eq)
        {
            let name = name.clone();
            let span = self.span();
            self.bump(); // var
            self.bump(); // =
            let value = self.parse_expr()?;
            return Ok(Stmt::Assign { name, value, span });
        }
        Ok(Stmt::Expr(self.parse_expr()?))
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let primary = self.parse_primary()?;
        // Only call-chains can carry `.method(...)` tails.
        match primary {
            Expr::Chain(mut chain) => {
                while matches!(self.peek(), Tok::Dot) {
                    self.bump(); // .
                    let span = self.span();
                    let method = match self.bump().tok {
                        Tok::Ident(s) => s,
                        other => {
                            return Err(ParseError {
                                message: format!("expected method name after '.', found {other:?}"),
                                span,
                            });
                        }
                    };
                    let args = self.parse_call_args()?;
                    chain.tail.push(Call {
                        path: vec![method],
                        args,
                        span,
                    });
                }
                chain.span = Span::new(
                    chain.head.span.line,
                    chain.head.span.col,
                    chain.head.span.len,
                );
                Ok(Expr::Chain(chain))
            }
            other => Ok(other),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            Tok::Var(name) => {
                let span = self.span();
                self.bump();
                Ok(Expr::Var { name, span })
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Literal(Literal::Str(s)))
            }
            Tok::Num(n) => {
                self.bump();
                Ok(Expr::Literal(Literal::Num(n)))
            }
            Tok::LBracket => self.parse_array(),
            Tok::LParen => {
                self.bump();
                let e = self.parse_expr()?;
                self.eat(&Tok::RParen)?;
                Ok(e)
            }
            Tok::Ident(first) => {
                // keyword literals
                match first.as_str() {
                    "true" => {
                        self.bump();
                        return Ok(Expr::Literal(Literal::Bool(true)));
                    }
                    "false" => {
                        self.bump();
                        return Ok(Expr::Literal(Literal::Bool(false)));
                    }
                    "null" => {
                        self.bump();
                        return Ok(Expr::Literal(Literal::Null));
                    }
                    _ => {}
                }
                self.parse_call_head()
            }
            other => Err(ParseError {
                message: format!("unexpected token {other:?}"),
                span: self.span(),
            }),
        }
    }

    fn parse_array(&mut self) -> Result<Expr, ParseError> {
        self.eat(&Tok::LBracket)?;
        let mut items = Vec::new();
        if !matches!(self.peek(), Tok::RBracket) {
            loop {
                items.push(self.parse_expr()?);
                match self.peek() {
                    Tok::Comma => {
                        self.bump();
                        if matches!(self.peek(), Tok::RBracket) {
                            break; // trailing comma
                        }
                    }
                    _ => break,
                }
            }
        }
        self.eat(&Tok::RBracket)?;
        Ok(Expr::Literal(Literal::Array(items)))
    }

    /// Parse a namespaced call head: `IDENT { "." IDENT } "(" args ")"`.
    /// Dotted idents are collected into the path until a `(` is reached.
    fn parse_call_head(&mut self) -> Result<Expr, ParseError> {
        let span = self.span();
        let mut path = Vec::new();
        match self.bump().tok {
            Tok::Ident(s) => path.push(s),
            other => {
                return Err(ParseError {
                    message: format!("expected identifier, found {other:?}"),
                    span,
                });
            }
        }
        // Extend path while the next two tokens are `.` IDENT (namespace), but
        // stop before the final segment's `(`.
        while matches!(self.peek(), Tok::Dot) && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.bump(); // .
            if let Tok::Ident(s) = self.bump().tok {
                path.push(s);
            }
        }
        let args = self.parse_call_args()?;
        Ok(Expr::Chain(Chain {
            head: Call { path, args, span },
            tail: Vec::new(),
            span,
        }))
    }

    fn parse_call_args(&mut self) -> Result<Vec<Arg>, ParseError> {
        self.eat(&Tok::LParen)?;
        let mut args = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                args.push(self.parse_arg()?);
                match self.peek() {
                    Tok::Comma => {
                        self.bump();
                        if matches!(self.peek(), Tok::RParen) {
                            break; // trailing comma
                        }
                    }
                    _ => break,
                }
            }
        }
        self.eat(&Tok::RParen)?;
        Ok(args)
    }

    fn parse_arg(&mut self) -> Result<Arg, ParseError> {
        // named arg: IDENT ":" expr
        if let Tok::Ident(name) = self.peek().clone()
            && matches!(self.peek_at(1), Tok::Colon)
        {
            let span = self.span();
            self.bump(); // ident
            self.bump(); // :
            let value = self.parse_expr()?;
            return Ok(Arg::Named { name, value, span });
        }
        Ok(Arg::Positional(self.parse_expr()?))
    }
}

/// Parse source text into a [`Program`].
pub fn parse(src: &str) -> Result<Program, ParseError> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    p.parse_program()
}
