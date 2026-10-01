use mf_runtime::{
    ExecutionContext, ExecutionScope, Inputs, IterationConfig, IterationErrorPolicy, IterationMode,
    Node, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration, NodeResult, Outputs,
    PortSpec, PreparedSubgraph, ValueType, deserialize_config,
};
use mf_telemetry::observation::{ItemObservation, IterationObservation};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
pub const MAX_PARALLEL_ITEMS: usize = 10;
use serde_json::Value;

pub const KIND: &str = mf_runtime::ITERATION_KIND;

// Body binding preserves the ordinary registry factory contract.
struct IterationDeclaration {
    mode: IterationMode,
    on_error: IterationErrorPolicy,
}

impl Node for IterationDeclaration {
    fn with_subgraph(
        self: Box<Self>,
        id: &str,
        _: Value,
        body: PreparedSubgraph,
    ) -> Result<Box<dyn Node>, NodeBuildError> {
        Ok(Box::new(IterationNode::new(
            id,
            self.mode,
            self.on_error,
            body,
        )?))
    }

    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(NodeExecutionError::ExecutionFailed {
            message: "iteration requires a compiled body".into(),
        })
    }
}

fn factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let config: IterationConfig = deserialize_config(config)?;
    Ok(Box::new(IterationDeclaration {
        mode: config.mode,
        on_error: config.on_error,
    }))
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        inputs: &[PortSpec::new("items", ValueType::Array, true)],
        outputs: &[PortSpec::new("results", ValueType::Array, true)],
        factory,
    }
}

pub struct IterationNode {
    id: String,
    mode: IterationMode,
    on_error: IterationErrorPolicy,
    result_type: ValueType,
    body: PreparedSubgraph,
}

impl IterationNode {
    pub fn new(
        id: impl Into<String>,
        mode: IterationMode,
        on_error: IterationErrorPolicy,
        body: PreparedSubgraph,
    ) -> Result<Self, NodeBuildError> {
        let result_type = body
            .outputs
            .iter()
            .find(|port| port.name == "result" && port.required)
            .ok_or_else(|| NodeBuildError::InvalidSubgraph {
                message: "iteration body must declare a required result output".into(),
            })?
            .value_type
            .clone();
        Ok(Self {
            id: id.into(),
            mode,
            on_error,
            result_type,
            body,
        })
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
        item: mf_runtime::ValueRef,
        index: usize,
        observation: Option<&IterationObservation>,
        parent: &ExecutionContext,
    ) -> Result<mf_runtime::ValueRef, NodeExecutionError> {
        let mut step = observation.and_then(|observation| observation.begin_item(index));
        let _context = step.as_ref().map(ItemObservation::enter);
        let body_observation = step.as_ref().map(ItemObservation::body_observation);
        let mut state = parent.fork_body(body_observation);
        let result = ExecutionScope::new(
            &self.id,
            mf_runtime::ITERATION_INPUT_ID,
            index,
            Outputs::from([("items".into(), item)]),
            BTreeMap::from([("items".into(), ValueType::Any)]),
        )
        .and_then(|scope| {
            let (mut outputs, _, _) = state
                .run_scope(scope, |state| self.body.execute_in_context(state))
                .map_err(|source| NodeExecutionError::PluginFailed {
                    source: Box::new(source),
                })?;
            outputs
                .remove("result")
                .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                    message: "iteration body did not produce `result`".into(),
                })
        });
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
        parent: &ExecutionContext,
    ) -> Result<Outputs, NodeExecutionError> {
        let items = inputs.remove("items");
        let Some(items) = items.as_ref().and_then(mf_runtime::ValueRef::as_array) else {
            return Err(NodeExecutionError::ExecutionFailed {
                message: "iteration requires an array input `items`".into(),
            });
        };
        let mut results = Vec::with_capacity(items.len());
        match self.mode {
            IterationMode::Sequential => {
                for (index, item) in items.iter().cloned().enumerate() {
                    results.push(self.run_item(item, index, observation.as_ref(), parent));
                    if matches!(self.on_error, IterationErrorPolicy::Terminate)
                        && results.last().is_some_and(Result::is_err)
                    {
                        break;
                    }
                }
            }
            IterationMode::Parallel => {
                let worker_count = items.len().min(MAX_PARALLEL_ITEMS);
                let queue = Mutex::new(items.iter().cloned().enumerate().collect::<VecDeque<_>>());
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
                                    let result = self.run_item(item, index, observation, parent);
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
                (IterationErrorPolicy::ContinueOnError, Err(_)) => {
                    values.push(mf_runtime::ValueRef::null())
                }
                (IterationErrorPolicy::RemoveFailed, Err(_)) => {}
            }
        }
        Ok(Outputs::from([(
            "results".into(),
            mf_runtime::ValueRef::array(values),
        )]))
    }
}

impl Node for IterationNode {
    fn ports(&self) -> Option<NodePorts> {
        Some(IterationNode::ports(self))
    }

    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        self.execute_items(inputs, None, &ExecutionContext::default())
    }

    fn execute_with_context(
        &self,
        inputs: Inputs,
        ctx: &ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_items(
            inputs,
            ctx.observation().and_then(|run| {
                run.iteration_observation(&self.id, &ctx.scope_path(), self.body.nodes.clone())
            }),
            ctx,
        )
        .map(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{NodeRegistry, execute_node_in_context};
    use serde_json::json;
    use std::sync::{Arc, atomic::AtomicUsize};
    use std::time::Duration;

    #[test]
    fn parallel_mode_is_bounded_and_returns_input_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let node = IterationNode::new(
            "iteration",
            IterationMode::Parallel,
            IterationErrorPolicy::Terminate,
            PreparedSubgraph::new(
                Vec::new(),
                vec![PortSpec::new("result", ValueType::Int64, true)],
                {
                    let active = Arc::clone(&active);
                    let peak = Arc::clone(&peak);
                    let source = mf_runtime::iteration_input_flow_node();
                    move |state| {
                        execute_node_in_context(&source, &[], state)?;
                        let item = state
                            .select_output(
                                "result",
                                mf_runtime::ITERATION_INPUT_ID,
                                "items",
                                false,
                            )?
                            .unwrap();
                        let concurrent = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(concurrent, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(2));
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(Outputs::from([("result".into(), item)]))
                    }
                },
            ),
        )
        .unwrap();
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
    #[test]
    fn registers_iteration_and_checks_its_configuration_shape() {
        let registration = NodeRegistry::from_inventory().unwrap().get(KIND).unwrap();
        assert!(registration.instantiate(json!({})).is_err());
        let node = registration
            .instantiate(json!({
                "body": {
                    "nodes": [],
                    "result": {"node": "%iteration", "port": "items"}
                }
            }))
            .unwrap();
        let ports = registration.effective_ports(node.as_ref());
        assert_eq!(ports.inputs[0].name, "items");
        assert_eq!(ports.outputs[0].name, "results");
    }
}
