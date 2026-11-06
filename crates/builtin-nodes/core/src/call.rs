use mf_runtime::{
    CallConfig, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec,
    PreparedNode, PreparedSubgraph, SubgraphNode, SubgraphStream, WorkflowArguments,
};
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};

struct CallNode {
    id: String,
    flow: String,
    body: PreparedSubgraph,
    arguments_object: bool,
    capture_errors: bool,
}

impl NodePortContract for CallNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: if self.arguments_object {
                vec![PortSpec::new(
                    "arguments",
                    mf_runtime::ValueType::Object,
                    true,
                )]
            } else {
                self.body
                    .inputs
                    .inputs
                    .iter()
                    .flat_map(|(node, ports)| {
                        ports.iter().map(move |(name, input)| {
                            PortSpec::owned(
                                mf_runtime::output_id(node, name),
                                input.value_type.clone(),
                                input.required
                                    && input.binding.env.is_none()
                                    && input.binding.default.is_none(),
                            )
                        })
                    })
                    .collect()
            },
            outputs: if self.capture_errors {
                vec![PortSpec::new("result", mf_runtime::ValueType::Object, true)]
            } else {
                self.body.outputs.clone()
            },
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(display("Flow call `{node}` ({flow}): {source}"))]
struct CallFailure {
    node: String,
    flow: String,
    source: mf_runtime::WorkflowRunError,
}

impl From<CallFailure> for NodeExecutionError {
    fn from(source: CallFailure) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl SubgraphNode for CallNode {
    fn body(&self) -> &PreparedSubgraph {
        &self.body
    }

    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn mf_runtime::EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_target(inputs, context, sink, None)
    }
}

impl CallNode {
    fn execute_target(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn mf_runtime::EmissionSink,
        binding: Option<&str>,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut stream = SubgraphStream::new(self.id.as_str(), context, sink);
        let inputs = if self.arguments_object {
            let arguments = inputs["arguments"]
                .as_object()
                .expect("validated argument object");
            arguments
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect()
        } else {
            inputs
        };
        let run = |state: &mut ExecutionContext| self.body.execute_with_sink(state, &mut stream);
        let result = match binding {
            Some(binding) => context.run_call_binding(
                &self.id,
                binding,
                WorkflowArguments(inputs),
                &self.body.nodes,
                run,
            ),
            None => context.run_call(&self.id, WorkflowArguments(inputs), &self.body.nodes, run),
        }
        .context(CallFailureSnafu {
            node: &self.id,
            flow: &self.flow,
        });
        let outputs = if self.capture_errors {
            if let Some(error) = context.cancellation().failure() {
                return Err(error.into());
            }
            let value = match result {
                Ok(outputs) => mf_runtime::ValueRef::object([(
                    "outputs".into(),
                    mf_runtime::ValueRef::object(
                        outputs
                            .into_iter()
                            .map(|(name, value)| (name.into(), value)),
                    ),
                )]),
                Err(error) => {
                    // Infrastructure failure must never be mistaken for a recoverable child result.
                    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
                    let mut infrastructure = false;
                    while let Some(cause) = source {
                        infrastructure |= cause.is::<mf_runtime::WorkerPoolError>()
                            || matches!(
                                cause.downcast_ref::<mf_runtime::WorkflowRunError>(),
                                Some(mf_runtime::WorkflowRunError::WorkerPool { .. })
                            )
                            || matches!(
                                cause.downcast_ref::<mf_runtime::StreamError>(),
                                Some(
                                    mf_runtime::StreamError::WorkerStartup { .. }
                                        | mf_runtime::StreamError::ThreadSpawn { .. }
                                )
                            );
                        source = cause.source();
                    }
                    if infrastructure {
                        return Err(error.into());
                    }
                    mf_runtime::ValueRef::from(serde_json::json!({"error": error.to_string()}))
                }
            };
            return Ok(NodeResult {
                outputs: mf_runtime::Outputs::from([("result".into(), value)]),
                ..Default::default()
            });
        } else {
            result?
        };
        let skipped = self
            .body
            .outputs
            .iter()
            .filter(|port| !outputs.contains_key(port.name.as_ref()))
            .map(|port| port.name.to_string())
            .collect();
        Ok(NodeResult {
            outputs,
            skipped,
            ..Default::default()
        })
    }
}

fn factory(
    id: &str,
    config: Value,
    _options: Value,
    body: PreparedSubgraph,
) -> Result<PreparedNode, NodeBuildError> {
    let config: CallConfig = mf_runtime::deserialize_config(config)?;
    ensure!(
        config.flow_ref.is_none(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "static calls cannot specify flow_ref",
        }
    );
    ensure!(
        !config.flow.trim().is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "Flow alias must not be blank"
        }
    );
    ensure!(
        body.inputs.stdin.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "Flow calls cannot read stdin"
        }
    );
    let mut input_bindings: std::collections::BTreeMap<_, _> = body
        .inputs
        .inputs
        .iter()
        .flat_map(|(node, ports)| {
            ports
                .iter()
                .filter(|(_, input)| input.binding.secret)
                .map(move |(name, _)| {
                    (
                        mf_runtime::output_id(node, name),
                        mf_runtime::InputBinding {
                            secret: true,
                            ..Default::default()
                        },
                    )
                })
        })
        .collect();
    if config.arguments_object {
        let secret = !input_bindings.is_empty();
        input_bindings.clear();
        if secret {
            input_bindings.insert(
                "arguments".into(),
                mf_runtime::InputBinding {
                    secret: true,
                    ..Default::default()
                },
            );
        }
    }
    let node = CallNode {
        id: id.into(),
        flow: config.flow,
        body,
        arguments_object: config.arguments_object,
        capture_errors: config.capture_errors,
    };
    let metadata = NodeMetadata {
        input_bindings,
        ..Default::default()
    };
    PreparedNode::subgraph(node, metadata)
}

struct ReferenceCallNode {
    id: String,
    selector: mf_runtime::ConfigTemplate,
    arguments: mf_runtime::PreparedFlowArguments,
    stream_outputs: Vec<PortSpec>,
}

impl NodePortContract for ReferenceCallNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new(
                "arguments",
                mf_runtime::ValueType::Object,
                true,
            )],
            outputs: vec![PortSpec::new("result", mf_runtime::ValueType::Object, true)],
        }
    }

    fn stream_outputs(&self) -> Vec<PortSpec> {
        self.stream_outputs.clone()
    }
}

impl mf_runtime::TaskNode for ReferenceCallNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_target(inputs, context, &mut mf_runtime::DiscardSink)
    }
}

impl mf_runtime::TaskStreamNode for ReferenceCallNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_target(inputs, context, emitter)
    }
}

impl ReferenceCallNode {
    fn execute_target(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn mf_runtime::EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        use snafu::OptionExt;
        let selected = self.selector.resolve(context).boxed()?;
        let name = selected
            .as_str()
            .context(mf_runtime::NodeExecutionFailedSnafu {
                message: "Flow parameter reference must select a string name",
            })?;
        let body = self
            .arguments
            .get(name)
            .context(mf_runtime::NodeExecutionFailedSnafu {
                message: format!("unknown passed Flow parameter `{name}`"),
            })?;
        execute_passed_flow(&self.id, name, body, inputs, context, sink)
    }
}

/// Runs a passed Flow to completion with the receiving node's delivery policy.
/// Stream emissions are incremental; only the returned result envelope is terminal.
pub(super) fn execute_passed_flow(
    id: &str,
    binding: &str,
    body: &PreparedSubgraph,
    inputs: Inputs,
    context: &mut ExecutionContext,
    sink: &mut dyn mf_runtime::EmissionSink,
) -> Result<NodeResult, NodeExecutionError> {
    CallNode {
        id: id.into(),
        flow: binding.into(),
        body: body.clone(),
        arguments_object: true,
        capture_errors: true,
    }
    .execute_target(inputs, context, sink, Some(binding))
}

pub(super) fn passed_flow_metadata(
    arguments: &mf_runtime::PreparedFlowArguments,
) -> Result<NodeMetadata, NodeBuildError> {
    for body in arguments.values() {
        ensure!(
            body.inputs.stdin.is_empty(),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: "Flow parameter calls cannot read stdin"
            }
        );
    }
    let sensitive = arguments.values().any(|body| {
        body.inputs
            .inputs
            .values()
            .any(|ports| ports.values().any(|input| input.binding.secret))
    });
    Ok(NodeMetadata {
        input_bindings: if sensitive {
            [(
                "arguments".into(),
                mf_runtime::InputBinding {
                    secret: true,
                    ..Default::default()
                },
            )]
            .into()
        } else {
            Default::default()
        },
        ..Default::default()
    })
}

/// Dynamic choices share emission names, but may omit each other's outputs.
/// A name has one exact type across all choices; requiredness is their intersection.
fn passed_flow_stream_outputs(
    arguments: &mf_runtime::PreparedFlowArguments,
) -> Result<Vec<PortSpec>, NodeBuildError> {
    let mut outputs = std::collections::BTreeMap::<String, PortSpec>::new();
    for body in arguments.values() {
        for port in &body.stream_outputs {
            if let Some(previous) = outputs.get(port.name.as_ref()) {
                ensure!(
                    previous.value_type == port.value_type,
                    mf_runtime::NodeInvalidSubgraphSnafu {
                        message: format!(
                            "passed Flow stream output `{}` has conflicting types",
                            port.name
                        )
                    }
                );
            } else {
                outputs.insert(port.name.to_string(), port.clone());
            }
        }
    }
    for port in outputs.values_mut() {
        port.required = arguments.values().all(|body| {
            body.stream_outputs
                .iter()
                .any(|candidate| candidate.name == port.name && candidate.required)
        });
    }
    Ok(outputs.into_values().collect())
}

fn reference_factory(
    id: &str,
    config: Value,
    arguments: mf_runtime::PreparedFlowArguments,
) -> Result<PreparedNode, NodeBuildError> {
    use snafu::OptionExt;
    let config: CallConfig = mf_runtime::deserialize_config(config)?;
    ensure!(
        config.flow.is_empty()
            && config.definition.is_none()
            && config.arguments_object
            && config.capture_errors,
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "reference calls require arguments_object and capture_errors, and cannot specify flow or definition"
        }
    );
    let mut metadata = passed_flow_metadata(&arguments)?;
    let selector = config
        .flow_ref
        .context(mf_runtime::NodeInvalidSubgraphSnafu {
            message: "reference calls require flow_ref",
        })?
        .template()
        .boxed()
        .map_err(NodeBuildError::from)?;
    metadata.context_references = selector.references("flow_ref");
    let stream_outputs = passed_flow_stream_outputs(&arguments)?;
    let node = ReferenceCallNode {
        id: id.into(),
        selector,
        arguments,
        stream_outputs,
    };
    if node.stream_outputs.is_empty() {
        PreparedNode::new(node, metadata)
    } else {
        PreparedNode::task_stream(node, metadata)
    }
}

inventory::submit! {
    NodeRegistration { kind: mf_runtime::CALL_KIND, factory: NodeFactory::Call { fixed: factory, reference: reference_factory } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body() -> PreparedSubgraph {
        PreparedSubgraph::new(vec![], vec![], |_| Ok(Default::default()))
    }

    fn reference_config() -> Value {
        json!({
            "flow_ref": {"$ref": "params.target"},
            "arguments_object": true,
            "capture_errors": true
        })
    }

    #[derive(Default)]
    struct CollectSink(Vec<NodeResult>);

    impl mf_runtime::EmissionSink for CollectSink {
        fn send(&mut self, result: NodeResult) -> Result<(), mf_runtime::StreamError> {
            self.0.push(result);
            Ok(())
        }
    }

    #[test]
    fn reference_calls_reflect_union_stream_contract_and_keep_terminal_choices_as_tasks() {
        use mf_runtime::{PreparedFlowArguments, ValueType};
        let first = PreparedSubgraph::streaming(
            vec![],
            vec![],
            vec![
                PortSpec::new("shared", ValueType::String, true),
                PortSpec::new("first", ValueType::Int64, true),
            ],
            |_, _| Ok(Default::default()),
        );
        let second = PreparedSubgraph::streaming(
            vec![],
            vec![],
            vec![PortSpec::new("shared", ValueType::String, true)],
            |_, _| Ok(Default::default()),
        );
        let arguments =
            PreparedFlowArguments::new([("first".into(), first), ("second".into(), second)].into());
        let prepared = reference_factory("invoke", reference_config(), arguments).unwrap();
        assert!(matches!(
            prepared.execution,
            mf_runtime::NodeExecution::TaskStream(_)
        ));
        assert_eq!(
            prepared.metadata.stream_outputs,
            vec![
                PortSpec::new("first", ValueType::Int64, false),
                PortSpec::new("shared", ValueType::String, true),
            ]
        );
        assert_eq!(prepared.metadata.ports.outputs[0].name, "result");

        let arguments = PreparedFlowArguments::new([("terminal".into(), body())].into());
        let prepared = reference_factory("invoke", reference_config(), arguments).unwrap();
        assert!(matches!(
            prepared.execution,
            mf_runtime::NodeExecution::Task(_)
        ));
    }

    #[test]
    fn reference_calls_reject_conflicting_stream_types_but_accept_mixed_lifecycles() {
        use mf_runtime::{PreparedFlowArguments, ValueType};
        let streaming = |value_type| {
            PreparedSubgraph::streaming(
                vec![],
                vec![],
                vec![PortSpec::new("value", value_type, true)],
                |_, _| Ok(Default::default()),
            )
        };
        let arguments = PreparedFlowArguments::new(
            [
                ("first".into(), streaming(ValueType::String)),
                ("second".into(), streaming(ValueType::Int64)),
            ]
            .into(),
        );
        let error = reference_factory("invoke", reference_config(), arguments)
            .err()
            .unwrap();
        assert!(error.to_string().contains("`value` has conflicting types"));

        let arguments = PreparedFlowArguments::new(
            [
                ("terminal".into(), body()),
                ("stream".into(), streaming(ValueType::String)),
            ]
            .into(),
        );
        let prepared = reference_factory("invoke", reference_config(), arguments).unwrap();
        assert!(!prepared.metadata.stream_outputs[0].required);
    }

    #[test]
    fn reference_calls_forward_incremental_values_and_return_authoritative_envelopes() {
        for fail in [false, true] {
            let body = PreparedSubgraph::streaming(
                vec![],
                vec![],
                vec![PortSpec::new("chunk", mf_runtime::ValueType::String, true)],
                move |state, sink| {
                    assert_eq!(state.workflow_arguments().0["params.value"], json!("input"));
                    for chunk in ["one", "two"] {
                        sink.send(NodeResult {
                            outputs: mf_runtime::Outputs::from([("chunk".into(), chunk.into())]),
                            ..Default::default()
                        })
                        .map_err(NodeExecutionError::from)
                        .context(mf_runtime::NodeExecutionSnafu {
                            definition_id: "child",
                        })?;
                    }
                    if fail {
                        Err(mf_runtime::WorkflowRunError::Context {
                            definition_id: "child".into(),
                            message: "domain failure after emissions".into(),
                        })
                    } else {
                        Ok(mf_runtime::Outputs::from([(
                            "answer".into(),
                            "final".into(),
                        )]))
                    }
                },
            );
            let arguments = mf_runtime::PreparedFlowArguments::new([("child".into(), body)].into());
            let node = ReferenceCallNode {
                id: "invoke".into(),
                selector: mf_runtime::ConfigTemplate::parse(json!("child")).unwrap(),
                stream_outputs: passed_flow_stream_outputs(&arguments).unwrap(),
                arguments,
            };
            let prepared = PreparedNode::task_stream(node, NodeMetadata::default()).unwrap();
            let mut sink = CollectSink::default();
            let result = prepared
                .execution
                .as_task_node()
                .unwrap()
                .execute_with_sink(
                    Inputs::from([("arguments".into(), json!({"params.value": "input"}).into())]),
                    &mut ExecutionContext::default(),
                    &mut sink,
                )
                .unwrap();
            assert_eq!(sink.0.len(), 2);
            assert_eq!(sink.0[0].outputs["chunk"], json!("one"));
            assert_eq!(sink.0[1].outputs["chunk"], json!("two"));
            assert_eq!(result.outputs["result"].get("error").is_some(), fail);
            if !fail {
                assert_eq!(
                    result.outputs["result"]["outputs"]["answer"],
                    json!("final")
                );
            }
        }
    }

    #[test]
    fn passed_flow_sink_failures_cancel_instead_of_becoming_result_envelopes() {
        struct RejectSink;
        impl mf_runtime::EmissionSink for RejectSink {
            fn send(&mut self, _: NodeResult) -> Result<(), mf_runtime::StreamError> {
                Err(mf_runtime::StreamError::InputFailure {
                    message: "receiver rejected emission".into(),
                })
            }
        }
        let body = PreparedSubgraph::streaming(
            vec![],
            vec![],
            vec![PortSpec::new("chunk", mf_runtime::ValueType::String, true)],
            |_, sink| {
                sink.send(NodeResult {
                    outputs: mf_runtime::Outputs::from([("chunk".into(), "one".into())]),
                    ..Default::default()
                })
                .map_err(NodeExecutionError::from)
                .context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "child",
                })?;
                Ok(Default::default())
            },
        );
        let mut context = ExecutionContext::default();
        let result = execute_passed_flow(
            "invoke",
            "child",
            &body,
            Inputs::from([("arguments".into(), json!({}).into())]),
            &mut context,
            &mut RejectSink,
        );
        assert!(result.is_err());
        assert!(context.cancellation().failure().is_some());
    }

    #[test]
    fn passed_flows_reject_stdin_even_when_the_receiver_accepts_streams() {
        let mut schema = mf_runtime::WorkflowInputSchema::default();
        schema
            .stdin
            .insert("read".into(), mf_runtime::StdinRequirement::Always);
        let arguments = mf_runtime::PreparedFlowArguments::new(
            [("child".into(), body().with_input_schema(schema))].into(),
        );
        assert!(
            reference_factory("invoke", reference_config(), arguments)
                .err()
                .unwrap()
                .to_string()
                .contains("cannot read stdin")
        );
    }

    #[test]
    fn object_calls_are_generic_and_capture_only_recoverable_failures() {
        for fail in [false, true] {
            let body = PreparedSubgraph::new(vec![], vec![], move |state| {
                assert_eq!(
                    state.workflow_arguments().0["params.value"].as_str(),
                    Some("input")
                );
                if fail {
                    Err(mf_runtime::WorkflowRunError::Context {
                        definition_id: "child".into(),
                        message: "domain failure".into(),
                    })
                } else {
                    Ok(mf_runtime::Outputs::from([(
                        "value".into(),
                        "output".into(),
                    )]))
                }
            });
            let node = CallNode {
                id: "invoke".into(),
                flow: "child".into(),
                body,
                arguments_object: true,
                capture_errors: true,
            };
            let result = SubgraphNode::execute(
                &node,
                Inputs::from([("arguments".into(), json!({"params.value":"input"}).into())]),
                &mut ExecutionContext::default(),
                &mut mf_runtime::DiscardSink,
            )
            .unwrap();
            assert_eq!(result.outputs["result"].get("error").is_some(), fail);
            if !fail {
                assert_eq!(
                    result.outputs["result"]["outputs"]["value"].as_str(),
                    Some("output")
                );
            }
        }
    }

    #[test]
    fn object_argument_ports_preserve_child_sensitivity() {
        let mut schema = mf_runtime::WorkflowInputSchema::default();
        schema.inputs.entry("params".into()).or_default().insert(
            "token".into(),
            mf_runtime::WorkflowInput {
                value_type: mf_runtime::ValueType::String,
                required: false,
                binding: mf_runtime::InputBinding {
                    secret: true,
                    default: Some(json!("hidden")),
                    ..Default::default()
                },
            },
        );
        let prepared = factory(
            "invoke",
            json!({"flow":"child", "arguments_object":true}),
            Value::Null,
            body().with_input_schema(schema),
        )
        .unwrap();
        assert!(prepared.metadata.input_bindings["arguments"].secret);
        assert_eq!(prepared.metadata.ports.inputs[0].name, "arguments");
    }

    #[test]
    fn captured_calls_do_not_recover_cancellation() {
        let body = PreparedSubgraph::new(vec![], vec![], |state| {
            state
                .cancellation()
                .cancel(mf_runtime::StreamError::InputFailure {
                    message: "cancelled".into(),
                });
            Ok(Default::default())
        });
        let node = CallNode {
            id: "invoke".into(),
            flow: "child".into(),
            body,
            arguments_object: false,
            capture_errors: true,
        };
        assert!(
            SubgraphNode::execute(
                &node,
                Inputs::default(),
                &mut ExecutionContext::default(),
                &mut mf_runtime::DiscardSink
            )
            .is_err()
        );
    }

    #[test]
    fn captured_calls_preserve_worker_failure_sources() {
        let body = PreparedSubgraph::new(vec![], vec![], |_| {
            Err(mf_runtime::WorkerPoolError {
                worker: 0,
                source: std::io::Error::other("synthetic worker failure"),
            }
            .into())
        });
        let node = CallNode {
            id: "invoke".into(),
            flow: "child".into(),
            body,
            arguments_object: false,
            capture_errors: true,
        };
        let error = SubgraphNode::execute(
            &node,
            Inputs::default(),
            &mut ExecutionContext::default(),
            &mut mf_runtime::DiscardSink,
        )
        .unwrap_err();
        assert!(error.to_string().contains("synthetic worker failure"));
    }

    #[test]
    fn captured_calls_preserve_invocation_startup_failure_sources() {
        let body =
            PreparedSubgraph::from_stream_factory(vec![], Default::default(), vec![], || {
                Err(NodeExecutionError::from(
                    mf_runtime::StreamError::ThreadSpawn {
                        thread: "workflow-stream".into(),
                        source: std::sync::Arc::new(std::io::Error::other(
                            "synthetic invocation startup failure",
                        )),
                    },
                ))
                .context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "child",
                })
            });
        let node = CallNode {
            id: "invoke".into(),
            flow: "child".into(),
            body,
            arguments_object: false,
            capture_errors: true,
        };
        let error = SubgraphNode::execute(
            &node,
            Inputs::default(),
            &mut ExecutionContext::default(),
            &mut mf_runtime::DiscardSink,
        )
        .unwrap_err();
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        let mut found = false;
        while let Some(cause) = source {
            found |= cause
                .downcast_ref::<std::io::Error>()
                .or_else(|| {
                    cause
                        .downcast_ref::<std::sync::Arc<std::io::Error>>()
                        .map(AsRef::as_ref)
                })
                .is_some_and(|cause| cause.to_string() == "synthetic invocation startup failure");
            source = cause.source();
        }
        assert!(found, "startup failure source was lost: {error}");
    }

    #[test]
    fn call_factories_reject_mixing_static_and_reference_configuration() {
        let error = factory(
            "invoke",
            json!({"flow":"child", "flow_ref":{"$ref":"params.target"}}),
            Value::Null,
            body(),
        )
        .err()
        .unwrap();
        assert!(matches!(error, NodeBuildError::InvalidSubgraph { .. }));
        assert!(
            error
                .to_string()
                .contains("static calls cannot specify flow_ref")
        );

        for config in [
            json!({"flow_ref":{"$ref":"params.target"}, "capture_errors":true}),
            json!({"flow_ref":{"$ref":"params.target"}, "arguments_object":true}),
            json!({"flow":"child", "flow_ref":{"$ref":"params.target"}, "arguments_object":true, "capture_errors":true}),
            json!({"definition":{"version":"2026-11-01", "dependencies":{}, "nodes":[]}, "flow_ref":{"$ref":"params.target"}, "arguments_object":true, "capture_errors":true}),
        ] {
            let error = reference_factory("invoke", config, Default::default())
                .err()
                .unwrap();
            assert!(matches!(error, NodeBuildError::InvalidSubgraph { .. }));
            assert!(
                error
                    .to_string()
                    .contains("reference calls require arguments_object and capture_errors")
            );
        }
    }

    #[test]
    fn direct_call_factories_reject_blank_names_and_stdin_before_execution() {
        assert!(
            factory("invoke", json!({"flow":" "}), Value::Null, body())
                .err()
                .unwrap()
                .to_string()
                .contains("must not be blank")
        );
        let mut inputs = mf_runtime::WorkflowInputSchema::default();
        inputs
            .stdin
            .insert("read".into(), mf_runtime::StdinRequirement::Always);
        assert!(
            factory(
                "invoke",
                json!({"flow":"child"}),
                Value::Null,
                body().with_input_schema(inputs)
            )
            .err()
            .unwrap()
            .to_string()
            .contains("cannot read stdin")
        );
    }
}
