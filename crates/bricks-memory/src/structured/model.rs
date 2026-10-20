//! The records of the structured memory and their identities.
//!
//! * An **episode** is a piece of source content (a user turn, an assistant
//!   reply, a tool output, a legacy memory): immutable, sourced, timestamped.
//! * An **entity** is something facts are about, identified *within a
//!   space* (a user, a project, a named memory space): two projects that
//!   both have a module called `auth` have two different `auth` entities.
//! * A **fact** is a statement `subject · predicate · value` in a space,
//!   with its evidence (episodes and verbatim quotes), its origin, its
//!   validity interval and its knowledge history.
//!
//! Identifiers are derived from content (SHA-256, hex, truncated), so
//! recording the same episode or extracting the same fact twice finds the
//! existing record instead of duplicating it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Milliseconds since the Unix epoch, UTC.
pub type Millis = i64;

/// A stable identifier from the given parts.
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    let digest = h.finalize();
    let hex: String = digest.iter().take(10).map(|b| format!("{b:02x}")).collect();
    format!("{prefix}_{hex}")
}

/// Lower-case, accents folded, inner whitespace collapsed: the form names
/// and aliases are compared in.
pub fn normalize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().chars().flat_map(char::to_lowercase) {
        let c = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            '’' => '\'',
            c => c,
        };
        if c.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// `snake_case` predicate: lower-case ASCII letters, digits and `_`.
pub fn normalize_predicate(s: &str) -> Option<String> {
    let mut out = String::new();
    for c in normalize_name(s).chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    (!out.is_empty() && out.len() <= 64).then_some(out)
}

/// Who produced the content a fact was extracted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Stated by the user.
    UserStatement,
    /// Proposed by the assistant: kept as a proposal until the user confirms.
    AssistantProposal,
    /// Observed in a tool's output.
    ToolOutput,
    /// Generated summary: never a confirmed fact by itself.
    Summary,
    /// Imported from the former `:Memory` nodes.
    Legacy,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserStatement => "user_statement",
            Self::AssistantProposal => "assistant_proposal",
            Self::ToolOutput => "tool_output",
            Self::Summary => "summary",
            Self::Legacy => "legacy",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "user_statement" => Self::UserStatement,
            "assistant_proposal" => Self::AssistantProposal,
            "tool_output" => Self::ToolOutput,
            "summary" => Self::Summary,
            "legacy" => Self::Legacy,
            _ => return None,
        })
    }

    /// The origin of facts extracted from an episode with this role.
    pub fn from_role(role: &str) -> Self {
        match role {
            "user" => Self::UserStatement,
            "assistant" => Self::AssistantProposal,
            "tool" => Self::ToolOutput,
            "summary" => Self::Summary,
            _ => Self::Legacy,
        }
    }

    /// May a fact of this origin replace or contest an existing one?
    pub fn is_authoritative(self) -> bool {
        matches!(self, Self::UserStatement | Self::ToolOutput)
    }
}

/// Current standing of a fact version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactStatus {
    /// Sourced and not contradicted.
    Active,
    /// From the assistant or a summary, not confirmed by the user.
    Proposed,
    /// Replaced by an explicit change (see `SUPERSEDES`).
    Superseded,
    /// In conflict with another assertion; no side was chosen.
    Contested,
    /// Withdrawn explicitly.
    Retracted,
    /// Every supporting episode was deleted.
    Unsupported,
}

impl FactStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Proposed => "proposed",
            Self::Superseded => "superseded",
            Self::Contested => "contested",
            Self::Retracted => "retracted",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "active" => Self::Active,
            "proposed" => Self::Proposed,
            "superseded" => Self::Superseded,
            "contested" => Self::Contested,
            "retracted" => Self::Retracted,
            "unsupported" => Self::Unsupported,
            _ => return None,
        })
    }
}

/// When a fact is true (validity). `None` bounds are *unknown*: they are
/// never filled with the time Bricks recorded something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Validity {
    /// Start, when stated.
    pub from: Option<Millis>,
    /// End (exclusive), when stated by the fact itself.
    pub until: Option<Millis>,
    /// When the fact was asserted (time of its source episode, else of its
    /// recording): it was true then, whatever its unknown start.
    pub asserted_at: Millis,
}

/// How a fact was replaced, as known by the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Supersession {
    /// The replacing fact's stated start: a known end of this one.
    pub until: Option<Millis>,
    /// When the change was asserted, when its date is unknown: the change
    /// happened at the latest then. An upper bound, not the change date.
    pub ended_by: Option<Millis>,
}

/// How a fact relates to a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Validness {
    /// Valid at that time by its known bounds.
    Valid,
    /// May have been valid: a bound needed to decide is unknown.
    Uncertain,
    /// Not valid at that time.
    Invalid,
}

impl Validity {
    /// Validity at `t`, given what is known of a replacement (if any). The
    /// start (when known) must be reached and every known end not reached.
    pub fn at(&self, t: Millis, replaced: Option<&Supersession>) -> Validness {
        if self.from.is_some_and(|f| t < f) {
            return Validness::Invalid;
        }
        if self.until.is_some_and(|u| t >= u) {
            return Validness::Invalid;
        }
        if let Some(r) = replaced {
            if r.until.is_some_and(|u| t >= u) || r.ended_by.is_some_and(|e| t >= e) {
                return Validness::Invalid;
            }
            // Replaced at an unknown moment before `ended_by`: certain only
            // up to the time the fact was asserted.
            if r.ended_by.is_some() && t > self.asserted_at {
                return Validness::Uncertain;
            }
        }
        if self.from.is_none() && t < self.asserted_at {
            return Validness::Uncertain;
        }
        Validness::Valid
    }
}

/// What the system knew, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Knowledge {
    /// When this version was recorded.
    pub recorded_at: Millis,
    /// When it was superseded (system time), if it was.
    pub superseded_at: Option<Millis>,
    /// When it was retracted (system time), if it was.
    pub retracted_at: Option<Millis>,
}

impl Fact {
    /// Validity at `t` as known at `k`: a replacement recorded after `k`
    /// was not known then and does not bound the fact.
    pub fn validity_at(&self, t: Millis, k: Millis) -> Validness {
        let replaced = self
            .knowledge
            .superseded_at
            .filter(|s| *s <= k)
            .map(|_| &self.supersession);
        self.validity.at(t, replaced)
    }
}

impl Knowledge {
    /// Was this version known, and not yet replaced or withdrawn, at `k`?
    pub fn current_at(&self, k: Millis) -> bool {
        self.recorded_at <= k
            && self.superseded_at.is_none_or(|s| s > k)
            && self.retracted_at.is_none_or(|r| r > k)
    }
}

/// A fact as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub space: String,
    pub subject_id: String,
    pub subject_name: String,
    pub predicate: String,
    /// Display form of the value.
    pub value: String,
    /// Normalised value (comparisons).
    pub value_norm: String,
    /// The value is itself an entity.
    pub object_id: Option<String>,
    pub negated: bool,
    pub origin: Origin,
    pub status: FactStatus,
    /// Extraction confidence in [0, 1], as given by the extractor. Not a
    /// probability of truth and not used to rank recall.
    pub confidence: Option<f64>,
    pub validity: Validity,
    /// Set when superseded (see [`Knowledge::superseded_at`]).
    pub supersession: Supersession,
    pub knowledge: Knowledge,
    /// Statement in words (`Bricks · framework: Actix`).
    pub statement: String,
}

/// Evidence for a fact: an episode and the verbatim quote it contains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub episode_id: String,
    pub quote: String,
    pub session_id: Option<String>,
    pub role: String,
    pub occurred_at: Option<Millis>,
}

/// An episode as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub id: String,
    pub space: String,
    pub session_id: Option<String>,
    pub role: String,
    pub author: Option<String>,
    pub content: String,
    /// When the source content was produced, if known.
    pub occurred_at: Option<Millis>,
    pub recorded_at: Millis,
    /// Where it came from (`session:<id>#<n>`, a file…).
    pub source_ref: Option<String>,
}

/// Input of [`crate::structured::StructuredMemory::record`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeInput {
    pub space: String,
    pub session_id: Option<String>,
    /// `user`, `assistant`, `tool`, `summary`.
    pub role: String,
    pub author: Option<String>,
    pub content: String,
    pub occurred_at: Option<Millis>,
    pub source_ref: Option<String>,
}

impl EpisodeInput {
    /// The identity of the episode: same space, session, role, time, source
    /// and content → same episode.
    pub fn id(&self) -> String {
        let at = self.occurred_at.map(|t| t.to_string()).unwrap_or_default();
        stable_id(
            "ep",
            &[
                &self.space,
                self.session_id.as_deref().unwrap_or(""),
                &self.role,
                &at,
                self.source_ref.as_deref().unwrap_or(""),
                &self.content,
            ],
        )
    }
}

/// Spaces: `user:<id>`, `project:<name>`, `space:<name>`.
pub fn valid_space(s: &str) -> bool {
    let Some((kind, name)) = s.split_once(':') else {
        return false;
    };
    matches!(kind, "user" | "project" | "space")
        && !name.is_empty()
        && name.len() <= 200
        && !name.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_are_stable_and_separated() {
        let a = stable_id("f", &["ab", "c"]);
        assert_eq!(a, stable_id("f", &["ab", "c"]));
        assert_ne!(a, stable_id("f", &["a", "bc"]), "parts are length-prefixed");
        assert!(a.starts_with("f_") && a.len() == 22);
    }

    #[test]
    fn names_and_predicates_normalise() {
        assert_eq!(normalize_name("  Élodie   Martin "), "elodie martin");
        assert_eq!(
            normalize_predicate("Uses Framework").as_deref(),
            Some("uses_framework")
        );
        assert_eq!(normalize_predicate("lives-in").as_deref(), Some("lives_in"));
        assert_eq!(normalize_predicate("!!!"), None);
    }

    #[test]
    fn validity_checks_both_bounds() {
        let v = Validity {
            from: Some(100),
            until: Some(200),
            asserted_at: 120,
        };
        assert_eq!(
            v.at(50, None),
            Validness::Invalid,
            "a future fact is not valid yet"
        );
        assert_eq!(v.at(150, None), Validness::Valid);
        assert_eq!(v.at(200, None), Validness::Invalid);
        // Unknown start: certain from the assertion on, uncertain before.
        let open = Validity {
            from: None,
            until: None,
            asserted_at: 100,
        };
        assert_eq!(open.at(150, None), Validness::Valid);
        assert_eq!(open.at(50, None), Validness::Uncertain);
        // Replaced with a known date.
        let dated = Supersession {
            until: Some(300),
            ended_by: None,
        };
        assert_eq!(open.at(250, Some(&dated)), Validness::Valid);
        assert_eq!(open.at(300, Some(&dated)), Validness::Invalid);
        // Replaced at an unknown moment, stated at 400.
        let undated = Supersession {
            until: None,
            ended_by: Some(400),
        };
        assert_eq!(open.at(100, Some(&undated)), Validness::Valid);
        assert_eq!(open.at(250, Some(&undated)), Validness::Uncertain);
        assert_eq!(open.at(400, Some(&undated)), Validness::Invalid);
    }

    #[test]
    fn knowledge_reconstructs_the_past() {
        let k = Knowledge {
            recorded_at: 10,
            superseded_at: Some(20),
            retracted_at: None,
        };
        assert!(!k.current_at(5));
        assert!(k.current_at(15));
        assert!(!k.current_at(25));
    }

    #[test]
    fn spaces_are_explicit() {
        assert!(valid_space("project:bricks"));
        assert!(valid_space("user:42"));
        assert!(!valid_space("bricks"));
        assert!(!valid_space("team:x"));
    }
}
