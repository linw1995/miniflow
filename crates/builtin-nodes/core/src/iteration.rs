use mf_runtime::{
    EmissionSink, ExecutionContext, ExecutionScope, Inputs, IterationConfig, IterationErrorPolicy,
    IterationMode, NodeBuildError, NodeExecutionError, NodePortContract, NodePorts,
    NodeRegistration, NodeResult, NodeValue, OutputDerivation, Outputs, PreparedSubgraph,
    StreamCancellation, StreamError, SubgraphNode, TaskNode, TypedNodeResult, TypedTaskNode,
    ValueKind, ValueRef, ValueType, deserialize_config, execute_typed_task,
};
use mf_telemetry::observation::{ItemObservation, IterationObservation};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
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
    let metadata = mf_runtime::NodeMetadata {
        output_derivations: vec![node.result_derivation()],
        ..Default::default()
    };
    mf_runtime::PreparedNode::subgraph(node, metadata)
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Subgraph(factory) }
}

pub struct IterationNode {
    id: String,
    mode: IterationMode,
    on_error: IterationErrorPolicy,
    result_type: ValueType,
    body: Arc<PreparedSubgraph>,
}

/// Fixed inputs shared by typed and dynamic Iteration task calls.
#[derive(NodeValue)]
pub struct IterationInputs {
    pub items: ValueRef,
}

/// Collected results shared by typed and dynamic Iteration task calls.
#[derive(NodeValue)]
pub struct IterationOutputs {
    pub results: Vec<ValueRef>,
}

#[derive(Debug, Snafu)]
#[snafu(display("iteration item {index} failed: {source}"))]
struct IterationItemFailure {
    index: usize,
    source: NodeExecutionError,
}

impl From<IterationItemFailure> for NodeExecutionError {
    fn from(source: IterationItemFailure) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
enum ForwardingError {
    #[snafu(display("iteration stream forwarding channel closed: {source}"))]
    Send { source: mpsc::SendError<()> },
    #[snafu(display("iteration stream acknowledgement closed: {source}"))]
    Acknowledge { source: mpsc::RecvError },
    #[snafu(display("could not start iteration stream coordinator: {source}"))]
    Coordinator { source: std::io::Error },
}

struct Emission {
    result: NodeResult,
    acknowledge: mpsc::SyncSender<Result<(), StreamError>>,
}

struct ItemSink {
    id: String,
    sender: mpsc::SyncSender<Emission>,
    cancellation: StreamCancellation,
}

impl EmissionSink for ItemSink {
    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        let result: Result<(), NodeExecutionError> = (|| {
            let (acknowledge, reply) = mpsc::sync_channel(0);
            self.sender
                .send(Emission {
                    result,
                    acknowledge,
                })
                // A disconnected channel needs no payload in its diagnostic.
                .map_err(|_| mpsc::SendError(()))
                .context(SendSnafu)
                .boxed()?;
            reply.recv().context(AcknowledgeSnafu).boxed()??;
            Ok(())
        })();
        result
            .context(mf_runtime::StreamProducerSnafu {
                definition_id: self.id.clone(),
            })
            .inspect_err(|error| self.cancellation.cancel(error.clone()))
    }
}

struct DeliverySink<'a> {
    sink: &'a mut dyn EmissionSink,
    cancellation: StreamCancellation,
}

impl EmissionSink for DeliverySink<'_> {
    fn is_connected(&self) -> bool {
        self.sink.is_connected()
    }

    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        self.sink
            .send(result)
            .inspect_err(|error| self.cancellation.cancel(error.clone()))
    }
}

impl IterationNode {
    pub fn new(
        id: impl Into<String>,
        mode: IterationMode,
        on_error: IterationErrorPolicy,
        body: PreparedSubgraph,
    ) -> Result<Self, NodeBuildError> {
        ensure!(
            body.stream_outputs
                .iter()
                .all(|port| port.name != "results"),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: "Iteration stream outputs must not overlap results"
            }
        );
        let result_type = body
            .outputs
            .iter()
            .find(|port| port.name == "result" && port.required)
            .with_context(|| mf_runtime::NodeInvalidSubgraphSnafu {
                message: "iteration body must declare a required result output",
            })?
            .value_type
            .clone();
        Ok(Self {
            id: id.into(),
            mode,
            on_error,
            result_type,
            body: Arc::new(body),
        })
    }

    pub fn ports(&self) -> NodePorts {
        NodePorts::from_types::<<Self as TypedTaskNode>::Input, <Self as TypedTaskNode>::Output>()
    }

    fn result_derivation(&self) -> OutputDerivation {
        let item_type = if matches!(self.on_error, IterationErrorPolicy::ContinueOnError) {
            ValueType::Any
        } else {
            self.result_type.clone()
        };
        OutputDerivation::known_type("results", ValueType::List(Box::new(item_type)))
    }

    fn run_item(
        id: &str,
        body: &PreparedSubgraph,
        (key, item): (ValueRef, ValueRef),
        index: usize,
        observation: Option<&IterationObservation>,
        parent: &ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<mf_runtime::ValueRef, NodeExecutionError> {
        let mut step = observation.and_then(|observation| observation.begin_item(index));
        let _context = step.as_ref().map(ItemObservation::enter);
        let body_observation = step.as_ref().map(ItemObservation::body_observation);
        let mut state = parent.fork_body(body_observation);
        let result = ExecutionScope::new(
            id,
            mf_runtime::ITERATION_INPUT_ID,
            index,
            Outputs::from([("item".into(), item), ("key".into(), key)]),
            BTreeMap::from([
                ("item".into(), ValueType::Any),
                ("key".into(), ValueType::Any),
            ]),
        )
        .and_then(|scope| {
            let (mut outputs, _, _) =
                state.run_scope(scope, |state| body.execute_with_sink(state, sink))?;
            outputs
                .remove("result")
                .with_context(|| mf_runtime::NodeExecutionFailedSnafu {
                    message: "iteration body did not produce `result`",
                })
        });
        let result = result.context(IterationItemFailureSnafu { index });
        if let Some(step) = step.as_mut() {
            let failure = result.as_ref().err().map(ToString::to_string);
            step.finish(failure.as_deref());
        }
        Ok(result?)
    }

    fn execute_items(
        &self,
        items: ValueRef,
        observation: Option<IterationObservation>,
        parent: &mut ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<IterationOutputs, NodeExecutionError> {
        let items: Box<dyn ExactSizeIterator<Item = (ValueRef, ValueRef)>> =
            match items.kind() {
                ValueKind::Array(items) => {
                    Box::new(items.iter().map(|item| (ValueRef::null(), item.clone())))
                }
                ValueKind::Object(items) => Box::new(items.iter().map(|(key, item)| {
                    (ValueRef::new(ValueKind::String(key.clone())), item.clone())
                })),
                _ => {
                    return mf_runtime::NodeExecutionFailedSnafu {
                        message: "iteration requires an array or object input `items`",
                    }
                    .fail();
                }
            };
        let mut results = Vec::with_capacity(items.len());
        // A single active item can borrow the caller's sink directly.
        let mode = if sink.is_connected() && parent.worker_limit().get() == 1 {
            IterationMode::Sequential
        } else {
            self.mode
        };
        match mode {
            IterationMode::Sequential => {
                for (index, (key, item)) in items.enumerate() {
                    if let Some(error) = parent.cancellation().failure() {
                        return Err(error.into());
                    }
                    results.push(Self::run_item(
                        &self.id,
                        &self.body,
                        (key, item),
                        index,
                        observation.as_ref(),
                        parent,
                        sink,
                    ));
                    if matches!(self.on_error, IterationErrorPolicy::Terminate)
                        && results.last().is_some_and(Result::is_err)
                    {
                        break;
                    }
                }
            }
            IterationMode::Parallel => {
                let worker_count = items.len().min(MAX_PARALLEL_ITEMS);
                let queue = Arc::new(Mutex::new(items.enumerate().collect::<VecDeque<_>>()));
                let completed = Arc::new(Mutex::new(Vec::new()));
                let stopped = Arc::new(AtomicBool::new(false));
                let observation = observation.map(Arc::new);
                let run_workers =
                    |parent: &mut ExecutionContext, sender: Option<mpsc::SyncSender<Emission>>| {
                        parent.run_parallel(worker_count, |parent| {
                            (0..worker_count)
                                .map(|_| {
                                    let queue = Arc::clone(&queue);
                                    let completed = Arc::clone(&completed);
                                    let stopped = Arc::clone(&stopped);
                                    let observation = observation.clone();
                                    let id = self.id.clone();
                                    let on_error = self.on_error;
                                    let body = Arc::clone(&self.body);
                                    let parent = parent.fork_body(None);
                                    let sender = sender.clone();
                                    move || loop {
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
                                        let mut forwarded =
                                            sender.as_ref().map(|sender| ItemSink {
                                                id: id.clone(),
                                                sender: sender.clone(),
                                                cancellation: parent.cancellation(),
                                            });
                                        let sink: &mut dyn EmissionSink = match &mut forwarded {
                                            Some(sink) => sink,
                                            None => &mut mf_runtime::DiscardSink,
                                        };
                                        let result = Self::run_item(
                                            &id,
                                            &body,
                                            (key, item),
                                            index,
                                            observation.as_deref(),
                                            &parent,
                                            sink,
                                        );
                                        if parent.cancellation().failure().is_some()
                                            || (result.is_err()
                                                && matches!(
                                                    on_error,
                                                    IterationErrorPolicy::Terminate
                                                ))
                                        {
                                            stopped.store(true, Ordering::Release);
                                        }
                                        completed.lock().unwrap().push((index, result));
                                    }
                                })
                                .collect()
                        })
                    };
                if sink.is_connected() && worker_count > 0 {
                    std::thread::scope(|scope| -> Result<(), NodeExecutionError> {
                        let (sender, receiver) = mpsc::sync_channel::<Emission>(0);
                        let worker = std::thread::Builder::new()
                            .name(format!("iteration-stream-{}", self.id))
                            .spawn_scoped(scope, || run_workers(parent, Some(sender)))
                            .context(CoordinatorSnafu)
                            .boxed()?;
                        for emission in receiver {
                            let result = sink.send(emission.result);
                            let _ = emission.acknowledge.send(result);
                        }
                        worker
                            .join()
                            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?;
                        Ok(())
                    })?;
                } else {
                    run_workers(parent, None)?;
                }
                let mut completed = completed.lock().unwrap().drain(..).collect::<Vec<_>>();
                completed.sort_by_key(|(index, _)| *index);
                results.extend(completed.into_iter().map(|(_, result)| result));
            }
        }
        if let Some(error) = parent.cancellation().failure() {
            return Err(error.into());
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
        Ok(IterationOutputs { results: values })
    }
}

impl TypedTaskNode for IterationNode {
    fn supports_success_dependencies(&self) -> bool {
        false
    }
    type Input = IterationInputs;
    type Output = IterationOutputs;

    fn execute(
        &self,
        inputs: IterationInputs,
        ctx: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let observation = ctx.observation().and_then(|run| {
            run.iteration_observation(&self.id, &ctx.invocation_path(), self.body.nodes.clone())
        });
        self.execute_items(inputs.items, observation, ctx, &mut mf_runtime::DiscardSink)
            .map(Into::into)
    }
}

impl TaskNode for IterationNode {
    fn supports_success_dependencies(&self) -> bool {
        false
    }
    fn execute(
        &self,
        inputs: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        execute_typed_task(self, inputs, ctx)
    }
}

impl NodePortContract for IterationNode {
    fn ports(&self) -> NodePorts {
        IterationNode::ports(self)
    }
}

impl SubgraphNode for IterationNode {
    fn body(&self) -> &PreparedSubgraph {
        &self.body
    }

    fn supports_success_dependencies(&self) -> bool {
        false
    }

    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        let inputs = <IterationInputs as mf_runtime::NodeInputs>::from_inputs(inputs).boxed()?;
        let observation = context.observation().and_then(|run| {
            run.iteration_observation(
                &self.id,
                &context.invocation_path(),
                self.body.nodes.clone(),
            )
        });
        let mut sink = DeliverySink {
            sink,
            cancellation: context.cancellation(),
        };
        let outputs = self.execute_items(inputs.items, observation, context, &mut sink)?;
        mf_runtime::encode_typed_result(outputs.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{NodeRegistry, PortSpec, execute_node_in_context};
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
            let output = TaskNode::execute(
                &node,
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
    fn typed_and_dynamic_iteration_share_values_and_validate_inputs() {
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
            let output = TaskNode::execute(
                &node,
                Inputs::from([("items".into(), items.clone())]),
                &mut ExecutionContext::default(),
            )
            .unwrap()
            .outputs;
            assert!(output["results"][0].ptr_eq(&items["a"]));
            assert!(output["results"][1].ptr_eq(&items["b"]));
            let typed = TypedTaskNode::execute(
                &node,
                IterationInputs {
                    items: items.clone(),
                },
                &mut ExecutionContext::default(),
            )
            .unwrap()
            .outputs;
            assert_eq!(typed.results, output["results"].as_array().unwrap());
            assert!(typed.results[0].ptr_eq(&items["a"]));
            assert!(typed.results[1].ptr_eq(&items["b"]));
            for (inputs, port) in [
                (Inputs::new(), "items"),
                (
                    Inputs::from([
                        ("items".into(), items.clone()),
                        ("unknown".into(), true.into()),
                    ]),
                    "unknown",
                ),
            ] {
                let error =
                    TaskNode::execute(&node, inputs, &mut ExecutionContext::default()).unwrap_err();
                let NodeExecutionError::InputDecode { source } = error else {
                    panic!("expected runtime decode failure");
                };
                assert_eq!(source.pointer(), format!("/{port}"));
            }
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
        assert_eq!(ports.inputs, IterationInputs::ports());
        assert_eq!(ports.inputs[0].name, "items");
        assert_eq!(ports.inputs[0].value_type, ValueType::Any);
        assert!(ports.inputs[0].required);
        assert_eq!(ports.outputs[0].name, "results");
        for policy in [
            IterationErrorPolicy::Terminate,
            IterationErrorPolicy::ContinueOnError,
            IterationErrorPolicy::RemoveFailed,
        ] {
            let node = IterationNode::new(
                "iteration",
                IterationMode::Sequential,
                policy,
                PreparedSubgraph::new(
                    Vec::new(),
                    vec![PortSpec::new("result", ValueType::Int64, true)],
                    |_| Ok(Outputs::new()),
                ),
            )
            .unwrap();
            let expected = if matches!(policy, IterationErrorPolicy::ContinueOnError) {
                ValueType::Any
            } else {
                ValueType::Int64
            };
            assert_eq!(node.ports().outputs, IterationOutputs::ports());
            assert_eq!(
                node.result_derivation(),
                OutputDerivation::known_type("results", ValueType::List(Box::new(expected)))
            );
            let expected_ports = node.ports();
            let prepared =
                mf_runtime::PreparedNode::typed_task(node, NodePorts::default()).unwrap();
            assert_eq!(prepared.metadata.ports, expected_ports);
        }
    }
}
