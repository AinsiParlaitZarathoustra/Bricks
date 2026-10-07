//! bricks-semantic: one code understanding engine per workspace.
//!
//! [`SemanticEngine::query`] answers a typed [`CodeQuery`] with a
//! [`CodeResponse`] whose items are sourced (path, exact range, content
//! revision, provenance) and bounded (limits, context budget). It chooses
//! between lexical search (ripgrep's libraries), syntax (Tree-sitter) and
//! language servers (optional, started on demand, shared), and says which
//! it used, what fell back and what is missing.
//!
//! Conventions: byte offsets, half-open ranges, 0-based lines and byte
//! columns inside the engine; 1-based line and character column at the
//! display frontier only ([`render`]).

pub mod compiler;
pub mod config;
pub mod context;
pub mod engine;
pub mod lexical;
pub mod lsp;
pub mod planner;
pub mod position;
pub mod query;
pub mod rank;
pub mod registry;
pub mod render;
pub mod result;
pub mod syntax;
pub mod view;

pub use config::{LspSettings, SemanticConfig};
pub use engine::{EngineStats, SemanticEngine};
pub use query::{
    CodeQuery, ContextPolicy, Detail, Intent, MatchMode, QueryLimits, Requester, ScopeFilter,
    Target,
};
pub use registry::SemanticRegistry;
pub use result::*;
pub use tokio_util::sync::CancellationToken;
pub use view::ViewHandle;
