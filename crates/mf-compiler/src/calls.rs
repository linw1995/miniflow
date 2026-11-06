use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{CallConfig, CompiledGraph};
use snafu::{ResultExt, ensure};
use std::collections::BTreeMap;

/// Built-in business policy is independent of receiving-field enumeration.
pub fn validate_owner(
    parent: &WorkflowDefinition,
    node: &NodeDefinition,
    body: Option<&WorkflowDefinition>,
) -> Result<(), WorkflowCompileError> {
    let policy_node = node_with_actual_body(node, body)?;
    let node = &policy_node;
    if node.kind == mf_runtime::LOOP_KIND {
        crate::loops::validate_boundary(parent, node)?;
    } else if node.kind == mf_runtime::ITERATION_KIND {
        let config = crate::iteration::parse_config(node)?;
        crate::iteration::validate_body_structure(parent.version, &config).context(
            crate::compiler::IterationValidationSnafu {
                definition_id: node.id.clone(),
            },
        )?;
    } else if node.kind == mf_runtime::CALL_KIND {
        validate_call(node)?;
    }
    Ok(())
}

/// Built-in decoders may inspect business fields only after the stored body
/// binding is confirmed. Replace their authoring-only body value with that actual
/// child for decoding; this is never a child-discovery path.
pub fn node_with_actual_body(
    node: &NodeDefinition,
    body: Option<&WorkflowDefinition>,
) -> Result<NodeDefinition, WorkflowCompileError> {
    let mut node = node.clone();
    if matches!(
        node.kind.as_str(),
        mf_runtime::LOOP_KIND | mf_runtime::ITERATION_KIND | mf_runtime::CALL_KIND
    ) {
        let Some(body) = body else {
            // Decode the owner's ordinary configuration for its typed diagnostic,
            // but never use a raw body when no actual binding was supplied.
            if let Some(config) = node.config.as_object_mut() {
                config.remove("body");
            }
            match node.kind.as_str() {
                mf_runtime::LOOP_KIND => {
                    crate::loops::parse_config(&node)?;
                }
                mf_runtime::ITERATION_KIND => {
                    crate::iteration::parse_config(&node)?;
                }
                mf_runtime::CALL_KIND => {
                    validate_call(&node)?;
                }
                _ => unreachable!("known built-in container"),
            }
            return crate::compiler::InvalidCallSnafu {
                definition_id: node.id.clone(),
                message: "built-in container requires its compiled body binding",
            }
            .fail();
        };
        node.set_node_flow("body", body.clone())
            .context(crate::compiler::FlowParametersSnafu {
                definition_id: node.id.clone(),
            })?;
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

/// Ordinary Flow arguments and Calls bind startup inputs and project selected
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
    match node.kind.as_str() {
        mf_runtime::LOOP_KIND => result.context(crate::compiler::LoopBodySnafu {
            path: serde_json::to_string(&[node.id.as_str()]).expect("definition IDs serialize"),
        }),
        mf_runtime::ITERATION_KIND => result.context(crate::compiler::IterationBodySnafu {
            definition_id: node.id.clone(),
        }),
        _ => result.context(crate::compiler::CallBodySnafu {
            definition_id: node.id.clone(),
        }),
    }
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
        crate::loops::validate_structure(&crate::compiler::project_graph_definition(body)?)?;
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
        crate::compiler::InvalidCallSnafu {
            definition_id: node.id.clone(),
            message: if node.kind == mf_runtime::CALL_KIND {
                "calls inside Iteration cannot contain Loop or Iteration constructs"
            } else {
                "Flow parameter calls inside Iteration cannot contain Loop or Iteration constructs"
            }
        }
    );
    Ok(())
}

fn validate_call(node: &NodeDefinition) -> Result<(), WorkflowCompileError> {
    let config: CallConfig = serde_json::from_value(node.config.clone()).context(
        crate::compiler::CallConfigurationSnafu {
            definition_id: node.id.clone(),
        },
    )?;
    ensure!(
        config.body.is_some(),
        crate::compiler::InvalidCallSnafu {
            definition_id: node.id.clone(),
            message: "Flow reference has not been resolved"
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
