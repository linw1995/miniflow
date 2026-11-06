use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::CompiledGraph;
use snafu::{ResultExt, ensure};
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

/// Ordinary Flow parameters and Calls bind startup inputs and project selected
/// stream outputs. Loop and Iteration instead have synthetic scope inputs.
pub fn call_like(node: &NodeDefinition) -> bool {
    !matches!(
        node.kind.as_str(),
        mf_runtime::LOOP_KIND | mf_runtime::ITERATION_KIND
    )
}

pub fn child_result<T>(
    node: &NodeDefinition,
    result: Result<T, WorkflowCompileError>,
) -> Result<T, WorkflowCompileError> {
    result.context(crate::compiler::ChildGraphSnafu {
        definition_id: node.id.clone(),
    })
}

pub fn validate_child(
    node: &NodeDefinition,
    body: &CompiledGraph,
    inside_iteration: bool,
) -> Result<(), WorkflowCompileError> {
    crate::validate_execution(&body.definition)
        .context(crate::compiler::StreamConstructionSnafu)?;
    if call_like(node) {
        validate_iteration_child(node, inside_iteration, contains_iteration_containers(body))?;
    }
    Ok(())
}

pub fn validate_iteration_child(
    node: &NodeDefinition,
    inside_iteration: bool,
    contains_containers: bool,
) -> Result<(), WorkflowCompileError> {
    ensure!(
        !call_like(node) || !inside_iteration || !contains_containers,
        crate::compiler::InvalidGraphSnafu {
            definition_id: node.id.clone(),
            message: "child scopes inside an item scope cannot contain container scopes"
        }
    );
    Ok(())
}

pub fn contains_iteration_containers(graph: &CompiledGraph) -> bool {
    graph.definition.nodes.iter().any(|node| {
        matches!(
            node.kind.as_str(),
            mf_runtime::LOOP_KIND | mf_runtime::ITERATION_KIND
        )
    }) || graph
        .children
        .values()
        .flat_map(BTreeMap::values)
        .any(contains_iteration_containers)
}
