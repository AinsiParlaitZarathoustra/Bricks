//! The dispatch seam between the interpreter and the underlying tools.
//!
//! The interpreter never constructs concrete tool structs; it dispatches by
//! tool *name* through [`ToolDispatch`]. This keeps the language decoupled from
//! both the built-in tool set and the (separately-built) dynamic tool registry
//! in `bricks-agentrl`, which supplies its own `ToolDispatch` impl.

use async_trait::async_trait;
use bricks_tools::{Tool, ToolContext, ToolResult};
use std::collections::HashMap;
use std::sync::Arc;

/// Wrapper so a `ToolDispatch` trait object can be stored in (and retrieved
/// from) `ToolContext.extensions`, which is keyed by concrete `Sized` type.
/// Insert `DispatchHandle(my_dispatch)`; read it back with
/// `ctx.extensions.get::<DispatchHandle>()`.
#[derive(Clone)]
pub struct DispatchHandle(pub Arc<dyn ToolDispatch>);

#[async_trait]
pub trait ToolDispatch: Send + Sync {
    /// Resolve and execute a tool by its canonical name (e.g. "Read").
    async fn call(&self, name: &str, input: serde_json::Value, ctx: &ToolContext) -> ToolResult;
    /// Whether a tool with this name is dispatchable.
    fn has(&self, name: &str) -> bool;
    /// All dispatchable tool names.
    fn list(&self) -> Vec<String>;
}

/// A `ToolDispatch` backed by a fixed set of `Tool` trait objects — the common
/// case for an agent's built-in toolset and for tests.
pub struct VecToolDispatch {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl VecToolDispatch {
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        let map = tools
            .into_iter()
            .map(|t| (t.name().to_string(), t))
            .collect();
        Self { tools: map }
    }

    /// Convenience constructor from owned boxes (e.g. `bricks_tools::all()`).
    pub fn from_boxed(tools: Vec<Box<dyn Tool>>) -> Self {
        let map = tools
            .into_iter()
            .map(|t| (t.name().to_string(), Arc::from(t)))
            .collect();
        Self { tools: map }
    }
}

#[async_trait]
impl ToolDispatch for VecToolDispatch {
    async fn call(&self, name: &str, input: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        match self.tools.get(name) {
            Some(t) => t.execute(input, ctx).await,
            None => ToolResult::error(format!("no such tool: {name}")),
        }
    }

    fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tools.keys().cloned().collect();
        v.sort();
        v
    }
}
