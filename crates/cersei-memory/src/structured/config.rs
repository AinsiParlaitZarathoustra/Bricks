//! `[memory]` settings of `bricks.toml`.
//!
//! ```toml
//! [memory]
//! space = "project:bricks"          # where new episodes and facts go
//! recall_spaces = ["project:bricks", "user:me"]
//! exclusive_predicates = ["uses_framework", "lives_in"]
//!
//! [memory.recall]
//! seeds = 10
//! hops = 2
//! weight_vector = 1.0
//! weight_relational = 1.2
//! weight_lexical = 0.8
//! rrf_k = 60
//! ```

use serde::Deserialize;
use std::time::Duration;

/// Recall limits and fusion parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallConfig {
    /// Vector candidates used as seeds of the relational expansion (5–15).
    pub seeds: usize,
    /// Candidates taken from each list (vector, lexical).
    pub candidates: usize,
    /// Relational hops (0, 1 or 2).
    pub hops: usize,
    /// Neighbours followed from one node.
    pub max_neighbors: usize,
    /// Nodes visited by one expansion.
    pub max_visited: usize,
    /// Wall-clock budget of one expansion.
    pub expansion_budget: Duration,
    /// Weighted reciprocal rank fusion: `w / (rrf_k + rank)`, ranks from 1.
    pub rrf_k: f64,
    pub weight_vector: f64,
    pub weight_relational: f64,
    pub weight_lexical: f64,
    /// Items returned.
    pub max_results: usize,
    /// Estimated tokens of the rendered recall.
    pub max_tokens: usize,
    /// Characters of an episode passage shown.
    pub passage_chars: usize,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            seeds: 10,
            candidates: 30,
            hops: 2,
            max_neighbors: 8,
            max_visited: 200,
            expansion_budget: Duration::from_millis(50),
            rrf_k: 60.0,
            weight_vector: 1.0,
            weight_relational: 1.2,
            weight_lexical: 0.8,
            max_results: 8,
            max_tokens: 1200,
            passage_chars: 600,
        }
    }
}

/// Extraction limits.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionConfig {
    /// Episode characters sent to the extractor (longer episodes are cut and
    /// marked so).
    pub max_input_chars: usize,
    /// Output tokens allowed to the extractor.
    pub max_output_tokens: u32,
    /// Characters of extractor output accepted.
    pub max_output_chars: usize,
    pub timeout: Duration,
    /// Facts accepted from one episode.
    pub max_facts: usize,
    /// Attempts before an episode's extraction is left failed.
    pub max_attempts: u32,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            max_input_chars: 12_000,
            max_output_tokens: 2_000,
            max_output_chars: 32_000,
            timeout: Duration::from_secs(60),
            max_facts: 40,
            max_attempts: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryConfig {
    /// Frontends attach the long-term memory only when this is true
    /// (default false: nothing is recorded).
    pub enabled: bool,
    /// The memory store (default: `~/.bricks/memory/memory.grafeo`, chosen
    /// by the frontend).
    pub path: Option<std::path::PathBuf>,
    /// Embedding model: `hashing[:dims]` (local, lexical, free — the
    /// default), `openai[:model]` or `gemini[:model]` (keys from
    /// `OPENAI_API_KEY` / `GEMINI_API_KEY`).
    pub embeddings: String,
    /// Model extracting facts (`provider_id/model_id`). Without one, turns
    /// are stored and embedded only (no facts).
    pub extractor_model: Option<String>,
    /// Space of new records.
    pub space: String,
    /// Spaces searched by recall (always includes `space`).
    pub recall_spaces: Vec<String>,
    /// Predicates with a single value per subject at a time: an explicit
    /// change replaces the previous value; a silent difference is a
    /// contradiction.
    pub exclusive_predicates: Vec<String>,
    pub recall: RecallConfig,
    pub extraction: ExtractionConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: None,
            embeddings: "hashing".into(),
            extractor_model: None,
            space: "space:default".into(),
            recall_spaces: Vec::new(),
            exclusive_predicates: [
                "uses_framework",
                "framework",
                "lives_in",
                "employer",
                "works_at",
                "job_title",
                "language",
                "database",
                "deadline",
                "status",
                "owner",
                "version",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            recall: RecallConfig::default(),
            extraction: ExtractionConfig::default(),
        }
    }
}

impl MemoryConfig {
    pub fn with_space(mut self, space: impl Into<String>) -> Self {
        self.space = space.into();
        self
    }

    /// Spaces recall searches.
    pub fn spaces(&self) -> Vec<String> {
        let mut v = vec![self.space.clone()];
        for s in &self.recall_spaces {
            if !v.contains(s) {
                v.push(s.clone());
            }
        }
        v
    }

    pub fn is_exclusive(&self, predicate: &str) -> bool {
        self.exclusive_predicates.iter().any(|p| p == predicate)
    }

    pub fn validate(&self) -> Result<(), String> {
        for s in self.spaces() {
            if !super::model::valid_space(&s) {
                return Err(format!(
                    "memory space `{s}` must be `user:<id>`, `project:<name>` or `space:<name>`"
                ));
            }
        }
        for p in &self.exclusive_predicates {
            if super::model::normalize_predicate(p).as_deref() != Some(p.as_str()) {
                return Err(format!("exclusive predicate `{p}` must be snake_case"));
            }
        }
        let r = &self.recall;
        let range = |name: &str, v: usize, lo: usize, hi: usize| {
            if v < lo || v > hi {
                Err(format!(
                    "memory.recall.{name} must be between {lo} and {hi} (got {v})"
                ))
            } else {
                Ok(())
            }
        };
        range("seeds", r.seeds, 1, 50)?;
        range("candidates", r.candidates, 1, 500)?;
        range("hops", r.hops, 0, 2)?;
        range("max_neighbors", r.max_neighbors, 1, 100)?;
        range("max_visited", r.max_visited, 1, 10_000)?;
        range("max_results", r.max_results, 1, 100)?;
        range("max_tokens", r.max_tokens, 50, 50_000)?;
        range("passage_chars", r.passage_chars, 50, 10_000)?;
        if !(r.rrf_k.is_finite() && r.rrf_k >= 0.0) {
            return Err("memory.recall.rrf_k must be a non-negative number".into());
        }
        for (n, w) in [
            ("weight_vector", r.weight_vector),
            ("weight_relational", r.weight_relational),
            ("weight_lexical", r.weight_lexical),
        ] {
            if !(w.is_finite() && w >= 0.0) {
                return Err(format!("memory.recall.{n} must be a non-negative number"));
            }
        }
        let kind = self.embeddings.split(':').next().unwrap_or("");
        if !["hashing", "openai", "gemini"].contains(&kind) {
            return Err(format!(
                "memory.embeddings `{}` must be `hashing[:dims]`, `openai[:model]` or `gemini[:model]`",
                self.embeddings
            ));
        }
        if let Some(m) = &self.extractor_model {
            if !m.contains('/') {
                return Err(format!(
                    "memory.extractor_model `{m}` must be `provider_id/model_id`"
                ));
            }
        }
        let e = &self.extraction;
        range("extraction.max_facts", e.max_facts, 1, 500)?;
        range(
            "extraction.max_input_chars",
            e.max_input_chars,
            100,
            1_000_000,
        )?;
        if e.max_attempts == 0 {
            return Err("memory.extraction.max_attempts must be at least 1".into());
        }
        Ok(())
    }

    /// The `[memory]` section of a `bricks.toml` text (defaults when absent).
    pub fn from_bricks_toml(text: &str) -> Result<Self, String> {
        let value: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut c = Self::default();
        let Some(section) = value.get("memory") else {
            return Ok(c);
        };
        let f: FileMemory = section
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| format!("[memory]: {}", e.to_string().trim()))?;
        set(&mut c.enabled, f.enabled);
        c.path = f.path.or(c.path);
        set(&mut c.embeddings, f.embeddings);
        c.extractor_model = f.extractor_model.or(c.extractor_model);
        if let Some(s) = f.space {
            c.space = s;
        }
        if let Some(s) = f.recall_spaces {
            c.recall_spaces = s;
        }
        if let Some(p) = f.exclusive_predicates {
            c.exclusive_predicates = p;
        }
        if let Some(r) = f.recall {
            let d = &mut c.recall;
            set(&mut d.seeds, r.seeds);
            set(&mut d.candidates, r.candidates);
            set(&mut d.hops, r.hops);
            set(&mut d.max_neighbors, r.max_neighbors);
            set(&mut d.max_visited, r.max_visited);
            if let Some(ms) = r.expansion_budget_ms {
                d.expansion_budget = Duration::from_millis(ms);
            }
            set(&mut d.rrf_k, r.rrf_k);
            set(&mut d.weight_vector, r.weight_vector);
            set(&mut d.weight_relational, r.weight_relational);
            set(&mut d.weight_lexical, r.weight_lexical);
            set(&mut d.max_results, r.max_results);
            set(&mut d.max_tokens, r.max_tokens);
            set(&mut d.passage_chars, r.passage_chars);
        }
        if let Some(e) = f.extraction {
            let d = &mut c.extraction;
            set(&mut d.max_input_chars, e.max_input_chars);
            set(&mut d.max_output_tokens, e.max_output_tokens);
            set(&mut d.max_output_chars, e.max_output_chars);
            if let Some(ms) = e.timeout_ms {
                d.timeout = Duration::from_millis(ms);
            }
            set(&mut d.max_facts, e.max_facts);
            set(&mut d.max_attempts, e.max_attempts);
        }
        c.validate()?;
        Ok(c)
    }
}

fn set<T>(slot: &mut T, v: Option<T>) {
    if let Some(v) = v {
        *slot = v;
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileMemory {
    enabled: Option<bool>,
    path: Option<std::path::PathBuf>,
    embeddings: Option<String>,
    extractor_model: Option<String>,
    space: Option<String>,
    recall_spaces: Option<Vec<String>>,
    exclusive_predicates: Option<Vec<String>>,
    recall: Option<FileRecall>,
    extraction: Option<FileExtraction>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecall {
    seeds: Option<usize>,
    candidates: Option<usize>,
    hops: Option<usize>,
    max_neighbors: Option<usize>,
    max_visited: Option<usize>,
    expansion_budget_ms: Option<u64>,
    rrf_k: Option<f64>,
    weight_vector: Option<f64>,
    weight_relational: Option<f64>,
    weight_lexical: Option<f64>,
    max_results: Option<usize>,
    max_tokens: Option<usize>,
    passage_chars: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileExtraction {
    max_input_chars: Option<usize>,
    max_output_tokens: Option<u32>,
    max_output_chars: Option<usize>,
    timeout_ms: Option<u64>,
    max_facts: Option<usize>,
    max_attempts: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_file_values() {
        let c = MemoryConfig::from_bricks_toml("[context]\nx = 1\n").unwrap();
        assert_eq!(c, MemoryConfig::default());
        let c = MemoryConfig::from_bricks_toml(
            "[memory]\nspace = \"project:bricks\"\nrecall_spaces = [\"user:me\"]\n[memory.recall]\nseeds = 5\nrrf_k = 30\n",
        )
        .unwrap();
        assert_eq!(
            c.spaces(),
            vec!["project:bricks".to_string(), "user:me".to_string()]
        );
        assert_eq!((c.recall.seeds, c.recall.rrf_k), (5, 30.0));
    }

    #[test]
    fn the_documented_example_loads() {
        let text = include_str!("../../../../docs/bricks.example.toml");
        let c = MemoryConfig::from_bricks_toml(text).unwrap();
        assert_eq!(c.space, "project:bricks");
        let d = MemoryConfig::default();
        assert_eq!(c.recall, d.recall, "the example documents the defaults");
        assert_eq!(c.extraction, d.extraction);
        assert_eq!(c.exclusive_predicates, d.exclusive_predicates);
    }

    #[test]
    fn invalid_values_are_refused() {
        for (t, needle) in [
            ("[memory]\nspace = \"bricks\"\n", "must be"),
            ("[memory.recall]\nhops = 3\n", "hops"),
            ("[memory.recall]\nweight_lexical = -1\n", "weight_lexical"),
            (
                "[memory]\nexclusive_predicates = [\"Uses Framework\"]\n",
                "snake_case",
            ),
            ("[memory.recall]\nbogus = 1\n", "unknown field"),
            ("[memory]\nembeddings = \"word2vec\"\n", "memory.embeddings"),
            (
                "[memory]\nextractor_model = \"gpt\"\n",
                "provider_id/model_id",
            ),
        ] {
            let e = MemoryConfig::from_bricks_toml(t).unwrap_err();
            assert!(e.contains(needle), "{t}: {e}");
        }
    }
}
