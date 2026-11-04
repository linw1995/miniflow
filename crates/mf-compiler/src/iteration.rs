use crate::compiler::IterationConfigurationSnafu;
use crate::{NodeDefinition, WorkflowCompileError};
use mf_runtime::{
    EXIT_LOOP_KIND, ITERATION_INPUT_ID, ITERATION_INPUT_KIND, ITERATION_KIND, IterationConfig,
    LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID,
};
use snafu::{ResultExt, Snafu, ensure};

#[derive(Debug, Snafu)]
pub enum IterationValidationError {
    #[snafu(display("Iteration child streams require version 2026-10-28"))]
    ChildStreamVersion,
    #[snafu(display("Iteration stream names must be nonblank and distinct from results"))]
    InvalidStreamName,
    #[snafu(display("Iteration body must declare exactly one required output named `result`"))]
    InvalidResult,
    #[snafu(display("body node ID `{node}` is reserved"))]
    ReservedNode { node: crate::DefinitionId },
    #[snafu(display("body node `{node}` uses an unsupported nested or internal kind"))]
    NestedIteration { node: crate::DefinitionId },
    #[snafu(display("body node `{node}` uses an unsupported Loop construct"))]
    LoopConstruct { node: crate::DefinitionId },
}

pub fn parse_config(node: &NodeDefinition) -> Result<IterationConfig, WorkflowCompileError> {
    serde_json::from_value(node.config.clone()).context(IterationConfigurationSnafu {
        definition_id: node.id.clone(),
    })
}

/// Validate the body boundary before parsing or resolving any child node config.
pub fn validate_body_structure(
    version: mf_runtime::WorkflowDefinitionVersion,
    config: &IterationConfig,
) -> Result<(), IterationValidationError> {
    ensure!(
        config.stream_outputs.is_empty() || version.supports_child_streams(),
        ChildStreamVersionSnafu
    );
    ensure!(
        !config
            .stream_outputs
            .keys()
            .any(|name| name.trim().is_empty() || name == "results"),
        InvalidStreamNameSnafu
    );
    ensure!(
        config.body.outputs.len() == 1
            && config.body.outputs[0].name == "result"
            && !config.body.outputs[0].optional,
        InvalidResultSnafu
    );
    for node in &config.body.nodes {
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
            NestedIterationSnafu {
                node: node.id.clone()
            }
        );
        ensure!(
            !matches!(
                node.kind.as_str(),
                LOOP_KIND | LOOP_ASSIGN_KIND | EXIT_LOOP_KIND | LOOP_SOURCE_ID
            ),
            LoopConstructSnafu {
                node: node.id.clone()
            }
        );
    }
    Ok(())
}
