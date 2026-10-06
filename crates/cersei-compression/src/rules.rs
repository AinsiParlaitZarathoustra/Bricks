//! Declarative rules for command outputs, loaded without recompiling.
//!
//! # Sources and priority
//!
//! Rules are loaded in three layers, later layers winning:
//!
//! 1. the built-in rules embedded in this crate (`src/rules/*.toml`);
//! 2. user files `~/.bricks/rules/*.toml`, in byte order of their file names;
//! 3. the `[compression.filters.<id>]` tables of a `bricks.toml`.
//!
//! Every rule has an id (its table name). A rule with the id of an existing
//! one replaces it entirely; `disabled = true` removes it. An invalid rule is
//! reported as a [`RuleDiagnostic`] naming its source, id and field, and is
//! skipped — the rest of the file, and any earlier valid rule with the same
//! id, stay active. Rules are read when a [`RuleSet`] is loaded, so a change
//! applies on the next load or start. Rules never execute anything.
//!
//! # Matching
//!
//! A rule matches the program and sub-commands a command line actually runs
//! (see [`crate::command`]), never a substring of the line. The most specific
//! match wins (longest sub-command path); ties go to the higher layer, then
//! to the later source, then to the smaller id.

use crate::command::Invocation;
use crate::errors::{BlockEnd, Detector, Severity};
use regex::{Regex, RegexSet};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Built-in rule files, in load order.
const BUILTIN_RULE_FILES: &[(&str, &str)] = &[
    ("builtin:generic.toml", include_str!("rules/generic.toml")),
    ("builtin:exact.toml", include_str!("rules/exact.toml")),
    ("builtin:git.toml", include_str!("rules/git.toml")),
    ("builtin:rust.toml", include_str!("rules/rust.toml")),
    ("builtin:go.toml", include_str!("rules/go.toml")),
    (
        "builtin:javascript.toml",
        include_str!("rules/javascript.toml"),
    ),
    ("builtin:python.toml", include_str!("rules/python.toml")),
    ("builtin:devops.toml", include_str!("rules/devops.toml")),
];

// ─── Schema ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    schema_version: u32,
    #[serde(default)]
    filters: BTreeMap<String, toml::Value>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct FilterDef {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    disabled: bool,
    #[serde(default, rename = "match")]
    matchers: Vec<MatchDef>,
    #[serde(default)]
    options_with_value: Vec<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    structured: Option<String>,
    #[serde(default)]
    replace: Vec<ReplaceDef>,
    #[serde(default)]
    match_output: Vec<MatchOutputDef>,
    #[serde(default)]
    strip_lines_matching: Vec<String>,
    #[serde(default)]
    keep_lines_matching: Vec<String>,
    #[serde(default)]
    summarize_lines: Vec<SummarizeDef>,
    #[serde(default)]
    protect_lines: Vec<String>,
    #[serde(default)]
    diagnostic_blocks: Vec<BlockDef>,
    #[serde(default)]
    truncate_lines_at: Option<usize>,
    #[serde(default)]
    head_lines: Option<usize>,
    #[serde(default)]
    tail_lines: Option<usize>,
    #[serde(default)]
    max_lines: Option<usize>,
    #[serde(default)]
    on_empty: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MatchDef {
    program: String,
    #[serde(default)]
    subcommands: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceDef {
    pattern: String,
    replacement: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MatchOutputDef {
    pattern: String,
    message: String,
    #[serde(default)]
    unless: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SummarizeDef {
    pattern: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockDef {
    start: String,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    inclusive: bool,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    warning_if: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    max_lines: Option<usize>,
}

// ─── Compiled form ───────────────────────────────────────────────────────────

/// How a rule treats the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleMode {
    /// Clean, filter and budget the log, keeping diagnostics first.
    Log,
    /// The output is a value the caller asked for (`cat`, `git diff`): never
    /// filtered, only capped with a reference to the full output.
    Exact,
}

/// Structured formats a rule may declare (they are also auto-detected).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Structured {
    CargoJson,
    GoTestJson,
}

#[derive(Debug, Clone)]
pub struct Matcher {
    pub program: String,
    pub subcommands: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct Summarize {
    pub pattern: Regex,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct MatchOutput {
    pub pattern: Regex,
    pub message: String,
    pub unless: Option<Regex>,
}

#[derive(Debug, Clone)]
pub enum LineFilter {
    None,
    Strip(RegexSet),
    Keep(RegexSet),
}

/// Where a rule came from, for diagnostics and reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOrigin {
    /// 0 = built-in, 1 = user rule file, 2 = bricks.toml.
    pub layer: u8,
    pub source: String,
    /// Load order across all sources, for deterministic tie-breaking.
    pub order: usize,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub description: Option<String>,
    pub origin: RuleOrigin,
    pub matchers: Vec<Matcher>,
    pub options_with_value: Vec<String>,
    pub mode: RuleMode,
    pub structured: Option<Structured>,
    pub replace: Vec<(Regex, String)>,
    pub match_output: Vec<MatchOutput>,
    pub line_filter: LineFilter,
    pub summarize: Vec<Summarize>,
    pub protect: Option<RegexSet>,
    pub detectors: Vec<Detector>,
    pub truncate_lines_at: Option<usize>,
    pub head_lines: Option<usize>,
    pub tail_lines: Option<usize>,
    pub max_lines: Option<usize>,
    pub on_empty: Option<String>,
}

impl Rule {
    /// Specificity of the match with `inv`, or `None`. `program = "*"`
    /// matches anything with specificity 0.
    pub fn specificity(&self, inv: &Invocation) -> Option<usize> {
        let positionals = inv.positionals(&self.options_with_value);
        let mut best = None;
        for m in &self.matchers {
            let score = if m.program == "*" {
                Some(0)
            } else if m.program == inv.program {
                if m.subcommands.is_empty() {
                    Some(1)
                } else {
                    m.subcommands
                        .iter()
                        .filter(|path| {
                            path.len() <= positionals.len()
                                && path.iter().zip(&positionals).all(|(a, b)| a == b)
                        })
                        .map(|path| 1 + path.len())
                        .max()
                }
            } else {
                None
            };
            best = best.max(score);
        }
        best
    }
}

/// A problem found while loading rules. Loading never fails as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDiagnostic {
    pub source: String,
    pub rule: Option<String>,
    pub message: String,
}

impl std::fmt::Display for RuleDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.rule {
            Some(r) => write!(f, "{}: rule `{}`: {}", self.source, r, self.message),
            None => write!(f, "{}: {}", self.source, self.message),
        }
    }
}

/// Where to look for rules beyond the built-in ones.
#[derive(Debug, Clone, Default)]
pub struct RuleSources {
    /// Directory of `*.toml` rule files (normally `~/.bricks/rules`).
    pub user_rules_dir: Option<PathBuf>,
    /// A `bricks.toml` whose `[compression.filters]` tables add rules.
    pub bricks_toml: Option<PathBuf>,
}

impl RuleSources {
    /// `~/.bricks/rules` and `<working_dir>/bricks.toml`.
    pub fn standard(working_dir: &Path) -> Self {
        Self {
            user_rules_dir: dirs::home_dir().map(|h| h.join(".bricks").join("rules")),
            bricks_toml: Some(working_dir.join("bricks.toml")),
        }
    }
}

/// The active rules plus what happened while loading them.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
    disabled: Vec<(String, String)>,
    diagnostics: Vec<RuleDiagnostic>,
}

impl RuleSet {
    /// Only the built-in rules.
    pub fn builtin() -> Self {
        Self::load(&RuleSources::default())
    }

    pub fn load(sources: &RuleSources) -> Self {
        let mut loader = Loader::default();
        for (name, text) in BUILTIN_RULE_FILES {
            loader.load_rule_file(0, name, text);
        }
        if let Some(dir) = &sources.user_rules_dir {
            loader.load_dir(dir);
        }
        if let Some(path) = &sources.bricks_toml {
            if path.exists() {
                match std::fs::read_to_string(path) {
                    Ok(text) => loader.load_bricks_toml(&path.display().to_string(), &text),
                    Err(e) => loader.diag(
                        &path.display().to_string(),
                        None,
                        format!("cannot read: {e}"),
                    ),
                }
            }
        }
        loader.finish()
    }

    /// Load from in-memory texts (tests, embedding).
    pub fn from_texts(user_files: &[(&str, &str)], bricks_toml: Option<&str>) -> Self {
        let mut loader = Loader::default();
        for (name, text) in BUILTIN_RULE_FILES {
            loader.load_rule_file(0, name, text);
        }
        for (name, text) in user_files {
            loader.load_rule_file(1, name, text);
        }
        if let Some(text) = bricks_toml {
            loader.load_bricks_toml("bricks.toml", text);
        }
        loader.finish()
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// Ids removed with `disabled = true`, with the source that disabled them.
    pub fn disabled(&self) -> &[(String, String)] {
        &self.disabled
    }

    pub fn diagnostics(&self) -> &[RuleDiagnostic] {
        &self.diagnostics
    }

    /// The best rule for an invocation (see the module documentation).
    pub fn find(&self, inv: &Invocation) -> Option<&Rule> {
        self.rules
            .iter()
            .filter_map(|r| r.specificity(inv).map(|s| (s, r)))
            .max_by(|(sa, a), (sb, b)| {
                sa.cmp(sb)
                    .then(a.origin.layer.cmp(&b.origin.layer))
                    .then(a.origin.order.cmp(&b.origin.order))
                    .then(b.id.cmp(&a.id))
            })
            .map(|(_, r)| r)
    }

    /// The catch-all rule (`program = "*"`) with the highest priority.
    pub fn fallback(&self) -> Option<&Rule> {
        self.rules
            .iter()
            .filter(|r| r.matchers.iter().any(|m| m.program == "*"))
            .max_by(|a, b| {
                a.origin
                    .layer
                    .cmp(&b.origin.layer)
                    .then(a.origin.order.cmp(&b.origin.order))
            })
    }
}

#[derive(Default)]
struct Loader {
    active: BTreeMap<String, Rule>,
    disabled: Vec<(String, String)>,
    diagnostics: Vec<RuleDiagnostic>,
    order: usize,
}

impl Loader {
    fn diag(&mut self, source: &str, rule: Option<&str>, message: String) {
        self.diagnostics.push(RuleDiagnostic {
            source: source.to_string(),
            rule: rule.map(str::to_string),
            message,
        });
    }

    fn load_dir(&mut self, dir: &Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                self.diag(
                    &dir.display().to_string(),
                    None,
                    format!("cannot list: {e}"),
                );
                return;
            }
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
            .collect();
        // Byte order of the file names: stable across platforms and runs.
        files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        for path in files {
            let source = path.display().to_string();
            match std::fs::read_to_string(&path) {
                Ok(text) => self.load_rule_file(1, &source, &text),
                Err(e) => self.diag(&source, None, format!("cannot read: {e}")),
            }
        }
    }

    fn load_rule_file(&mut self, layer: u8, source: &str, text: &str) {
        let file: RuleFile = match toml::from_str(text) {
            Ok(f) => f,
            Err(e) => {
                self.diag(
                    source,
                    None,
                    format!("invalid TOML: {}", one_line(&e.to_string())),
                );
                return;
            }
        };
        if file.schema_version != 1 {
            self.diag(
                source,
                None,
                format!(
                    "unsupported schema_version {} (expected 1)",
                    file.schema_version
                ),
            );
            return;
        }
        for (id, value) in file.filters {
            self.add(layer, source, id, value);
        }
    }

    fn load_bricks_toml(&mut self, source: &str, text: &str) {
        let doc: toml::Value = match toml::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                self.diag(
                    source,
                    None,
                    format!("invalid TOML: {}", one_line(&e.to_string())),
                );
                return;
            }
        };
        let Some(filters) = doc.get("compression").and_then(|c| c.get("filters")) else {
            return;
        };
        let Some(table) = filters.as_table() else {
            self.diag(
                source,
                None,
                "`compression.filters` must be a table of rules".into(),
            );
            return;
        };
        for (id, value) in table.clone() {
            self.add(2, source, id, value);
        }
    }

    fn add(&mut self, layer: u8, source: &str, id: String, value: toml::Value) {
        self.order += 1;
        if !valid_id(&id) {
            self.diag(
                source,
                Some(&id),
                "invalid id: use letters, digits, `-`, `_` and `.` only".into(),
            );
            return;
        }
        let def: FilterDef = match serde_path_to_error::deserialize(value) {
            Ok(d) => d,
            Err(e) => {
                let path = e.path().to_string();
                let msg = one_line(&e.into_inner().to_string());
                let msg = if path == "." {
                    msg
                } else {
                    format!("field `{path}`: {msg}")
                };
                self.diag(source, Some(&id), msg);
                return;
            }
        };
        if def.disabled {
            if self.active.remove(&id).is_none() {
                self.diag(
                    source,
                    Some(&id),
                    "disables a rule that is not defined (ignored)".into(),
                );
            }
            self.disabled.push((id, source.to_string()));
            return;
        }
        let origin = RuleOrigin {
            layer,
            source: source.to_string(),
            order: self.order,
        };
        match compile(&id, def, origin) {
            Ok(rule) => {
                self.active.insert(id, rule);
            }
            Err(msg) => {
                let kept = if self.active.contains_key(&id) {
                    " (the previously loaded rule with this id stays active)"
                } else {
                    ""
                };
                self.diag(source, Some(&id), format!("{msg}{kept}"));
            }
        }
    }

    fn finish(self) -> RuleSet {
        RuleSet {
            rules: self.active.into_values().collect(),
            disabled: self.disabled,
            diagnostics: self.diagnostics,
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn regex(field: &str, pattern: &str) -> Result<Regex, String> {
    Regex::new(pattern).map_err(|e| {
        format!(
            "field `{field}`: invalid regex {pattern:?}: {}",
            one_line(&e.to_string())
        )
    })
}

fn regex_set(field: &str, patterns: &[String]) -> Result<RegexSet, String> {
    for (i, p) in patterns.iter().enumerate() {
        regex(&format!("{field}[{i}]"), p)?;
    }
    RegexSet::new(patterns).map_err(|e| format!("field `{field}`: {}", one_line(&e.to_string())))
}

fn compile(id: &str, def: FilterDef, origin: RuleOrigin) -> Result<Rule, String> {
    if def.matchers.is_empty() {
        return Err("field `match`: at least one { program = \"…\" } entry is required".into());
    }
    let mut matchers = Vec::new();
    for (i, m) in def.matchers.iter().enumerate() {
        let program = m.program.trim();
        if program.is_empty() || program.contains(char::is_whitespace) || program.contains('/') {
            return Err(format!(
                "field `match[{i}].program`: must be a single program name (e.g. \"cargo\") or \"*\""
            ));
        }
        let mut subcommands = Vec::new();
        for (j, s) in m.subcommands.iter().enumerate() {
            let words: Vec<String> = s.split_whitespace().map(str::to_string).collect();
            if words.is_empty() {
                return Err(format!(
                    "field `match[{i}].subcommands[{j}]`: must not be empty"
                ));
            }
            subcommands.push(words);
        }
        if program == "*" && !subcommands.is_empty() {
            return Err(format!(
                "field `match[{i}].subcommands`: not allowed with program = \"*\""
            ));
        }
        matchers.push(Matcher {
            program: program.to_string(),
            subcommands,
        });
    }
    let mode = match def.mode.as_deref() {
        None | Some("log") => RuleMode::Log,
        Some("exact") => RuleMode::Exact,
        Some(other) => {
            return Err(format!(
                "field `mode`: unknown mode {other:?} (expected \"log\" or \"exact\")"
            ))
        }
    };
    let structured = match def.structured.as_deref() {
        None => None,
        Some("cargo-json") => Some(Structured::CargoJson),
        Some("go-test-json") => Some(Structured::GoTestJson),
        Some(other) => {
            return Err(format!(
                "field `structured`: unknown format {other:?} (expected \"cargo-json\" or \"go-test-json\")"
            ))
        }
    };
    if !def.strip_lines_matching.is_empty() && !def.keep_lines_matching.is_empty() {
        return Err(
            "`strip_lines_matching` and `keep_lines_matching` are mutually exclusive".into(),
        );
    }
    let line_filter = if !def.strip_lines_matching.is_empty() {
        LineFilter::Strip(regex_set(
            "strip_lines_matching",
            &def.strip_lines_matching,
        )?)
    } else if !def.keep_lines_matching.is_empty() {
        LineFilter::Keep(regex_set("keep_lines_matching", &def.keep_lines_matching)?)
    } else {
        LineFilter::None
    };
    let mut replace = Vec::new();
    for (i, r) in def.replace.iter().enumerate() {
        replace.push((
            regex(&format!("replace[{i}].pattern"), &r.pattern)?,
            r.replacement.clone(),
        ));
    }
    let mut match_output = Vec::new();
    for (i, m) in def.match_output.iter().enumerate() {
        match_output.push(MatchOutput {
            pattern: regex(&format!("match_output[{i}].pattern"), &m.pattern)?,
            message: m.message.clone(),
            unless: m
                .unless
                .as_deref()
                .map(|u| regex(&format!("match_output[{i}].unless"), u))
                .transpose()?,
        });
    }
    let mut summarize = Vec::new();
    for (i, s) in def.summarize_lines.iter().enumerate() {
        summarize.push(Summarize {
            pattern: regex(&format!("summarize_lines[{i}].pattern"), &s.pattern)?,
            label: s.label.clone(),
        });
    }
    let protect = if def.protect_lines.is_empty() {
        None
    } else {
        Some(regex_set("protect_lines", &def.protect_lines)?)
    };
    let mut detectors = Vec::new();
    for (i, b) in def.diagnostic_blocks.iter().enumerate() {
        let field = |f: &str| format!("diagnostic_blocks[{i}].{f}");
        let end = match (b.end.as_deref(), b.until.as_deref()) {
            (Some("until") | None, Some(u)) => BlockEnd::Until {
                pattern: regex(&field("until"), u)?,
                inclusive: b.inclusive,
            },
            (Some("single") | None, None) => BlockEnd::Single,
            (Some("blank"), None) => BlockEnd::Blank,
            (Some("indented"), None) => BlockEnd::Indented,
            (Some("until"), None) => {
                return Err(format!(
                    "field `{}`: `end = \"until\"` needs `until`",
                    field("end")
                ))
            }
            (Some(other), _) => {
                return Err(format!(
                    "field `{}`: unknown end {other:?} (expected single, blank, indented or until)",
                    field("end")
                ))
            }
        };
        let severity = match b.severity.as_deref() {
            None => Severity::Error,
            Some(s) => Severity::parse(s).ok_or_else(|| {
                format!(
                    "field `{}`: expected \"error\" or \"warning\"",
                    field("severity")
                )
            })?,
        };
        detectors.push(Detector {
            kind: b.kind.clone().unwrap_or_else(|| format!("{id} diagnostic")),
            start: regex(&field("start"), &b.start)?,
            end,
            severity,
            warning_if: b
                .warning_if
                .as_deref()
                .map(|w| regex(&field("warning_if"), w))
                .transpose()?,
            min_lines: 1,
            max_lines: b.max_lines.unwrap_or(200),
        });
    }
    for (name, v) in [
        ("max_lines", def.max_lines),
        ("truncate_lines_at", def.truncate_lines_at),
    ] {
        if v == Some(0) {
            return Err(format!("field `{name}`: must be greater than 0"));
        }
    }
    Ok(Rule {
        id: id.to_string(),
        description: def.description,
        origin,
        matchers,
        options_with_value: def.options_with_value,
        mode,
        structured,
        replace,
        match_output,
        line_filter,
        summarize,
        protect,
        detectors,
        truncate_lines_at: def.truncate_lines_at,
        head_lines: def.head_lines,
        tail_lines: def.tail_lines,
        max_lines: def.max_lines,
        on_empty: def.on_empty,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::analyze;

    fn inv(line: &str) -> Invocation {
        analyze(line).primary().unwrap().producer().unwrap().clone()
    }

    #[test]
    fn builtin_rules_load_without_diagnostics() {
        let set = RuleSet::builtin();
        assert!(set.diagnostics().is_empty(), "{:#?}", set.diagnostics());
        assert!(set.rules().len() >= 25, "{}", set.rules().len());
        assert!(set.fallback().is_some());
    }

    #[test]
    fn most_specific_rule_wins() {
        let set = RuleSet::builtin();
        assert_eq!(set.find(&inv("cargo test -p x")).unwrap().id, "cargo-test");
        assert_eq!(
            set.find(&inv("cargo clippy --all-targets")).unwrap().id,
            "cargo-build"
        );
        assert_eq!(set.find(&inv("cargo audit")).unwrap().id, "cargo-audit");
        assert_eq!(set.find(&inv("cargo fmt")).unwrap().id, "cargo");
        assert_eq!(set.find(&inv("unknown-tool --x")).unwrap().id, "generic");
    }

    #[test]
    fn user_files_override_and_disable_in_order() {
        let a = r#"
schema_version = 1
[filters.cargo-test]
match = [{ program = "cargo", subcommands = ["test"] }]
max_lines = 7
"#;
        let b = r#"
schema_version = 1
[filters.cargo-test]
match = [{ program = "cargo", subcommands = ["test"] }]
max_lines = 9
[filters.git-log]
disabled = true
"#;
        // `b-...` sorts after `a-...`: it wins.
        let set = RuleSet::from_texts(&[("a-first.toml", a), ("b-second.toml", b)], None);
        assert_eq!(set.get("cargo-test").unwrap().max_lines, Some(9));
        assert!(set.get("git-log").is_none());
        assert_eq!(set.disabled()[0].0, "git-log");

        // bricks.toml beats every user file.
        let toml = r#"
[compression.filters.cargo-test]
match = [{ program = "cargo", subcommands = ["test"] }]
max_lines = 11
"#;
        let set = RuleSet::from_texts(&[("a.toml", a), ("b.toml", b)], Some(toml));
        let r = set.get("cargo-test").unwrap();
        assert_eq!((r.max_lines, r.origin.layer), (Some(11), 2));
    }

    #[test]
    fn an_invalid_rule_is_reported_and_does_not_disable_others() {
        let bad = r#"
schema_version = 1
[filters.cargo-test]
match = [{ program = "cargo", subcommands = ["test"] }]
strip_lines_matching = ["(unclosed"]

[filters.typo]
match = [{ program = "x" }]
max_line = 3

[filters.good]
match = [{ program = "mytool" }]
max_lines = 5
"#;
        let set = RuleSet::from_texts(&[("mine.toml", bad)], None);
        let d = set.diagnostics();
        assert_eq!(d.len(), 2, "{d:#?}");
        assert!(d[0].message.contains("strip_lines_matching[0]"), "{}", d[0]);
        assert!(d[0].message.contains("stays active"), "{}", d[0]);
        assert_eq!(d[1].rule.as_deref(), Some("typo"));
        assert!(d[1].message.contains("max_line"), "{}", d[1]);
        // The built-in cargo-test is still there, and the valid rule loaded.
        assert_eq!(set.get("cargo-test").unwrap().origin.layer, 0);
        assert!(set.get("good").is_some());
    }

    #[test]
    fn a_broken_file_does_not_stop_the_next_one() {
        let set = RuleSet::from_texts(
            &[
                ("a.toml", "schema_version = 1\n[filters.x\n"),
                (
                    "b.toml",
                    "schema_version = 1\n[filters.mine]\nmatch = [{ program = \"mine\" }]\n",
                ),
                ("c.toml", "schema_version = 2\n"),
            ],
            None,
        );
        assert_eq!(set.diagnostics().len(), 2);
        assert!(set.diagnostics()[0].message.starts_with("invalid TOML"));
        assert!(set.diagnostics()[1].message.contains("schema_version 2"));
        assert!(set.get("mine").is_some());
    }

    #[test]
    fn ties_are_broken_by_layer_then_order() {
        let user = r#"
schema_version = 1
[filters.my-cargo-test]
match = [{ program = "cargo", subcommands = ["test"] }]
"#;
        let set = RuleSet::from_texts(&[("u.toml", user)], None);
        assert_eq!(set.find(&inv("cargo test")).unwrap().id, "my-cargo-test");
    }

    #[test]
    fn rules_load_from_disk_in_file_name_order() {
        let dir = tempfile::tempdir().unwrap();
        let rule = |n: usize| {
            format!("schema_version = 1\n[filters.mine]\nmatch = [{{ program = \"mine\" }}]\nmax_lines = {n}\n")
        };
        std::fs::write(dir.path().join("20-late.toml"), rule(20)).unwrap();
        std::fs::write(dir.path().join("10-early.toml"), rule(10)).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let toml = dir.path().join("bricks.toml");
        let sources = RuleSources {
            user_rules_dir: Some(dir.path().to_path_buf()),
            bricks_toml: Some(toml.clone()),
        };
        assert_eq!(
            RuleSet::load(&sources).get("mine").unwrap().max_lines,
            Some(20)
        );
        // A later edit is picked up by the next load: no recompilation.
        std::fs::write(
            &toml,
            "[compression.filters.mine]\nmatch = [{ program = \"mine\" }]\nmax_lines = 30\n",
        )
        .unwrap();
        assert_eq!(
            RuleSet::load(&sources).get("mine").unwrap().max_lines,
            Some(30)
        );
    }
}
