//! The response side of the public contract.
//!
//! Three things are kept apart on every item:
//! * `certainty`: how the relation was established (a language server
//!   confirmed it, the syntax tree shows it, or only the text matches);
//! * `freshness`: whether the item still matches the current content;
//! * `score`: a ranking heuristic for this intent, **not** a probability.

use crate::position::LineCol;
use crate::view::Revision;
use serde::{Deserialize, Serialize};

/// A range in one document version: half-open byte offsets, plus 0-based
/// line / byte-column for each end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start: LineCol,
    pub end: LineCol,
}

impl SourceRange {
    /// `line:col`, 1-based, column in characters (display convention).
    pub fn display_start(&self, line_text: &str) -> String {
        let chars = line_text
            .get(..self.start.col as usize)
            .map(|s| s.chars().count())
            .unwrap_or(self.start.col as usize);
        format!("{}:{}", self.start.line + 1, chars + 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Definition,
    Declaration,
    Reference,
    /// The text matches; nothing says it is the symbol.
    TextMention,
    /// A definition whose name matches the query (homonyms possible).
    Candidate,
    Documentation,
    Diagnostic,
    /// Context the requester asked for (enclosing scope, hover).
    Context,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::Declaration => "declaration",
            Self::Reference => "reference",
            Self::TextMention => "text_mention",
            Self::Candidate => "candidate",
            Self::Documentation => "documentation",
            Self::Diagnostic => "diagnostic",
            Self::Context => "context",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    /// Only the text matches.
    Textual,
    /// The syntax tree shows a definition with that name; which symbol a
    /// use refers to is not established.
    Syntactic,
    /// A language server answered for this exact document version.
    Confirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Freshness {
    /// Established on the content the response refers to, still current
    /// when the response was built.
    Current,
    /// The file changed during the query, or the backend answered for
    /// another version.
    Stale { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Lexical,
    Syntax,
    Lsp,
    Cache,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Provenance {
    pub backend: Backend,
    /// The method that established the relation (`regex match`,
    /// `tree-sitter function_item`, `textDocument/references`...).
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRef {
    pub name: String,
    pub kind: String,
}

/// How a snippet relates to the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetKind {
    /// Verbatim source of `range`.
    Full,
    /// Verbatim pieces with explicit gaps (`parts`).
    Excerpt,
    /// Only the signature line(s).
    Signature,
}

/// One verbatim piece of source with its original range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetPart {
    pub range: SourceRange,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub kind: SnippetKind,
    /// The scope the snippet was taken from (`function_item`, `lines`...).
    pub scope: String,
    /// The scope's full range (larger than the parts when excerpted).
    pub scope_range: SourceRange,
    pub parts: Vec<SnippetPart>,
    /// Lines of the scope not included.
    pub omitted_lines: u32,
    /// The syntax tree had errors around this place, or the language is
    /// not supported (`lines` fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeItem {
    /// `path#start-end@hash`: reusable as a target.
    pub id: String,
    /// Relative to the workspace root.
    pub path: String,
    pub uri: String,
    pub range: SourceRange,
    pub revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<SymbolRef>,
    pub relation: Relation,
    pub certainty: Certainty,
    pub freshness: Freshness,
    /// Ranking heuristic for this intent; not a probability.
    pub score: f32,
    /// The matched line, verbatim.
    pub line_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<Snippet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    pub provenance: Vec<Provenance>,
}

pub fn item_id(path: &str, range: &SourceRange, revision: &Revision) -> String {
    format!(
        "{path}#{}-{}@{}",
        range.start_byte, range.end_byte, revision.hash
    )
}

/// Parse `path#start-end@hash`.
pub fn parse_item_id(id: &str) -> Option<(String, usize, usize, String)> {
    let (path, rest) = id.rsplit_once('#')?;
    let (span, hash) = rest.split_once('@')?;
    let (s, e) = span.split_once('-')?;
    Some((
        path.to_string(),
        s.parse().ok()?,
        e.parse().ok()?,
        hash.to_string(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ResultStatus {
    /// Every planned step ran to completion. Zero items then means there
    /// is nothing to find in the scope.
    Complete,
    /// Some steps did not run or stopped at a limit: absence of an item
    /// proves nothing (see `omissions` and `plan.fallbacks`).
    Partial,
    /// Several symbols match; nothing was chosen silently.
    Ambiguous,
    /// The requested capability is not available (see `plan`).
    Unavailable {
        reason: String,
    },
    Cancelled,
    Error {
        message: String,
    },
}

impl ResultStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Ambiguous => "ambiguous",
            Self::Unavailable { .. } => "unavailable",
            Self::Cancelled => "cancelled",
            Self::Error { .. } => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    Ok,
    /// Ran, but stopped at a limit.
    Partial,
    /// Not available (no server, capability not supported...).
    Unavailable,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub backend: Backend,
    pub action: String,
    pub outcome: StepOutcome,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReport {
    /// The intent actually executed (after `auto` routing).
    pub intent: String,
    pub strategy: String,
    /// Why this route (deterministic rules).
    pub reason: String,
    pub steps: Vec<Step>,
    /// What was used instead of what failed, and what is missing.
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Omission {
    /// A limit stopped the work: results may be missing.
    LimitReached {
        limit: String,
        value: u64,
    },
    Deadline {
        ms: u64,
    },
    /// Files skipped (too large, binary, not UTF-8, unreadable).
    FilesSkipped {
        reason: String,
        count: usize,
        examples: Vec<String>,
    },
    /// Items found but left out of the response (budget, max results).
    ItemsOmitted {
        reason: String,
        count: usize,
    },
    /// Paths outside the requester's scope were requested.
    OutOfScope {
        paths: Vec<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetReport {
    pub limit_tokens: u64,
    /// Central estimate of what the response contains.
    pub used_tokens: u64,
    /// Conservative bound the selection kept under `limit_tokens`.
    pub used_upper_tokens: u64,
    /// Always an estimate: the provider's tokenizer is not known here.
    pub method: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ambiguity {
    pub name: String,
    pub candidates: usize,
    pub hint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryMetrics {
    pub elapsed_ms: u64,
    pub files_listed: usize,
    pub files_searched: usize,
    pub bytes_searched: u64,
    pub files_parsed: usize,
    pub tree_cache_hits: usize,
    pub result_cache_hit: bool,
    /// The response came from an identical query already running.
    pub shared_inflight: bool,
    pub lsp_requests: usize,
    pub lsp_ms: u64,
    /// Items found before selection.
    pub candidates: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeResponse {
    pub status: ResultStatus,
    pub plan: PlanReport,
    pub items: Vec<CodeItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambiguity: Option<Ambiguity>,
    pub omissions: Vec<Omission>,
    pub budget: BudgetReport,
    pub metrics: QueryMetrics,
    /// How to see more (narrower scope, a target id, `deep`...).
    pub continuation: Vec<String>,
    /// Workspace generation the response was computed at.
    pub generation: u64,
    /// Diagnostics state, for `diagnostics`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<DiagnosticsReport>,
}

impl CodeResponse {
    pub fn empty(status: ResultStatus) -> Self {
        Self {
            status,
            plan: PlanReport::default(),
            items: Vec::new(),
            ambiguity: None,
            omissions: Vec::new(),
            budget: BudgetReport::default(),
            metrics: QueryMetrics::default(),
            continuation: Vec::new(),
            generation: 0,
            diagnostics: None,
        }
    }
}

/// What is known about a file's diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum DiagnosticsState {
    /// The server analyzed exactly this version (versioned publish, pulled
    /// report, or a publish received after this version was sent).
    Analyzed { version: i64 },
    /// Diagnostics exist, but for an earlier version.
    Outdated { version: Option<i64> },
    /// The server has not reported on this version yet.
    Pending,
    /// No server: absence of diagnostics proves nothing.
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    pub path: String,
    pub state: DiagnosticsState,
    pub method: String,
    pub errors: usize,
    pub warnings: usize,
    /// Compared with an earlier analyzed version of the file, when one is
    /// known: diagnostics that were not there before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduced: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preexisting: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_revision: Option<String>,
}
