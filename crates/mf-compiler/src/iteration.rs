use crate::compiler::{IterationBodySnafu, IterationConfigurationSnafu, IterationValidationSnafu};
use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition, WorkflowOutputDefinition};
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

pub fn body_definition(
    parent: &WorkflowDefinition,
    config: &IterationConfig,
) -> Result<WorkflowDefinition, IterationValidationError> {
    ensure!(
        config.stream_outputs.is_empty() || parent.version.supports_child_streams(),
        ChildStreamVersionSnafu
    );
    ensure!(
        !config
            .stream_outputs
            .keys()
            .any(|name| name.trim().is_empty() || name == "results"),
        InvalidStreamNameSnafu
    );
    for node in &config.body.nodes {
        ensure!(
            node.id.as_str() != ITERATION_INPUT_ID,
            ReservedNodeSnafu {
                node: node.id.clone()
            }
        );
        ensure!(
            node.kind != ITERATION_KIND && node.kind != ITERATION_INPUT_KIND,
            NestedIterationSnafu {
                node: node.id.clone()
            }
        );
        ensure!(
            !matches!(
                node.kind.as_str(),
                LOOP_KIND | LOOP_ASSIGN_KIND | EXIT_LOOP_KIND | LOOP_SOURCE_ID
            ) && node.loop_definition.is_none(),
            LoopConstructSnafu {
                node: node.id.clone()
            }
        );
    }
    let mut nodes = Vec::with_capacity(config.body.nodes.len() + 1);
    nodes.push(NodeDefinition {
        id: ITERATION_INPUT_ID.into(),
        kind: ITERATION_INPUT_KIND.into(),
        config: serde_json::json!({}),
        loop_definition: None,
    });
    nodes.extend(config.body.nodes.iter().cloned());
    Ok(WorkflowDefinition {
        version: parent.version,
        annotation: None,
        execution: None,
        flows: Default::default(),
        dependencies: parent.dependencies.clone(),
        nodes,
        edges: config.body.edges.clone(),
        control_edges: config.body.control_edges.clone(),
        order_edges: config.body.order_edges.clone(),
        outputs: vec![WorkflowOutputDefinition {
            name: "result".into(),
            node: config.body.result.node.clone(),
            port: config.body.result.port.clone().into(),
            optional: false,
        }],
        clusters: config.body.clusters.clone(),
    })
}

pub fn normalize_config(
    parent: &WorkflowDefinition,
    node: &NodeDefinition,
) -> Result<serde_json::Value, WorkflowCompileError> {
    let mut config = parse_config(node)?;
    for call in config
        .body
        .nodes
        .iter()
        .filter(|node| node.kind == mf_runtime::CALL_KIND)
    {
        let child = crate::calls::body_definition(call)?;
        snafu::ensure!(
            !crate::calls::contains_iteration_containers(&child)?,
            crate::compiler::InvalidCallSnafu {
                definition_id: call.id.clone(),
                message: "calls inside Iteration cannot contain Loop or Iteration constructs"
            }
        );
    }
    let body = body_definition(parent, &config).context(IterationValidationSnafu {
        definition_id: node.id.clone(),
    })?;
    let order = crate::structural_order(&body).context(IterationBodySnafu {
        definition_id: node.id.clone(),
    })?;
    let mut normalized = crate::compiler::normalize_plan(&body, order)
        .context(IterationBodySnafu {
            definition_id: node.id.clone(),
        })?
        .definition;
    normalized
        .nodes
        .retain(|node| node.id.as_str() != ITERATION_INPUT_ID);
    config.body.nodes = normalized.nodes;
    config.body.edges = normalized.edges;
    config.body.control_edges = normalized.control_edges;
    config.body.order_edges = normalized.order_edges;
    config.body.clusters = normalized.clusters;
    serde_json::to_value(config).context(IterationConfigurationSnafu {
        definition_id: node.id.clone(),
    })
}
