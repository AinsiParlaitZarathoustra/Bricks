//! `AgentStep` — wraps a `cersei_agent::Agent` as a workflow step.
//!
//! The node input is rendered into a prompt via a simple `{{field}}` template,
//! the agent runs to completion, and its output is mapped into a JSON object.

use crate::step::{Step, StepContext, StepOutcome};
use async_trait::async_trait;
use cersei_agent::Agent;
use cersei_types::Result;
use serde_json::{json, Value};
use std::sync::Arc;

/// A step that runs an agent. Output JSON: `{ text, turns, stop_reason }`.
pub struct AgentStep {
    id: String,
    agent: Arc<Agent>,
    /// Prompt template. `{{input}}` injects the whole input as a string;
    /// `{{field}}` injects a top-level input field.
    prompt_template: String,
}

impl AgentStep {
    pub fn new(
        id: impl Into<String>,
        agent: Arc<Agent>,
        prompt_template: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            agent,
            prompt_template: prompt_template.into(),
        }
    }

    fn render_prompt(&self, input: &Value) -> String {
        let mut out = self.prompt_template.clone();
        // Whole-input substitution.
        let whole = match input {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out = out.replace("{{input}}", &whole);
        // Per-field substitution for object inputs.
        if let Value::Object(map) = input {
            for (k, v) in map {
                let needle = format!("{{{{{}}}}}", k);
                let replacement = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                out = out.replace(&needle, &replacement);
            }
        }
        out
    }
}

#[async_trait]
impl Step for AgentStep {
    fn id(&self) -> &str {
        &self.id
    }

    fn description(&self) -> &str {
        "Runs a Cersei agent and returns its text output"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" },
                "turns": { "type": "integer" },
                "stop_reason": { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: &StepContext) -> Result<StepOutcome> {
        let prompt = self.render_prompt(&input);
        let out = self.agent.run(&prompt).await?;
        Ok(StepOutcome::Done(json!({
            "text": out.text(),
            "turns": out.turns,
            "stop_reason": format!("{:?}", out.stop_reason),
        })))
    }
}
