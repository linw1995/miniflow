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
    let config = json!({"value": 42, "subgraph": "opaque"});
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
