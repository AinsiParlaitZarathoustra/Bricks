//! Serializable branch conditions.
//!
//! A small, side-effect-free predicate enum that an xyflow form can build
//! directly. Paths are JSON Pointers (`/steps/foo/value`) resolved against the
//! run scope `{ input, state, steps: { <node_id>: <output> } }`.
//!
//! We deliberately do NOT reuse `cersei-agentlang`'s `Expr` here: it is a parsed
//! AST that is not `Serialize`/`Deserialize`, so it cannot round-trip to the UI.
//! A future `Condition::Expr(String)` variant behind an `agentlang-conditions`
//! feature can offer the full language as an opt-in escape hatch.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A serializable predicate over the workflow run scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// Always true.
    Always,
    /// `scope[path] == value`.
    Eq {
        path: String,
        value: Value,
    },
    /// `scope[path] != value`.
    Ne {
        path: String,
        value: Value,
    },
    /// `scope[path] > value` (numeric).
    Gt {
        path: String,
        value: f64,
    },
    /// `scope[path] < value` (numeric).
    Lt {
        path: String,
        value: f64,
    },
    /// The path resolves to a value (not missing, not null).
    Exists {
        path: String,
    },
    /// The path resolves to a truthy value (`true`, non-zero, non-empty).
    Truthy {
        path: String,
    },
    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),
}

impl Condition {
    /// Evaluate against a scope `Value`. Missing paths evaluate to absent.
    pub fn eval(&self, scope: &Value) -> bool {
        match self {
            Condition::Always => true,
            Condition::Eq { path, value } => {
                lookup(scope, path).map(|v| v == value).unwrap_or(false)
            }
            Condition::Ne { path, value } => {
                lookup(scope, path).map(|v| v != value).unwrap_or(true)
            }
            Condition::Gt { path, value } => lookup(scope, path)
                .and_then(as_f64)
                .map(|n| n > *value)
                .unwrap_or(false),
            Condition::Lt { path, value } => lookup(scope, path)
                .and_then(as_f64)
                .map(|n| n < *value)
                .unwrap_or(false),
            Condition::Exists { path } => {
                matches!(lookup(scope, path), Some(v) if !v.is_null())
            }
            Condition::Truthy { path } => lookup(scope, path).map(is_truthy).unwrap_or(false),
            Condition::And(cs) => cs.iter().all(|c| c.eval(scope)),
            Condition::Or(cs) => cs.iter().any(|c| c.eval(scope)),
            Condition::Not(c) => !c.eval(scope),
        }
    }
}

fn lookup<'a>(scope: &'a Value, path: &str) -> Option<&'a Value> {
    // Accept both leading-slash JSON Pointers and bare dotted-free pointers.
    let ptr = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    scope.pointer(&ptr)
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
