use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::CallConfig;
use snafu::{ResultExt, ensure};

pub fn body_definition(node: &NodeDefinition) -> Result<WorkflowDefinition, WorkflowCompileError> {
    let config = config(node)?;
    ensure!(
        config.definition.is_some(),
        crate::compiler::InvalidCallSnafu {
            definition_id: node.id.clone(),
            message: "Flow reference has not been resolved"
        }
    );
    Ok(*config.definition.expect("resolved call has a body"))
}

pub fn contains_iteration_containers(
    definition: &WorkflowDefinition,
) -> Result<bool, WorkflowCompileError> {
    for node in &definition.nodes {
        if matches!(
            node.kind.as_str(),
            mf_runtime::LOOP_KIND | mf_runtime::ITERATION_KIND
        ) {
            return Ok(true);
        }
        if node.kind == mf_runtime::CALL_KIND
            && contains_iteration_containers(&body_definition(node)?)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A provider can request metadata for one frozen subgraph without introducing a runtime policy.
pub fn is_flow(node: &NodeDefinition) -> bool {
    is_flow_with_registry(node, None)
}

pub fn is_metadata(kind: &str, registry: Option<&mf_runtime::NodeRegistry>) -> bool {
    let registration = if let Some(registry) = registry {
        registry.get(kind)
    } else {
        inventory::iter::<mf_runtime::NodeRegistration>
            .into_iter()
            .find(|registration| registration.kind == kind)
    };
    registration.is_some_and(|registration| {
        matches!(registration.factory, mf_runtime::NodeFactory::Metadata(_))
    })
}

pub fn is_flow_with_registry(
    node: &NodeDefinition,
    registry: Option<&mf_runtime::NodeRegistry>,
) -> bool {
    node.kind == mf_runtime::CALL_KIND || is_metadata(&node.kind, registry)
}

pub fn config(node: &NodeDefinition) -> Result<CallConfig, WorkflowCompileError> {
    let value = if node.kind == mf_runtime::CALL_KIND {
        &node.config
    } else {
        &node.config["subgraph"]
    };
    serde_json::from_value(value.clone()).context(crate::compiler::CallConfigurationSnafu {
        definition_id: node.id.clone(),
    })
}

pub fn config_mut(node: &mut NodeDefinition) -> &mut serde_json::Value {
    if node.kind == mf_runtime::CALL_KIND {
        &mut node.config
    } else {
        &mut node.config["subgraph"]
    }
}
