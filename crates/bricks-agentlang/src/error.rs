//! Error and source-span types for the AgentTemplate language.
//!
//! Every error carries a [`Span`] (1-based line/col) so failures can be
//! reported back to an LLM in a self-correctable `L:C: message` form.

use std::fmt;

/// A 1-based source location. `len` is the token width in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
    pub len: u32,
}

impl Span {
    pub fn new(line: u32, col: u32, len: u32) -> Self {
        Self { line, col, len }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A parse-time error with the offending location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at {}: {}", self.span, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Categories of runtime failure, for programmatic handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    UnknownBuiltin,
    ArityMismatch,
    TypeMismatch,
    UndefinedVar,
    PermissionDenied,
    ToolError,
    Unsupported,
    StepLimitExceeded,
}

/// An evaluation-time error with the offending location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    pub span: Span,
}

impl RuntimeError {
    pub fn new(kind: RuntimeErrorKind, message: impl Into<String>, span: Span) -> Self {
        Self {
            kind,
            message: message.into(),
            span,
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime error at {}: {}", self.span, self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// Top-level error returned by `run_program`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramError {
    Parse(ParseError),
    Runtime(RuntimeError),
}

impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProgramError::Parse(e) => write!(f, "{e}"),
            ProgramError::Runtime(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProgramError {}

impl From<ParseError> for ProgramError {
    fn from(e: ParseError) -> Self {
        ProgramError::Parse(e)
    }
}

impl From<RuntimeError> for ProgramError {
    fn from(e: RuntimeError) -> Self {
        ProgramError::Runtime(e)
    }
}
