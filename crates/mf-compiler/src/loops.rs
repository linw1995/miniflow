use crate::{DefinitionId, ValueType, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{
    EXIT_LOOP_KIND, LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID, LoopBodyDefinition,
    LoopComparisonOperator, LoopDefinition, MAX_LOOP_DEPTH, MAX_LOOP_ITERATIONS, NodeDefinition,
    WorkflowDefinitionVersion,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

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
    WorkflowCompileError::InvalidLoop {
        path: path_label(path),
        message: message.into(),
    }
}

pub fn assignment_target(config: &Value) -> Result<String, String> {
    let assignment: AssignmentConfig =
        serde_json::from_value(config.clone()).map_err(|error| error.to_string())?;
    if assignment.variable.trim().is_empty() {
        return Err("assignment variable must not be blank".into());
    }
    Ok(assignment.variable)
}

pub fn body_definition(
    body: &LoopBodyDefinition,
    dependencies: &BTreeMap<String, mf_runtime::NodeDependency>,
) -> WorkflowDefinition {
    let mut nodes = Vec::with_capacity(body.nodes.len() + 1);
    nodes.push(NodeDefinition {
        id: LOOP_SOURCE_ID.into(),
        kind: LOOP_SOURCE_ID.into(),
        config: json!({}),
        loop_definition: None,
    });
    nodes.extend(body.nodes.iter().cloned());
    WorkflowDefinition {
        version: WorkflowDefinitionVersion::V2026_09_29,
        execution: None,
        dependencies: dependencies.clone(),
        nodes,
        edges: body.edges.clone(),
        control_edges: body.control_edges.clone(),
        outputs: Vec::new(),
    }
}

pub fn validate_structure(definition: &WorkflowDefinition) -> Result<(), WorkflowCompileError> {
    for node in &definition.nodes {
        if definition.version == WorkflowDefinitionVersion::V2026_09_26
            && (node.loop_definition.is_some()
                || matches!(
                    node.kind.as_str(),
                    LOOP_KIND | LOOP_ASSIGN_KIND | EXIT_LOOP_KIND | LOOP_SOURCE_ID
                ))
        {
            return Err(invalid(
                std::slice::from_ref(&node.id),
                "Loop constructs require definition version 2026-09-29",
            ));
        }
        validate_node(node, None, 0, &[], &definition.dependencies)?;
    }
    Ok(())
}

fn validate_node(
    node: &NodeDefinition,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    depth: usize,
    path: &[DefinitionId],
    dependencies: &BTreeMap<String, mf_runtime::NodeDependency>,
) -> Result<(), WorkflowCompileError> {
    let mut node_path = path.to_vec();
    node_path.push(node.id.clone());
    match node.kind.as_str() {
        LOOP_KIND => {
            if depth >= MAX_LOOP_DEPTH {
                return Err(invalid(&node_path, "Loop nesting depth exceeds four"));
            }
            if node.config != json!({}) {
                return Err(invalid(&node_path, "Loop config must be empty"));
            }
            let loop_definition = node
                .loop_definition
                .as_deref()
                .ok_or_else(|| invalid(&node_path, "Loop definition is missing"))?;
            validate_loop(loop_definition, depth + 1, &node_path, dependencies)
        }
        LOOP_ASSIGN_KIND => {
            if node.loop_definition.is_some() {
                return Err(invalid(&node_path, "assignment cannot contain a Loop body"));
            }
            let types =
                enclosing.ok_or_else(|| invalid(&node_path, "assignment requires a Loop"))?;
            let target =
                assignment_target(&node.config).map_err(|message| invalid(&node_path, message))?;
            if !types.contains_key(&target) {
                return Err(invalid(&node_path, format!("unknown variable `{target}`")));
            }
            Ok(())
        }
        EXIT_LOOP_KIND => {
            if enclosing.is_none() {
                return Err(invalid(&node_path, "exit requires a Loop"));
            }
            if node.loop_definition.is_some() || node.config != json!({}) {
                return Err(invalid(&node_path, "exit config must be empty"));
            }
            Ok(())
        }
        LOOP_SOURCE_ID => Err(invalid(
            &node_path,
            "`%loop` is reserved for the body source",
        )),
        _ if node.loop_definition.is_some() => Err(invalid(
            &node_path,
            "only workflow.loop may contain a Loop body",
        )),
        _ => Ok(()),
    }
}

fn validate_loop(
    loop_definition: &LoopDefinition,
    depth: usize,
    path: &[DefinitionId],
    dependencies: &BTreeMap<String, mf_runtime::NodeDependency>,
) -> Result<(), WorkflowCompileError> {
    if !(1..=MAX_LOOP_ITERATIONS).contains(&loop_definition.max_iterations) {
        return Err(invalid(path, "max_iterations must be in 1..=1000"));
    }
    let types = mf_runtime::loop_variable_types(&loop_definition.variables)
        .map_err(|message| invalid(path, message))?;
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
            ValueType::Any | ValueType::Number | ValueType::Int64 | ValueType::Float64
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
                ValueType::Number | ValueType::Int64 | ValueType::Float64 => {
                    condition.value.is_number()
                }
                ValueType::Null => condition.value.is_null(),
                ValueType::Boolean => condition.value.is_boolean(),
                ValueType::String => condition.value.is_string(),
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
    if loop_definition.body.nodes.is_empty() {
        return Err(invalid(path, "Loop body must contain at least one node"));
    }
    let body = body_definition(&loop_definition.body, dependencies);
    crate::compiler::structural_order_graph(&body)
        .map_err(|error| invalid(path, error.to_string()))?;
    let mut names = BTreeSet::new();
    for node in &loop_definition.body.nodes {
        if !names.insert(node.id.clone()) {
            return Err(invalid(path, format!("duplicate body node `{}`", node.id)));
        }
        validate_node(node, Some(&types), depth, path, dependencies)?;
    }
    Ok(())
}
