use crate::definition::DefinitionId;
use crate::{NodeBuildError, NodeExecutionError, NodeRegistry};
use serde_json::Value;
use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum WorkflowRunError {
    #[snafu(transparent)]
    WorkflowInputs { source: crate::WorkflowInputError },
    #[snafu(transparent)]
    FlowBuild { source: crate::FlowBuildError },
    #[snafu(
        display("node `{definition_id}` dependency `{input}`: {source}"),
        visibility(pub)
    )]
    Dependency {
        definition_id: DefinitionId,
        input: String,
        source: NodeExecutionError,
    },
    #[snafu(
        display("node `{definition_id}` input `{input}`: {source}"),
        visibility(pub)
    )]
    InputType {
        definition_id: DefinitionId,
        input: String,
        source: crate::TypeMismatch,
    },
    #[snafu(display("node `{definition_id}`: {message}"), visibility(pub))]
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
    #[snafu(display("node `{definition_id}` failed: {source}"), visibility(pub))]
    NodeExecution {
        source: NodeExecutionError,
        definition_id: DefinitionId,
    },
}

pub fn instantiate_node_with_metadata(
    registry: &NodeRegistry,
    definition_id: &str,
    kind: &str,
    config_json: &str,
) -> Result<crate::FlowNode, WorkflowRunError> {
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
    let node = registration
        .instantiate(config)
        .context(NodeConstructionSnafu {
            definition_id: DefinitionId::from(definition_id),
        })?;
    Ok(crate::FlowNode::new(definition_id, node))
}

pub fn instantiate_subgraph_with_metadata(
    registry: &NodeRegistry,
    definition_id: &str,
    kind: &str,
    config_json: &str,
    options_json: &str,
    body: crate::PreparedSubgraph,
) -> Result<crate::FlowNode, WorkflowRunError> {
    let Some(registration) = registry.get(kind) else {
        return UnknownKindSnafu {
            definition_id: DefinitionId::from(definition_id),
            kind: kind.to_owned(),
        }
        .fail();
    };
    let parse = |value| {
        serde_json::from_str(value).context(InvalidEmbeddedConfigSnafu {
            definition_id: DefinitionId::from(definition_id),
        })
    };
    let prepared = registration
        .instantiate_subgraph(
            definition_id,
            parse(config_json)?,
            parse(options_json)?,
            body,
        )
        .context(NodeConstructionSnafu {
            definition_id: DefinitionId::from(definition_id),
        })?;
    Ok(crate::FlowNode::new(definition_id, prepared))
}
