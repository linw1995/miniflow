use mf_runtime::{
    CallConfig, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec,
    PreparedNode, PreparedSubgraph, TaskNode,
};
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};

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
                .public_inputs()
                .into_iter()
                .map(|(name, input)| PortSpec::owned(name, input.value_type, input.required))
                .collect(),
            outputs: self.body.outputs.clone(),
        }
    }

    fn stream_outputs(&self) -> Vec<PortSpec> {
        self.body.stream_outputs.clone()
    }
}

#[derive(Debug, Snafu)]
#[snafu(display("Flow call `{node}` ({flow}): {source}"))]
struct CallFailure {
    node: String,
    flow: String,
    source: mf_runtime::WorkflowRunError,
}

#[derive(Debug, Snafu)]
#[snafu(display("Flow call `{node}` arguments: {source}"))]
struct ArgumentsError {
    node: String,
    source: mf_runtime::WorkflowInputError,
}
impl From<ArgumentsError> for NodeExecutionError {
    fn from(source: ArgumentsError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl From<CallFailure> for NodeExecutionError {
    fn from(source: CallFailure) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl TaskNode for CallNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.run(inputs, context, &mut mf_runtime::DiscardSink)
    }
}

impl mf_runtime::TaskStreamNode for CallNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.run(inputs, context, emitter)
    }
}

impl CallNode {
    fn run(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn mf_runtime::EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        let arguments = self
            .body
            .bind_arguments(inputs)
            .context(ArgumentsSnafu { node: &self.id })?;
        let outputs = context
            .run_call(&self.id, arguments, &self.body.nodes, |state| {
                self.body.execute_with_sink(state, sink)
            })
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
    _options: Value,
    mut parameters: mf_runtime::PreparedSubgraphs,
) -> Result<PreparedNode, NodeBuildError> {
    let body = mf_runtime::take_subgraph(&mut parameters, "body")?;
    ensure!(
        parameters.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "unexpected subgraph parameter"
        }
    );

    let config: CallConfig = mf_runtime::deserialize_config(config)?;
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
    let body = body.project_inputs(config.node_source, &config.input_projections)?;
    let inputs = body.public_inputs();
    let input_bindings = inputs
        .iter()
        .filter(|(_, input)| input.secret)
        .map(|(name, _)| {
            (
                name.clone(),
                mf_runtime::InputBinding {
                    secret: true,
                    ..Default::default()
                },
            )
        })
        .collect();
    let input_schemas = inputs
        .into_iter()
        .filter_map(|(name, input)| input.schema.map(|schema| (name, schema)))
        .collect();
    let streaming = !body.stream_outputs.is_empty();
    let node = CallNode {
        id: id.into(),
        flow: config.flow,
        body,
    };
    let metadata = NodeMetadata {
        input_bindings,
        input_schemas,
        ..Default::default()
    };
    if streaming {
        PreparedNode::task_stream(node, metadata)
    } else {
        PreparedNode::new(node, metadata)
    }
}

inventory::submit! {
    NodeRegistration { kind: mf_runtime::CALL_KIND, factory: NodeFactory::Subgraphs(factory) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body() -> PreparedSubgraph {
        PreparedSubgraph::new(vec![], vec![], |_| Ok(Default::default()))
    }

    fn parameters(body: PreparedSubgraph) -> mf_runtime::PreparedSubgraphs {
        std::collections::BTreeMap::from([(
            "body".into(),
            mf_runtime::PreparedSubgraphParameter::Single(body),
        )])
    }

    #[test]
    fn direct_call_factories_reject_blank_names_and_stdin_before_execution() {
        assert!(
            factory(
                "invoke",
                json!({"flow":" "}),
                Value::Null,
                parameters(body())
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
                json!({"flow":"child"}),
                Value::Null,
                parameters(body().with_input_schema(inputs))
            )
            .err()
            .unwrap()
            .to_string()
            .contains("cannot read stdin")
        );
    }
}
