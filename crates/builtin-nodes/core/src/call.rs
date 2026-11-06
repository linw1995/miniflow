use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeConfig, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec,
    PreparedNode, PreparedSubgraph, SubgraphNode, SubgraphStream, WorkflowArguments,
};
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    flow: String,
    body: PreparedSubgraph,
}

fn plain(_config: Value) -> Result<PreparedNode, NodeBuildError> {
    mf_runtime::NodeInvalidSubgraphSnafu {
        message: "workflow.call requires a prepared `body` binding",
    }
    .fail()
}

struct CallNode {
    id: String,
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
        let mut stream = SubgraphStream::new(self.id.as_str(), context, sink);
        let outputs = context
            .run_call(
                &self.id,
                WorkflowArguments(inputs),
                &self.body.nodes,
                |state| self.body.execute_with_sink(state, &mut stream),
            )
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

fn factory(
    id: &str,
    config: Value,
    bodies: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let Config { flow, body } = Config::from_config(config, bodies)?;
    ensure!(
        !flow.trim().is_empty(),
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
    let input_bindings = body
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
    let node = CallNode {
        id: id.into(),
        flow,
        body,
    };
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
            flow_parameters: Config::flow_parameters,
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
        for config in [json!({}), json!({"body": null, "flow": {"nodes": []}})] {
            assert!(registration.flow_parameters(&config).is_empty());
        }
        assert!(registration.instantiate(config.clone()).is_err());
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
    fn direct_call_factories_reject_blank_names_and_stdin_before_execution() {
        assert!(
            factory(
                "invoke",
                json!({"flow":" ", "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]}}),
                BTreeMap::from([("body".into(), body())]),
            )
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
                json!({"flow":"child", "body": {"version":"2026-10-28", "dependencies":{}, "nodes":[]}}),
                BTreeMap::from([("body".into(), body().with_input_schema(inputs))]),
            )
            .err()
            .unwrap()
            .to_string()
            .contains("cannot read stdin")
        );
    }
}
