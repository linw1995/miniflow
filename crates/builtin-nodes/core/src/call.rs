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
                                Some(mf_runtime::StreamError::WorkerStartup { .. })
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
}

impl mf_runtime::TaskNode for ReferenceCallNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
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
        CallNode {
            id: self.id.clone(),
            flow: name.into(),
            body: body.clone(),
            arguments_object: true,
            capture_errors: true,
        }
        .execute_target(inputs, context, &mut mf_runtime::DiscardSink, Some(name))
    }
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
    for body in arguments.values() {
        ensure!(
            body.stream_outputs.is_empty() && body.inputs.stdin.is_empty(),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: "Flow parameter calls require terminal children without stdin"
            }
        );
    }
    let selector = config
        .flow_ref
        .context(mf_runtime::NodeInvalidSubgraphSnafu {
            message: "reference calls require flow_ref",
        })?
        .template()
        .boxed()
        .map_err(NodeBuildError::from)?;
    let sensitive = arguments.values().any(|body| {
        body.inputs
            .inputs
            .values()
            .any(|ports| ports.values().any(|input| input.binding.secret))
    });
    let metadata = NodeMetadata {
        context_references: selector.references("flow_ref"),
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
    };
    PreparedNode::new(
        ReferenceCallNode {
            id: id.into(),
            selector,
            arguments,
        },
        metadata,
    )
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
