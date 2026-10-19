extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, plan_definition};
use serde_json::{Value, json};

fn reference_workflow() -> Value {
    json!({
        "version":"2026-10-19", "dependencies":{},
        "nodes":[
            {"id":"source.with.dots", "kind":"builtin.constant", "config":{"value":42}},
            {"id":"consumer", "kind":"builtin.identity", "inputs":{
                "input":{"from":{"node":"source.with.dots", "port":"value"}}
            }}
        ],
        "outputs":[{"name":"result", "node":"consumer", "port":"value"}]
    })
}

#[test]
fn references_normalize_before_every_compiler_entry() {
    let definition: WorkflowDefinition = serde_json::from_value(reference_workflow()).unwrap();
    let original = definition.clone();
    assert_eq!(definition.edges.len(), 1);
    let registry = NodeRegistry::from_inventory().unwrap();
    let structural = plan_definition(&definition).unwrap();
    let prepared = compile_definition(&definition, &registry).unwrap();
    assert_eq!(structural, prepared);
    assert_eq!(structural.execution_order[0].as_str(), "source.with.dots");
    let result = mf_compiler::execute_compiled(&prepared, &registry, None).unwrap();
    assert_eq!(serde_json::to_value(result).unwrap(), json!({"result":42}));
    assert_eq!(definition, original);
    let encoded = serde_json::to_value(&prepared).unwrap();
    assert!(encoded["definition"].get("control_edges").is_none());
    assert!(encoded["definition"]["nodes"][1].get("inputs").is_some());
    let decoded: mf_compiler::CompiledWorkflow = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, prepared);
}

#[test]
fn legacy_and_reference_bindings_have_equivalent_graphs() {
    let modern: WorkflowDefinition = serde_json::from_value(reference_workflow()).unwrap();
    let mut legacy = modern.clone();
    legacy.version = mf_compiler::WorkflowDefinitionVersion::V2026_10_03;
    let encoded = serde_json::to_value(&legacy).unwrap();
    assert!(encoded["nodes"][1].get("inputs").is_none());
    assert_eq!(encoded["edges"][0]["to_input"], "input");
    let decoded: WorkflowDefinition = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, legacy);
    assert_eq!(
        plan_definition(&legacy).unwrap().execution_order,
        plan_definition(&modern).unwrap().execution_order
    );
}

#[test]
fn versions_and_edge_shapes_cannot_be_mixed() {
    let base = reference_workflow();
    let mut invalid = Vec::new();
    let mut legacy = base.clone();
    legacy["version"] = json!("2026-10-03");
    invalid.push(legacy);
    let mut controls = base.clone();
    controls["control_edges"] = json!([]);
    invalid.push(controls);
    for edge in [
        json!({"from_node":"source.with.dots","to_node":"consumer","to_input":"input","from_output":"value"}),
        json!({"from_node":"source.with.dots","to_node":"consumer","from_output":null}),
        json!({"from_node":"source.with.dots","to_node":"consumer","from_output":""}),
        json!({"from_node":"source.with.dots","to_node":"consumer","exclusive":true}),
    ] {
        let mut definition = base.clone();
        definition["edges"] = json!([edge]);
        invalid.push(definition);
    }
    for definition in invalid {
        assert!(
            serde_json::from_value::<WorkflowDefinition>(definition.clone()).is_err(),
            "{definition}"
        );
    }
}

#[test]
fn duplicate_input_keys_and_malformed_groups_fail() {
    let input = r#"{"version":"2026-10-19","dependencies":{},"nodes":[{"id":"n","kind":"k","inputs":{"x":{"from":{"node":"a","port":"p"}},"x":{"from":{"node":"b","port":"p"}}}}]}"#;
    assert!(
        serde_json::from_str::<WorkflowDefinition>(input)
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
    for binding in [
        json!(null),
        json!({"exclusive":[]}),
        json!({"exclusive":[{"node":"a","port":"p"},{"node":"a","port":"p"}]}),
        json!({"from":{"node":"a","port":"p"},"exclusive":[]}),
        json!({"from":{"node":"a","port":"p","path":"/x"}}),
    ] {
        let mut definition = reference_workflow();
        definition["nodes"][1]["inputs"]["input"] = binding;
        assert!(serde_json::from_value::<WorkflowDefinition>(definition).is_err());
    }
}

#[test]
fn success_edges_participate_in_structural_order_and_cycles() {
    let mut value = reference_workflow();
    value["nodes"][1].as_object_mut().unwrap().remove("inputs");
    value["edges"] = json!([{"from_node":"source.with.dots","to_node":"consumer"}]);
    let definition: WorkflowDefinition = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        plan_definition(&definition).unwrap().execution_order[0].as_str(),
        "source.with.dots"
    );
    assert_eq!(
        serde_json::to_value(&definition).unwrap()["edges"],
        value["edges"]
    );
    value["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"consumer","to_node":"source.with.dots"}));
    assert!(plan_definition(&serde_json::from_value(value).unwrap()).is_err());
}

#[test]
fn nested_iteration_uses_the_root_version_and_preserves_bindings() {
    let mut value = reference_workflow();
    value["nodes"] = json!([{"id":"each","kind":"builtin.iteration","config":{
        "body":{"nodes":[{"id":"echo","kind":"builtin.identity","inputs":{
            "input":{"from":{"node":"%iteration","port":"item"}}
        }}],"result":{"node":"echo","port":"value"}}
    }}]);
    value["outputs"] = json!([]);
    let definition: WorkflowDefinition = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        definition.nodes[0].config["body"]["edges"][0]["from_node"],
        "%iteration"
    );
    let encoded = serde_json::to_value(&definition).unwrap();
    assert_eq!(
        encoded["nodes"][0]["config"]["body"]["nodes"][0]["inputs"],
        value["nodes"][0]["config"]["body"]["nodes"][0]["inputs"]
    );
    value["nodes"][0]["config"]["body"]["control_edges"] = json!([]);
    assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
}
