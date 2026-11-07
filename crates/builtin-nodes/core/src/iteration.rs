use mf_runtime::{
    EmissionSink, ExecutionContext, ExecutionScope, Inputs, IterationErrorPolicy, IterationMode,
    NodeBuildError, NodeConfig, NodeExecutionError, NodePortContract, NodePorts, NodeRegistration,
    NodeResult, NodeValue, OutputDerivation, Outputs, PreparedSubgraph, SubgraphNode,
    SubgraphStream, SubgraphStreamSender, TaskNode, TypedNodeResult, TypedTaskNode, ValueKind,
    ValueRef, ValueType, execute_typed_task,
};
use mf_telemetry::observation::{ItemObservation, IterationObservation};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
pub const MAX_PARALLEL_ITEMS: usize = 10;
use serde_json::Value;

pub const KIND: &str = mf_runtime::ITERATION_KIND;

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    body: PreparedSubgraph,
    #[serde(default)]
    mode: IterationMode,
    #[serde(default)]
    on_error: IterationErrorPolicy,
    #[serde(default)]
    stream_outputs: BTreeMap<String, mf_runtime::OutputReferenceDefinition>,
}

fn plain(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    mf_runtime::NodeInvalidSubgraphSnafu {
        message: "builtin.iteration requires a prepared `body` binding",
    }
    .fail()
}

fn factory(
    id: &str,
    config: Value,
    bodies: BTreeMap<String, PreparedSubgraph>,
) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let Config {
        body,
        mode,
        on_error,
        stream_outputs,
    } = Config::from_config(config, bodies)?;
    validate_stream_names(stream_outputs.keys().map(String::as_str))?;
    let node = IterationNode::new(id, mode, on_error, body)?;
    let metadata = mf_runtime::NodeMetadata {
        output_derivations: vec![node.result_derivation()],
        ..Default::default()
    };
    mf_runtime::PreparedNode::subgraph(node, metadata)
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: mf_runtime::NodeFactory::Subgraph {
            flow_parameters: Config::flow_parameters,
            plain,
            prepared: factory,
        },
    }
}

#[derive(Debug, Snafu)]
pub enum IterationValidationError {
    #[snafu(display("Iteration stream names must be nonblank and distinct from results"))]
    InvalidStreamName,
    #[snafu(display("Iteration body must declare exactly one required output named `result`"))]
    InvalidResult,
    #[snafu(display(
        "Iteration body scope bindings must be immutable item/key values of type Any"
    ))]
    InvalidScopeBindings,
}

impl From<IterationValidationError> for NodeBuildError {
    fn from(source: IterationValidationError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

fn validate_stream_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Result<(), IterationValidationError> {
    ensure!(
        names
            .into_iter()
            .all(|name| !name.trim().is_empty() && name != "results"),
        InvalidStreamNameSnafu
    );
    Ok(())
}

pub struct IterationNode {
    id: String,
    scope_key: String,
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

impl IterationNode {
    pub fn new(
        id: impl Into<String>,
        mode: IterationMode,
        on_error: IterationErrorPolicy,
        body: PreparedSubgraph,
    ) -> Result<Self, NodeBuildError> {
        validate_stream_names(body.stream_outputs.iter().map(|port| port.name.as_ref()))?;
        if let Some(relations) = &body.relations {
            ensure!(
                relations.scope.as_ref().is_some_and(|scope| {
                    scope.values
                        == BTreeMap::from([
                            ("item".into(), ValueType::Any),
                            ("key".into(), ValueType::Any),
                        ])
                        && scope.state_outputs.is_empty()
                        && scope.writes.is_empty()
                        && scope.exits.is_empty()
                }),
                InvalidScopeBindingsSnafu
            );
        }
        ensure!(
            body.outputs.len() == 1 && body.outputs[0].name == "result" && body.outputs[0].required,
            InvalidResultSnafu
        );
        let result_type = body.outputs[0].value_type.clone();
        let id = id.into();
        let scope_key = body.scope_key(&id, "/body");
        Ok(Self {
            id,
            scope_key,
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
        scope_key: &str,
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
            scope_key,
            body.scope_input().map_or(
                mf_runtime::ITERATION_INPUT_ID,
                mf_runtime::DefinitionId::as_str,
            ),
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
        sink: &mut SubgraphStream<'_>,
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
                        &self.scope_key,
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
                    |parent: &mut ExecutionContext, sender: Option<SubgraphStreamSender>| {
                        parent.run_parallel(worker_count, |parent| {
                            (0..worker_count)
                                .map(|_| {
                                    let queue = Arc::clone(&queue);
                                    let completed = Arc::clone(&completed);
                                    let stopped = Arc::clone(&stopped);
                                    let observation = observation.clone();
                                    let scope_key = self.scope_key.clone();
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
                                        let mut forwarded = sender.clone();
                                        let sink: &mut dyn EmissionSink = match &mut forwarded {
                                            Some(sink) => sink,
                                            None => &mut mf_runtime::DiscardSink,
                                        };
                                        let result = Self::run_item(
                                            &scope_key,
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
                    sink.forward(|sender| {
                        run_workers(parent, Some(sender))?;
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
            run.iteration_observation(
                &self.id,
                &self.scope_key,
                &ctx.invocation_path(),
                self.body.nodes.clone(),
            )
        });
        let mut discard = mf_runtime::DiscardSink;
        let mut stream = SubgraphStream::new(self.id.as_str(), ctx, &mut discard);
        self.execute_items(inputs.items, observation, ctx, &mut stream)
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
                &self.scope_key,
                &context.invocation_path(),
                self.body.nodes.clone(),
            )
        });
        let mut stream = SubgraphStream::new(self.id.as_str(), context, sink);
        let outputs = self.execute_items(inputs.items, observation, context, &mut stream)?;
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
    fn iteration_factory_rejects_missing_and_unexpected_body_handles() {
        let config = json!({
            "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]},
        });
        for bodies in [
            BTreeMap::new(),
            BTreeMap::from([(
                "unexpected".into(),
                PreparedSubgraph::new(Vec::new(), Vec::new(), |_| Ok(Default::default())),
            )]),
        ] {
            let error = factory("each", config.clone(), bodies).err().unwrap();
            assert!(error.to_string().contains("prepared Flow field"), "{error}");
        }
    }

    fn body_relations(input: &str) -> mf_runtime::GraphRelations {
        mf_runtime::GraphRelations {
            scope: Some(mf_runtime::ScopeRelations {
                input: input.into(),
                values: BTreeMap::from([
                    ("item".into(), ValueType::Any),
                    ("key".into(), ValueType::Any),
                ]),
                state_outputs: BTreeMap::new(),
                writes: BTreeMap::new(),
                exits: Default::default(),
            }),
            ..Default::default()
        }
    }

    fn result_body() -> PreparedSubgraph {
        PreparedSubgraph::new(
            Vec::new(),
            vec![PortSpec::new("result", ValueType::Any, true)],
            |_| Ok(Outputs::new()),
        )
    }

    fn business_error(error: &NodeBuildError) -> &IterationValidationError {
        let NodeBuildError::FactoryFailed { source } = error else {
            panic!("expected owning-node business error, got {error}");
        };
        source.downcast_ref().unwrap()
    }

    #[test]
    fn iteration_invocation_paths_use_the_actual_child_edge_key() {
        for mode in [IterationMode::Sequential, IterationMode::Parallel] {
            for compiled in [false, true] {
                let key = mf_runtime::ScopePathEntry::binding_key("mapped-owner", "selected");
                let expected = if compiled { key.clone() } else { "each".into() };
                let mut body = PreparedSubgraph::new(
                    Vec::new(),
                    vec![PortSpec::new("result", ValueType::Any, true)],
                    move |state| {
                        let path = state.invocation_path();
                        assert_eq!(path.len(), 1);
                        assert_eq!(path[0].node_id(), expected);
                        assert_eq!(path[0].index().get(), 0);
                        Ok(Outputs::from([("result".into(), 1.into())]))
                    },
                );
                if compiled {
                    let mut relations = body_relations("input");
                    relations.key = Some(key);
                    body = body.with_relations(relations);
                }
                let node = IterationNode::new("each", mode, IterationErrorPolicy::Terminate, body)
                    .unwrap();
                let mut context = ExecutionContext::default();
                context
                    .with_node("each".into(), |context| {
                        let result = TaskNode::execute(
                            &node,
                            Inputs::from([("items".into(), json!([7]).into())]),
                            context,
                        );
                        assert_eq!(context.current_node(), Some("each"));
                        assert!(context.invocation_path().is_empty());
                        result
                    })
                    .unwrap();
                assert!(context.current_node().is_none());
            }
        }
    }

    #[test]
    fn iteration_checks_compiled_scope_bindings_before_execution() {
        assert!(
            IterationNode::new(
                "each",
                IterationMode::Sequential,
                IterationErrorPolicy::Terminate,
                result_body().with_relations(body_relations("custom-input")),
            )
            .is_ok()
        );
        let mut invalid = vec![mf_runtime::GraphRelations::default()];
        for mismatch in 0..7 {
            let mut relations = body_relations("custom-input");
            let scope = relations.scope.as_mut().unwrap();
            match mismatch {
                0 => scope.values.clear(),
                1 => {
                    scope.values.insert("item".into(), ValueType::String);
                }
                2 => {
                    scope.values.remove("key");
                }
                3 => {
                    scope.values.insert("extra".into(), ValueType::Any);
                }
                4 => {
                    scope.state_outputs.insert("item".into(), "item".into());
                }
                5 => {
                    scope.writes.insert("assign".into(), "item".into());
                }
                6 => {
                    scope.exits.insert("exit".into());
                }
                _ => unreachable!(),
            }
            invalid.push(relations);
        }
        for relations in invalid {
            let body = PreparedSubgraph::new(
                Vec::new(),
                vec![PortSpec::new("result", ValueType::Any, true)],
                |_| panic!("invalid scope bindings must be rejected before execution"),
            )
            .with_relations(relations);
            let error = IterationNode::new(
                "each",
                IterationMode::Sequential,
                IterationErrorPolicy::Terminate,
                body,
            )
            .err()
            .unwrap();
            assert!(matches!(
                business_error(&error),
                IterationValidationError::InvalidScopeBindings
            ));
        }
    }

    #[test]
    fn iteration_requires_exactly_one_required_result() {
        for outputs in [
            vec![],
            vec![PortSpec::new("other", ValueType::Any, true)],
            vec![PortSpec::new("result", ValueType::Any, false)],
            vec![
                PortSpec::new("result", ValueType::Any, true),
                PortSpec::new("other", ValueType::Any, true),
            ],
            vec![
                PortSpec::new("result", ValueType::Any, true),
                PortSpec::new("result", ValueType::Any, true),
            ],
        ] {
            let mut body = result_body();
            body.outputs = outputs;
            let error = IterationNode::new(
                "each",
                IterationMode::Sequential,
                IterationErrorPolicy::Terminate,
                body,
            )
            .err()
            .unwrap();
            assert!(matches!(
                business_error(&error),
                IterationValidationError::InvalidResult
            ));
        }
    }

    #[test]
    fn iteration_checks_prepared_and_configured_stream_names() {
        for name in ["", " ", "results"] {
            let mut body = result_body();
            body.stream_outputs
                .push(PortSpec::owned(name, ValueType::Any, true));
            let error = IterationNode::new(
                "each",
                IterationMode::Sequential,
                IterationErrorPolicy::Terminate,
                body,
            )
            .err()
            .unwrap();
            assert!(matches!(
                business_error(&error),
                IterationValidationError::InvalidStreamName
            ));

            let error = factory(
                "each",
                json!({
                    "body": {"poison": "not a workflow"},
                    "stream_outputs": {(name): {"node":"identity", "port":"value"}},
                }),
                BTreeMap::from([("/body".into(), result_body())]),
            )
            .err()
            .unwrap();
            assert!(matches!(
                business_error(&error),
                IterationValidationError::InvalidStreamName
            ));
        }
    }

    #[test]
    fn iteration_uses_actual_prepared_body_and_keeps_codec_errors_typed() {
        let config = json!({"body": {"outputs": "poison", "version": false}});
        assert!(
            factory(
                "each",
                config.clone(),
                BTreeMap::from([("/body".into(), result_body())]),
            )
            .is_ok()
        );
        let mut invalid = config.clone();
        invalid["mode"] = json!(false);
        let error = factory(
            "each",
            invalid,
            BTreeMap::from([("/body".into(), result_body())]),
        )
        .err()
        .unwrap();
        assert!(matches!(error, NodeBuildError::InvalidConfiguration { .. }));

        let mut body = result_body();
        body.outputs.clear();
        let error = factory("each", config, BTreeMap::from([("/body".into(), body)]))
            .err()
            .unwrap();
        assert!(matches!(
            business_error(&error),
            IterationValidationError::InvalidResult
        ));
    }

    #[test]
    fn iteration_assignment_refreshes_the_prepared_scope_input_or_manual_default() {
        for scope_input in [None, Some("custom-iteration-input")] {
            let source_id = scope_input.unwrap_or(mf_runtime::ITERATION_INPUT_ID);
            let source = mf_runtime::prepared_scope_source(
                source_id,
                &BTreeMap::from([
                    ("item".into(), ValueType::Any),
                    ("key".into(), ValueType::Any),
                ]),
            )
            .into_task()
            .unwrap();
            let assign = mf_runtime::prepared_loop_assign("assign", "item", ValueType::Any)
                .into_task()
                .unwrap();
            let mut body = PreparedSubgraph::new(
                Vec::new(),
                vec![PortSpec::new("result", ValueType::Any, true)],
                move |state| {
                    execute_node_in_context(&source, &[], state)?;
                    execute_node_in_context(
                        &assign,
                        &[mf_runtime::ExecutionDependency {
                            exclusive: false,
                            input: Some("value"),
                            source_node: source_id,
                            source_output: "index",
                            source_key: mf_runtime::output_id(source_id, "index").into(),
                        }],
                        state,
                    )?;
                    let item = state
                        .select_output("result", source_id, "item", false)?
                        .unwrap();
                    Ok(Outputs::from([("result".into(), item)]))
                },
            );
            if let Some(input) = scope_input {
                body = body.with_relations(body_relations(input));
            }
            for mode in [IterationMode::Sequential, IterationMode::Parallel] {
                let node =
                    IterationNode::new("each", mode, IterationErrorPolicy::Terminate, body.clone())
                        .unwrap();
                let result = TaskNode::execute(
                    &node,
                    Inputs::from([("items".into(), json!([7, 8]).into())]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
                assert_eq!(result.outputs["results"], json!([0, 1]));
            }
        }
    }

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
        let config = json!({
            "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]},
            "mode": {"nodes": []}, "on_error": {"nodes": []},
            "stream_outputs": {"nodes": []},
        });
        assert_eq!(
            registration.flow_parameters(&config).unwrap(),
            BTreeMap::from([("/body".into(), config["body"].clone())])
        );
        for config in [json!({}), json!({"body": null})] {
            assert_eq!(
                registration.flow_parameters(&config).unwrap(),
                BTreeMap::from([("/body".into(), Value::Null)])
            );
        }
        assert!(registration.instantiate(json!({})).is_err());
        let node = registration
            .instantiate_subgraphs(
                "iteration",
                json!({
                    "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]}
                }),
                BTreeMap::from([(
                    "/body".into(),
                    PreparedSubgraph::new(
                        Vec::new(),
                        vec![PortSpec::new("result", ValueType::Any, true)],
                        |_| Ok(Outputs::new()),
                    ),
                )]),
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
