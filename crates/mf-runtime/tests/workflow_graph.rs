use mf_runtime::*;
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn graph_scope_restores_on_failure_and_unwinding_and_domain_forks_keep_it() {
    let graph = Arc::new(WorkflowGraph::default());
    let mut context = ExecutionContext::default();
    let result: Result<(), ()> = context.with_workflow_graph(&graph, |context| {
        assert!(std::ptr::eq(
            context.workflow_graph().unwrap(),
            graph.as_ref()
        ));
        assert!(
            context
                .fork_domain_with_observation(None)
                .workflow_graph()
                .is_some()
        );
        assert!(context.fork_body(None).workflow_graph().is_none());
        Err(())
    });
    assert!(result.is_err());
    assert!(context.workflow_graph().is_none());
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), ()> = context.with_workflow_graph(&graph, |_| panic!("provider panic"));
    }));
    assert!(panic.is_err());
    assert!(context.workflow_graph().is_none());
}

#[test]
fn descriptor_contains_ports_and_sensitivity_without_environment_or_default_values() {
    let metadata = NodeMetadata {
        ports: NodePorts {
            inputs: vec![PortSpec::new("token", ValueType::String, false)],
            outputs: vec![],
        },
        input_bindings: BTreeMap::from([(
            "token".into(),
            InputBinding {
                secret: true,
                env: Some("PRIVATE_ENVIRONMENT_NAME".into()),
                default: Some("private-default-value".into()),
            },
        )]),
        ..Default::default()
    };
    let node = FlowNode {
        definition_id: "source".into(),
        node: (),
        metadata,
        output_keys: Default::default(),
    };
    let graph = WorkflowGraph::from_connections(&[node], &[], vec![]);
    let debug = format!("{graph:?}");
    assert!(!debug.contains("token") && !debug.contains("private-default-value"));
    let encoded = serde_json::to_string(&graph).unwrap();
    assert!(encoded.contains("token"));
    assert!(
        !encoded.contains("PRIVATE_ENVIRONMENT_NAME") && !encoded.contains("private-default-value")
    );
    assert!(
        graph.nodes[&DefinitionId::from("source")]
            .secret_inputs
            .contains("token")
    );
}

#[test]
fn graph_paths_are_literal_node_ids_and_do_not_execute_subgraphs() {
    let leaf = GraphNode {
        ports: NodePorts::default(),
        stream_outputs: vec![],
        input_schemas: BTreeMap::new(),
        secret_inputs: Default::default(),
        exclusive_outputs: vec![],
        subgraph: None,
    };
    let child = Arc::new(WorkflowGraph {
        nodes: BTreeMap::from([("node.with.dots".into(), leaf.clone())]),
        ..Default::default()
    });
    let graph = WorkflowGraph {
        nodes: BTreeMap::from([(
            "body".into(),
            GraphNode {
                subgraph: Some(child),
                ..leaf
            },
        )]),
        ..Default::default()
    };
    assert!(
        graph
            .locate(&["body".into(), "node.with.dots".into()])
            .is_some()
    );
    assert!(graph.locate(&["body.node.with.dots".into()]).is_none());
    assert!(graph.locate(&[]).is_none());
}
