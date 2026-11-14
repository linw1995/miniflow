use crate::{DefinitionId, ValueType, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{
    EXIT_LOOP_KIND, LOOP_SOURCE_ID, LoopVariableDefinition, NodeDefinition,
    OutputReferenceDefinition, SubgraphScope, WorkflowDefinitionVersion,
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
    subgraphs: &BTreeMap<String, SubgraphScope>,
) -> Result<(), WorkflowCompileError> {
    for node in &definition.nodes {
        let mut node_path = path.to_vec();
        node_path.push(node.id.clone());
        if definition.version == WorkflowDefinitionVersion::V2026_09_26
            && (matches!(node.kind.as_str(), EXIT_LOOP_KIND | LOOP_SOURCE_ID)
                || subgraphs.get(node.kind.as_str()) == Some(&SubgraphScope::Loop))
        {
            return Err(invalid(
                &node_path,
                "scope constructs require definition version 2026-09-29",
            ));
        }
        match node.kind.as_str() {
            mf_runtime::ROOT_SOURCE_ID => ensure!(
                root.is_some()
                    && node.id.as_str() == mf_runtime::ROOT_SOURCE_ID
                    && node.config == json!({}),
                crate::compiler::InvalidScopeSnafu {
                    path: path_label(&node_path),
                    message: "reserved scope source requires its canonical enclosing scope",
                }
            ),
            LOOP_SOURCE_ID => ensure!(
                enclosing.is_some()
                    && node.id.as_str() == LOOP_SOURCE_ID
                    && node.config == json!({}),
                crate::compiler::InvalidScopeSnafu {
                    path: path_label(&node_path),
                    message: "reserved scope source requires its canonical enclosing scope",
                }
            ),
            mf_runtime::ASSIGN_KIND => {
                // Lowering validates the declaration version; inside a Loop body the
                // assignment keeps the pre-existing immediate-write contract.
                let types = enclosing
                    .or(root)
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
