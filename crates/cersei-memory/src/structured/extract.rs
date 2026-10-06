//! Extraction of entities and facts from an episode.
//!
//! An [`Extractor`] returns raw JSON; [`validate`] turns it into candidates
//! or refuses it. Validation is strict and the same for every extractor:
//!
//! * the output must be a JSON object `{ "entities": [...], "facts": [...] }`
//!   within `max_output_chars`;
//! * every fact must quote the episode **verbatim** (`quote` is a substring
//!   of the episode): a fact without evidence is dropped, with a note;
//! * predicates are normalised to `snake_case`; dates must parse
//!   (`YYYY-MM-DD` or RFC 3339) or are treated as unknown, never guessed;
//! * at most `max_facts` facts are kept.
//!
//! [`LlmExtractor`] calls a configured provider through the provider
//! abstraction (`cersei_provider::Provider`): no model is hard-coded, the
//! call has a timeout and output limit, and can be cancelled.

use super::clock::parse_time;
use super::config::ExtractionConfig;
use super::model::{normalize_name, normalize_predicate, Millis};
use async_trait::async_trait;
use cersei_provider::{CompletionRequest, Provider};
use cersei_types::Message;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// What an extractor sees.
#[derive(Debug, Clone)]
pub struct ExtractionRequest {
    pub episode_id: String,
    pub space: String,
    pub role: String,
    /// Possibly cut to `max_input_chars` (see `truncated`).
    pub content: String,
    pub truncated: bool,
    /// Reference date for relative expressions ("yesterday"), when known.
    pub reference_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExtractError {
    #[error("the extractor timed out")]
    Timeout,
    #[error("the extraction was cancelled")]
    Cancelled,
    #[error("the extractor failed: {0}")]
    Provider(String),
    #[error("malformed extraction: {0}")]
    Malformed(String),
}

impl ExtractError {
    /// Worth another attempt later (not a malformed answer to the same input).
    pub fn retryable(&self) -> bool {
        !matches!(self, Self::Cancelled)
    }
}

#[async_trait]
pub trait Extractor: Send + Sync {
    /// Identifies the extractor in provenance (`provider/model`).
    fn id(&self) -> String;

    /// Raw JSON output for one episode.
    async fn extract(
        &self,
        request: &ExtractionRequest,
        cancel: &CancellationToken,
    ) -> Result<String, ExtractError>;
}

// ─── Candidates ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateEntity {
    /// Reference used by facts of the same output (`e1`).
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateFact {
    /// Entity reference of the subject.
    pub subject: String,
    pub predicate: String,
    /// The value, as text.
    pub value: String,
    /// Entity reference when the value is an entity.
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub negated: bool,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_until: Option<String>,
    /// The episode states an explicit change of this value ("moves from X
    /// to Y", "switched to", "no longer").
    #[serde(default)]
    pub explicit_change: bool,
    /// Verbatim excerpt of the episode supporting the fact.
    pub quote: String,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Candidates {
    #[serde(default)]
    pub entities: Vec<CandidateEntity>,
    #[serde(default)]
    pub facts: Vec<CandidateFact>,
}

/// A validated fact: references resolved to candidate entities, predicate
/// normalised, dates parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidFact {
    pub subject: CandidateEntity,
    pub predicate: String,
    pub value: String,
    pub object: Option<CandidateEntity>,
    pub negated: bool,
    pub valid_from: Option<Millis>,
    pub valid_until: Option<Millis>,
    pub explicit_change: bool,
    pub quote: String,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Validated {
    pub entities: Vec<CandidateEntity>,
    pub facts: Vec<ValidFact>,
    /// Why some candidates were dropped.
    pub notes: Vec<String>,
}

/// Parse and check an extractor's output against the episode it came from.
pub fn validate(
    raw: &str,
    episode: &str,
    cfg: &ExtractionConfig,
) -> Result<Validated, ExtractError> {
    if raw.len() > cfg.max_output_chars {
        return Err(ExtractError::Malformed(format!(
            "output of {} characters exceeds the limit of {}",
            raw.len(),
            cfg.max_output_chars
        )));
    }
    let json = strip_fences(raw);
    let c: Candidates = serde_json::from_str(json)
        .map_err(|e| ExtractError::Malformed(format!("not the expected JSON object: {e}")))?;
    let mut v = Validated::default();
    let mut entities: Vec<CandidateEntity> = Vec::new();
    for e in c.entities {
        let name = e.name.trim().to_string();
        if name.is_empty() || name.chars().count() > 200 || e.reference.trim().is_empty() {
            v.notes.push(format!(
                "entity `{}` dropped: empty or too long name",
                e.reference
            ));
            continue;
        }
        if entities.iter().any(|x| x.reference == e.reference) {
            v.notes
                .push(format!("entity reference `{}` repeated", e.reference));
            continue;
        }
        let mut aliases: Vec<String> = e
            .aliases
            .into_iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty() && a.chars().count() <= 200)
            .collect();
        aliases.dedup();
        entities.push(CandidateEntity {
            reference: e.reference,
            name,
            kind: normalize_predicate(&e.kind).unwrap_or_else(|| "thing".into()),
            aliases,
        });
    }
    for f in c.facts {
        if v.facts.len() >= cfg.max_facts {
            v.notes
                .push(format!("facts beyond {} dropped", cfg.max_facts));
            break;
        }
        let quote = f.quote.trim();
        if quote.is_empty() || !episode.contains(quote) {
            v.notes.push(format!(
                "fact `{} {}` dropped: its quote is not a verbatim excerpt of the episode",
                f.subject, f.predicate
            ));
            continue;
        }
        let Some(subject) = entities.iter().find(|e| e.reference == f.subject).cloned() else {
            v.notes
                .push(format!("fact dropped: unknown subject `{}`", f.subject));
            continue;
        };
        let Some(predicate) = normalize_predicate(&f.predicate) else {
            v.notes
                .push(format!("fact dropped: invalid predicate `{}`", f.predicate));
            continue;
        };
        let value = f.value.trim().to_string();
        if value.is_empty() || value.chars().count() > 500 {
            v.notes.push(format!(
                "fact `{predicate}` dropped: empty or too long value"
            ));
            continue;
        }
        let object = match &f.object {
            Some(r) => match entities.iter().find(|e| &e.reference == r) {
                Some(e) => Some(e.clone()),
                None => {
                    v.notes
                        .push(format!("unknown object `{r}` ignored (value kept as text)"));
                    None
                }
            },
            None => None,
        };
        let date = |d: &Option<String>, which: &str, notes: &mut Vec<String>| {
            d.as_deref().filter(|s| !s.trim().is_empty()).and_then(|s| {
                let t = parse_time(s);
                if t.is_none() {
                    notes.push(format!(
                        "{which} `{s}` of `{predicate}` is not a date: unknown"
                    ));
                }
                t
            })
        };
        let valid_from = date(&f.valid_from, "valid_from", &mut v.notes);
        let valid_until = date(&f.valid_until, "valid_until", &mut v.notes);
        if let (Some(a), Some(b)) = (valid_from, valid_until) {
            if b <= a {
                v.notes.push(format!(
                    "fact `{predicate}` dropped: validity ends before it starts"
                ));
                continue;
            }
        }
        let confidence = f
            .confidence
            .filter(|c| c.is_finite())
            .map(|c| c.clamp(0.0, 1.0));
        v.facts.push(ValidFact {
            subject,
            predicate,
            value,
            object,
            negated: f.negated,
            valid_from,
            valid_until,
            explicit_change: f.explicit_change,
            quote: quote.to_string(),
            confidence,
        });
    }
    v.entities = entities;
    Ok(v)
}

fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

/// Cut `content` to `max` characters (on a character boundary).
pub fn bounded(content: &str, max: usize) -> (String, bool) {
    if content.chars().count() <= max {
        return (content.to_string(), false);
    }
    (content.chars().take(max).collect(), true)
}

// ─── LLM extractor ───────────────────────────────────────────────────────────

const SYSTEM: &str = r#"You extract durable facts from one message for a long-term memory.

Return ONLY a JSON object:
{"entities":[{"ref":"e1","name":"...","type":"person|project|library|place|organization|thing","aliases":["..."]}],
 "facts":[{"subject":"e1","predicate":"snake_case_property","value":"...","object":"e2 or null",
           "negated":false,"valid_from":"YYYY-MM-DD or null","valid_until":"YYYY-MM-DD or null",
           "explicit_change":false,"quote":"exact excerpt of the message","confidence":0.0-1.0}]}

Rules:
- Only facts the message states. Do not infer, guess or generalise.
- "quote" must be copied exactly from the message (same characters).
- Dates: only when stated, or derivable from the reference date; otherwise null.
- "explicit_change": true only when the message says a value changed
  (e.g. "moved from A to B", "switched to", "no longer", "passe de A à B").
- "negated": true when the message says something is NOT the case.
- Use stable, generic predicates (uses_framework, lives_in, works_at,
  prefers, owns, deadline, language...). One fact per property.
- No facts: {"entities":[],"facts":[]}"#;

/// Extraction with a configured provider.
pub struct LlmExtractor {
    provider: Arc<dyn Provider>,
    model: String,
    cfg: ExtractionConfig,
}

impl LlmExtractor {
    /// `model` is the label sent with the request (the provider is bound to
    /// its configured model; see `cersei_provider`).
    pub fn new(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        cfg: ExtractionConfig,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            cfg,
        }
    }
}

#[async_trait]
impl Extractor for LlmExtractor {
    fn id(&self) -> String {
        format!("{}/{}", self.provider.name(), self.model)
    }

    async fn extract(
        &self,
        req: &ExtractionRequest,
        cancel: &CancellationToken,
    ) -> Result<String, ExtractError> {
        let mut request = CompletionRequest::new(self.model.clone());
        request.system = Some(SYSTEM.to_string());
        request.max_tokens = self.cfg.max_output_tokens;
        request.temperature = Some(0.0);
        let header = format!(
            "Role: {}\nSpace: {}\nReference date: {}{}\n\nMessage:\n",
            req.role,
            req.space,
            req.reference_date.as_deref().unwrap_or("unknown"),
            if req.truncated {
                "\n(The message was cut; extract only from the part shown.)"
            } else {
                ""
            }
        );
        request.messages = vec![Message::user(format!("{header}{}", req.content))];
        let call = async {
            let r = self
                .provider
                .complete_blocking(request)
                .await
                .map_err(|e| ExtractError::Provider(e.to_string()))?;
            Ok::<String, ExtractError>(r.message.get_text().unwrap_or_default().to_string())
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(ExtractError::Cancelled),
            r = tokio::time::timeout(self.cfg.timeout, call) => match r {
                Ok(r) => r,
                Err(_) => Err(ExtractError::Timeout),
            },
        }
    }
}

/// Entities and facts are matched by normalised name within a space.
pub fn norm(name: &str) -> String {
    normalize_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ExtractionConfig {
        ExtractionConfig::default()
    }

    const EP: &str = "Le projet Bricks passe d'Axum à Actix depuis le 2026-03-01.";

    #[test]
    fn valid_output_is_checked_against_the_episode() {
        let raw = r#"```json
{"entities":[{"ref":"e1","name":"Bricks","type":"Project","aliases":["bricks-rs"]},{"ref":"e2","name":"Actix","type":"library"}],
 "facts":[
  {"subject":"e1","predicate":"Uses Framework","value":"Actix","object":"e2","valid_from":"2026-03-01","explicit_change":true,"quote":"passe d'Axum à Actix","confidence":1.4},
  {"subject":"e1","predicate":"license","value":"MIT","quote":"MIT licensed"},
  {"subject":"e9","predicate":"x","value":"y","quote":"Bricks"},
  {"subject":"e1","predicate":"since","value":"x","valid_from":"last spring","quote":"depuis"}
 ]}
```"#;
        let v = validate(raw, EP, &cfg()).unwrap();
        assert_eq!(v.facts.len(), 2, "{v:#?}");
        let f = &v.facts[0];
        assert_eq!(f.predicate, "uses_framework");
        assert_eq!(f.object.as_ref().unwrap().name, "Actix");
        assert!(f.explicit_change);
        assert_eq!(f.confidence, Some(1.0));
        assert!(f.valid_from.is_some());
        assert_eq!(
            v.facts[1].valid_from, None,
            "an unparseable date is unknown"
        );
        assert!(v.notes.iter().any(|n| n.contains("not a verbatim excerpt")));
        assert!(v.notes.iter().any(|n| n.contains("unknown subject")));
        assert!(v.notes.iter().any(|n| n.contains("last spring")));
        assert_eq!(v.entities[0].kind, "project");
    }

    #[test]
    fn malformed_outputs_are_errors_not_panics() {
        for raw in [
            "",
            "not json",
            "[1,2]",
            "{\"facts\": 3}",
            "{\"facts\":[{\"subject\":1}]}",
        ] {
            assert!(
                matches!(validate(raw, EP, &cfg()), Err(ExtractError::Malformed(_))),
                "{raw}"
            );
        }
        let mut c = cfg();
        c.max_output_chars = 10;
        assert!(matches!(
            validate("{\"entities\":[],\"facts\":[]}", EP, &c),
            Err(ExtractError::Malformed(m)) if m.contains("exceeds")
        ));
        assert_eq!(validate("{}", EP, &cfg()).unwrap(), Validated::default());
    }

    /// A provider that answers after `delay` with `text`, recording requests.
    struct FakeProvider {
        text: String,
        delay: std::time::Duration,
        seen: parking_lot::Mutex<Vec<CompletionRequest>>,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn name(&self) -> &str {
            "fake"
        }
        fn context_window(&self, _: &str) -> u64 {
            100_000
        }
        async fn complete(
            &self,
            _: CompletionRequest,
        ) -> cersei_types::Result<cersei_provider::CompletionStream> {
            Err(cersei_types::CerseiError::Provider(
                "streaming not used".into(),
            ))
        }
        async fn complete_blocking(
            &self,
            request: CompletionRequest,
        ) -> cersei_types::Result<cersei_provider::CompletionResponse> {
            self.seen.lock().push(request);
            tokio::time::sleep(self.delay).await;
            Ok(cersei_provider::CompletionResponse {
                message: Message::assistant(self.text.clone()),
                usage: Default::default(),
                stop_reason: cersei_types::StopReason::EndTurn,
            })
        }
    }

    fn request() -> ExtractionRequest {
        ExtractionRequest {
            episode_id: "ep".into(),
            space: "project:x".into(),
            role: "user".into(),
            content: EP.into(),
            truncated: false,
            reference_date: Some("2026-03-01".into()),
        }
    }

    #[tokio::test]
    async fn the_llm_extractor_uses_the_configured_provider_with_limits() {
        let fake = Arc::new(FakeProvider {
            text: "{\"entities\":[],\"facts\":[]}".into(),
            delay: std::time::Duration::ZERO,
            seen: Default::default(),
        });
        let x = LlmExtractor::new(fake.clone(), "any-configured-model", cfg());
        assert_eq!(x.id(), "fake/any-configured-model");
        let out = x
            .extract(&request(), &CancellationToken::new())
            .await
            .unwrap();
        assert!(validate(&out, EP, &cfg()).is_ok());
        let sent = fake.seen.lock()[0].clone();
        assert_eq!(sent.model, "any-configured-model");
        assert_eq!(sent.max_tokens, cfg().max_output_tokens);
        assert_eq!(sent.temperature, Some(0.0));
        let user = sent.messages[0].get_text().unwrap().to_string();
        assert!(
            user.contains("Reference date: 2026-03-01") && user.contains(EP),
            "{user}"
        );

        // Timeout and cancellation are errors, not hangs.
        let slow = Arc::new(FakeProvider {
            text: "{}".into(),
            delay: std::time::Duration::from_secs(5),
            seen: Default::default(),
        });
        let mut short = cfg();
        short.timeout = std::time::Duration::from_millis(50);
        let x = LlmExtractor::new(slow.clone(), "m", short);
        assert_eq!(
            x.extract(&request(), &CancellationToken::new()).await,
            Err(ExtractError::Timeout)
        );
        let token = CancellationToken::new();
        token.cancel();
        let x = LlmExtractor::new(slow, "m", cfg());
        assert_eq!(
            x.extract(&request(), &token).await,
            Err(ExtractError::Cancelled)
        );
    }

    #[test]
    fn inputs_are_bounded_on_characters() {
        let (s, cut) = bounded("ééééé", 3);
        assert_eq!((s.as_str(), cut), ("ééé", true));
    }
}
