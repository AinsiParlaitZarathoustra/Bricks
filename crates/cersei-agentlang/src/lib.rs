//! cersei-agentlang: the AgentTemplate DSL — a small functional language that
//! LLMs and agents author and that executes on top of the Cersei runtime.
//!
//! Pipeline: [`parse`] → [`Program`] → [`run_program`] over an [`EvalCtx`].
//! Builtins (`io.*`, `net.*`, `agent.*`, `kv.*`) dispatch by tool name through
//! the [`ToolDispatch`] trait, so the language is decoupled from the concrete
//! tool set. See [`AGENTLANG_SPEC`] for the LLM-facing spec.

pub mod ast;
pub mod builtins;
pub mod error;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod registry;
pub mod run_tool;
pub mod spec;

pub use ast::{Arg, Call, Chain, Expr, Literal, Program, Stmt};
pub use error::{ParseError, ProgramError, RuntimeError, RuntimeErrorKind, Span};
pub use interp::{EvalCtx, Limits, run_program};
pub use parser::parse;
pub use registry::{DispatchHandle, ToolDispatch, VecToolDispatch};
pub use run_tool::RunAgentTemplateTool;
pub use spec::{AGENTLANG_SPEC, language_spec};
