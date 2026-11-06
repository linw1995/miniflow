use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::CompiledGraph;
use snafu::ResultExt;
use std::collections::BTreeMap;

/// Project every authoritative child binding without interpreting its receiver.
/// Configuration outside these bindings remains opaque.
pub fn node_with_actual_flows<'a>(
    node: &NodeDefinition,
    flows: impl IntoIterator<Item = (&'a str, &'a WorkflowDefinition)>,
) -> Result<NodeDefinition, WorkflowCompileError> {
    let mut node = node.clone();
    for (name, definition) in flows {
        node.set_node_flow(name, definition.clone()).context(
            crate::compiler::FlowParametersSnafu {
                definition_id: node.id.clone(),
            },
        )?;
    }
    Ok(node)
}

/// The persisted child tree is the only authority for executable scopes.
pub fn compiled_node_flows<'a>(
    parent: &'a CompiledGraph,
    node: &NodeDefinition,
) -> Result<BTreeMap<String, &'a CompiledGraph>, WorkflowCompileError> {
    Ok(parent
        .children
        .get(&node.id)
        .into_iter()
        .flatten()
        .map(|(name, graph)| (name.clone(), graph))
        .collect())
}

pub fn child_result<T>(
    node: &NodeDefinition,
    result: Result<T, WorkflowCompileError>,
) -> Result<T, WorkflowCompileError> {
    result.context(crate::compiler::ChildGraphSnafu {
        definition_id: node.id.clone(),
    })
}

/// Child execution constraints are independent of the receiving provider's kind.
/// Relationship integrity and nesting are validated over the actual graph tree.
pub fn validate_child(
    node: &NodeDefinition,
    body: &CompiledGraph,
) -> Result<(), WorkflowCompileError> {
    child_result(
        node,
        crate::validate_execution(&body.definition)
            .context(crate::compiler::StreamConstructionSnafu),
    )
}
