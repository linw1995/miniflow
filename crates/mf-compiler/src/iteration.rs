use crate::{NodeDefinition, WorkflowDefinition, WorkflowOutputDefinition};
use mf_runtime::{
    EXIT_LOOP_KIND, ITERATION_INPUT_ID, ITERATION_INPUT_KIND, ITERATION_KIND, IterationConfig,
    LOOP_ASSIGN_KIND, LOOP_KIND, LOOP_SOURCE_ID,
};

pub fn parse_config(node: &NodeDefinition) -> Result<IterationConfig, String> {
    serde_json::from_value(node.config.clone()).map_err(|error| error.to_string())
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
        dependencies: parent.dependencies.clone(),
        nodes,
        edges: config.body.edges.clone(),
        control_edges: config.body.control_edges.clone(),
        outputs: vec![WorkflowOutputDefinition {
            name: "result".into(),
            node: config.body.result.node.clone(),
            port: config.body.result.port.clone(),
            optional: false,
        }],
    })
}

pub fn normalize_config(
    parent: &WorkflowDefinition,
    node: &NodeDefinition,
) -> Result<serde_json::Value, String> {
    let mut config = parse_config(node)?;
    let body = body_definition(parent, &config)?;
    let order = crate::structural_order(&body).map_err(|error| error.to_string())?;
    let nodes: std::collections::BTreeMap<_, _> = config
        .body
        .nodes
        .iter()
        .map(|node| (&node.id, node))
        .collect();
    config.body.nodes = order
        .iter()
        .filter(|id| id.as_str() != ITERATION_INPUT_ID)
        .map(|id| (*nodes[id]).clone())
        .collect();
    config.body.edges.sort_by(|left, right| {
        (
            &left.from_node,
            &left.from_output,
            &left.to_node,
            &left.to_input,
        )
            .cmp(&(
                &right.from_node,
                &right.from_output,
                &right.to_node,
                &right.to_input,
            ))
    });
    config.body.control_edges.sort();
    serde_json::to_value(config).map_err(|error| error.to_string())
}
