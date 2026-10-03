use mf_runtime::{
    ExecutionContext, ExecutionScope, Inputs, IterationConfig, IterationErrorPolicy, IterationMode,
    NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec,
    PreparedSubgraph, TaskNode, ValueKind, ValueRef, ValueType, deserialize_config,
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

fn factory(
    id: &str,
    config: Value,
    _: Value,
    body: PreparedSubgraph,
) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: IterationConfig = deserialize_config(config)?;
    let node = IterationNode::new(id, config.mode, config.on_error, body)?;
    let ports = node.ports();
    Ok(mf_runtime::PreparedNode::new(node, ports))
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Subgraph(factory) }
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
            inputs: vec![PortSpec::new("items", ValueType::Any, true)],
            outputs: vec![PortSpec::owned(
                "results",
                ValueType::List(Box::new(item_type)),
                true,
            )],
        }
    }

    fn run_item(
        &self,
        item: ValueRef,
        key: ValueRef,
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
            Outputs::from([("item".into(), item), ("key".into(), key)]),
            BTreeMap::from([
                ("item".into(), ValueType::Any),
                ("key".into(), ValueType::Any),
            ]),
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
        let items: Box<dyn ExactSizeIterator<Item = (ValueRef, ValueRef)>> =
            match items.as_ref().map(ValueRef::kind) {
                Some(ValueKind::Array(items)) => {
                    Box::new(items.iter().map(|item| (ValueRef::null(), item.clone())))
                }
                Some(ValueKind::Object(items)) => Box::new(items.iter().map(|(key, item)| {
                    (ValueRef::new(ValueKind::String(key.clone())), item.clone())
                })),
                _ => {
                    return Err(NodeExecutionError::ExecutionFailed {
                        message: "iteration requires an array or object input `items`".into(),
                    });
                }
            };
        let mut results = Vec::with_capacity(items.len());
        match self.mode {
            IterationMode::Sequential => {
                for (index, (key, item)) in items.enumerate() {
                    results.push(self.run_item(item, key, index, observation.as_ref(), parent));
                    if matches!(self.on_error, IterationErrorPolicy::Terminate)
                        && results.last().is_some_and(Result::is_err)
                    {
                        break;
                    }
                }
            }
            IterationMode::Parallel => {
                let worker_count = items.len().min(MAX_PARALLEL_ITEMS);
                let queue = Mutex::new(items.enumerate().collect::<VecDeque<_>>());
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
                                    let Some((index, (key, item))) = next else {
                                        break;
                                    };
                                    let result =
                                        self.run_item(item, key, index, observation, parent);
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

impl TaskNode for IterationNode {
    fn execute(
        &self,
        inputs: Inputs,
        ctx: &mut ExecutionContext,
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
        let values: Vec<_> = (0..64).map(Value::from).collect();
        for items in [
            Value::Array(values.clone()),
            Value::Object(
                values
                    .iter()
                    .enumerate()
                    .map(|(index, item)| (format!("{index:02}"), item.clone()))
                    .collect(),
            ),
        ] {
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
                        let source = mf_runtime::iteration_input_flow_node().into_task().unwrap();
                        move |state| {
                            execute_node_in_context(&source, &[], state)?;
                            let item = state
                                .select_output(
                                    "result",
                                    mf_runtime::ITERATION_INPUT_ID,
                                    "item",
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
            let output = node
                .execute(
                    Inputs::from([("items".into(), items.into())]),
                    &mut mf_runtime::ExecutionContext::default(),
                )
                .unwrap()
                .outputs;
            assert_eq!(output["results"], Value::Array(values.clone()));
            assert!((2..=MAX_PARALLEL_ITEMS).contains(&peak.load(Ordering::SeqCst)));
            assert_eq!(active.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn map_iteration_shares_value_handles_and_rejects_missing_input() {
        for mode in [IterationMode::Sequential, IterationMode::Parallel] {
            let source = mf_runtime::iteration_input_flow_node().into_task().unwrap();
            let node = IterationNode::new(
                "iteration",
                mode,
                IterationErrorPolicy::Terminate,
                PreparedSubgraph::new(
                    Vec::new(),
                    vec![PortSpec::new("result", ValueType::Any, true)],
                    move |state| {
                        execute_node_in_context(&source, &[], state)?;
                        let item = state
                            .select_output("result", mf_runtime::ITERATION_INPUT_ID, "item", false)?
                            .unwrap();
                        Ok(Outputs::from([("result".into(), item)]))
                    },
                ),
            )
            .unwrap();
            let items: ValueRef = json!({"b": [2, 3], "a": {"nested": [1]}}).into();
            let output = node
                .execute(
                    Inputs::from([("items".into(), items.clone())]),
                    &mut ExecutionContext::default(),
                )
                .unwrap()
                .outputs;
            assert!(output["results"][0].ptr_eq(&items["a"]));
            assert!(output["results"][1].ptr_eq(&items["b"]));
            let error = node
                .execute(Inputs::new(), &mut ExecutionContext::default())
                .unwrap_err();
            assert!(error.to_string().contains("array or object input `items`"));
        }
    }

    #[test]
    fn registers_iteration_and_checks_its_configuration_shape() {
        let registration = NodeRegistry::from_inventory().unwrap().get(KIND).unwrap();
        assert!(registration.instantiate(json!({})).is_err());
        let node = registration
            .instantiate_subgraph(
                "iteration",
                json!({
                    "body": {
                        "nodes": [],
                        "result": {"node": "%iteration", "port": "item"}
                    }
                }),
                Value::Null,
                PreparedSubgraph::new(
                    Vec::new(),
                    vec![PortSpec::new("result", ValueType::Any, true)],
                    |_| Ok(Outputs::new()),
                ),
            )
            .unwrap();
        let ports = &node.metadata.ports;
        assert_eq!(ports.inputs[0].name, "items");
        assert_eq!(ports.outputs[0].name, "results");
    }
}
