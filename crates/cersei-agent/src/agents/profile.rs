//! Agent profiles: Markdown files with a YAML frontmatter.
//!
//! ```text
//! ---
//! name: inspecteur
//! description: Locates what must change, with evidence.
//! model: inherit          # inherit | auto | provider/model
//! reasoning: high         # inherit | a reasoning profile id
//! permissions: inherit    # only `inherit`
//! tools: inherit          # only `inherit`
//! isolation: auto         # auto | shared | worktree (worktree: 10.5)
//! background: false       # true: 10.5
//! skills: []
//! ---
//! Instructions (the Markdown body, kept verbatim).
//! ```
//!
//! The frontmatter must open the file (`---` on the first line, an optional
//! BOM before it) and close on a line that is exactly `---`. It is parsed
//! with `serde-saphyr` under a small budget (depth, nodes, aliases, scalar
//! bytes), unknown and duplicate keys are refused, tags are not executed,
//! nothing is interpolated from the environment.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Largest profile file.
pub const MAX_PROFILE_BYTES: usize = 64 * 1024;
/// Largest frontmatter.
pub const MAX_FRONTMATTER_BYTES: usize = 8 * 1024;

/// Where a profile comes from, by decreasing priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileScope {
    Project,
    User,
    BuiltIn,
}

impl ProfileScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
            Self::BuiltIn => "built_in",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSource {
    pub scope: ProfileScope,
    /// The file (`None` for a built-in, which is compiled in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// `builtin:<name>.md` or the path, for messages.
    pub label: String,
    /// First 12 hex digits of the SHA-256 of the file.
    pub revision: String,
}

/// The model a profile asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ModelPref {
    Inherit,
    /// Only through an explicit `[agents] auto_model` rule; otherwise the
    /// inherited model, with a warning.
    Auto,
    /// `provider_id/model_id`.
    Explicit(String),
}

impl ModelPref {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        match s {
            "inherit" => Ok(Self::Inherit),
            "auto" => Ok(Self::Auto),
            _ if is_selection(s) => Ok(Self::Explicit(s.to_string())),
            _ => Err(format!(
                "model `{s}`: expected `inherit`, `auto` or `provider_id/model_id`"
            )),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Inherit => "inherit".into(),
            Self::Auto => "auto".into(),
            Self::Explicit(s) => s.clone(),
        }
    }
}

fn is_selection(s: &str) -> bool {
    match s.split_once('/') {
        Some((p, m)) => {
            !p.is_empty()
                && !m.is_empty()
                && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        }
        None => false,
    }
}

/// The reasoning profile a profile asks for: `inherit`, or an id of the
/// model's own catalogue (free identifiers; there is no closed list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ReasoningPref {
    Inherit,
    Id(String),
}

impl ReasoningPref {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s == "inherit" {
            return Ok(Self::Inherit);
        }
        if !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            Ok(Self::Id(s.to_string()))
        } else {
            Err(format!(
                "reasoning `{s}`: expected `inherit` or a reasoning profile id (letters, digits, `_`, `-`, `.`)"
            ))
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Inherit => "inherit".into(),
            Self::Id(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// Foreground in 10.1: the shared workspace.
    Auto,
    Shared,
    /// A separate worktree (Sprint 10.5).
    Worktree,
}

impl Isolation {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "auto" => Ok(Self::Auto),
            "shared" => Ok(Self::Shared),
            "worktree" => Ok(Self::Worktree),
            other => Err(format!(
                "isolation `{other}`: expected `auto`, `shared` or `worktree`"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Shared => "shared",
            Self::Worktree => "worktree",
        }
    }
}

/// A parsed, validated profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub description: String,
    pub model: ModelPref,
    pub reasoning: ReasoningPref,
    /// Always `inherit` (the only implemented value).
    pub permissions: String,
    /// Always `inherit` (the only implemented value).
    pub tools: String,
    pub isolation: Isolation,
    pub background: bool,
    pub skills: Vec<String>,
    /// The Markdown body, verbatim.
    pub instructions: String,
    pub source: ProfileSource,
    /// Things worth saying about a valid profile (a former key that is
    /// ignored), reported as diagnostics by the registry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    permissions: Option<String>,
    #[serde(default)]
    tools: Option<String>,
    #[serde(default)]
    isolation: Option<String>,
    #[serde(default)]
    background: Option<bool>,
    /// Former turn limit (removed in 0.4.8): accepted, ignored, reported.
    #[serde(default)]
    max_turns: Option<serde_json::Value>,
    #[serde(default)]
    skills: Option<Vec<String>>,
}

/// A profile name: 1–64 of `a-z`, `0-9`, `_`, `-`, starting with a letter
/// or a digit.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

pub fn revision_of(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Split `---` frontmatter / body.
pub fn split_frontmatter(text: &str) -> Result<(&str, &str), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first_end = text.find('\n').unwrap_or(text.len());
    if text[..first_end].trim_end() != "---" {
        return Err("the file must start with a `---` line opening the frontmatter".into());
    }
    let rest = &text[(first_end + 1).min(text.len())..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']).trim_end() == "---" {
            let front = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return Ok((front, body));
        }
        offset += line.len();
    }
    Err("the frontmatter is not closed by a `---` line".into())
}

fn parse_options() -> serde_saphyr::Options {
    let mut budget = serde_saphyr::Budget::default();
    budget.max_depth = 8;
    budget.max_nodes = 256;
    budget.max_events = 2_048;
    budget.max_aliases = 16;
    budget.max_anchors = 16;
    budget.max_documents = 1;
    budget.max_total_scalar_bytes = 16 * 1024;
    let mut o = serde_saphyr::Options::default();
    o.budget = Some(budget);
    o.duplicate_keys = serde_saphyr::DuplicateKeyPolicy::Error;
    o.reject_unsupported_tags = true;
    o.with_snippet = false;
    o
}

/// Parse and validate a profile file.
pub fn parse_profile(text: &str, source: ProfileSource) -> Result<AgentProfile, String> {
    if text.len() > MAX_PROFILE_BYTES {
        return Err(format!(
            "{} bytes: profiles are limited to {MAX_PROFILE_BYTES} bytes",
            text.len()
        ));
    }
    let (front, body) = split_frontmatter(text)?;
    if front.len() > MAX_FRONTMATTER_BYTES {
        return Err(format!(
            "frontmatter of {} bytes: limited to {MAX_FRONTMATTER_BYTES} bytes",
            front.len()
        ));
    }
    let f: Frontmatter = serde_saphyr::from_str_with_options(front, parse_options())
        .map_err(|e| format!("frontmatter: {e}"))?;
    let name = f.name.trim().to_string();
    if !valid_name(&name) {
        return Err(format!(
            "name `{}`: 1–64 characters among a-z, 0-9, `_`, `-`, starting with a letter or digit",
            f.name
        ));
    }
    let description = f.description.trim().to_string();
    if crate::subagent::is_blank(&description) {
        return Err("description is empty".into());
    }
    if description.chars().count() > 1_024 {
        return Err("description is longer than 1024 characters".into());
    }
    let model = ModelPref::parse(f.model.as_deref().unwrap_or("inherit"))?;
    let reasoning = ReasoningPref::parse(f.reasoning.as_deref().unwrap_or("inherit"))?;
    for (key, value) in [("permissions", &f.permissions), ("tools", &f.tools)] {
        if let Some(v) = value {
            if v.trim() != "inherit" {
                return Err(format!(
                    "{key} `{v}`: only `inherit` is implemented (a profile never widens or narrows \
                     the parent's {key}; the approval policy is in bricks.toml)"
                ));
            }
        }
    }
    let isolation = Isolation::parse(f.isolation.as_deref().unwrap_or("auto"))?;
    let mut notes = Vec::new();
    if f.max_turns.is_some() {
        notes.push(
            "max_turns: turn limits were removed in 0.4.8; this key is ignored — remove it".into(),
        );
    }
    let skills = f.skills.unwrap_or_default();
    if skills.len() > 32 {
        return Err("more than 32 skills".into());
    }
    if let Some(s) = skills.iter().find(|s| crate::subagent::is_blank(s)) {
        return Err(format!("skill `{s}` is empty"));
    }
    Ok(AgentProfile {
        name,
        description,
        model,
        reasoning,
        permissions: "inherit".into(),
        tools: "inherit".into(),
        isolation,
        background: f.background.unwrap_or(false),
        skills,
        instructions: body.to_string(),
        source,
        notes,
    })
}

/// The name a broken file was meant to define (its `name:` line, else the
/// file stem), so that an invalid file still shadows lower scopes.
pub fn intended_name(text: &str, stem: &str) -> String {
    if let Ok((front, _)) = split_frontmatter(text) {
        for line in front.lines() {
            if let Some(v) = line.strip_prefix("name:") {
                let v = v.trim().trim_matches(['"', '\'']);
                if valid_name(v) {
                    return v.to_string();
                }
            }
        }
    }
    stem.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> ProfileSource {
        ProfileSource {
            scope: ProfileScope::Project,
            path: None,
            label: "test.md".into(),
            revision: "x".into(),
        }
    }

    const GOOD: &str = "---\nname: inspecteur\ndescription: >\n  Finds things.\nmodel: inherit\nreasoning: high\npermissions: inherit\ntools: inherit\nisolation: auto\nbackground: false\nskills: []\n---\n\n# Inspecteur\n\nBody with --- inside a line.\n";

    #[test]
    fn parses_a_complete_profile_and_keeps_the_body() {
        let p = parse_profile(GOOD, src()).unwrap();
        assert_eq!(p.name, "inspecteur");
        assert_eq!(p.description, "Finds things.");
        assert_eq!(p.reasoning, ReasoningPref::Id("high".into()));
        assert!(p.notes.is_empty());
        assert_eq!(
            p.instructions,
            "\n# Inspecteur\n\nBody with --- inside a line.\n"
        );
        // CRLF and a BOM.
        let crlf = format!("\u{feff}{}", GOOD.replace('\n', "\r\n"));
        assert_eq!(parse_profile(&crlf, src()).unwrap().name, "inspecteur");
    }

    #[test]
    fn a_former_turn_limit_is_ignored_and_reported() {
        let with = |extra: &str| format!("---\nname: a\ndescription: d\n{extra}---\nbody\n");
        for v in ["40", "0", "100000", "\"many\""] {
            let p = parse_profile(&with(&format!("max_turns: {v}\n")), src()).unwrap();
            assert_eq!(p.notes.len(), 1, "{v}");
            assert!(p.notes[0].contains("removed") && p.notes[0].contains("remove it"));
        }
        // Only that key: the others are checked as before.
        let e = parse_profile(&with("max_turns: 40\ncolour: red\n"), src()).unwrap_err();
        assert!(e.contains("unknown field"), "{e}");
        let e = parse_profile(&with("max_turns: 4\nmax_turns: 5\n"), src()).unwrap_err();
        assert!(e.to_lowercase().contains("duplicate"), "{e}");
    }

    #[test]
    fn refuses_what_is_not_implemented_or_inconsistent() {
        let with = |extra: &str| format!("---\nname: a\ndescription: d\n{extra}---\nbody\n");
        for (extra, needle) in [
            ("permissions: allow_all\n", "only `inherit`"),
            ("tools: [Read]\n", "frontmatter"),
            ("model: gpt\n", "provider_id/model_id"),
            ("reasoning: \"very high\"\n", "reasoning"),
            ("isolation: vm\n", "isolation"),
            ("colour: red\n", "unknown field"),
            ("name: b\n", "duplicate"),
            ("skills: [\" \"]\n", "empty"),
        ] {
            let e = parse_profile(&with(extra), src()).unwrap_err();
            assert!(
                e.to_lowercase().contains(&needle.to_lowercase()),
                "{extra}: {e}"
            );
        }
        assert!(parse_profile("no frontmatter", src()).is_err());
        assert!(parse_profile("---\nname: a\n", src())
            .unwrap_err()
            .contains("not closed"));
        assert!(parse_profile("---\nname: A B\ndescription: d\n---\n", src()).is_err());
        assert!(parse_profile("---\nname: a\ndescription: \"  \"\n---\n", src()).is_err());
    }

    #[test]
    fn bounded_yaml() {
        // Alias expansion ("billion laughs") stays within the budget.
        let mut bomb =
            String::from("---\nname: a\ndescription: d\nskills: &a [x, x, x, x, x, x, x, x]\n");
        for i in 0..20 {
            bomb.push_str(&format!("k{i}: &b{i} [*a, *a, *a, *a, *a, *a, *a, *a]\n"));
        }
        bomb.push_str("---\n");
        assert!(parse_profile(&bomb, src()).is_err());
        // Deep nesting.
        let deep = format!(
            "---\nname: a\ndescription: {}x{}\n---\n",
            "[".repeat(50),
            "]".repeat(50)
        );
        assert!(parse_profile(&deep, src()).is_err());
        // Size.
        let big = format!(
            "---\nname: a\ndescription: d\n---\n{}",
            "x".repeat(MAX_PROFILE_BYTES)
        );
        assert!(parse_profile(&big, src()).unwrap_err().contains("limited"));
        // A tag is data at most, never executed; an unknown one is refused.
        assert!(
            parse_profile("---\nname: a\ndescription: !!python/object x\n---\n", src()).is_err()
        );
        // No environment interpolation.
        let p = parse_profile("---\nname: a\ndescription: ${HOME}\n---\n", src()).unwrap();
        assert_eq!(p.description, "${HOME}");
    }

    #[test]
    fn background_and_worktree_are_parsed_not_dropped() {
        let p = parse_profile(
            "---\nname: a\ndescription: d\nbackground: true\nisolation: worktree\n---\n",
            src(),
        )
        .unwrap();
        assert!(p.background);
        assert_eq!(p.isolation, Isolation::Worktree);
    }

    #[test]
    fn intended_name_of_a_broken_file() {
        assert_eq!(
            intended_name("---\nname: x1\ncolour: [\n---\n", "file"),
            "x1"
        );
        assert_eq!(intended_name("garbage", "file"), "file");
    }
}
