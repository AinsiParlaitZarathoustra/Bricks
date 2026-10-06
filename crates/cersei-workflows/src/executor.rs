//! The executor: a memoized, recursive DAG walker.
//!
//! Execution starts at `def.entry` and follows edges by kind. Outputs are
//! memoized per node (so diamonds and branch reconvergence run each node once),
//! which also makes suspend/resume a snapshot replay: preload the cache, re-walk
//! from the entry, and every already-computed node short-circuits until the
//! suspended node runs with its resume data.
//!
//! `walk` returns `Result<Option<Value>>`: `Some(v)` is a value, `None` means the
//! run suspended and the call stack should unwind.

use crate::compile::Workflow;
use crate::events::{WorkflowControl, WorkflowEvent, WorkflowStream};
use crate::ir::{EdgeKind, JoinStrategy, LoopMode, NodeId, NodeKind};
use crate::result::{RunStatus, StepResult, SuspendPoint, WorkflowResult};
use crate::step::{StepContext, StepOutcome};
use crate::store::RunSnapshot;
use cersei_tools::Extensions;
use cersei_types::{CerseiError, Result};
use dashmap::DashMap;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

const MAX_LOOP_ITERS: u32 = 10_000;

/// Per-run mutable state, shared (by `&`) across the recursive walk.
struct RunCtx {
    run_id: String,
    input: Value,
    state: Arc<Mutex<Value>>,
    outputs: DashMap<NodeId, Value>,
    results: DashMap<NodeId, StepResult>,
    events: mpsc::Sender<WorkflowEvent>,
    extensions: Extensions,
    /// (node_id, data) for the single node being resumed this run.
    resume: Option<(NodeId, Value)>,
    /// Set when a step suspends; halts further scheduling.
    suspend: Mutex<Option<SuspendPoint>>,
}

type WalkFut<'a> = Pin<Box<dyn Future<Output = Result<Option<Value>>> + Send + 'a>>;

impl Workflow {
    /// Run to completion (or terminal failure/suspension), returning the result.
    pub async fn start(&self, input: Value) -> Result<WorkflowResult> {
        let (tx, mut rx) = mpsc::channel::<WorkflowEvent>(1024);
        // Drain events so senders never block.
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let res = self
            .run_inner(new_run_id(), input, tx, None, HashMap::new())
            .await;
        let _ = drain.await;
        Ok(res)
    }

    /// Run with a live event stream for UI consumption.
    pub fn stream(self: &Arc<Self>, input: Value) -> WorkflowStream {
        let (event_tx, event_rx) = mpsc::channel::<WorkflowEvent>(1024);
        let (control_tx, mut control_rx) = mpsc::channel::<WorkflowControl>(64);
        let wf = Arc::clone(self);
        tokio::spawn(async move {
            // Drain control messages (cancel/resume) — minimal handling in MVP.
            tokio::spawn(async move { while control_rx.recv().await.is_some() {} });
            let result = wf
                .run_inner(new_run_id(), input, event_tx.clone(), None, HashMap::new())
                .await;
            let _ = event_tx
                .send(WorkflowEvent::WorkflowCompleted {
                    result: Box::new(result),
                })
                .await;
        });
        WorkflowStream::new(event_rx, control_tx)
    }

    /// Resume a suspended run from a stored snapshot.
    pub async fn resume(
        &self,
        run_id: &str,
        node_id: &NodeId,
        data: Value,
    ) -> Result<WorkflowResult> {
        let snapshot = self
            .store
            .load(run_id)
            .ok_or_else(|| CerseiError::Config(format!("no suspended run '{}'", run_id)))?;
        let (tx, mut rx) = mpsc::channel::<WorkflowEvent>(1024);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        // Restore shared state from the snapshot before re-walking.
        let res = self
            .run_inner_with_state(
                run_id.to_string(),
                snapshot.input,
                snapshot.state,
                tx,
                Some((node_id.clone(), data)),
                snapshot.outputs,
            )
            .await;
        let _ = drain.await;
        Ok(res)
    }

    async fn run_inner(
        &self,
        run_id: String,
        input: Value,
        events: mpsc::Sender<WorkflowEvent>,
        resume: Option<(NodeId, Value)>,
        preload: HashMap<NodeId, Value>,
    ) -> WorkflowResult {
        self.run_inner_with_state(run_id, input, json!({}), events, resume, preload)
            .await
    }

    async fn run_inner_with_state(
        &self,
        run_id: String,
        input: Value,
        state: Value,
        events: mpsc::Sender<WorkflowEvent>,
        resume: Option<(NodeId, Value)>,
        preload: HashMap<NodeId, Value>,
    ) -> WorkflowResult {
        let outputs: DashMap<NodeId, Value> = preload.into_iter().collect();
        let rctx = RunCtx {
            run_id: run_id.clone(),
            input: input.clone(),
            state: Arc::new(Mutex::new(state)),
            outputs,
            results: DashMap::new(),
            events: events.clone(),
            extensions: Extensions::default(),
            resume,
            suspend: Mutex::new(None),
        };

        let _ = events
            .send(WorkflowEvent::WorkflowStarted {
                run_id: run_id.clone(),
            })
            .await;

        let entry = self.def.entry.clone();
        let walk_res = self.walk(&rctx, &entry, input.clone(), None).await;

        self.assemble_result(&rctx, walk_res)
    }

    fn assemble_result(&self, rctx: &RunCtx, walk_res: Result<Option<Value>>) -> WorkflowResult {
        let steps: HashMap<NodeId, StepResult> = rctx
            .results
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        let state = rctx.state.lock().clone();

        let mut result = WorkflowResult {
            run_id: rctx.run_id.clone(),
            status: RunStatus::Success,
            input: rctx.input.clone(),
            steps,
            result: None,
            state: state.clone(),
            error: None,
            suspended: Vec::new(),
        };

        match walk_res {
            Ok(Some(output)) => {
                result.status = RunStatus::Success;
                result.result = Some(output);
            }
            Ok(None) => {
                // Suspended — persist a snapshot for resume.
                result.status = RunStatus::Suspended;
                if let Some(sp) = rctx.suspend.lock().take() {
                    result.suspended.push(sp);
                }
                let outputs: HashMap<NodeId, Value> = rctx
                    .outputs
                    .iter()
                    .map(|e| (e.key().clone(), e.value().clone()))
                    .collect();
                self.store.save(RunSnapshot {
                    run_id: rctx.run_id.clone(),
                    input: rctx.input.clone(),
                    state,
                    outputs,
                });
            }
            Err(e) => {
                result.status = RunStatus::Failed;
                result.error = Some(e.to_string());
            }
        }
        result
    }

    /// Recursively execute starting at `node_id`. `stop_at` bounds parallel fork
    /// branches at their join node.
    fn walk<'a>(
        &'a self,
        rctx: &'a RunCtx,
        node_id: &'a str,
        input: Value,
        stop_at: Option<&'a str>,
    ) -> WalkFut<'a> {
        Box::pin(async move {
            if Some(node_id) == stop_at {
                return Ok(Some(input));
            }
            if rctx.suspend.lock().is_some() {
                return Ok(None);
            }
            let node = match self.def.node(node_id) {
                Some(n) => n,
                None => return Err(CerseiError::Config(format!("missing node '{}'", node_id))),
            };

            match &node.kind {
                NodeKind::Step { step_id, config } => {
                    self.exec_step(rctx, node_id, step_id, config, input, stop_at)
                        .await
                }
                NodeKind::Map { mapping } => {
                    let scope = self.build_scope(rctx, &input);
                    let mut obj = Map::new();
                    for (field, ptr) in &mapping.fields {
                        let val = scope
                            .pointer(&normalize_ptr(ptr))
                            .cloned()
                            .unwrap_or(Value::Null);
                        obj.insert(field.clone(), val);
                    }
                    let output = Value::Object(obj);
                    rctx.outputs.insert(node_id.to_string(), output.clone());
                    self.continue_from(rctx, node_id, output, stop_at).await
                }
                NodeKind::Parallel => self.exec_parallel(rctx, node_id, input, stop_at).await,
                NodeKind::Join { .. } => {
                    // Reached directly (e.g. branch reconvergence): passthrough.
                    rctx.outputs.insert(node_id.to_string(), input.clone());
                    self.continue_from(rctx, node_id, input, stop_at).await
                }
                NodeKind::Branch => self.exec_branch(rctx, node_id, input, stop_at).await,
                NodeKind::Loop {
                    mode,
                    body,
                    condition,
                } => {
                    self.exec_loop(
                        rctx,
                        node_id,
                        body,
                        mode,
                        condition.as_ref(),
                        input,
                        stop_at,
                    )
                    .await
                }
            }
        })
    }

    async fn exec_step(
        &self,
        rctx: &RunCtx,
        node_id: &str,
        step_id: &str,
        config: &Value,
        input: Value,
        stop_at: Option<&str>,
    ) -> Result<Option<Value>> {
        let is_resume_target = rctx
            .resume
            .as_ref()
            .map(|(n, _)| n == node_id)
            .unwrap_or(false);

        // Memoized replay: a completed, non-target node returns its cached output.
        if !is_resume_target {
            if let Some(cached) = rctx.outputs.get(node_id) {
                let out = cached.clone();
                return self.continue_from(rctx, node_id, out, stop_at).await;
            }
        }

        let step = self
            .steps
            .get(step_id)
            .ok_or_else(|| CerseiError::Tool(format!("unknown step: '{}'", step_id)))?;

        let effective_input = merge_config(config, input);
        let _ = rctx
            .events
            .send(WorkflowEvent::StepStarted {
                node_id: node_id.to_string(),
                step_id: step_id.to_string(),
                input: effective_input.clone(),
            })
            .await;

        let started = now_ms();
        let step_ctx = StepContext {
            run_id: rctx.run_id.clone(),
            state: Arc::clone(&rctx.state),
            events: rctx.events.clone(),
            extensions: rctx.extensions.clone(),
            resume_data: if is_resume_target {
                rctx.resume.as_ref().map(|(_, d)| d.clone())
            } else {
                None
            },
        };

        match step.execute(effective_input, &step_ctx).await {
            Ok(StepOutcome::Done(output)) => {
                rctx.outputs.insert(node_id.to_string(), output.clone());
                rctx.results.insert(
                    node_id.to_string(),
                    StepResult {
                        status: RunStatus::Success,
                        output: Some(output.clone()),
                        error: None,
                        started_at: started,
                        ended_at: Some(now_ms()),
                    },
                );
                let _ = rctx
                    .events
                    .send(WorkflowEvent::StepCompleted {
                        node_id: node_id.to_string(),
                        output: output.clone(),
                        duration_ms: (now_ms() - started).max(0) as u64,
                    })
                    .await;
                self.continue_from(rctx, node_id, output, stop_at).await
            }
            Ok(StepOutcome::Suspended {
                resume_schema,
                payload,
            }) => {
                rctx.results.insert(
                    node_id.to_string(),
                    StepResult {
                        status: RunStatus::Suspended,
                        output: None,
                        error: None,
                        started_at: started,
                        ended_at: None,
                    },
                );
                *rctx.suspend.lock() = Some(SuspendPoint {
                    node_id: node_id.to_string(),
                    resume_schema: resume_schema.clone(),
                    payload,
                });
                let _ = rctx
                    .events
                    .send(WorkflowEvent::StepSuspended {
                        node_id: node_id.to_string(),
                        resume_schema,
                    })
                    .await;
                Ok(None)
            }
            Err(e) => {
                rctx.results.insert(
                    node_id.to_string(),
                    StepResult {
                        status: RunStatus::Failed,
                        output: None,
                        error: Some(e.to_string()),
                        started_at: started,
                        ended_at: Some(now_ms()),
                    },
                );
                let _ = rctx
                    .events
                    .send(WorkflowEvent::StepFailed {
                        node_id: node_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
                Err(e)
            }
        }
    }

    async fn exec_parallel(
        &self,
        rctx: &RunCtx,
        node_id: &str,
        input: Value,
        stop_at: Option<&str>,
    ) -> Result<Option<Value>> {
        let edges = self.outgoing.get(node_id).cloned().unwrap_or_default();
        let fork_targets: Vec<NodeId> = edges
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::Fork))
            .map(|e| e.to.clone())
            .collect();

        // The join is the common `Merge` target of the fork branches.
        let join = self.find_join(&fork_targets);
        let join_ref = join.as_deref();

        let strategy = join
            .as_ref()
            .and_then(|j| self.def.node(j))
            .and_then(|n| match &n.kind {
                NodeKind::Join { strategy } => Some(*strategy),
                _ => None,
            })
            .unwrap_or(JoinStrategy::AllOrFail);

        // Run every branch concurrently, each bounded at the join node.
        let futs: Vec<_> = fork_targets
            .iter()
            .map(|t| self.walk(rctx, t, input.clone(), join_ref))
            .collect();
        let results = futures::future::join_all(futs).await;

        let mut branch_outputs = Vec::with_capacity(results.len());
        for r in results {
            match r {
                Ok(Some(v)) => branch_outputs.push(v),
                Ok(None) => return Ok(None), // a branch suspended
                Err(e) => match strategy {
                    JoinStrategy::AllOrFail => return Err(e),
                    JoinStrategy::AllSettled => branch_outputs.push(Value::Null),
                },
            }
        }

        let assembled = Value::Array(branch_outputs);
        match join {
            Some(j) => {
                rctx.outputs.insert(j.clone(), assembled.clone());
                self.continue_from(rctx, &j, assembled, stop_at).await
            }
            None => Ok(Some(assembled)),
        }
    }

    async fn exec_branch(
        &self,
        rctx: &RunCtx,
        node_id: &str,
        input: Value,
        stop_at: Option<&str>,
    ) -> Result<Option<Value>> {
        let scope = self.build_scope(rctx, &input);
        let edges = self.outgoing.get(node_id).cloned().unwrap_or_default();

        for edge in &edges {
            if let EdgeKind::When { condition } = &edge.kind {
                if condition.eval(&scope) {
                    let _ = rctx
                        .events
                        .send(WorkflowEvent::BranchTaken {
                            node_id: node_id.to_string(),
                            edge_to: edge.to.clone(),
                        })
                        .await;
                    return self.walk(rctx, &edge.to, input, stop_at).await;
                }
            }
        }
        // No arm matched — fall through a `Then` (else) edge if present.
        if let Some(then) = edges.iter().find(|e| matches!(e.kind, EdgeKind::Then)) {
            let _ = rctx
                .events
                .send(WorkflowEvent::BranchTaken {
                    node_id: node_id.to_string(),
                    edge_to: then.to.clone(),
                })
                .await;
            return self.walk(rctx, &then.to, input, stop_at).await;
        }
        Ok(Some(input))
    }

    #[allow(clippy::too_many_arguments)]
    async fn exec_loop(
        &self,
        rctx: &RunCtx,
        node_id: &str,
        body: &str,
        mode: &LoopMode,
        condition: Option<&crate::condition::Condition>,
        input: Value,
        stop_at: Option<&str>,
    ) -> Result<Option<Value>> {
        let body_nodes = self.reachable(body, node_id);

        let output = match mode {
            LoopMode::ForEach { .. } => {
                let items = match &input {
                    Value::Array(a) => a.clone(),
                    _ => return Err(CerseiError::Config("foreach input is not an array".into())),
                };
                let mut collected = Vec::with_capacity(items.len());
                for item in items {
                    self.clear_nodes(rctx, &body_nodes);
                    match self.walk(rctx, body, item, Some(node_id)).await? {
                        Some(v) => collected.push(v),
                        None => return Ok(None),
                    }
                }
                Value::Array(collected)
            }
            LoopMode::DoWhile | LoopMode::DoUntil => {
                let mut acc = input;
                let mut iter = 0u32;
                loop {
                    self.clear_nodes(rctx, &body_nodes);
                    match self.walk(rctx, body, acc.clone(), Some(node_id)).await? {
                        Some(v) => acc = v,
                        None => return Ok(None),
                    }
                    iter += 1;
                    let _ = rctx
                        .events
                        .send(WorkflowEvent::LoopIteration {
                            node_id: node_id.to_string(),
                            iteration: iter,
                        })
                        .await;
                    let scope = self.build_scope(rctx, &acc);
                    let cond = condition.map(|c| c.eval(&scope)).unwrap_or(false);
                    let stop = match mode {
                        LoopMode::DoWhile => !cond,
                        LoopMode::DoUntil => cond,
                        _ => true,
                    };
                    if stop || iter >= MAX_LOOP_ITERS {
                        break;
                    }
                }
                acc
            }
        };

        rctx.outputs.insert(node_id.to_string(), output.clone());
        self.continue_from(rctx, node_id, output, stop_at).await
    }

    /// Follow the single `Then` successor, if any.
    fn continue_from<'a>(
        &'a self,
        rctx: &'a RunCtx,
        node_id: &'a str,
        output: Value,
        stop_at: Option<&'a str>,
    ) -> WalkFut<'a> {
        Box::pin(async move {
            let next = self.outgoing.get(node_id).and_then(|edges| {
                edges
                    .iter()
                    .find(|e| matches!(e.kind, EdgeKind::Then))
                    .map(|e| e.to.clone())
            });
            match next {
                Some(n) if Some(n.as_str()) != stop_at => {
                    self.walk(rctx, &n, output, stop_at).await
                }
                _ => Ok(Some(output)),
            }
        })
    }

    /// Build the condition/map scope: `{ input, state, steps, current }`.
    fn build_scope(&self, rctx: &RunCtx, current: &Value) -> Value {
        let mut steps = Map::new();
        for e in rctx.outputs.iter() {
            steps.insert(e.key().clone(), e.value().clone());
        }
        json!({
            "input": rctx.input,
            "state": *rctx.state.lock(),
            "steps": Value::Object(steps),
            "current": current,
        })
    }

    /// The join node for a set of parallel fork branches: the first `Merge`
    /// target reachable from any branch.
    fn find_join(&self, fork_targets: &[NodeId]) -> Option<NodeId> {
        let reachable: HashSet<NodeId> = fork_targets
            .iter()
            .flat_map(|t| self.reachable(t, ""))
            .collect();
        for edge in &self.def.edges {
            if matches!(edge.kind, EdgeKind::Merge) && reachable.contains(&edge.from) {
                return Some(edge.to.clone());
            }
        }
        None
    }

    /// Node ids reachable from `start`, following non-`LoopBack` edges, without
    /// crossing `stop` (pass `""` for no stop).
    fn reachable(&self, start: &str, stop: &str) -> HashSet<NodeId> {
        let mut seen = HashSet::new();
        let mut stack = vec![start.to_string()];
        while let Some(n) = stack.pop() {
            if n == stop || !seen.insert(n.clone()) {
                continue;
            }
            if let Some(edges) = self.outgoing.get(&n) {
                for e in edges {
                    if matches!(e.kind, EdgeKind::LoopBack) {
                        continue;
                    }
                    if e.to != stop {
                        stack.push(e.to.clone());
                    }
                }
            }
        }
        seen
    }

    fn clear_nodes(&self, rctx: &RunCtx, nodes: &HashSet<NodeId>) {
        for n in nodes {
            rctx.outputs.remove(n);
        }
    }
}

/// Merge a node's static `config` (base) with its dynamic `input` (overlay).
fn merge_config(config: &Value, input: Value) -> Value {
    match (config, &input) {
        (Value::Object(c), Value::Object(i)) => {
            let mut merged = c.clone();
            for (k, v) in i {
                merged.insert(k.clone(), v.clone());
            }
            Value::Object(merged)
        }
        _ => input,
    }
}

fn normalize_ptr(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
