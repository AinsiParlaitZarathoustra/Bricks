//! `FnStep` — a step backed by an async closure. The `createStep` primitive.

use crate::step::{Step, StepContext, StepOutcome};
use async_trait::async_trait;
use bricks_types::Result;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

type StepFn = Box<
    dyn Fn(Value, StepContext) -> Pin<Box<dyn Future<Output = Result<StepOutcome>> + Send>>
        + Send
        + Sync,
>;

/// A step whose logic is an arbitrary async closure.
pub struct FnStep {
    id: String,
    description: String,
    input_schema: Value,
    output_schema: Value,
    f: StepFn,
}

impl FnStep {
    /// Build a step from a closure returning a plain `Value` output.
    pub fn new<F, Fut>(id: impl Into<String>, f: F) -> Self
    where
        F: Fn(Value, StepContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        let f: StepFn = Box::new(move |input, ctx| {
            let fut = f(input, ctx);
            Box::pin(async move { fut.await.map(StepOutcome::Done) })
        });
        Self {
            id: id.into(),
            description: String::new(),
            input_schema: Value::Null,
            output_schema: Value::Null,
            f,
        }
    }

    /// Build a step from a closure returning a full [`StepOutcome`] (for suspend).
    pub fn with_outcome<F, Fut>(id: impl Into<String>, f: F) -> Self
    where
        F: Fn(Value, StepContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<StepOutcome>> + Send + 'static,
    {
        let f: StepFn = Box::new(move |input, ctx| Box::pin(f(input, ctx)));
        Self {
            id: id.into(),
            description: String::new(),
            input_schema: Value::Null,
            output_schema: Value::Null,
            f,
        }
    }

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    pub fn input_schema(mut self, s: Value) -> Self {
        self.input_schema = s;
        self
    }

    pub fn output_schema(mut self, s: Value) -> Self {
        self.output_schema = s;
        self
    }
}

#[async_trait]
impl Step for FnStep {
    fn id(&self) -> &str {
        &self.id
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn output_schema(&self) -> Value {
        self.output_schema.clone()
    }

    async fn execute(&self, input: Value, ctx: &StepContext) -> Result<StepOutcome> {
        (self.f)(input, ctx.clone()).await
    }
}
