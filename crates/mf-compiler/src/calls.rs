use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::CallConfig;
use snafu::{ResultExt, ensure};
use std::collections::BTreeMap;

/// Built-in business policy is independent of receiving-field enumeration.
pub fn validate_owner(
    parent: &WorkflowDefinition,
    node: &NodeDefinition,
) -> Result<(), WorkflowCompileError> {
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
        // Preserve the authored Call configuration / unresolved-reference errors.
        validate_call(node)?;
    }
    Ok(())
}

/// Registry-free traversal uses frozen receiver declarations, never JSON keys or
/// order paths. Order facts remain independently validated execution prerequisites.
pub fn compiled_node_flows(
    parent: &WorkflowDefinition,
    node: &NodeDefinition,
    declarations: &mf_runtime::NodeParameterTypes,
) -> Result<BTreeMap<String, WorkflowDefinition>, WorkflowCompileError> {
    validate_owner(parent, node)?;
    node.declared_flows(declarations)
        .context(crate::compiler::FlowParametersSnafu {
            definition_id: node.id.clone(),
        })
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
    body: &WorkflowDefinition,
    inside_iteration: bool,
    contracts: &mf_runtime::NodeParameterTypes,
) -> Result<(), WorkflowCompileError> {
    crate::validate_execution(body).context(crate::compiler::StreamConstructionSnafu)?;
    if call_like(node) {
        ensure!(
            !inside_iteration || !contains_iteration_containers(body, contracts)?,
            crate::compiler::InvalidCallSnafu {
                definition_id: node.id.clone(),
                message: if node.kind == mf_runtime::CALL_KIND {
                    "calls inside Iteration cannot contain Loop or Iteration constructs"
                } else {
                    "Flow parameter calls inside Iteration cannot contain Loop or Iteration constructs"
                }
            }
        );
        crate::loops::validate_structure(body)?;
    }
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

pub fn contains_iteration_containers(
    definition: &WorkflowDefinition,
    contracts: &mf_runtime::NodeParameterTypes,
) -> Result<bool, WorkflowCompileError> {
    for node in &definition.nodes {
        if matches!(
            node.kind.as_str(),
            mf_runtime::LOOP_KIND | mf_runtime::ITERATION_KIND
        ) {
            return Ok(true);
        }
        let parameters = contracts
            .get(&node.kind)
            .into_iter()
            .flat_map(|parameters| parameters.iter())
            .filter(|(_, value_type)| value_type.is_flow())
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        for body in node
            .flow_parameters(&parameters)
            .context(crate::compiler::FlowParametersSnafu {
                definition_id: node.id.clone(),
            })?
            .values()
        {
            if contains_iteration_containers(body, contracts)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
