use mf_runtime::{
    CallConfig, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec,
    PreparedNode, PreparedSubgraph, TaskNode, WorkflowArguments,
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

impl TaskNode for CallNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let outputs = context
            .run_call(
                &self.id,
                WorkflowArguments(inputs),
                &self.body.nodes,
                |state| self.body.execute_in_context(state),
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
    _options: Value,
    body: PreparedSubgraph,
) -> Result<PreparedNode, NodeBuildError> {
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
    let secret_inputs = body
        .inputs
        .inputs
        .iter()
        .flat_map(|(node, ports)| {
            ports
                .iter()
                .filter(|(_, input)| input.binding.secret)
                .map(move |(name, _)| mf_runtime::output_id(node, name))
        })
        .collect();
    PreparedNode::new(
        CallNode {
            id: id.into(),
            flow: config.flow,
            body,
        },
        NodeMetadata {
            secret_inputs,
            ..Default::default()
        },
    )
}

inventory::submit! {
    NodeRegistration { kind: mf_runtime::CALL_KIND, factory: NodeFactory::Subgraph(factory) }
}
