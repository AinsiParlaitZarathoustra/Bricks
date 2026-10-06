//! `RunAgentTemplate` — the single tool surface through which an agent submits
//! and executes an AgentTemplate program.
//!
//! Backends are pulled from `ToolContext.extensions`:
//! - a [`DispatchHandle`] (required) — how builtins reach the underlying tools
//! - a `Mailbox` (optional) — for `agent.send`
//! - a `KvStore` (optional) — for `kv.*`
//! - an `Arc<dyn Sandbox>` (optional) — supplies this agent's identity

use crate::interp::{run_program, EvalCtx};
use crate::registry::DispatchHandle;
use async_trait::async_trait;
use cersei_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use cersei_vms::{KvStore, Mailbox, SandboxId};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct RunAgentTemplateTool;

#[async_trait]
impl Tool for RunAgentTemplateTool {
    fn name(&self) -> &str {
        "RunAgentTemplate"
    }

    fn description(&self) -> &str {
        "Execute an AgentTemplate program (a small functional DSL) on the Cersei runtime. \
         Use it to author and run a chain of io/net/agent operations. \
         Each builtin is permission-gated; parse and runtime errors are returned with line:col."
    }

    fn permission_level(&self) -> PermissionLevel {
        // The program may perform writes/exec; individual builtins are gated
        // again by the active policy during evaluation.
        PermissionLevel::Execute
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Orchestration
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "program": { "type": "string", "description": "The AgentTemplate source to execute" },
                "vars": { "type": "object", "description": "Optional initial variable bindings ($name -> value)" }
            },
            "required": ["program"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let program = match input.get("program").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return ToolResult::error("missing required field: program"),
        };

        let dispatch = match ctx.extensions.get::<DispatchHandle>() {
            Some(h) => h.0.clone(),
            None => {
                return ToolResult::error(
                    "RunAgentTemplate requires a DispatchHandle in the tool context extensions",
                )
            }
        };

        let mut ev = EvalCtx::new(ctx, dispatch);

        if let Some(mb) = ctx.extensions.get::<Mailbox>() {
            ev = ev.with_mailbox(Arc::new((*mb).clone()));
        }
        if let Some(kv) = ctx.extensions.get::<KvStore>() {
            ev = ev.with_kv(Arc::new((*kv).clone()));
        }
        if let Some(sb) = ctx.extensions.get::<Arc<dyn cersei_vms::Sandbox>>() {
            ev = ev.with_self_id(sb.id().clone());
        } else {
            ev = ev.with_self_id(SandboxId::from(ctx.session_id.as_str()));
        }

        if let Some(Value::Object(vars)) = input.get("vars") {
            for (k, v) in vars {
                ev.vars.insert(k.clone(), v.clone());
            }
        }

        match run_program(&program, &mut ev).await {
            Ok(value) => {
                let vars = Value::Object(
                    ev.vars
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                );
                let summary = match &value {
                    Value::Null => "program completed".to_string(),
                    other => format!("program result: {other}"),
                };
                ToolResult::success(summary).with_metadata(json!({ "value": value, "vars": vars }))
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}
