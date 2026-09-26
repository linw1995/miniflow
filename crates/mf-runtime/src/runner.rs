use crate::definition::DefinitionId;
use crate::{Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistry, Outputs};
use serde_json::Value;
use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum WorkflowRunError {
    #[snafu(display("node `{definition_id}`: {message}"))]
    Context {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(display("node `{definition_id}` references unavailable kind `{kind}`"))]
    UnknownKind {
        definition_id: DefinitionId,
        kind: String,
    },
    #[snafu(display("could not read embedded config for node `{definition_id}`: {source}"))]
    InvalidEmbeddedConfig {
        source: serde_json::Error,
        definition_id: DefinitionId,
    },
    #[snafu(display("could not construct node `{definition_id}`: {source}"))]
    NodeConstruction {
        source: NodeBuildError,
        definition_id: DefinitionId,
    },
    #[snafu(display("node `{definition_id}` failed: {source}"))]
    NodeExecution {
        source: NodeExecutionError,
        definition_id: DefinitionId,
    },
    #[snafu(display("node `{definition_id}` did not produce output `{port}`"))]
    MissingOutput {
        definition_id: DefinitionId,
        port: String,
    },
}

pub fn instantiate_node_with_metadata(
    registry: &NodeRegistry,
    definition_id: &str,
    kind: &str,
    config_json: &str,
) -> Result<crate::FlowNode, WorkflowRunError> {
    let node = instantiate_node(registry, definition_id, kind, config_json)?;
    let ports = registry.get(kind).unwrap().effective_ports(node.as_ref());
    Ok(crate::FlowNode::new(definition_id, node).with_ports(ports))
}

pub fn instantiate_node(
    registry: &NodeRegistry,
    definition_id: &str,
    kind: &str,
    config_json: &str,
) -> Result<Box<dyn Node>, WorkflowRunError> {
    let Some(registration) = registry.get(kind) else {
        return UnknownKindSnafu {
            definition_id: DefinitionId::from(definition_id),
            kind: kind.to_owned(),
        }
        .fail();
    };
    let config: Value = serde_json::from_str(config_json).context(InvalidEmbeddedConfigSnafu {
        definition_id: DefinitionId::from(definition_id),
    })?;
    registration
        .instantiate(config)
        .context(NodeConstructionSnafu {
            definition_id: DefinitionId::from(definition_id),
        })
}

pub fn execute_node(
    node: &dyn Node,
    inputs: Inputs,
    definition_id: &str,
) -> Result<Outputs, WorkflowRunError> {
    node.execute(inputs).context(NodeExecutionSnafu {
        definition_id: DefinitionId::from(definition_id),
    })
}

pub fn required_output(
    outputs: &Outputs,
    definition_id: &str,
    port: &str,
) -> Result<Value, WorkflowRunError> {
    outputs
        .get(port)
        .cloned()
        .ok_or_else(|| WorkflowRunError::MissingOutput {
            definition_id: DefinitionId::from(definition_id),
            port: port.to_owned(),
        })
}
