use crate::{
    ContextValue, ExecutionContext, FlowNode, Inputs, Node, NodeExecutionError, NodePorts, Outputs,
    PortSpec, ValueType,
};
use mf_telemetry::{
    event::NodeIdentity,
    observation::{BodyObservation, ItemObservation, IterationObservation},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

pub const ITERATION_KIND: &str = "builtin.iteration";
pub const ITERATION_INPUT_KIND: &str = "builtin.iteration_input";
pub const ITERATION_INPUT_ID: &str = "@iteration";
pub const ITERATION_SEED_ITEM: &str = "@iteration_seed.items";
pub const ITERATION_SEED_INDEX: &str = "@iteration_seed.index";
pub const MAX_PARALLEL_ITEMS: usize = 10;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IterationMode {
    #[default]
    Sequential,
    Parallel,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IterationErrorPolicy {
    #[default]
    Terminate,
    ContinueOnError,
    RemoveFailed,
}

pub type IterationBody = dyn Fn(Value, usize, Option<BodyObservation>) -> Result<Value, NodeExecutionError>
    + Send
    + Sync
    + 'static;

pub struct IterationNode {
    id: String,
    body_nodes: Vec<NodeIdentity>,
    mode: IterationMode,
    on_error: IterationErrorPolicy,
    result_type: ValueType,
    body: Box<IterationBody>,
}

impl IterationNode {
    pub fn new(
        id: impl Into<String>,
        body_nodes: Vec<NodeIdentity>,
        mode: IterationMode,
        on_error: IterationErrorPolicy,
        result_type: ValueType,
        body: impl Fn(Value, usize, Option<BodyObservation>) -> Result<Value, NodeExecutionError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            body_nodes,
            mode,
            on_error,
            result_type,
            body: Box::new(body),
        }
    }

    pub fn ports(&self) -> NodePorts {
        let item_type = if matches!(self.on_error, IterationErrorPolicy::ContinueOnError) {
            ValueType::Any
        } else {
            self.result_type.clone()
        };
        NodePorts {
            inputs: vec![PortSpec::new("items", ValueType::Array, true)],
            outputs: vec![PortSpec::owned(
                "results",
                ValueType::List(Box::new(item_type)),
                true,
            )],
        }
    }

    fn run_item(
        &self,
        item: Value,
        index: usize,
        observation: Option<&IterationObservation>,
    ) -> Result<Value, NodeExecutionError> {
        let mut step = observation.and_then(|observation| observation.begin_item(index));
        let _context = step.as_ref().map(ItemObservation::enter);
        let body_observation = step.as_ref().map(ItemObservation::body_observation);
        let result = (self.body)(item, index, body_observation);
        if let Some(step) = step.as_mut() {
            let failure = result.as_ref().err().map(ToString::to_string);
            step.finish(failure.as_deref());
        }
        result.map_err(|error| NodeExecutionError::ExecutionFailed {
            message: format!("iteration item {index} failed: {error}"),
        })
    }

    fn execute_items(
        &self,
        mut inputs: Inputs,
        observation: Option<IterationObservation>,
    ) -> Result<Outputs, NodeExecutionError> {
        let Some(Value::Array(items)) = inputs.remove("items") else {
            return Err(NodeExecutionError::ExecutionFailed {
                message: "iteration requires an array input `items`".into(),
            });
        };
        let mut results = Vec::with_capacity(items.len());
        match self.mode {
            IterationMode::Sequential => {
                for (index, item) in items.into_iter().enumerate() {
                    results.push(self.run_item(item, index, observation.as_ref()));
                    if matches!(self.on_error, IterationErrorPolicy::Terminate)
                        && results.last().is_some_and(Result::is_err)
                    {
                        break;
                    }
                }
            }
            IterationMode::Parallel => {
                let worker_count = items.len().min(MAX_PARALLEL_ITEMS);
                let queue = Mutex::new(items.into_iter().enumerate().collect::<VecDeque<_>>());
                let completed = Mutex::new(Vec::new());
                let stopped = AtomicBool::new(false);
                std::thread::scope(|scope| {
                    let handles: Vec<_> = (0..worker_count)
                        .map(|_| {
                            let queue = &queue;
                            let completed = &completed;
                            let stopped = &stopped;
                            let observation = observation.as_ref();
                            scope.spawn(move || {
                                loop {
                                    let next = {
                                        let mut queue = queue.lock().unwrap();
                                        if stopped.load(Ordering::Acquire) {
                                            None
                                        } else {
                                            queue.pop_front()
                                        }
                                    };
                                    let Some((index, item)) = next else {
                                        break;
                                    };
                                    let result = self.run_item(item, index, observation);
                                    if result.is_err()
                                        && matches!(self.on_error, IterationErrorPolicy::Terminate)
                                    {
                                        stopped.store(true, Ordering::Release);
                                    }
                                    completed.lock().unwrap().push((index, result));
                                }
                            })
                        })
                        .collect();
                    for handle in handles {
                        if let Err(payload) = handle.join() {
                            std::panic::resume_unwind(payload);
                        }
                    }
                });
                let mut completed = completed.into_inner().unwrap();
                completed.sort_by_key(|(index, _)| *index);
                results.extend(completed.into_iter().map(|(_, result)| result));
            }
        }
        let mut values = Vec::with_capacity(results.len());
        for result in results {
            match (self.on_error, result) {
                (_, Ok(value)) => values.push(value),
                (IterationErrorPolicy::Terminate, Err(error)) => return Err(error),
                (IterationErrorPolicy::ContinueOnError, Err(_)) => values.push(Value::Null),
                (IterationErrorPolicy::RemoveFailed, Err(_)) => {}
            }
        }
        Ok(Outputs::from([("results".into(), Value::Array(values))]))
    }
}

impl Node for IterationNode {
    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        self.execute_items(inputs, None)
    }

    fn execute_with_context(
        &self,
        inputs: Inputs,
        ctx: &ExecutionContext,
    ) -> Result<crate::NodeResult, NodeExecutionError> {
        self.execute_items(
            inputs,
            ctx.iteration_observation(&self.id, &self.body_nodes),
        )
        .map(Into::into)
    }

    fn ports(&self) -> Option<NodePorts> {
        Some(self.ports())
    }
}

struct IterationInputNode;

impl Node for IterationInputNode {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(NodeExecutionError::ExecutionFailed {
            message: "iteration input requires an item context".into(),
        })
    }

    fn execute_with_context(
        &self,
        _: Inputs,
        ctx: &ExecutionContext,
    ) -> Result<crate::NodeResult, NodeExecutionError> {
        let read = |id: &str| match ctx.output(id)? {
            ContextValue::Value(value) => Ok(value.clone()),
            ContextValue::Skipped => Err(NodeExecutionError::ExecutionFailed {
                message: format!("iteration input `{id}` was skipped"),
            }),
        };
        Ok(Outputs::from([
            ("items".into(), read(ITERATION_SEED_ITEM)?),
            ("index".into(), read(ITERATION_SEED_INDEX)?),
        ])
        .into())
    }
}

pub fn iteration_input_flow_node() -> FlowNode {
    FlowNode::new(
        ITERATION_INPUT_ID,
        Box::new(IterationInputNode),
        NodePorts {
            inputs: vec![],
            outputs: vec![
                PortSpec::new("items", ValueType::Any, true),
                PortSpec::new("index", ValueType::Int64, true),
            ],
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicUsize};
    use std::time::Duration;

    #[test]
    fn parallel_mode_is_bounded_and_returns_input_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let node = IterationNode::new(
            "iteration",
            Vec::new(),
            IterationMode::Parallel,
            IterationErrorPolicy::Terminate,
            ValueType::Int64,
            {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                move |item, _, _| {
                    let concurrent = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(concurrent, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(2));
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(item)
                }
            },
        );
        let items: Vec<_> = (0..64).map(Value::from).collect();
        let output = node
            .execute(Inputs::from([(
                "items".into(),
                Value::Array(items.clone()),
            )]))
            .unwrap();
        assert_eq!(output["results"], Value::Array(items));
        assert!((2..=MAX_PARALLEL_ITEMS).contains(&peak.load(Ordering::SeqCst)));
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}
