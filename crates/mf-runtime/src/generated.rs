//! Initialization and binding for compiler-generated executable layouts.

use crate::{
    DefinitionId, FlowNode, NodeBuildError, NodeContract, NodeExecution, NodeRegistry,
    PreparedSubgraph, TaskFlowNode,
};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutorKind {
    Task,
    TaskStream,
    Event,
    Stream,
}

impl ExecutorKind {
    pub fn of(node: &NodeExecution) -> Self {
        match node {
            NodeExecution::Task(_) => Self::Task,
            NodeExecution::TaskStream(_) => Self::TaskStream,
            NodeExecution::Event(_) => Self::Event,
            NodeExecution::Stream(_) => Self::Stream,
        }
    }
}

#[derive(Debug, Snafu)]
pub enum BindError {
    #[snafu(display("compiled node `{definition_id}` requires unavailable provider `{kind}`"))]
    Unknown {
        definition_id: DefinitionId,
        kind: String,
    },
    #[snafu(
        display("could not decode compiled configuration for `{definition_id}`: {source}"),
        visibility(pub)
    )]
    Configuration {
        definition_id: DefinitionId,
        source: serde_json::Error,
    },
    #[snafu(
        display("could not initialize compiled node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    Node {
        definition_id: DefinitionId,
        source: NodeBuildError,
    },
    #[snafu(display(
        "compiled node `{definition_id}` executor kind differs from its compiled contract"
    ))]
    Kind { definition_id: DefinitionId },
    #[snafu(display(
        "compiled node `{definition_id}` Flow bindings differ from its provider declaration"
    ))]
    Bindings { definition_id: DefinitionId },
    #[snafu(
        display("could not bind child of compiled node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    Child {
        definition_id: DefinitionId,
        #[snafu(source(from(BindError, Box::new)))]
        source: Box<BindError>,
    },
    #[snafu(
        display("invalid compiled observation identity: {source}"),
        visibility(pub)
    )]
    Identity { source: mf_telemetry::ContractError },
}

pub fn configuration(
    registry: &NodeRegistry,
    id: &str,
    kind: &str,
    json: &str,
    bindings: &[&str],
) -> Result<(&'static crate::NodeRegistration, serde_json::Value), BindError> {
    let registration = registry.get(kind).with_context(|| UnknownSnafu {
        definition_id: DefinitionId::from(id),
        kind,
    })?;
    let config = serde_json::from_str(json).context(ConfigurationSnafu {
        definition_id: DefinitionId::from(id),
    })?;
    ensure!(
        registration
            .compiled_bindings(&config)
            .iter()
            .map(String::as_str)
            .eq(bindings.iter().copied()),
        BindingsSnafu {
            definition_id: DefinitionId::from(id)
        }
    );
    Ok((registration, config))
}

pub fn instantiate(
    registration: &crate::NodeRegistration,
    id: &str,
    config: serde_json::Value,
    children: BTreeMap<String, PreparedSubgraph>,
) -> Result<FlowNode, BindError> {
    let node = if children.is_empty() {
        registration.instantiate(config)
    } else {
        registration.instantiate_subgraphs(id, config, children)
    }
    .context(NodeSnafu {
        definition_id: DefinitionId::from(id),
    })?;
    Ok(FlowNode::new(id, node))
}

pub fn bind(
    mut node: FlowNode,
    expected: &NodeContract,
    resolved: &NodeContract,
    kind: ExecutorKind,
    supports_success_dependencies: bool,
) -> Result<FlowNode, BindError> {
    crate::verify_generated_metadata(&node.metadata, expected).context(NodeSnafu {
        definition_id: node.definition_id.clone(),
    })?;
    ensure!(
        node.node
            .as_ref()
            .is_some_and(|node| ExecutorKind::of(node) == kind),
        KindSnafu {
            definition_id: node.definition_id.clone()
        }
    );
    ensure!(
        node.node
            .as_ref()
            .and_then(NodeExecution::as_task_node)
            .is_some_and(|task| task.supports_success_dependencies())
            == supports_success_dependencies,
        KindSnafu {
            definition_id: node.definition_id.clone()
        }
    );
    // Inference belongs to the compiler; keep the provider's executor and attach its proved output types.
    node.metadata.ports = resolved.ports.clone();
    node.metadata.stream_outputs = resolved.stream_outputs.clone();
    Ok(node)
}

pub fn into_task(node: FlowNode) -> Result<TaskFlowNode, BindError> {
    let definition_id = node.definition_id.clone();
    node.into_task().context(KindSnafu { definition_id })
}
