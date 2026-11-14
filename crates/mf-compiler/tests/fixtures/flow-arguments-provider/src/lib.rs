use mf_runtime::NodeConfig;
use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec, PreparedNode,
    PreparedSubgraph, TaskNode, ValueType, WorkflowArguments,
};
use serde_json::Value;
use snafu::{OptionExt, ensure};
use std::collections::BTreeMap;

pub const KIND: &str = "fixture.choose_flow";

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    left: Option<PreparedSubgraph>,
    #[serde(default)]
    right: Option<PreparedSubgraph>,
}

struct ChooseFlow {
    id: String,
    arguments: BTreeMap<String, PreparedSubgraph>,
}

impl NodePortContract for ChooseFlow {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("select", ValueType::String, true)],
            outputs: vec![PortSpec::new("content", ValueType::String, true)],
        }
    }
}

impl TaskNode for ChooseFlow {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let selection = inputs["select"].as_str().expect("validated string port");
        let body = self.arguments.get(selection).with_context(|| {
            mf_runtime::NodeExecutionFailedSnafu {
                message: format!("unknown local Flow binding `{selection}`"),
            }
        })?;
        let outputs = context.run_call(
            &body.scope_key(&self.id, selection),
            WorkflowArguments::default(),
            &body.nodes,
            |child| body.execute_in_context(child),
        )?;
        Ok(outputs.into())
    }
}

fn plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    prepared("", config, BTreeMap::new())
}

fn prepared(
    id: &str,
    config: Value,
    arguments: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let config = Config::from_config(config, arguments)?;
    let arguments: BTreeMap<_, _> = [("left", config.left), ("right", config.right)]
        .into_iter()
        .filter_map(|(name, body)| body.map(|body| (name.into(), body)))
        .collect();
    for body in arguments.values() {
        ensure!(
            body.inputs.inputs.values().all(BTreeMap::is_empty)
                && body.inputs.stdin.is_empty()
                && body.stream_outputs.is_empty()
                && body.outputs == vec![PortSpec::new("content", ValueType::String, true)],
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: "expected a terminal string-content body without inputs"
            }
        );
    }
    PreparedNode::new(
        ChooseFlow {
            id: id.into(),
            arguments,
        },
        NodeMetadata::default(),
    )
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::Subgraph { scope: None, flow_parameters: Config::flow_parameters, plain, prepared },
    }
}
