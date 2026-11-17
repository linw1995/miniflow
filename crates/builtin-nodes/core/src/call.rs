use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeConfig, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec,
    PreparedNode, PreparedSubgraph, SubgraphNode, SubgraphStream, WorkflowArguments,
};
use serde_json::Value;
use snafu::{IntoError, ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    flow: String,
    body: PreparedSubgraph,
}

fn plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    crate::call_input::factory(config)
}

fn flow_parameters(config: &Value) -> BTreeMap<String, Value> {
    // Only static mode owns executable configuration. Mixed input-call fields
    // cannot authorize a child. Other ordinary fields remain decoder-owned, so
    // invalid static configuration keeps its typed factory error.
    if let Some(fields) = config.as_object()
        && !fields.contains_key("inputs")
        && !fields.contains_key("outputs")
        && !fields.contains_key("stream_outputs")
        && fields.get("flow").is_some_and(Value::is_string)
        && fields.get("body").is_some_and(|body| !body.is_null())
    {
        Config::flow_parameters(config)
    } else {
        BTreeMap::new()
    }
}

pub struct CallNode {
    id: String,
    scope_key: String,
    flow: String,
    body: PreparedSubgraph,
}

impl NodePortContract for CallNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: self
                .body
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
                .collect(),
            outputs: self.body.outputs.clone(),
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(display("Flow call `{node}` ({flow}): {source}"))]
pub struct CallFailure {
    node: String,
    flow: String,
    pub source: mf_runtime::WorkflowRunError,
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
        let outputs = self
            .invoke(WorkflowArguments(inputs), context, sink)
            .context(CallFailureSnafu {
                node: &self.id,
                flow: &self.flow,
            })?;
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

impl CallNode {
    pub fn new(
        id: &str,
        binding: &str,
        flow: String,
        body: PreparedSubgraph,
    ) -> Result<Self, NodeBuildError> {
        ensure!(!flow.trim().is_empty(), BlankFlowSnafu);
        ensure!(body.inputs.stdin.is_empty(), StdinSnafu);
        ensure!(
            body.relations
                .as_ref()
                .is_none_or(|relations| relations.is_callable()),
            ScopedBodySnafu
        );
        Ok(Self {
            id: id.into(),
            scope_key: body.scope_key(id, binding),
            flow,
            body,
        })
    }

    fn invoke(
        &self,
        arguments: WorkflowArguments,
        context: &mut ExecutionContext,
        sink: &mut dyn mf_runtime::EmissionSink,
    ) -> Result<mf_runtime::FlowOutputs, mf_runtime::WorkflowRunError> {
        let mut stream = SubgraphStream::new(self.id.as_str(), context, sink);
        context.run_call(&self.scope_key, arguments, &self.body.nodes, |state| {
            self.body.invoke_with_sink(state, &mut stream)
        })
    }

    /// Uses the same Call boundary with flat arguments and a terminal result envelope.
    pub fn execute_packed(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_packed_configured(inputs, context, None)
    }

    pub fn execute_packed_with_flow(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        flow: &mf_runtime::FlowValue,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_packed_configured(inputs, context, Some(flow))
    }

    fn execute_packed_configured(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        flow: Option<&mf_runtime::FlowValue>,
    ) -> Result<NodeResult, NodeExecutionError> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let mut arguments = WorkflowArguments(
            inputs["arguments"]
                .as_object()
                .expect("validated argument object")
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect(),
        );
        let outputs = (|| {
            if let Some(flow) = flow {
                arguments = flow.invocation_arguments(arguments)?;
            }
            arguments.qualify_inputs(&self.body.inputs)?;
            self.body.inputs.mark_secrets(&mut arguments);
            self.invoke(arguments, context, &mut mf_runtime::DiscardSink)
        })();
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let value = match outputs {
            Ok(outputs) => mf_runtime::ValueRef::object([(
                "outputs".into(),
                mf_runtime::ValueRef::object(
                    outputs
                        .into_iter()
                        .map(|(name, value)| (name.into(), value)),
                ),
            )]),
            Err(error) if is_business_failure(&error) => {
                mf_runtime::ValueRef::from(serde_json::json!({"error":error.to_string()}))
                    .into_secret()
            }
            Err(error) => {
                return Err(CallFailureSnafu {
                    node: &self.id,
                    flow: &self.flow,
                }
                .into_error(error)
                .into());
            }
        };
        Ok(mf_runtime::Outputs::from([("result".into(), value)]).into())
    }
}

// Only explicit domain failures become tool results; cancellation, resources,
// stream failures, and invalid runtime contracts retain their typed error chain.
fn is_business_failure(error: &mf_runtime::WorkflowRunError) -> bool {
    use mf_runtime::WorkflowRunError;
    if matches!(error, WorkflowRunError::WorkflowInputs { .. }) {
        return true;
    }
    let WorkflowRunError::NodeExecution { source, .. } = error else {
        return false;
    };
    match source {
        NodeExecutionError::ExecutionFailed { .. } => true,
        NodeExecutionError::PluginFailed { source } => {
            if let Some(error) = source.downcast_ref::<WorkflowRunError>() {
                return is_business_failure(error);
            }
            if let Some(error) = source.downcast_ref::<CallFailure>() {
                return is_business_failure(&error.source);
            }
            match source.downcast_ref::<crate::CallInputError>() {
                Some(crate::CallInputError::Execution { source, .. }) => {
                    is_business_failure(source)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

#[derive(Debug, Snafu)]
pub enum CallBuildError {
    #[snafu(display("Flow alias must not be blank"))]
    BlankFlow,
    #[snafu(display("Flow calls cannot read stdin"))]
    Stdin,
    #[snafu(display(
        "Flow calls require a callable prepared child without an implicit scope frame"
    ))]
    ScopedBody,
}

impl From<CallBuildError> for NodeBuildError {
    fn from(source: CallBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

fn factory(
    id: &str,
    config: Value,
    bodies: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let Config { flow, body } = Config::from_config(config, bodies)?;
    let input_bindings: BTreeMap<_, _> = body
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
    let node = CallNode::new(id, "body", flow, body)?;
    let metadata = NodeMetadata {
        input_bindings,
        ..Default::default()
    };
    PreparedNode::subgraph(node, metadata)
}

inventory::submit! {
    NodeRegistration {
        kind: mf_runtime::CALL_KIND,
        factory: NodeFactory::Subgraph {
            scope: Some(mf_runtime::SubgraphScope::Host),
            flow_parameters,
            plain,
            prepared: factory,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body() -> PreparedSubgraph {
        PreparedSubgraph::new(vec![], vec![], |_| Ok(Default::default()))
    }

    fn packed(
        body: PreparedSubgraph,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let node = CallNode::new("invoke", "body", "tool".into(), body).unwrap();
        context.with_node("invoke".into(), |context| {
            node.execute_packed(
                Inputs::from([("arguments".into(), mf_runtime::ValueRef::from(json!({})))]),
                context,
            )
        })
    }

    fn has_source<T: std::error::Error + 'static>(error: &dyn std::error::Error) -> bool {
        let mut source = error.source();
        while let Some(cause) = source {
            if cause.is::<T>() {
                return true;
            }
            source = cause.source();
        }
        false
    }

    #[test]
    fn packed_calls_recover_only_business_failures_and_preserve_typed_infrastructure_sources() {
        use mf_runtime::WorkflowRunError;
        #[derive(mf_runtime::NodeValue)]
        struct InvalidOutput {
            value: f64,
        }
        for nested in [false, true] {
            for kind in [
                "business", "context", "stream", "worker", "contract", "encode", "plugin",
            ] {
                let inner = PreparedSubgraph::new(vec![], vec![], move |_| {
                    let source = match kind {
                        "business" => mf_runtime::NodeExecutionFailedSnafu {
                            message: "domain failure with private data",
                        }
                        .build(),
                        "context" => {
                            return Err(WorkflowRunError::Context {
                                definition_id: "inner".into(),
                                message: "scope/resource failure".into(),
                            });
                        }
                        "stream" => mf_runtime::StreamExecutionSnafu {
                            message: "stream failure",
                        }
                        .build()
                        .into(),
                        "worker" => mf_runtime::WorkerPoolError {
                            worker: 0,
                            source: std::io::Error::other("worker failure"),
                        }
                        .into(),
                        "contract" => NodeExecutionError::from(Box::<
                            dyn std::error::Error + Send + Sync,
                        >::from(
                            mf_telemetry::ContractError::Invalid {
                                message: "contract failure".into(),
                            },
                        )),
                        "encode" => {
                            return mf_runtime::encode_typed_result(
                                mf_runtime::TypedNodeResult::from(InvalidOutput {
                                    value: f64::NAN,
                                }),
                            )
                            .map(|result| result.outputs)
                            .context(mf_runtime::NodeExecutionSnafu {
                                definition_id: "inner",
                            });
                        }
                        _ => NodeExecutionError::from(
                            Box::<dyn std::error::Error + Send + Sync>::from(
                                std::io::Error::other("unknown plugin failure"),
                            ),
                        ),
                    };
                    Err(source).context(mf_runtime::NodeExecutionSnafu {
                        definition_id: "inner",
                    })
                });
                let body = if nested {
                    let call = factory(
                        "nested",
                        json!({"flow":"child","body":{}}),
                        BTreeMap::from([("body".into(), inner)]),
                    )
                    .unwrap()
                    .execution
                    .into_task_node()
                    .unwrap();
                    PreparedSubgraph::new(vec![], vec![], move |context| {
                        call.execute(Inputs::new(), context)
                            .map(|result| result.outputs)
                            .context(mf_runtime::NodeExecutionSnafu {
                                definition_id: "nested",
                            })
                    })
                } else {
                    inner
                };
                let result = packed(body, &mut ExecutionContext::default());
                if kind == "business" {
                    let result = result.unwrap().outputs.remove("result").unwrap();
                    assert!(result["error"].as_str().unwrap().contains("domain failure"));
                    assert_eq!(
                        serde_json::to_value(result.redacted()).unwrap(),
                        json!("[REDACTED]")
                    );
                    continue;
                }
                let error = result.unwrap_err();
                assert!(has_source::<CallFailure>(&error));
                match kind {
                    "stream" => assert!(has_source::<mf_runtime::StreamError>(&error)),
                    "worker" => assert!(has_source::<mf_runtime::WorkerPoolError>(&error)),
                    "contract" => assert!(has_source::<mf_telemetry::ContractError>(&error)),
                    "encode" => assert!(has_source::<Box<mf_runtime::OutputEncodeError>>(&error)),
                    "plugin" => assert!(has_source::<std::io::Error>(&error)),
                    _ => assert!(error.to_string().contains("scope/resource failure")),
                }
            }
        }
    }

    #[test]
    fn packed_calls_never_recover_cancellation_even_after_body_completion() {
        let body = PreparedSubgraph::new(vec![], vec![], |state| {
            state
                .cancellation()
                .cancel(mf_runtime::StreamError::InputFailure {
                    message: "cancelled".into(),
                });
            Ok(Default::default())
        });
        assert!(packed(body, &mut ExecutionContext::default()).is_err());
        let mut context = ExecutionContext::default();
        context
            .cancellation()
            .cancel(mf_runtime::StreamError::InputFailure {
                message: "already cancelled".into(),
            });
        let body = PreparedSubgraph::new(vec![], vec![], |_| panic!("a cancelled call cannot run"));
        assert!(packed(body, &mut context).is_err());
    }

    #[test]
    fn packed_calls_drain_stream_emissions_and_keep_native_output_privacy() {
        let body = PreparedSubgraph::streaming(
            vec![],
            vec![mf_runtime::PortSpec::new(
                "value",
                mf_runtime::ValueType::String,
                true,
            )],
            vec![mf_runtime::PortSpec::new(
                "partial",
                mf_runtime::ValueType::String,
                true,
            )],
            |_, sink| {
                sink.send(
                    mf_runtime::Outputs::from([("partial".into(), "discarded".into())]).into(),
                )
                .map_err(NodeExecutionError::from)
                .context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "stream",
                })?;
                Ok(mf_runtime::Outputs::from([(
                    "value".into(),
                    mf_runtime::ValueRef::from("private").into_secret(),
                )]))
            },
        );
        let result = packed(body, &mut ExecutionContext::default()).unwrap();
        assert_eq!(
            result.outputs["result"]["outputs"]["value"].as_str(),
            Some("private")
        );
        assert!(result.outputs["result"].contains_secret());
    }

    #[test]
    fn call_invocation_paths_use_the_actual_child_edge_key() {
        for compiled in [false, true] {
            for mapped in [false, true] {
                let key = mf_runtime::ScopePathEntry::binding_key("mapped-owner", "selected");
                let expected = if !compiled {
                    "invoke".to_owned()
                } else if mapped {
                    key.clone()
                } else {
                    mf_runtime::ScopePathEntry::binding_key("invoke", "body")
                };
                let mut body = PreparedSubgraph::new(Vec::new(), Vec::new(), move |state| {
                    let path = state.invocation_path();
                    assert_eq!(path.len(), 1);
                    assert_eq!(path[0].node_id(), expected);
                    assert!(path[0].is_call());
                    Ok(Default::default())
                });
                if compiled {
                    body = body.with_relations(mf_runtime::GraphRelations {
                        key: mapped.then_some(key),
                        ..Default::default()
                    });
                }
                let node = mf_runtime::FlowNode::new(
                    "invoke",
                    factory(
                        "invoke",
                        json!({"flow":"child", "body":{}}),
                        BTreeMap::from([("body".into(), body)]),
                    )
                    .unwrap(),
                )
                .into_task()
                .unwrap();
                let mut context = ExecutionContext::default();
                context
                    .with_node("invoke".into(), |context| {
                        let result = node.node.execute(Inputs::new(), context);
                        assert_eq!(context.current_node(), Some("invoke"));
                        assert!(context.invocation_path().is_empty());
                        result
                    })
                    .unwrap();
                assert!(context.current_node().is_none());
            }
        }
    }

    #[test]
    fn call_input_mode_is_plain_and_has_no_executable_configuration() {
        let registration = mf_runtime::NodeRegistry::from_inventory()
            .unwrap()
            .get(mf_runtime::CALL_KIND)
            .unwrap();
        let config = json!({
            "inputs": [PortSpec::new("params.value", mf_runtime::ValueType::Int64, true)],
            "outputs": [PortSpec::new("value", mf_runtime::ValueType::Int64, true)]
        });
        assert!(mf_runtime::CallConfig::is_dynamic(&config));
        assert!(registration.flow_parameters(&config).is_empty());
        let prepared = registration.instantiate(config).unwrap();
        assert!(matches!(
            prepared.execution,
            mf_runtime::NodeExecution::Task(_)
        ));
        assert_eq!(
            prepared.metadata.ports.inputs,
            [
                PortSpec::new("flow", mf_runtime::ValueType::Flow, true),
                PortSpec::new("params.value", mf_runtime::ValueType::Int64, true),
            ]
        );
        let mixed = json!({"inputs":[], "outputs":[], "flow":"child", "body":{}});
        assert!(!mf_runtime::CallConfig::is_dynamic(&mixed));
        assert!(registration.flow_parameters(&mixed).is_empty());
        assert!(registration.instantiate(mixed.clone()).is_err());
        assert!(factory("call", mixed, BTreeMap::from([("body".into(), body())])).is_err());
    }

    #[test]
    fn call_factory_requires_a_valid_config_and_exact_prepared_body() {
        let config = json!({
            "flow": "child",
            "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]}
        });
        let registration = mf_runtime::NodeRegistry::from_inventory()
            .unwrap()
            .get(mf_runtime::CALL_KIND)
            .unwrap();
        assert_eq!(
            registration.flow_parameters(&config),
            BTreeMap::from([("body".into(), config["body"].clone())])
        );
        for config in [
            json!({}),
            json!({"body": null, "flow": "child"}),
            json!({"body": {}, "flow": {"nodes": []}}),
        ] {
            assert!(registration.flow_parameters(&config).is_empty());
        }
        assert!(registration.instantiate(config.clone()).is_err());
        let mut unknown = config.clone();
        unknown["unexpected"] = json!(true);
        assert_eq!(
            registration.flow_parameters(&unknown),
            registration.flow_parameters(&config)
        );
        assert!(matches!(
            factory("invoke", unknown, BTreeMap::from([("body".into(), body())])),
            Err(NodeBuildError::InvalidConfiguration { .. })
        ));
        for bodies in [
            BTreeMap::new(),
            BTreeMap::from([("other".into(), body())]),
            BTreeMap::from([("body".into(), body()), ("extra".into(), body())]),
        ] {
            assert!(factory("invoke", config.clone(), bodies).is_err());
        }
        let mut invalid = config.clone();
        invalid["body"] = Value::Null;
        assert!(factory("invoke", invalid, BTreeMap::from([("body".into(), body())])).is_err());
        assert!(factory("invoke", config, BTreeMap::from([("body".into(), body())])).is_ok());
    }

    #[test]
    fn call_factory_uses_actual_body_and_keeps_codec_errors_typed() {
        let config = json!({"flow":"child", "body":{"version":false, "poison":true}});
        assert!(
            factory(
                "invoke",
                config.clone(),
                BTreeMap::from([("body".into(), body())]),
            )
            .is_ok()
        );
        let mut invalid = config;
        invalid["flow"] = json!(false);
        let error = factory("invoke", invalid, BTreeMap::from([("body".into(), body())]))
            .err()
            .unwrap();
        assert!(matches!(error, NodeBuildError::InvalidConfiguration { .. }));
    }

    fn business_error(error: &NodeBuildError) -> &CallBuildError {
        let NodeBuildError::FactoryFailed { source } = error else {
            panic!("expected owning-node business error, got {error}");
        };
        source.downcast_ref().unwrap()
    }

    #[test]
    fn direct_call_factories_reject_blank_names_and_stdin_before_execution() {
        let error = factory(
            "invoke",
            json!({"flow":" ", "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]}}),
            BTreeMap::from([("body".into(), body())]),
        )
        .err()
        .unwrap();
        assert!(matches!(business_error(&error), CallBuildError::BlankFlow));
        let mut inputs = mf_runtime::WorkflowInputSchema::default();
        inputs
            .stdin
            .insert("read".into(), mf_runtime::StdinRequirement::Always);
        let error = factory(
            "invoke",
            json!({"flow":"child", "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]}}),
            BTreeMap::from([("body".into(), body().with_input_schema(inputs))]),
        )
        .err()
        .unwrap();
        assert!(matches!(business_error(&error), CallBuildError::Stdin));
        let scoped = body().with_relations(mf_runtime::GraphRelations {
            scope: Some(mf_runtime::ScopeRelations {
                input: "scope".into(),
                values: BTreeMap::new(),
                state_outputs: BTreeMap::new(),
                writes: BTreeMap::new(),
                exits: Default::default(),
            }),
            ..Default::default()
        });
        let error = factory(
            "invoke",
            json!({"flow":"child", "body":{}}),
            BTreeMap::from([("body".into(), scoped)]),
        )
        .err()
        .unwrap();
        assert!(matches!(business_error(&error), CallBuildError::ScopedBody));
    }
}
