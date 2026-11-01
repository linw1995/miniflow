extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{NodeBuildError, NodeFactory, NodeRegistration, PreparedNode, SubgraphMetadata};
use serde_json::{Value, json};

fn metadata_factory(
    _config: Value,
    metadata: SubgraphMetadata,
) -> Result<PreparedNode, NodeBuildError> {
    assert!(!metadata.outputs.is_empty());
    NodeRegistry::from_inventory()
        .unwrap()
        .get("builtin.constant")
        .unwrap()
        .instantiate(json!({"value": 42}))
}

inventory::submit! {
    NodeRegistration {
        kind: "test.metadata_reference",
        factory: NodeFactory::Metadata(metadata_factory),
    }
}

fn definition(nodes: Value, outputs: Value) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version": "2026-11-01", "dependencies": {}, "nodes": nodes, "outputs": outputs
    }))
    .unwrap()
}

fn iteration(body: Value) -> WorkflowDefinition {
    definition(
        json!([
            {"id": "items", "kind": "builtin.constant", "config": {"value": [1]}},
            {"id": "each", "kind": "builtin.iteration", "config": {"body": {
                "nodes": [body], "result": {"node": "inspect", "port": "value"}
            }}, "inputs": {"items": {"from": {"node": "items", "port": "value"}}}}
        ]),
        json!([{"name": "results", "node": "each", "port": "results"}]),
    )
}

fn container_definitions() -> [WorkflowDefinition; 2] {
    [
        definition(
            json!([{
                "id": "repeat", "kind": "workflow.loop",
                "loop": {
                    "max_iterations": 1,
                    "variables": [{"name": "value", "type": "string"}],
                    "body": {"nodes": [
                        {"id": "constant", "kind": "builtin.constant", "config": {"value": "seed"}}
                    ]}
                }
            }]),
            json!([{"name": "value", "node": "repeat", "port": "value"}]),
        ),
        iteration(json!({"id": "inspect", "kind": "builtin.constant", "config": {"value": 42}})),
    ]
}

#[test]
fn review_plain_provider_subgraph_config_is_opaque() {
    let config = json!({
        "value": 42,
        "subgraph": "opaque",
        "subgraph_metadata": {"arbitrary": "provider-owned"},
        "metadata": {"definition": {"nodes": "not a graph"}}
    });
    let definition = definition(
        json!([{"id": "constant", "kind": "builtin.constant", "config": config}]),
        json!([{"name": "value", "node": "constant", "port": "value"}]),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::resolve_flows(&definition, None).unwrap();
    assert_eq!(resolved.definition.nodes[0].config, config);
    let plan = mf_compiler::plan_definition(&definition).unwrap();
    assert_eq!(plan.definition.nodes[0].config, config);
    let plan = compile_definition(&definition, &registry).unwrap();
    assert_eq!(plan.definition.nodes[0].config, config);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["value"]
            .as_i64(),
        Some(42)
    );
}

#[test]
fn ordinary_metadata_definition_references_use_host_bound_interfaces() {
    let child = definition(
        json!([{"id": "remote", "kind": "unlinked.provider"}]),
        json!([{"name": "value", "node": "remote", "port": "value"}]),
    );
    let reference = json!({"id": "inspect", "kind": "test.metadata_reference", "config": {
        "subgraph": {"flow": "host.child", "definition": child}
    }});
    let node = serde_json::from_value(reference.clone()).unwrap();
    let caller = definition(
        json!([reference]),
        json!([{"name": "value", "node": "inspect", "port": "value"}]),
    );
    let mut registry = NodeRegistry::from_inventory().unwrap();
    assert!(mf_compiler::describe_subgraph(&node, &registry).is_err());
    registry
        .register_flow(
            "host.child",
            child,
            mf_runtime::PreparedSubgraph::new(
                vec![],
                vec![mf_runtime::PortSpec::new(
                    "value",
                    mf_runtime::ValueType::String,
                    true,
                )],
                |_| panic!("metadata inspection must not execute the host Flow"),
            ),
        )
        .unwrap();
    let metadata = mf_compiler::describe_subgraph(&node, &registry).unwrap();
    assert_eq!(metadata.outputs.len(), 1);
    assert_eq!(
        metadata.outputs[0].value_type,
        mf_runtime::ValueType::String
    );
    let plan = compile_definition(&caller, &registry).unwrap();
    // Ordinary Metadata providers retain their existing definition-reference wire format.
    assert_eq!(plan.definition.nodes[0].config, caller.nodes[0].config);
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(output["value"].as_i64(), Some(42));
}

#[test]
fn review_iteration_metadata_references_do_not_nest_executable_containers() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for child in container_definitions() {
        let metadata = json!({"id": "inspect", "kind": "test.metadata_reference", "config": {
            "subgraph": {"flow": "child", "definition": child}
        }});
        let wrapper = definition(
            json!([metadata]),
            json!([{"name": "value", "node": "inspect", "port": "value"}]),
        );
        for node in [
            metadata,
            json!({"id": "inspect", "kind": "workflow.call", "config": {
                "flow": "wrapper", "definition": wrapper
            }}),
        ] {
            let definition = iteration(node);
            mf_compiler::plan_definition(&definition).unwrap();
            let plan = compile_definition(&definition, &registry).unwrap();
            let output = instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute()
                .unwrap();
            assert_eq!(
                serde_json::to_value(output).unwrap(),
                json!({"results": [42]})
            );
        }
    }
}

#[test]
fn precomputed_interfaces_need_no_definition_and_reject_conflicting_sources() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let metadata = json!({
        "inputs": {"inputs": {}, "stdin": {}},
        "outputs": [{"name": "result", "value_type": "int", "required": true}],
        "stream_outputs": []
    });
    let config = json!({"subgraph_metadata": metadata});
    let compiled = compile_definition(
        &definition(
            json!([{"id": "inspect", "kind": "test.metadata_reference", "config": config}]),
            json!([{"name": "value", "node": "inspect", "port": "value"}]),
        ),
        &registry,
    )
    .unwrap();
    let result = instantiate_compiled(&compiled, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(result["value"].as_i64(), Some(42));
    let generated = compiled.generate_execution_plans(&registry).unwrap();
    let manifest = mf_runtime::WorkflowManifest::from_bytes(&generated.manifest_bytes).unwrap();
    assert_eq!(manifest.definition.nodes[0].config, config);
    assert!(!generated.rust_source.contains("unlinked.provider"));

    for invalid in [
        json!({"subgraph_metadata": metadata, "subgraph": {"flow": "conflict"}}),
        json!({"subgraph_metadata": {"inputs": {"inputs": {}, "stdin": {}},
            "outputs": [
                {"name": "duplicate", "value_type": "int", "required": true},
                {"name": "duplicate", "value_type": "int", "required": true}
            ], "stream_outputs": []}}),
    ] {
        assert!(compile_definition(&definition(
            json!([{"id": "inspect", "kind": "test.metadata_reference", "config": invalid}]),
            json!([]),
        ), &registry).is_err());
    }
}

#[test]
fn review_iteration_executable_calls_still_reject_nested_containers() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for child in container_definitions() {
        let definition = iteration(json!({"id": "inspect", "kind": "workflow.call", "config": {
            "flow": "child", "definition": child
        }}));
        for error in [
            mf_compiler::plan_definition(&definition).unwrap_err(),
            compile_definition(&definition, &registry).unwrap_err(),
        ] {
            assert!(
                error
                    .to_string()
                    .contains("cannot contain Loop or Iteration"),
                "{error}"
            );
        }
    }
}
