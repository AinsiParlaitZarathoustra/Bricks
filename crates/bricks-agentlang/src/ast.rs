//! Abstract syntax tree for the AgentTemplate language.
//!
//! A program is a sequence of statements; each is either an assignment
//! (`$x = expr`) or a bare expression. The only compound expression is the
//! call-chain (`io.read($f).write($g)`).

use crate::error::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `$name = value`
    Assign {
        name: String,
        value: Expr,
        span: Span,
    },
    /// A bare expression evaluated for its side effects / final value.
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Literal),
    /// `$name`
    Var {
        name: String,
        span: Span,
    },
    /// A call, optionally followed by chained `.method(...)` tails.
    Chain(Chain),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chain {
    pub head: Call,
    /// Chained tails, e.g. the `.write(...)` and `.delete()` in
    /// `io.read().write().delete()`. Each tail has a single-segment path.
    pub tail: Vec<Call>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// Dotted namespace path, e.g. `["io","read"]` or `["agent","tools","call"]`.
    /// A chain tail has a single segment, e.g. `["write"]`.
    pub path: Vec<String>,
    pub args: Vec<Arg>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Positional(Expr),
    Named {
        name: String,
        value: Expr,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Str(String),
    Num(f64),
    Bool(bool),
    Array(Vec<Expr>),
    Null,
}
