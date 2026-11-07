use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{
    EXIT_LOOP_KIND, ITERATION_INPUT_ID, ITERATION_INPUT_KIND, ITERATION_KIND, LOOP_ASSIGN_KIND,
    LOOP_KIND, LOOP_SOURCE_ID,
};
use serde::Deserialize;
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

/// A lowering view, not the receiving node's configuration schema.
#[derive(Deserialize)]
pub struct LoweringConfig {
    #[serde(default)]
    pub stream_outputs: BTreeMap<String, mf_runtime::OutputReferenceDefinition>,
}

#[derive(Debug, Snafu)]
pub enum ScopeStructureError {
    #[snafu(display("body node ID `{node}` is reserved for a canonical scope source"))]
    ReservedNode { node: crate::DefinitionId },
    #[snafu(display("body node `{node}` uses an unsupported nested or internal scope kind"))]
    NestedScope { node: crate::DefinitionId },
    #[snafu(display("body node `{node}` uses an unsupported state-scope construct"))]
    StateScope { node: crate::DefinitionId },
}

pub fn parse_config(node: &NodeDefinition) -> Result<LoweringConfig, WorkflowCompileError> {
    serde_json::from_value(node.config.clone()).context(crate::compiler::NodeConfigurationSnafu {
        definition_id: node.id.clone(),
    })
}

/// The supported item-scope grammar is a compiler contract. Result shape,
/// ordinary configuration and stream-name business rules belong to the receiver.
pub fn validate_body_structure(body: &WorkflowDefinition) -> Result<(), ScopeStructureError> {
    for node in &body.nodes {
        if node.id.as_str() == ITERATION_INPUT_ID || node.kind == ITERATION_INPUT_KIND {
            ensure!(
                node.id.as_str() == ITERATION_INPUT_ID
                    && node.kind == ITERATION_INPUT_KIND
                    && node.config == serde_json::json!({}),
                ReservedNodeSnafu {
                    node: node.id.clone()
                }
            );
            continue;
        }
        ensure!(
            node.kind != ITERATION_KIND,
            NestedScopeSnafu {
                node: node.id.clone()
            }
        );
        ensure!(
            !matches!(
                node.kind.as_str(),
                LOOP_KIND | LOOP_ASSIGN_KIND | EXIT_LOOP_KIND | LOOP_SOURCE_ID
            ),
            StateScopeSnafu {
                node: node.id.clone()
            }
        );
    }
    Ok(())
}
