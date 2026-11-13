use crate::{DefinitionId, ValueType, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{
    EXIT_LOOP_KIND, LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID, LoopVariableDefinition,
    NodeDefinition, OutputReferenceDefinition, WorkflowDefinitionVersion,
};
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::collections::BTreeMap;

/// Only the fields required to lower synthetic sources and stream projections.
/// The receiving node owns all remaining configuration and business validation.
#[derive(Deserialize)]
pub struct LoweringConfig {
    #[serde(default)]
    pub inputs: BTreeMap<String, ValueType>,
    pub variables: Vec<LoopVariableDefinition>,
    #[serde(default)]
    pub stream_outputs: BTreeMap<String, OutputReferenceDefinition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignmentConfig {
    variable: String,
}

pub fn path_label(path: &[DefinitionId]) -> String {
    let ids: Vec<_> = path.iter().map(DefinitionId::as_str).collect();
    serde_json::to_string(&ids).expect("definition IDs serialize")
}

fn invalid(path: &[DefinitionId], message: impl Into<String>) -> WorkflowCompileError {
    crate::compiler::InvalidScopeSnafu {
        path: path_label(path),
        message,
    }
    .build()
}

pub fn assignment_target(
    config: &Value,
    path: &[DefinitionId],
) -> Result<String, WorkflowCompileError> {
    let assignment: AssignmentConfig = serde_json::from_value(config.clone()).context(
        crate::compiler::ScopeConfigurationSnafu {
            path: path_label(path),
        },
    )?;
    ensure!(
        !assignment.variable.trim().is_empty(),
        crate::compiler::InvalidScopeSnafu {
            path: path_label(path),
            message: "assignment variable must not be blank",
        }
    );
    Ok(assignment.variable)
}

pub fn parse_config(node: &NodeDefinition) -> Result<LoweringConfig, WorkflowCompileError> {
    serde_json::from_value(node.config.clone()).context(crate::compiler::NodeConfigurationSnafu {
        definition_id: node.id.clone(),
    })
}

/// Validate engine syntax in this scope only. Child traversal is driven by the
/// actual graph, never by executable-looking values in ordinary configuration.
pub fn validate_scope(
    definition: &WorkflowDefinition,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    root: Option<&BTreeMap<String, ValueType>>,
    path: &[DefinitionId],
) -> Result<(), WorkflowCompileError> {
    for node in &definition.nodes {
        let mut node_path = path.to_vec();
        node_path.push(node.id.clone());
        if definition.version == WorkflowDefinitionVersion::V2026_09_26
            && matches!(
                node.kind.as_str(),
                LOOP_KIND | LOOP_ASSIGN_KIND | EXIT_LOOP_KIND | LOOP_SOURCE_ID
            )
        {
            return Err(invalid(
                &node_path,
                "scope constructs require definition version 2026-09-29",
            ));
        }
        if matches!(
            node.kind.as_str(),
            mf_runtime::VARIABLES_KIND | mf_runtime::ASSIGN_KIND
        ) {
            ensure!(
                definition.version.supports_workflow_variables(),
                crate::compiler::InvalidScopeSnafu {
                    path: path_label(&node_path),
                    message: "workflow variables and assignments require definition version 2026-11-13",
                }
            );
        }
        match node.kind.as_str() {
            LOOP_SOURCE_ID => ensure!(
                enclosing.is_some()
                    && node.id.as_str() == LOOP_SOURCE_ID
                    && node.config == json!({}),
                crate::compiler::InvalidScopeSnafu {
                    path: path_label(&node_path),
                    message: "reserved scope source requires its canonical enclosing scope",
                }
            ),
            LOOP_ASSIGN_KIND | mf_runtime::ASSIGN_KIND => {
                let types = enclosing
                    .or_else(|| {
                        (node.kind == mf_runtime::ASSIGN_KIND)
                            .then_some(root)
                            .flatten()
                    })
                    .ok_or_else(|| invalid(&node_path, "assignment requires a state scope"))?;
                let target = assignment_target(&node.config, &node_path)?;
                ensure!(
                    types.contains_key(&target),
                    crate::compiler::InvalidScopeSnafu {
                        path: path_label(&node_path),
                        message: format!("unknown variable `{target}`"),
                    }
                );
            }
            EXIT_LOOP_KIND => {
                ensure!(
                    enclosing.is_some(),
                    crate::compiler::InvalidScopeSnafu {
                        path: path_label(&node_path),
                        message: "exit requires a state scope",
                    }
                );
                ensure!(
                    node.config == json!({}),
                    crate::compiler::InvalidScopeSnafu {
                        path: path_label(&node_path),
                        message: "exit config must be empty",
                    }
                );
            }
            _ => {}
        }
    }
    Ok(())
}
