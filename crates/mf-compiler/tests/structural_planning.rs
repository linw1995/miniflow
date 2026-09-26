mod common;
use mf_compiler::{WorkflowCompileError, plan_definition};

#[test]
fn rejects_invalid_ids_without_loading_plugins() {
    let mut definition = common::fixture_definition();
    definition.nodes[0].id = "  ".into();
    assert!(matches!(
        plan_definition(&definition),
        Err(WorkflowCompileError::InvalidNodeId { position: 1 })
    ));
    definition.nodes[0].id = definition.nodes[1].id.clone();
    assert!(matches!(
        plan_definition(&definition),
        Err(WorkflowCompileError::DuplicateNodeId { definition_id }) if definition_id.as_str() == "b"
    ));
}

#[test]
fn rejects_dangling_targets_with_connection_context() {
    let mut definition = common::fixture_definition();
    definition.edges[0].to_node = "missing".into();
    assert!(matches!(
        plan_definition(&definition),
        Err(WorkflowCompileError::UnknownEdgeTarget { from_node, from_output, to_node, to_input })
            if from_node.as_str() == "a" && from_output == "value"
                && to_node.as_str() == "missing" && to_input == "input"
    ));
}

#[test]
fn rejects_ambiguous_input_bindings_without_port_metadata() {
    let mut definition = common::fixture_definition();
    definition.edges.push(definition.edges[0].clone());
    assert!(matches!(
        plan_definition(&definition),
        Err(WorkflowCompileError::DuplicateInputConnection { node_id, port })
            if node_id.as_str() == "b" && port == "input"
    ));
}

#[test]
fn rejects_invalid_output_selections_without_loading_plugins() {
    let mut definition = common::fixture_definition();
    definition.outputs.push(definition.outputs[0].clone());
    assert!(matches!(
        plan_definition(&definition),
        Err(WorkflowCompileError::DuplicateWorkflowOutputName { name }) if name == "result"
    ));
    definition.outputs.pop();
    definition.outputs[0].node = "missing".into();
    assert!(matches!(
        plan_definition(&definition),
        Err(WorkflowCompileError::UnknownWorkflowOutputNode { name, node_id })
            if name == "result" && node_id.as_str() == "missing"
    ));
}

#[test]
fn rejects_cycles_before_plugin_contract_checks() {
    let mut definition = common::fixture_definition();
    let mut back_edge = definition.edges[0].clone();
    back_edge.from_node = "b".into();
    back_edge.to_node = "a".into();
    definition.edges.push(back_edge);
    assert!(matches!(
        plan_definition(&definition),
        Err(WorkflowCompileError::Cycle { path }) if path.to_string() == "a -> b -> a"
    ));
}
