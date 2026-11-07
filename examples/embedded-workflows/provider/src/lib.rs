//! A host-defined node that receives and executes a nested typed Flow.
use mf_runtime::{
    ExecutionContext, Flow, Inputs, NodeBuildError, NodeConfig, NodeExecutionError, NodeFactory,
    NodeIdentity, NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult,
    PortSpec, PreparedNode, PreparedSubgraph, TaskNode, WorkflowArguments,
};
use serde_json::Value;
use snafu::{OptionExt, ensure};
use std::collections::BTreeMap;

pub const KIND: &str = "example.apply";

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    policy: Policy,
}

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Policy {
    body: Flow,
}

struct ApplyFlow {
    scope_key: String,
    argument: String,
    input: PortSpec,
    outputs: Vec<PortSpec>,
    nodes: Vec<NodeIdentity>,
    body: Flow,
}

impl NodePortContract for ApplyFlow {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![self.input.clone()],
            outputs: self.outputs.clone(),
        }
    }
}

impl TaskNode for ApplyFlow {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        // Use the compiled child's scope key; do not invent another binding protocol.
        let arguments =
            WorkflowArguments([(self.argument.clone(), inputs["value"].clone())].into());
        let outputs = context.run_call(&self.scope_key, arguments, &self.nodes, |child| {
            self.body.execute_in_context(child)
        })?;
        Ok(outputs.into())
    }
}

fn plain(_config: Value) -> Result<PreparedNode, NodeBuildError> {
    mf_runtime::NodeInvalidSubgraphSnafu {
        message: "example.apply requires a prepared `/policy/body` binding",
    }
    .fail()
}

fn prepared(
    id: &str,
    config: Value,
    arguments: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let body = arguments
        .get("/policy/body")
        .context(mf_runtime::NodeInvalidSubgraphSnafu {
            message: "missing prepared `/policy/body` binding",
        })?;
    let mut inputs = body.inputs.inputs.iter().flat_map(|(node, ports)| {
        ports
            .iter()
            .map(move |(port, input)| (mf_runtime::output_id(node, port), input))
    });
    let (argument, input) = inputs
        .next()
        .context(mf_runtime::NodeInvalidSubgraphSnafu {
            message: "the body must have one required startup input",
        })?;
    ensure!(
        inputs.next().is_none()
            && input.required
            && input.binding.env.is_none()
            && input.binding.default.is_none()
            && body.inputs.stdin.is_empty()
            && body.stream_outputs.is_empty()
            && body.outputs.len() == 1
            && body.outputs[0].name == "answer"
            && body.outputs[0].required,
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "the body must have one explicit required input and one terminal `answer` output"
        }
    );
    let input_port = PortSpec::new("value", input.value_type.clone(), true);
    let metadata = NodeMetadata {
        input_bindings: [("value".into(), input.binding.clone())].into(),
        ..Default::default()
    };
    let outputs = body.outputs.clone();
    let nodes = body.nodes.clone();
    let scope_key = body.scope_key(id, "/policy/body");
    let Config {
        policy: Policy { body },
    } = Config::from_config(config, arguments)?;
    PreparedNode::new(
        ApplyFlow {
            scope_key,
            argument,
            input: input_port,
            outputs,
            nodes,
            body,
        },
        metadata,
    )
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::Subgraph {
            flow_parameters: Config::flow_parameters,
            plain,
            prepared,
        },
    }
}
