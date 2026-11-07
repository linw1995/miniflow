use mf_runtime::NodeConfig;
use mf_runtime::{
    Emitter, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec,
    PreparedNode, PreparedSubgraph, TaskStreamNode, ValueType, WorkflowArguments,
};
use serde_json::Value;
use snafu::ensure;
use std::collections::BTreeMap;

pub const KIND: &str = "fixture.consume_flow_stream";
pub const SOURCE_KIND: &str = "fixture.flow_argument_increments";

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    local: Option<PreparedSubgraph>,
    #[serde(default)]
    optional: bool,
}

struct Source;

impl NodePortContract for Source {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: terminal_ports(),
        }
    }

    fn stream_outputs(&self) -> Vec<PortSpec> {
        vec![PortSpec::new("delta", ValueType::Uint64, true)]
    }
}

fn terminal_ports() -> Vec<PortSpec> {
    vec![
        PortSpec::new("count", ValueType::Uint64, true),
        PortSpec::new("subscribed", ValueType::Boolean, true),
    ]
}

impl TaskStreamNode for Source {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        if emitter.is_connected() {
            for delta in 0_u64..3 {
                emitter.send(Outputs::from([("delta".into(), delta.into())]).into())?;
            }
        }
        Ok(Outputs::from([
            ("count".into(), 3_u64.into()),
            ("subscribed".into(), emitter.is_connected().into()),
        ])
        .into())
    }
}

struct Consumer {
    id: String,
    body: PreparedSubgraph,
}

impl NodePortContract for Consumer {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: terminal_ports(),
        }
    }

    fn stream_outputs(&self) -> Vec<PortSpec> {
        self.body.stream_outputs.clone()
    }
}

impl TaskStreamNode for Consumer {
    fn execute(
        &self,
        _: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        let body = &self.body;
        let outputs = context.run_call(
            &body.scope_key(&self.id, "/local"),
            WorkflowArguments::default(),
            &body.nodes,
            |child| body.execute_with_sink(child, emitter),
        )?;
        Ok(outputs.into())
    }
}

fn plain(_: Value) -> Result<PreparedNode, NodeBuildError> {
    mf_runtime::NodeInvalidSubgraphSnafu {
        message: "consumer requires a prepared Flow argument",
    }
    .fail()
}

fn prepared(
    id: &str,
    config: Value,
    arguments: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let config = Config::from_config(config, arguments)?;
    let body = config.local.ok_or_else(|| {
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "expected exactly the local alias",
        }
        .build()
    })?;
    ensure!(
        body.inputs
            .inputs
            .values()
            .all(std::collections::BTreeMap::is_empty)
            && body.inputs.stdin.is_empty()
            && body.outputs == terminal_ports()
            && body.stream_outputs
                == vec![PortSpec::new(
                    "increment",
                    ValueType::Uint64,
                    !config.optional,
                )],
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "expected separate terminal and increment stream interfaces"
        }
    );
    PreparedNode::task_stream(
        Consumer {
            id: id.into(),
            body,
        },
        NodeMetadata::default(),
    )
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::Subgraph { flow_parameters: Config::flow_parameters, plain, prepared },
    }
}

inventory::submit! {
    NodeRegistration {
        kind: SOURCE_KIND,
        factory: NodeFactory::Plain(|_| PreparedNode::task_stream(Source, NodeMetadata::default())),
    }
}
