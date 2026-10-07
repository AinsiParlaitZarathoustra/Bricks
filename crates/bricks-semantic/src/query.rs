//! The request side of the public contract.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

use crate::view::ViewHandle;

/// How `text` matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    #[default]
    Literal,
    Regex,
}

/// What the requester wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// Deterministic routing (see `planner`): never an LLM call.
    #[default]
    Auto,
    TextSearch,
    FindSymbol,
    Definition,
    References,
    Understand,
    Diagnostics,
}

impl Intent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::TextSearch => "text_search",
            Self::FindSymbol => "find_symbol",
            Self::Definition => "definition",
            Self::References => "references",
            Self::Understand => "understand",
            Self::Diagnostics => "diagnostics",
        }
    }
}

/// A precise target. `Position` uses the display convention (1-based line,
/// 1-based column counted in characters); the engine converts it once, at
/// the frontier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Target {
    Position {
        path: String,
        line: u32,
        column: u32,
    },
    /// Byte offset in the document's current text.
    Offset { path: String, offset: usize },
    /// An item id from an earlier response (`path#start-end@hash`).
    Item { id: String },
    /// A whole file (diagnostics, outline).
    File { path: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopeFilter {
    /// Files or folders (relative to the workspace) to search; empty: the
    /// requester's whole scope.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Extensions without the dot (`rs`, `ts`).
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Gitignore-style globs to exclude, on top of the ignore files.
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Requested limits. Every value is capped by the configuration: a request
/// can lower a limit, never lift it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueryLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_matches: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
}

/// How much code around each result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ContextPolicy {
    None,
    /// The matching line and `n` lines on each side.
    Lines {
        n: u32,
    },
    /// The innermost enclosing block.
    Block,
    /// The enclosing function (or method).
    Function,
    /// The enclosing type (struct, class, impl, trait...).
    Type,
    /// Function for code, lines elsewhere.
    #[default]
    Auto,
}

/// Amount of context; never permissions, time or memory ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    Compact,
    #[default]
    Normal,
    Deep,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CodeQuery {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub mode: MatchMode,
    #[serde(default)]
    pub case_insensitive: bool,
    #[serde(default)]
    pub intent: Intent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    #[serde(default)]
    pub scope: ScopeFilter,
    #[serde(default)]
    pub limits: QueryLimits,
    #[serde(default)]
    pub context: ContextPolicy,
    #[serde(default)]
    pub detail: Detail,
}

impl CodeQuery {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn intent(mut self, intent: Intent) -> Self {
        self.intent = intent;
        self
    }

    pub fn target(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }

    pub fn detail(mut self, detail: Detail) -> Self {
        self.detail = detail;
        self
    }

    pub fn context(mut self, context: ContextPolicy) -> Self {
        self.context = context;
        self
    }
}

/// Who asks, with what access, and how to stop.
#[derive(Clone)]
pub struct Requester {
    /// Free label for metrics and logs (`agent`, `tui`, ...).
    pub client: String,
    /// Absolute folders this requester may read. Results outside are never
    /// returned, cached or not. Must lie inside the engine's root.
    pub scope: Vec<PathBuf>,
    /// The view to read (shared by default).
    pub view: Option<ViewHandle>,
    pub cancel: CancellationToken,
    /// Whether this request may start a language server (a process).
    pub allow_lsp_start: bool,
    /// The file the requester is working in, when it really is known.
    pub active_file: Option<PathBuf>,
}

impl Requester {
    pub fn new(client: impl Into<String>, scope: impl Into<PathBuf>) -> Self {
        Self {
            client: client.into(),
            scope: vec![scope.into()],
            view: None,
            cancel: CancellationToken::new(),
            allow_lsp_start: true,
            active_file: None,
        }
    }

    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn with_view(mut self, view: ViewHandle) -> Self {
        self.view = Some(view);
        self
    }

    pub fn without_lsp_start(mut self) -> Self {
        self.allow_lsp_start = false;
        self
    }
}
