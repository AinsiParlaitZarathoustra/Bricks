//! `SKILL.md`: YAML frontmatter, then Markdown instructions.
//!
//! Follows the [Agent Skills specification](https://agentskills.io/specification)
//! and the normalisation of its reference validator (`skills-ref`):
//!
//! * `name` (required): NFKC-normalised, 1–64 characters, lowercase, letters,
//!   digits and `-` only (Unicode letters allowed), no leading, trailing or
//!   doubled `-`, equal to the (NFKC-normalised) name of its directory;
//! * `description` (required): 1–1024 characters;
//! * `license`, `compatibility` (≤ 500 characters), `metadata` (string keys
//!   to string values), `allowed-tools` (experimental) are optional.
//!
//! Bricks' own limits, distinct from the standard: a `SKILL.md` of at most
//! [`MAX_SKILL_BYTES`] bytes, a frontmatter of at most
//! [`MAX_FRONTMATTER_BYTES`] bytes parsed under a budget (depth, nodes,
//! aliases, scalar bytes), duplicate keys and YAML tags refused. Nothing is
//! repaired: a document that does not parse is reported, not guessed at.
//!
//! Fields outside the standard are sorted in two groups. Informational ones
//! (`version`, `argument-hint`, `tags`…) produce a note and are ignored.
//! Claude Code directives that would change how the skill runs, is
//! activated or what it may do (`context`, `agent`, `model`, `effort`,
//! `hooks`, `paths`, `shell`, `disable-model-invocation: true`,
//! `user-invocable: false`) are not implemented by Bricks: such a document
//! is *incompatible* — it can be inspected, never activated.

use serde_json::Value;
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;

/// Bricks' bound on a `SKILL.md` file (not a limit of the standard).
pub const MAX_SKILL_BYTES: usize = 100 * 1024;
/// Bricks' bound on the frontmatter.
pub const MAX_FRONTMATTER_BYTES: usize = 32 * 1024;
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_DESCRIPTION_CHARS: usize = 1024;
pub const MAX_COMPATIBILITY_CHARS: usize = 500;

/// The standard's fields.
const STANDARD_FIELDS: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];

/// Directives (Claude Code extensions) that change execution, activation or
/// permissions. Bricks does not implement them.
const BEHAVIOURAL_FIELDS: &[&str] = &[
    "context",
    "agent",
    "model",
    "effort",
    "hooks",
    "paths",
    "shell",
    "disable-model-invocation",
    "user-invocable",
];

/// `allowed-tools`, kept as written. Bricks applies neither the permissions
/// nor the restrictions it describes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AllowedTools {
    /// The value as written (a list is joined with spaces).
    pub raw: String,
    /// Its entries (space-separated in the standard).
    pub tools: Vec<String>,
}

/// A parsed, valid `SKILL.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDocument {
    /// NFKC-normalised name.
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub allowed_tools: Option<AllowedTools>,
    /// The Markdown instructions (line endings normalised to `\n`).
    pub body: String,
    /// Informational notes (fields ignored, text not substituted…).
    pub notes: Vec<String>,
    /// Directives Bricks does not implement: non-empty means incompatible.
    pub unsupported: Vec<String>,
}

impl SkillDocument {
    pub fn is_compatible(&self) -> bool {
        self.unsupported.is_empty()
    }
}

/// Why a document was refused. The message names the cause; it never
/// repeats the body or a field's value beyond the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentError(pub String);

impl std::fmt::Display for DocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DocumentError {}

fn err(msg: impl Into<String>) -> DocumentError {
    DocumentError(msg.into())
}

/// NFKC normalisation, as the reference validator applies it.
pub fn normalize_name(s: &str) -> String {
    s.trim().nfkc().collect()
}

/// Problems of a skill name (empty when valid), per the reference
/// validator. `dir` is the directory's name, when there is one.
pub fn name_problems(name: &str, dir: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let name = normalize_name(name);
    if name.is_empty() {
        out.push("`name` is empty".into());
        return out;
    }
    let n = name.chars().count();
    if n > MAX_NAME_CHARS {
        out.push(format!(
            "`name` has {n} characters (at most {MAX_NAME_CHARS})"
        ));
    }
    if name.to_lowercase() != name {
        out.push("`name` must be lowercase".into());
    }
    if name.starts_with('-') || name.ends_with('-') {
        out.push("`name` cannot start or end with `-`".into());
    }
    if name.contains("--") {
        out.push("`name` cannot contain `--`".into());
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-') {
        out.push("`name` may only contain letters, digits and `-`".into());
    }
    if let Some(dir) = dir
        && normalize_name(dir) != name
    {
        out.push(format!(
            "`name` `{name}` does not match its directory `{dir}`"
        ));
    }
    out
}

/// Split the frontmatter from the body. The delimiters are whole `---`
/// lines (LF or CRLF); a BOM is ignored.
pub fn split_frontmatter(text: &str) -> Result<(&str, &str), DocumentError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first_end = text.find('\n').unwrap_or(text.len());
    if text[..first_end].trim_end_matches('\r') != "---" {
        return Err(err("no frontmatter: the file must start with a `---` line"));
    }
    let rest = &text[(first_end + 1).min(text.len())..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            return Ok((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err(err("the frontmatter is not closed by a `---` line"))
}

fn yaml_options() -> serde_saphyr::Options {
    let mut budget = serde_saphyr::Budget::default();
    budget.max_depth = 8;
    budget.max_nodes = 512;
    budget.max_events = 4_096;
    budget.max_aliases = 16;
    budget.max_anchors = 16;
    budget.max_documents = 1;
    budget.max_total_scalar_bytes = MAX_FRONTMATTER_BYTES;
    let mut o = serde_saphyr::Options::default();
    o.budget = Some(budget);
    o.duplicate_keys = serde_saphyr::DuplicateKeyPolicy::Error;
    o.reject_unsupported_tags = true;
    o.with_snippet = false;
    o
}

fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Parse and validate a `SKILL.md`. `dir` is the name of its directory
/// (`None` for a built-in skill).
pub fn parse_document(text: &str, dir: Option<&str>) -> Result<SkillDocument, DocumentError> {
    if text.len() > MAX_SKILL_BYTES {
        return Err(err(format!(
            "{} bytes: Bricks reads SKILL.md files of at most {MAX_SKILL_BYTES} bytes",
            text.len()
        )));
    }
    let (front, body) = split_frontmatter(text)?;
    if front.len() > MAX_FRONTMATTER_BYTES {
        return Err(err(format!(
            "frontmatter of {} bytes (at most {MAX_FRONTMATTER_BYTES})",
            front.len()
        )));
    }
    let value: Value = serde_saphyr::from_str_with_options(front, yaml_options())
        .map_err(|e| err(format!("frontmatter is not valid YAML: {e}")))?;
    let Value::Object(map) = value else {
        return Err(err("the frontmatter must be a YAML mapping"));
    };

    let mut problems = Vec::new();
    let mut notes = Vec::new();
    let mut unsupported = Vec::new();

    let text_field = |key: &str, problems: &mut Vec<String>| -> Option<String> {
        match map.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.trim().to_string()),
            Some(_) => {
                problems.push(format!("`{key}` must be a string"));
                None
            }
        }
    };

    let name = match text_field("name", &mut problems) {
        None if !map.contains_key("name") => {
            problems.push("`name` is required".into());
            String::new()
        }
        None => String::new(),
        Some(n) => {
            problems.extend(name_problems(&n, dir));
            normalize_name(&n)
        }
    };
    let description = match text_field("description", &mut problems) {
        None => {
            if !map.contains_key("description") {
                problems.push("`description` is required".into());
            }
            String::new()
        }
        Some(d) if d.is_empty() => {
            problems.push("`description` is empty".into());
            d
        }
        Some(d) => {
            let n = d.chars().count();
            if n > MAX_DESCRIPTION_CHARS {
                problems.push(format!(
                    "`description` has {n} characters (at most {MAX_DESCRIPTION_CHARS})"
                ));
            }
            d
        }
    };
    let license = text_field("license", &mut problems);
    let compatibility = text_field("compatibility", &mut problems);
    if let Some(c) = &compatibility {
        let n = c.chars().count();
        if n > MAX_COMPATIBILITY_CHARS {
            problems.push(format!(
                "`compatibility` has {n} characters (at most {MAX_COMPATIBILITY_CHARS})"
            ));
        }
    }
    let mut metadata = BTreeMap::new();
    match map.get("metadata") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (k, v) in m {
                match scalar_string(v) {
                    Some(s) => {
                        metadata.insert(k.clone(), s);
                    }
                    None => problems.push(format!(
                        "`metadata.{k}` must be a string (the standard maps strings to strings)"
                    )),
                }
            }
        }
        Some(_) => problems.push("`metadata` must be a mapping of strings".into()),
    }
    let allowed_tools = match map.get("allowed-tools") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(AllowedTools {
            raw: s.trim().to_string(),
            tools: s.split_whitespace().map(str::to_string).collect(),
        }),
        Some(Value::Array(items)) => {
            let tools: Vec<String> = items.iter().filter_map(scalar_string).collect();
            if tools.len() != items.len() {
                problems.push("`allowed-tools` must be a string".into());
            }
            notes.push(
                "`allowed-tools` is a list; the standard writes it as one space-separated string"
                    .into(),
            );
            Some(AllowedTools {
                raw: tools.join(" "),
                tools,
            })
        }
        Some(_) => {
            problems.push("`allowed-tools` must be a string".into());
            None
        }
    };

    for (key, value) in &map {
        if STANDARD_FIELDS.contains(&key.as_str()) {
            continue;
        }
        if BEHAVIOURAL_FIELDS.contains(&key.as_str()) {
            // The defaults of the two invocation flags change nothing.
            let neutral = matches!(
                (key.as_str(), value),
                ("disable-model-invocation", Value::Bool(false))
                    | ("user-invocable", Value::Bool(true))
            );
            if neutral {
                notes.push(format!("`{key}` has its default value; ignored"));
            } else {
                unsupported.push(format!(
                    "`{key}` (a Claude Code directive Bricks does not implement)"
                ));
            }
        } else {
            notes.push(format!("`{key}` is not part of the standard; ignored"));
        }
    }

    if !problems.is_empty() {
        return Err(err(problems.join("; ")));
    }

    let body = body.replace("\r\n", "\n");
    let body = body.trim_start_matches('\n').to_string();
    if body.trim().is_empty() {
        notes.push("the instructions are empty".into());
    }
    if body.contains("$ARGUMENTS") {
        notes.push("`$ARGUMENTS` is not substituted: the request is given separately".into());
    }
    if body.contains("!`") {
        notes.push("`!`command`` lines are not executed: they stay text".into());
    }

    Ok(SkillDocument {
        name,
        description,
        license,
        compatibility,
        metadata,
        allowed_tools,
        body,
        notes,
        unsupported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(front: &str, body: &str) -> String {
        format!("---\n{front}---\n{body}")
    }

    #[test]
    fn a_minimal_standard_document_loads() {
        let d = parse_document(
            &doc(
                "name: pdf-tools\ndescription: Work with PDF files.\n",
                "# Steps\n1. Read.\n",
            ),
            Some("pdf-tools"),
        )
        .unwrap();
        assert_eq!(d.name, "pdf-tools");
        assert_eq!(d.description, "Work with PDF files.");
        assert!(d.license.is_none() && d.metadata.is_empty() && d.allowed_tools.is_none());
        assert_eq!(d.body, "# Steps\n1. Read.\n");
        assert!(d.notes.is_empty() && d.is_compatible());
    }

    #[test]
    fn optional_fields_are_kept() {
        let d = parse_document(
            &doc(
                "name: x\ndescription: d\nlicense: Apache-2.0\ncompatibility: Needs git\n\
                 metadata:\n  author: me\n  version: 1.2\nallowed-tools: Bash(git:*) Read\n",
                "b",
            ),
            Some("x"),
        )
        .unwrap();
        assert_eq!(d.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(d.compatibility.as_deref(), Some("Needs git"));
        assert_eq!(d.metadata["author"], "me");
        assert_eq!(d.metadata["version"], "1.2");
        let a = d.allowed_tools.unwrap();
        assert_eq!(a.raw, "Bash(git:*) Read");
        assert_eq!(a.tools, ["Bash(git:*)", "Read"]);
    }

    #[test]
    fn crlf_and_multiline_descriptions() {
        let text = "---\r\nname: x\r\ndescription: >-\r\n  first line\r\n  second line\r\n---\r\nbody\r\nline 2\r\n";
        let d = parse_document(text, Some("x")).unwrap();
        assert_eq!(d.description, "first line second line");
        assert_eq!(d.body, "body\nline 2\n");
        let text = "---\nname: x\ndescription: |\n  a\n  b\n---\n";
        let d = parse_document(text, Some("x")).unwrap();
        assert_eq!(d.description, "a\nb");
        assert!(d.notes.iter().any(|n| n.contains("empty")));
    }

    #[test]
    fn delimiters_are_whole_lines() {
        // `---` inside a value or a longer line does not close.
        let text = "---\nname: x\ndescription: a --- b\n----\n---\nbody";
        assert!(parse_document(text, Some("x")).is_err());
        let text = "---\nname: x\ndescription: |\n  a\n  ---\n---\nbody";
        let d = parse_document(text, Some("x")).unwrap();
        assert_eq!(d.description, "a\n---");
        assert_eq!(d.body, "body");
        assert!(parse_document("name: x\n---\n", Some("x")).is_err());
        assert!(parse_document("---\nname: x\n", Some("x")).is_err());
    }

    #[test]
    fn unicode_names_are_normalised_and_matched_to_their_directory() {
        // NFKC: the "ﬁ" ligature normalises to "fi".
        let d = parse_document(
            &doc("name: ﬁle-tools\ndescription: d\n", ""),
            Some("file-tools"),
        )
        .unwrap();
        assert_eq!(d.name, "file-tools");
        // Unicode letters are allowed (lowercase).
        assert!(name_problems("données-pdf", Some("données-pdf")).is_empty());
        assert!(name_problems("日本語", Some("日本語")).is_empty());
        // Composed vs decomposed forms of the same name match.
        assert!(name_problems("caf\u{e9}", Some("cafe\u{301}")).is_empty());
        for bad in ["Pdf", "-pdf", "pdf-", "a--b", "a_b", "a b", "a.b"] {
            assert!(!name_problems(bad, None).is_empty(), "{bad}");
        }
        assert!(!name_problems("pdf", Some("other")).is_empty());
        // 64 characters (not bytes) at most.
        let long: String = "é".repeat(64);
        assert!(name_problems(&long, None).is_empty());
        assert!(!name_problems(&"é".repeat(65), None).is_empty());
    }

    #[test]
    fn invalid_documents_are_refused_not_repaired() {
        let cases = [
            ("description: d\n", "required"),
            ("name: x\n", "required"),
            ("name: x\ndescription: \"\"\n", "empty"),
            ("name: x\ndescription: d\nname: y\n", "duplicate"),
            ("name: [x\n", "YAML"),
            (
                "name: x\ndescription: d\nmetadata:\n  a: {b: c}\n",
                "metadata.a",
            ),
            ("name: x\ndescription: d\nmetadata: [a]\n", "metadata"),
            ("name: 3\ndescription: d\n", "string"),
            ("- a\n- b\n", "mapping"),
            ("name: x\ndescription: !!python/object x\n", "YAML"),
        ];
        for (front, want) in cases {
            let e = parse_document(&doc(front, ""), Some("x")).unwrap_err().0;
            assert!(
                e.to_lowercase().contains(&want.to_lowercase()),
                "{front:?}: {e}"
            );
        }
        let long = format!("name: x\ndescription: {}\n", "d".repeat(1025));
        assert!(parse_document(&doc(&long, ""), Some("x")).is_err());
        let long = format!(
            "name: x\ndescription: d\ncompatibility: {}\n",
            "c".repeat(501)
        );
        assert!(parse_document(&doc(&long, ""), Some("x")).is_err());
    }

    #[test]
    fn abusive_yaml_is_refused() {
        let mut deep = String::from("name: x\ndescription: d\nmetadata:\n  a: ");
        deep.push_str(&"[".repeat(40));
        deep.push_str(&"]".repeat(40));
        deep.push('\n');
        assert!(parse_document(&doc(&deep, ""), Some("x")).is_err());
        let mut bomb = String::from("a0: &a0 [x, x, x, x, x, x, x, x]\n");
        for i in 1..30 {
            bomb.push_str(&format!(
                "a{i}: &a{i} [*a{p}, *a{p}, *a{p}, *a{p}]\n",
                p = i - 1
            ));
        }
        bomb.push_str("name: x\ndescription: d\n");
        assert!(parse_document(&doc(&bomb, ""), Some("x")).is_err());
    }

    #[test]
    fn size_is_bounded_in_bytes() {
        let front = "name: x\ndescription: d\n";
        let fill = MAX_SKILL_BYTES - doc(front, "").len();
        assert!(parse_document(&doc(front, &"a".repeat(fill)), Some("x")).is_ok());
        let e = parse_document(&doc(front, &"a".repeat(fill + 1)), Some("x")).unwrap_err();
        assert!(e.0.contains("bytes"), "{e}");
    }

    #[test]
    fn claude_directives_make_a_document_incompatible() {
        for front in [
            "context: fork\n",
            "agent: Explore\n",
            "model: opus\n",
            "effort: high\n",
            "hooks:\n  PreToolUse: []\n",
            "disable-model-invocation: true\n",
            "user-invocable: false\n",
            "paths: \"src/**\"\n",
        ] {
            let d = parse_document(
                &doc(&format!("name: x\ndescription: d\n{front}"), ""),
                Some("x"),
            )
            .unwrap();
            assert!(!d.is_compatible(), "{front}");
        }
        // Default values change nothing; informational fields are notes.
        let d = parse_document(
            &doc(
                "name: x\ndescription: d\ndisable-model-invocation: false\nuser-invocable: true\n\
                 version: 1.0.0\nargument-hint: \"[file]\"\n",
                "Use $ARGUMENTS.\n!`date`\n",
            ),
            Some("x"),
        )
        .unwrap();
        assert!(d.is_compatible());
        assert!(d.notes.iter().any(|n| n.contains("`version`")));
        assert!(d.notes.iter().any(|n| n.contains("$ARGUMENTS")));
        assert!(d.notes.iter().any(|n| n.contains("not executed")));
        assert!(d.body.contains("$ARGUMENTS"), "nothing is substituted");
    }

    #[test]
    fn errors_do_not_echo_the_body() {
        let e = parse_document(&doc("description: d\n", "SECRET-BODY"), Some("x")).unwrap_err();
        assert!(!e.0.contains("SECRET-BODY"));
    }
}
