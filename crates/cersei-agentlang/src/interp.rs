//! Async evaluator for the AgentTemplate language.
//!
//! Values are `serde_json::Value` throughout (the workspace's universal tool-IO
//! type). Every side-effecting builtin passes through the permission policy in
//! [`ToolContext`] before it runs, and a step budget bounds total work.

use crate::ast::*;
use crate::builtins::{resolve, Op};
use crate::error::{ProgramError, RuntimeError, RuntimeErrorKind, Span};
use crate::parser::parse;
use crate::registry::ToolDispatch;
use cersei_tools::permissions::{PermissionDecision, PermissionRequest};
use cersei_tools::{PermissionLevel, ToolContext, ToolResult};
use cersei_vms::{KvStore, Mailbox, SandboxId};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Limits guarding a program run.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_steps: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_steps: 1000 }
    }
}

/// Evaluation context: variable bindings + handles to the runtime backends.
pub struct EvalCtx<'a> {
    pub vars: HashMap<String, Value>,
    pub tools: &'a ToolContext,
    pub dispatch: Arc<dyn ToolDispatch>,
    pub mailbox: Option<Arc<Mailbox>>,
    pub kv: Option<Arc<KvStore>>,
    pub self_id: SandboxId,
    pub limits: Limits,
    steps: u32,
}

impl<'a> EvalCtx<'a> {
    pub fn new(tools: &'a ToolContext, dispatch: Arc<dyn ToolDispatch>) -> Self {
        Self {
            vars: HashMap::new(),
            tools,
            dispatch,
            mailbox: None,
            kv: None,
            self_id: SandboxId("agentlang".to_string()),
            limits: Limits::default(),
            steps: 0,
        }
    }

    pub fn with_mailbox(mut self, mb: Arc<Mailbox>) -> Self {
        self.mailbox = Some(mb);
        self
    }

    pub fn with_kv(mut self, kv: Arc<KvStore>) -> Self {
        self.kv = Some(kv);
        self
    }

    pub fn with_self_id(mut self, id: SandboxId) -> Self {
        self.self_id = id;
        self
    }

    pub fn with_var(mut self, name: impl Into<String>, value: Value) -> Self {
        self.vars.insert(name.into(), value);
        self
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    fn tick(&mut self, span: Span) -> Result<(), RuntimeError> {
        self.steps += 1;
        if self.steps > self.limits.max_steps {
            return Err(RuntimeError::new(
                RuntimeErrorKind::StepLimitExceeded,
                format!("step limit ({}) exceeded", self.limits.max_steps),
                span,
            ));
        }
        Ok(())
    }

    fn eval_expr<'s>(
        &'s mut self,
        expr: &'s Expr,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 's>> {
        Box::pin(async move {
            match expr {
                Expr::Literal(lit) => self.eval_literal(lit).await,
                Expr::Var { name, span } => self.vars.get(name).cloned().ok_or_else(|| {
                    RuntimeError::new(
                        RuntimeErrorKind::UndefinedVar,
                        format!("undefined variable ${name}"),
                        *span,
                    )
                }),
                Expr::Chain(chain) => self.eval_chain(chain).await,
            }
        })
    }

    async fn eval_literal(&mut self, lit: &Literal) -> Result<Value, RuntimeError> {
        Ok(match lit {
            Literal::Str(s) => Value::String(s.clone()),
            Literal::Num(n) => serde_json::Number::from_f64(*n)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            Literal::Bool(b) => Value::Bool(*b),
            Literal::Null => Value::Null,
            Literal::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval_expr(it).await?);
                }
                Value::Array(out)
            }
        })
    }

    async fn eval_chain(&mut self, chain: &Chain) -> Result<Value, RuntimeError> {
        let mut value = self
            .eval_call(&chain.head.path, &chain.head.args, chain.head.span, None)
            .await?;
        // Chain tails inherit the head's namespace, e.g. `io.read(..).write(..)`
        // resolves the tail `write` as `io.write`.
        let ns = &chain.head.path[..chain.head.path.len().saturating_sub(1)];
        for tail in &chain.tail {
            let mut path = ns.to_vec();
            path.extend(tail.path.iter().cloned());
            // thread the previous result as $_ and as the pipe-target
            self.vars.insert("_".to_string(), value.clone());
            value = self
                .eval_call(&path, &tail.args, tail.span, Some(value))
                .await?;
        }
        Ok(value)
    }

    async fn eval_call(
        &mut self,
        path: &[String],
        args: &[Arg],
        span: Span,
        prev: Option<Value>,
    ) -> Result<Value, RuntimeError> {
        self.tick(span)?;
        let op = resolve(path).ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::UnknownBuiltin,
                format!("unknown builtin: {}", path.join(".")),
                span,
            )
        })?;

        let (pos, named) = self.eval_args(args).await?;

        match op {
            Op::Tool {
                name,
                positional,
                pipe_target,
                perm,
            } => {
                let input = build_tool_input(positional, &pos, &named, pipe_target, prev);
                self.gate(name, &input, perm, span).await?;
                let res = self.dispatch.call(name, input, self.tools).await;
                ok_or_tool_err(res, name, span)
            }
            Op::DeleteViaBash => {
                let path = first_string(&pos, prev.as_ref()).ok_or_else(|| {
                    RuntimeError::new(
                        RuntimeErrorKind::ArityMismatch,
                        "io.delete requires a file path",
                        span,
                    )
                })?;
                let input = serde_json::json!({ "command": format!("rm -- {path}") });
                self.gate("Bash", &input, PermissionLevel::Write, span)
                    .await?;
                let res = self.dispatch.call("Bash", input, self.tools).await;
                ok_or_tool_err(res, "io.delete", span)
            }
            Op::AgentSend => self.eval_agent_send(&pos, &named, span),
            Op::ToolsCall => self.eval_tools_call(&pos, &named, span).await,
            Op::ToolsList => Ok(Value::Array(
                self.dispatch
                    .list()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            )),
            Op::ToolsRegister => Err(RuntimeError::new(
                RuntimeErrorKind::Unsupported,
                "agent.tools.register requires the cersei-agentrl registry (not wired in this context)",
                span,
            )),
            Op::PermissionAsk => self.eval_permission_ask(&pos, span).await,
            Op::KvGet => self.eval_kv_get(&pos, span),
            Op::KvSet => self.eval_kv_set(&pos, span),
        }
    }

    async fn eval_args(
        &mut self,
        args: &[Arg],
    ) -> Result<(Vec<Value>, HashMap<String, Value>), RuntimeError> {
        let mut pos = Vec::new();
        let mut named = HashMap::new();
        for a in args {
            match a {
                Arg::Positional(e) => pos.push(self.eval_expr(e).await?),
                Arg::Named { name, value, .. } => {
                    let v = self.eval_expr(value).await?;
                    named.insert(name.clone(), v);
                }
            }
        }
        Ok((pos, named))
    }

    async fn gate(
        &self,
        tool_name: &str,
        input: &Value,
        perm: PermissionLevel,
        span: Span,
    ) -> Result<(), RuntimeError> {
        if perm == PermissionLevel::None {
            return Ok(());
        }
        let req = PermissionRequest {
            tool_name: tool_name.to_string(),
            tool_input: input.clone(),
            permission_level: perm,
            description: format!("agentlang: {tool_name}"),
            id: uuid_like(span),
            preview: None,
            agent_id: None,
        };
        match self.tools.permissions.check(&req).await {
            PermissionDecision::Allow
            | PermissionDecision::AllowOnce
            | PermissionDecision::AllowForSession => Ok(()),
            PermissionDecision::Deny(reason) => Err(RuntimeError::new(
                RuntimeErrorKind::PermissionDenied,
                reason,
                span,
            )),
        }
    }

    fn eval_agent_send(
        &self,
        pos: &[Value],
        named: &HashMap<String, Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        let mailbox = self.mailbox.as_ref().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::Unsupported,
                "agent.send requires a Mailbox (none in this context)",
                span,
            )
        })?;
        // topic = named `to` or first positional; payload = the remaining value
        let (topic, payload) = match named.get("to") {
            Some(t) => (
                value_as_string(t, span)?,
                pos.first().cloned().unwrap_or(Value::Null),
            ),
            None => {
                let t = pos.first().ok_or_else(|| {
                    RuntimeError::new(
                        RuntimeErrorKind::ArityMismatch,
                        "agent.send requires a recipient (to:) and a payload",
                        span,
                    )
                })?;
                (
                    value_as_string(t, span)?,
                    pos.get(1).cloned().unwrap_or(Value::Null),
                )
            }
        };
        let seq = mailbox
            .publish(topic, self.self_id.clone(), payload)
            .map_err(|e| {
                RuntimeError::new(RuntimeErrorKind::ToolError, format!("mailbox: {e}"), span)
            })?;
        Ok(Value::from(seq))
    }

    async fn eval_tools_call(
        &self,
        pos: &[Value],
        named: &HashMap<String, Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        let name = pos
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::ArityMismatch,
                    "agent.tools.call requires a tool name as the first argument",
                    span,
                )
            })?
            .to_string();
        // input = named args, plus an optional object as the second positional
        let mut map: Map<String, Value> = named.clone().into_iter().collect();
        if let Some(Value::Object(o)) = pos.get(1) {
            for (k, v) in o {
                map.insert(k.clone(), v.clone());
            }
        }
        let res = self
            .dispatch
            .call(&name, Value::Object(map), self.tools)
            .await;
        ok_or_tool_err(res, &name, span)
    }

    async fn eval_permission_ask(&self, pos: &[Value], span: Span) -> Result<Value, RuntimeError> {
        let mode = pos.first().and_then(|v| v.as_str()).unwrap_or("r");
        let perm = match mode {
            "r" => PermissionLevel::ReadOnly,
            "w" | "rw" | "wr" => PermissionLevel::Write,
            "x" => PermissionLevel::Execute,
            other => {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::TypeMismatch,
                    format!("unknown permission mode '{other}' (use r|w|rw|x)"),
                    span,
                ))
            }
        };
        let req = PermissionRequest {
            tool_name: "agent.permission.ask".to_string(),
            tool_input: Value::String(mode.to_string()),
            permission_level: perm,
            description: format!("agentlang: permission.ask('{mode}')"),
            id: uuid_like(span),
            preview: None,
            agent_id: None,
        };
        let granted = matches!(
            self.tools.permissions.check(&req).await,
            PermissionDecision::Allow
                | PermissionDecision::AllowOnce
                | PermissionDecision::AllowForSession
        );
        Ok(Value::Bool(granted))
    }

    fn eval_kv_get(&self, pos: &[Value], span: Span) -> Result<Value, RuntimeError> {
        let kv = self.require_kv(span)?;
        let key = pos.first().and_then(|v| v.as_str()).ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::ArityMismatch,
                "kv.get requires a key",
                span,
            )
        })?;
        Ok(match kv.get(key) {
            Some(entry) => bytes_to_value(&entry.value),
            None => Value::Null,
        })
    }

    fn eval_kv_set(&self, pos: &[Value], span: Span) -> Result<Value, RuntimeError> {
        let kv = self.require_kv(span)?;
        let key = pos
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::ArityMismatch,
                    "kv.set requires a key and a value",
                    span,
                )
            })?
            .to_string();
        let val = pos.get(1).cloned().unwrap_or(Value::Null);
        let bytes = serde_json::to_vec(&val).unwrap_or_default();
        let entry = kv.set(key, bytes).map_err(|e| {
            RuntimeError::new(RuntimeErrorKind::ToolError, format!("kv: {e}"), span)
        })?;
        Ok(Value::from(entry.version))
    }

    fn require_kv(&self, span: Span) -> Result<&Arc<KvStore>, RuntimeError> {
        self.kv.as_ref().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::Unsupported,
                "kv.* requires a KvStore (none in this context)",
                span,
            )
        })
    }
}

// ─── Free helpers ──────────────────────────────────────────────────────────

fn build_tool_input(
    positional_keys: &[&str],
    pos: &[Value],
    named: &HashMap<String, Value>,
    pipe_target: Option<&str>,
    prev: Option<Value>,
) -> Value {
    let mut map = Map::new();
    for (i, key) in positional_keys.iter().enumerate() {
        if let Some(v) = pos.get(i) {
            map.insert(key.to_string(), v.clone());
        }
    }
    for (k, v) in named {
        map.insert(k.clone(), v.clone());
    }
    if let (Some(pt), Some(p)) = (pipe_target, prev) {
        map.entry(pt.to_string()).or_insert(p);
    }
    Value::Object(map)
}

/// Coerce a tool result into a value, surfacing tool errors as runtime errors.
fn ok_or_tool_err(res: ToolResult, label: &str, span: Span) -> Result<Value, RuntimeError> {
    if res.is_error {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ToolError,
            format!("{label}: {}", res.content),
            span,
        ));
    }
    Ok(if !res.content.is_empty() {
        Value::String(res.content)
    } else {
        res.metadata.unwrap_or(Value::Null)
    })
}

fn first_string(pos: &[Value], prev: Option<&Value>) -> Option<String> {
    pos.first()
        .or(prev)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

fn value_as_string(v: &Value, span: Span) -> Result<String, RuntimeError> {
    v.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::TypeMismatch, "expected a string", span))
}

fn bytes_to_value(bytes: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => v,
        Err(_) => Value::String(String::from_utf8_lossy(bytes).to_string()),
    }
}

/// A deterministic, non-secret request id derived from the span (avoids pulling
/// randomness; the id only needs to be unique-ish within a run).
fn uuid_like(span: Span) -> String {
    format!("agentlang-{}-{}", span.line, span.col)
}

/// Parse and evaluate a program, returning the value of its last statement.
pub async fn run_program(src: &str, ctx: &mut EvalCtx<'_>) -> Result<Value, ProgramError> {
    let program = parse(src)?;
    let mut last = Value::Null;
    for stmt in &program.stmts {
        last = match stmt {
            Stmt::Assign { name, value, .. } => {
                let v = ctx.eval_expr(value).await?;
                ctx.vars.insert(name.clone(), v);
                Value::Null
            }
            Stmt::Expr(e) => ctx.eval_expr(e).await?,
        };
    }
    Ok(last)
}
