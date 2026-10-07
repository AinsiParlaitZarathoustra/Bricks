//! `bricks.toml` and `~/.bricks/rules/*.toml`: context, compression and web
//! settings loaded without recompiling.
//!
//! Loading never fails: an unreadable file or an invalid section yields a
//! diagnostic and the defaults for that section, and an invalid rule only
//! disables itself (see [`cersei_compression::rules`]). Changes apply the next
//! time the configuration is loaded (normally at start).

use crate::context::{policy_from_bricks_toml, ContextPolicy};
use cersei_compression::{
    CompressionConfig, CompressionLevel, CompressionSection, RuleSet, RuleSources,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct BricksConfig {
    pub context: ContextPolicy,
    pub compression: CompressionConfig,
    /// `[compression] level`, when set.
    pub compression_level: Option<CompressionLevel>,
    pub rules: Arc<RuleSet>,
    /// `[compression] raw_output_dir`, when set.
    pub raw_output_dir: Option<PathBuf>,
    /// `[web]`: search providers, fetch limits, passages (keys resolved from
    /// the variables the file names).
    pub web: cersei_web::WebConfig,
    /// Problems found while loading, ready to show.
    pub diagnostics: Vec<String>,
    /// `[agent]`: default model, reasoning profile and limits.
    pub agent: crate::control::AgentSettings,
    /// `[permissions]`: the approval policy shared by every frontend.
    pub permissions: crate::control::ApprovalRules,
    /// `[semantic]`: limits of the shared code understanding engine.
    pub semantic: bricks_semantic::SemanticConfig,
    /// `[agents]`: sub-agents (the `Agent` tool) and their runtime limits.
    pub agents: crate::agents::DelegationSettings,
    /// `[background]`: background jobs (`Bash` with `background: true`,
    /// the `Job` tool).
    pub background: cersei_tools::jobs::JobSettings,
}

impl Default for BricksConfig {
    fn default() -> Self {
        Self {
            context: ContextPolicy::default(),
            compression: CompressionConfig::default(),
            compression_level: None,
            rules: RuleSet::builtin_shared(),
            raw_output_dir: None,
            web: cersei_web::WebConfig::default(),
            diagnostics: Vec::new(),
            agent: crate::control::AgentSettings::default(),
            permissions: crate::control::ApprovalRules::default(),
            semantic: bricks_semantic::SemanticConfig::default(),
            agents: crate::agents::DelegationSettings::default(),
            background: cersei_tools::jobs::JobSettings::default(),
        }
    }
}

impl BricksConfig {
    /// `<working_dir>/bricks.toml` and `~/.bricks/rules/*.toml`.
    pub fn load(working_dir: &Path) -> Self {
        let sources = RuleSources::standard(working_dir);
        Self::from_sources(&sources)
    }

    pub fn from_sources(sources: &RuleSources) -> Self {
        let text = match &sources.bricks_toml {
            Some(p) if p.exists() => match std::fs::read_to_string(p) {
                Ok(t) => Some((p.display().to_string(), t)),
                Err(e) => {
                    let mut c = Self::default();
                    c.diagnostics
                        .push(format!("{}: cannot read: {e}", p.display()));
                    c.rules = Arc::new(RuleSet::load(&RuleSources {
                        bricks_toml: None,
                        ..sources.clone()
                    }));
                    return c;
                }
            },
            _ => None,
        };
        let rules = RuleSet::load(sources);
        Self::assemble(text.as_ref().map(|(n, t)| (n.as_str(), t.as_str())), rules)
    }

    /// From a `bricks.toml` text and user rule files held in memory.
    pub fn from_texts(bricks_toml: Option<&str>, user_rule_files: &[(&str, &str)]) -> Self {
        let rules = RuleSet::from_texts(user_rule_files, bricks_toml);
        Self::assemble(bricks_toml.map(|t| ("bricks.toml", t)), rules)
    }

    fn assemble(toml: Option<(&str, &str)>, rules: RuleSet) -> Self {
        let mut c = Self::default();
        for d in rules.diagnostics() {
            c.diagnostics.push(d.to_string());
        }
        c.rules = Arc::new(rules);
        let Some((name, text)) = toml else {
            return c;
        };
        match policy_from_bricks_toml(text) {
            Ok(p) => c.context = p,
            Err(e) => c
                .diagnostics
                .push(format!("{name}: {e} (context defaults used)")),
        }
        match CompressionSection::from_bricks_toml(text) {
            Ok(section) => match section.apply(&c.compression) {
                Ok(cfg) => {
                    c.compression = cfg;
                    c.compression_level = section.level;
                    c.raw_output_dir = section.raw_output_dir;
                }
                Err(e) => c
                    .diagnostics
                    .push(format!("{name}: {e} (compression defaults used)")),
            },
            Err(e) => c
                .diagnostics
                .push(format!("{name}: {e} (compression defaults used)")),
        }
        match crate::control::settings::from_bricks_toml(text) {
            Ok((agent, permissions)) => {
                c.agent = agent;
                c.permissions = permissions;
            }
            Err(e) => c.diagnostics.push(format!(
                "{name}: {e} (agent and permission defaults used: approval is asked before \
                 writing or executing)"
            )),
        }
        match table_from_bricks_toml::<crate::agents::DelegationSettings>(text, "agents") {
            Ok(Some(a)) => c.agents = a,
            Ok(None) => {}
            Err(e) => c
                .diagnostics
                .push(format!("{name}: [agents]: {e} (sub-agent defaults used)")),
        }
        match table_from_bricks_toml::<cersei_tools::jobs::JobSettings>(text, "background") {
            Ok(Some(b)) => c.background = b,
            Ok(None) => {}
            Err(e) => c
                .diagnostics
                .push(format!("{name}: [background]: {e} (job defaults used)")),
        }
        match semantic_from_bricks_toml(text) {
            Ok(Some(sem)) => c.semantic = sem,
            Ok(None) => {}
            Err(e) => c
                .diagnostics
                .push(format!("{name}: [semantic]: {e} (semantic defaults used)")),
        }
        match cersei_web::WebConfig::from_bricks_toml(text) {
            Ok(loaded) => {
                c.web = loaded.config;
                c.diagnostics.extend(
                    loaded
                        .diagnostics
                        .into_iter()
                        .map(|d| format!("{name}: {d}")),
                );
            }
            Err(e) => c
                .diagnostics
                .push(format!("{name}: {e} (web defaults used)")),
        }
        c
    }
}

/// A whole table, when present.
fn table_from_bricks_toml<T: serde::de::DeserializeOwned>(
    text: &str,
    table: &str,
) -> Result<Option<T>, String> {
    let doc: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    match doc.get(table) {
        None => Ok(None),
        Some(v) => v
            .clone()
            .try_into::<T>()
            .map(Some)
            .map_err(|e| e.to_string()),
    }
}

/// The `[semantic]` table, when present.
fn semantic_from_bricks_toml(
    text: &str,
) -> Result<Option<bricks_semantic::SemanticConfig>, String> {
    let doc: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    match doc.get("semantic") {
        None => Ok(None),
        Some(v) => v
            .clone()
            .try_into::<bricks_semantic::SemanticConfig>()
            .map(Some)
            .map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_file_configures_both_sections_and_rules() {
        let c = BricksConfig::from_texts(
            Some(
                "[context]\ncompact_threshold = 0.7\n\n[compression]\nlevel = \"minimal\"\nmax_error_lines = 99\n\n[compression.filters.mytool]\nmatch = [{ program = \"mytool\" }]\nmax_lines = 5\n",
            ),
            &[],
        );
        assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);
        assert_eq!(c.context.compact_threshold, 0.7);
        assert_eq!(c.compression.log.max_error_lines, 99);
        assert_eq!(c.compression_level, Some(CompressionLevel::Minimal));
        assert_eq!(c.rules.get("mytool").unwrap().max_lines, Some(5));
    }

    #[test]
    fn invalid_sections_fall_back_with_a_diagnostic() {
        let c = BricksConfig::from_texts(
            Some("[context]\ncompact_threshold = 3.0\n\n[compression]\nmax_lines = 0\n"),
            &[("bad.toml", "schema_version = 1\n[filters.x]\nmatch = 1\n")],
        );
        assert_eq!(c.diagnostics.len(), 3, "{:#?}", c.diagnostics);
        assert_eq!(c.context, ContextPolicy::default());
        assert_eq!(c.compression, CompressionConfig::default());
        assert!(
            c.rules.get("cargo-test").is_some(),
            "built-in rules still load"
        );
    }

    #[test]
    fn loads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bricks.toml"),
            "[context]\nsafety_margin_tokens = 4096\n",
        )
        .unwrap();
        let rules = dir.path().join("rules");
        std::fs::create_dir(&rules).unwrap();
        std::fs::write(
            rules.join("a.toml"),
            "schema_version = 1\n[filters.mine]\nmatch = [{ program = \"mine\" }]\n",
        )
        .unwrap();
        let c = BricksConfig::from_sources(&RuleSources {
            user_rules_dir: Some(rules),
            bricks_toml: Some(dir.path().join("bricks.toml")),
        });
        assert_eq!(c.context.safety_margin_tokens, 4096);
        assert!(c.rules.get("mine").is_some());
    }

    #[test]
    fn the_documented_example_loads_cleanly() {
        let text = include_str!("../../../docs/bricks.example.toml");
        let c = BricksConfig::from_texts(Some(text), &[]);
        assert!(c.diagnostics.is_empty(), "{:#?}", c.diagnostics);
        assert_eq!(
            c.context,
            ContextPolicy::default(),
            "the example documents the defaults"
        );
        assert_eq!(c.compression, CompressionConfig::default());
        assert!(c.rules.get("make").is_some());
        assert_eq!(
            c.web,
            cersei_web::WebConfig::default(),
            "web defaults documented"
        );
    }

    /// The commented `[agents]` and `[background]` values of the example,
    /// uncommented, are the defaults.
    #[test]
    fn the_documented_agent_and_job_limits_are_the_defaults() {
        let text = include_str!("../../../docs/bricks.example.toml");
        let mut out = String::new();
        let mut section = "";
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                section = if t == "[agents]" || t == "[background]" {
                    t
                } else {
                    ""
                };
                if !section.is_empty() {
                    out.push_str(t);
                    out.push('\n');
                }
                continue;
            }
            let Some(rest) = t.strip_prefix("# ") else {
                continue;
            };
            let key = rest.split('=').next().unwrap_or("").trim();
            let known = [
                "max_concurrent",
                "max_depth",
                "max_total_per_run",
                "max_batch",
                "max_queued",
                "admission_timeout_ms",
                "background_drain_ms",
                "default_isolation",
                "skills_max_bytes",
                "max_jobs",
                "output_buffer_bytes",
                "max_line_bytes",
                "raw_log_bytes",
                "stop_grace_ms",
                "max_page_bytes",
            ];
            if !section.is_empty() && known.contains(&key) {
                out.push_str(rest.split(" #").next().unwrap_or(rest));
                out.push('\n');
            }
        }
        let c = BricksConfig::from_texts(Some(&out), &[]);
        assert!(c.diagnostics.is_empty(), "{:#?}\n{out}", c.diagnostics);
        assert_eq!(
            c.agents,
            crate::agents::DelegationSettings::default(),
            "{out}"
        );
        assert_eq!(
            c.background,
            cersei_tools::jobs::JobSettings::default(),
            "{out}"
        );
        assert_eq!(out.matches(" = ").count(), 15, "{out}");
    }

    #[test]
    fn an_invalid_web_section_falls_back_with_a_diagnostic() {
        let c = BricksConfig::from_texts(
            Some(
                "[web.fetch]
per_host = 0
",
            ),
            &[],
        );
        assert_eq!(c.web, cersei_web::WebConfig::default());
        assert!(c.diagnostics[0].contains("per_host"), "{:?}", c.diagnostics);
    }

    #[test]
    fn semantic_section_sets_limits_and_bad_values_fall_back() {
        let c = BricksConfig::from_texts(
            Some("[semantic]\nmax_results = 7\n\n[semantic.lsp]\nenabled = false\n"),
            &[],
        );
        assert_eq!(c.semantic.max_results, 7);
        assert!(!c.semantic.lsp.enabled);
        assert_eq!(
            c.semantic.max_files,
            bricks_semantic::SemanticConfig::default().max_files
        );
        let c = BricksConfig::from_texts(Some("[semantic]\nmax_results = \"many\"\n"), &[]);
        assert_eq!(
            c.semantic.max_results,
            bricks_semantic::SemanticConfig::default().max_results
        );
        assert!(c.diagnostics.iter().any(|d| d.contains("[semantic]")));
    }
}
