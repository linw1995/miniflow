use crate::compiler::{InvalidIterationSnafu, IterationBodySnafu, IterationConfigurationSnafu};
use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition, WorkflowOutputDefinition};
use mf_runtime::{
    EXIT_LOOP_KIND, ITERATION_INPUT_ID, ITERATION_INPUT_KIND, ITERATION_KIND, IterationConfig,
    LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID,
};
use snafu::ResultExt;

pub fn parse_config(node: &NodeDefinition) -> Result<IterationConfig, WorkflowCompileError> {
    serde_json::from_value(node.config.clone()).context(IterationConfigurationSnafu {
        definition_id: node.id.clone(),
    })
}

pub fn body_definition(
    parent: &WorkflowDefinition,
    config: &IterationConfig,
) -> Result<WorkflowDefinition, String> {
    for node in &config.body.nodes {
        if node.id.as_str() == ITERATION_INPUT_ID {
            return Err(format!("body node ID `{ITERATION_INPUT_ID}` is reserved"));
        }
        if node.kind == ITERATION_KIND || node.kind == ITERATION_INPUT_KIND {
            return Err(format!(
                "body node `{}` uses an unsupported nested or internal kind",
                node.id
            ));
        }
        if matches!(
            node.kind.as_str(),
            LOOP_KIND | LOOP_ASSIGN_KIND | EXIT_LOOP_KIND | LOOP_SOURCE_ID
        ) || node.loop_definition.is_some()
        {
            return Err(format!(
                "body node `{}` uses an unsupported Loop construct",
                node.id
            ));
        }
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
        execution: None,
        dependencies: parent.dependencies.clone(),
        nodes,
        edges: config.body.edges.clone(),
        control_edges: config.body.control_edges.clone(),
        outputs: vec![WorkflowOutputDefinition {
            name: "result".into(),
            node: config.body.result.node.clone(),
            port: config.body.result.port.clone().into(),
            optional: false,
        }],
    })
}

pub fn normalize_config(
    parent: &WorkflowDefinition,
    node: &NodeDefinition,
) -> Result<serde_json::Value, WorkflowCompileError> {
    let mut config = parse_config(node)?;
    let body = body_definition(parent, &config).map_err(|message| {
        InvalidIterationSnafu {
            definition_id: node.id.clone(),
            message,
        }
        .build()
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
    serde_json::to_value(config).context(IterationConfigurationSnafu {
        definition_id: node.id.clone(),
    })
}
