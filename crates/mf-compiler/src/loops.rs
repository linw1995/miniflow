use crate::{DefinitionId, ValueType, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{
    EXIT_LOOP_KIND, LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID, LoopComparisonOperator,
    MAX_LOOP_DEPTH, MAX_LOOP_ITERATIONS, NodeDefinition, WorkflowDefinitionVersion,
};
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignmentConfig {
    variable: String,
}

fn path_label(path: &[DefinitionId]) -> String {
    let ids: Vec<_> = path.iter().map(DefinitionId::as_str).collect();
    serde_json::to_string(&ids).expect("definition IDs serialize")
}

fn invalid(path: &[DefinitionId], message: impl Into<String>) -> WorkflowCompileError {
    crate::compiler::InvalidLoopSnafu {
        path: path_label(path),
        message,
    }
    .build()
}

pub fn assignment_target(
    config: &Value,
    path: &[DefinitionId],
) -> Result<String, WorkflowCompileError> {
    let assignment: AssignmentConfig =
        serde_json::from_value(config.clone()).with_context(|_| {
            crate::compiler::LoopAssignmentConfigurationSnafu {
                path: path_label(path),
            }
        })?;
    ensure!(
        !assignment.variable.trim().is_empty(),
        crate::compiler::InvalidLoopSnafu {
            path: path_label(path),
            message: "assignment variable must not be blank",
        }
    );
    Ok(assignment.variable)
}

pub fn validate_structure(definition: &WorkflowDefinition) -> Result<(), WorkflowCompileError> {
    for node in &definition.nodes {
        if definition.version == WorkflowDefinitionVersion::V2026_09_26
            && matches!(
                node.kind.as_str(),
                LOOP_KIND | LOOP_ASSIGN_KIND | EXIT_LOOP_KIND | LOOP_SOURCE_ID
            )
        {
            return Err(invalid(
                std::slice::from_ref(&node.id),
                "Loop constructs require definition version 2026-09-29",
            ));
        }
        validate_node(node, None, 0, &[], definition)?;
    }
    Ok(())
}

pub fn parse_config(
    node: &NodeDefinition,
) -> Result<mf_runtime::LoopDefinition, WorkflowCompileError> {
    parse_config_at(node, std::slice::from_ref(&node.id))
}

fn parse_config_at(
    node: &NodeDefinition,
    path: &[DefinitionId],
) -> Result<mf_runtime::LoopDefinition, WorkflowCompileError> {
    serde_json::from_value(node.config.clone()).context(crate::compiler::LoopConfigurationSnafu {
        path: path_label(path),
    })
}

/// Validate the owner's policy without adapting its receiving field.
pub fn validate_boundary(
    parent: &WorkflowDefinition,
    node: &NodeDefinition,
) -> Result<(), WorkflowCompileError> {
    let path = std::slice::from_ref(&node.id);
    if node.kind != LOOP_KIND {
        return Err(invalid(path, "only workflow.loop may contain a Loop body"));
    }
    if parent.version == WorkflowDefinitionVersion::V2026_09_26 {
        return Err(invalid(
            path,
            "Loop constructs require definition version 2026-09-29",
        ));
    }
    parse_config(node)?;
    Ok(())
}

fn validate_node(
    node: &NodeDefinition,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    depth: usize,
    path: &[DefinitionId],
    parent: &WorkflowDefinition,
) -> Result<(), WorkflowCompileError> {
    let mut node_path = path.to_vec();
    node_path.push(node.id.clone());
    if node.id.as_str() == LOOP_SOURCE_ID || node.kind == LOOP_SOURCE_ID {
        ensure!(
            enclosing.is_some()
                && node.id.as_str() == LOOP_SOURCE_ID
                && node.kind == LOOP_SOURCE_ID
                && node.config == json!({}),
            crate::compiler::InvalidLoopSnafu {
                path: path_label(&node_path),
                message: "`%loop` is reserved for the canonical Loop body source",
            }
        );
        return Ok(());
    }
    match node.kind.as_str() {
        LOOP_KIND => {
            if depth >= MAX_LOOP_DEPTH {
                return Err(invalid(&node_path, "Loop nesting depth exceeds four"));
            }
            validate_loop(node, depth + 1, &node_path, parent)
        }
        LOOP_ASSIGN_KIND => {
            let types =
                enclosing.ok_or_else(|| invalid(&node_path, "assignment requires a Loop"))?;
            let target = assignment_target(&node.config, &node_path)?;
            if !types.contains_key(&target) {
                return Err(invalid(&node_path, format!("unknown variable `{target}`")));
            }
            Ok(())
        }
        EXIT_LOOP_KIND => {
            if enclosing.is_none() {
                return Err(invalid(&node_path, "exit requires a Loop"));
            }
            if node.config != json!({}) {
                return Err(invalid(&node_path, "exit config must be empty"));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_loop(
    owner: &NodeDefinition,
    depth: usize,
    path: &[DefinitionId],
    parent: &WorkflowDefinition,
) -> Result<(), WorkflowCompileError> {
    let loop_definition = parse_config_at(owner, path)?;
    let version = parent.version;
    if !loop_definition.stream_outputs.is_empty() && !version.supports_child_streams() {
        return Err(invalid(
            path,
            "Loop child streams require version 2026-10-28",
        ));
    }
    for name in loop_definition.stream_outputs.keys() {
        if name.trim().is_empty()
            || loop_definition
                .variables
                .iter()
                .any(|variable| &variable.name == name)
        {
            return Err(invalid(
                path,
                "Loop stream names must be nonblank and distinct from variables",
            ));
        }
    }
    if !(1..=MAX_LOOP_ITERATIONS).contains(&loop_definition.max_iterations) {
        return Err(invalid(path, "max_iterations must be in 1..=1000"));
    }
    let types = mf_runtime::loop_variable_types(&loop_definition.variables).context(
        crate::compiler::LoopVariablesSnafu {
            path: path_label(path),
        },
    )?;
    if let Some(condition) = &loop_definition.until {
        let value_type = types.get(&condition.variable).ok_or_else(|| {
            invalid(
                path,
                format!("unknown condition variable `{}`", condition.variable),
            )
        })?;
        if condition.value.is_array() || condition.value.is_object() {
            return Err(invalid(path, "condition comparison value must be scalar"));
        }
        let numeric = matches!(
            value_type,
            ValueType::Any
                | ValueType::Number
                | ValueType::Int64
                | ValueType::Uint64
                | ValueType::Usize
                | ValueType::Float32
                | ValueType::Float64
        );
        let order = matches!(
            condition.operator,
            LoopComparisonOperator::Gt
                | LoopComparisonOperator::Gte
                | LoopComparisonOperator::Lt
                | LoopComparisonOperator::Lte
        );
        if order && (!numeric || !condition.value.is_number()) {
            return Err(invalid(
                path,
                "ordering condition requires numeric operands",
            ));
        }
        if !order
            && matches!(
                value_type,
                ValueType::Array | ValueType::Object | ValueType::List(_) | ValueType::Map(_)
            )
        {
            return Err(invalid(
                path,
                "condition variable must have a scalar-compatible type",
            ));
        }
        if !order {
            let compatible = match value_type {
                ValueType::Any => true,
                ValueType::Number
                | ValueType::Int64
                | ValueType::Uint64
                | ValueType::Usize
                | ValueType::Float32
                | ValueType::Float64 => condition.value.is_number(),
                ValueType::Null => condition.value.is_null(),
                ValueType::Boolean => condition.value.is_boolean(),
                ValueType::String => condition.value.is_string(),
                ValueType::Nullable(_) => value_type.validate_value(&condition.value).is_ok(),
                ValueType::Array | ValueType::Object | ValueType::List(_) | ValueType::Map(_) => {
                    false
                }
            };
            if !compatible {
                return Err(invalid(
                    path,
                    "condition literal conflicts with variable type",
                ));
            }
        }
    }
    let body = &loop_definition.body;
    if !body
        .nodes
        .iter()
        .any(|node| node.id.as_str() != LOOP_SOURCE_ID)
    {
        return Err(invalid(path, "Loop body must contain at least one node"));
    }
    if !body.outputs.is_empty() {
        return Err(invalid(path, "Loop body outputs must be empty"));
    }
    // Structural ordering is checked by normalization, after its implicit source
    // has been inserted. This pass only validates the owner's business policy.
    for node in &body.nodes {
        if body.version == WorkflowDefinitionVersion::V2026_09_26
            && matches!(
                node.kind.as_str(),
                LOOP_KIND | LOOP_ASSIGN_KIND | EXIT_LOOP_KIND | LOOP_SOURCE_ID
            )
        {
            return Err(invalid(
                path,
                "Loop constructs require definition version 2026-09-29",
            ));
        }
        validate_node(node, Some(&types), depth, path, body)?;
    }
    Ok(())
}
