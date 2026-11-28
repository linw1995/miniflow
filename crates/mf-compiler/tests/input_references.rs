extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, plan_definition};
use serde_json::{Value, json};

fn reference_workflow() -> Value {
    json!({
        "version":mf_compiler::WorkflowDefinitionVersion::CURRENT, "dependencies":{},
        "nodes":[
            {"id":"source.with.dots", "kind":"builtin.constant", "config":{"value":42}},
            {"id":"consumer", "kind":"builtin.identity", "inputs":{
                "input":{"$ref":"source.with.dots.value"}
            }}
        ],
        "outputs":[{"name":"result", "node":"consumer", "port":"value"}]
    })
}

#[test]
fn references_normalize_before_every_compiler_entry() {
    let value = reference_workflow();
    let source = value["nodes"][1]["inputs"]["input"].clone();
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let original = definition.clone();
    assert_eq!(definition.edges.len(), 1);
    let registry = NodeRegistry::from_inventory().unwrap();
    let structural = plan_definition(&definition).unwrap();
    let prepared = compile_definition(&definition, &registry).unwrap();
    assert_eq!(structural, prepared);
    assert_eq!(
        structural.graph.execution_order[0].as_str(),
        "source.with.dots"
    );
    let result = mf_compiler::execute_compiled(&prepared, &registry, None).unwrap();
    assert_eq!(serde_json::to_value(result).unwrap(), json!({"result":42}));
    assert_eq!(definition, original);
    let encoded = serde_json::to_value(&prepared.graph.definition).unwrap();
    assert!(encoded.get("control_edges").is_none());
    assert_eq!(encoded["nodes"][1]["inputs"]["input"], source);
}

#[test]
fn legacy_and_internal_graph_fields_are_rejected() {
    let base = reference_workflow();
    let mut invalid = Vec::new();
    let mut controls = base.clone();
    controls["control_edges"] = json!([]);
    invalid.push(controls);
    let mut internal = base.clone();
    internal["order_edges"] = json!([]);
    invalid.push(internal);
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
    let input = r#"{"version":"2026-10-19","dependencies":{},"nodes":[{"id":"n","kind":"k","inputs":{"x":{"$ref":"a.p"},"x":{"$ref":"b.p"}}}]}"#;
    assert!(
        serde_json::from_str::<WorkflowDefinition>(input)
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
    for binding in [
        json!(null),
        json!({"exclusive":[]}),
        json!({"exclusive":[{"$ref":"source.with.dots.value"},{"$ref":"source.with.dots.value"}]}),
        json!({"from":{"node":"a","port":"p"},"exclusive":[]}),
        json!({"from":{"node":"a","port":"p","path":"/x"}}),
        json!({"from":{"node":"a","port":""}}),
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
        plan_definition(&definition).unwrap().graph.execution_order[0].as_str(),
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
fn nested_iteration_uses_its_own_version_and_preserves_bindings() {
    let mut value = reference_workflow();
    value["nodes"] = json!([{"id":"each","kind":"builtin.iteration","config":{
        "body":{"version":"2026-10-19", "dependencies":{}, "nodes":[{"id":"echo","kind":"builtin.identity","inputs":{
            "input":{"$ref":"%iteration.item"}
        }}],"outputs": [{"name":"result", "node":"echo","port":"value"}]}
    }}]);
    value["outputs"] = json!([]);
    let definition: WorkflowDefinition = serde_json::from_value(value.clone()).unwrap();
    let children = definition.nodes[0].flow_parameters(&["body"]).unwrap();
    assert_eq!(children["body"].edges[0].from_node.as_str(), "%iteration");
    let encoded = serde_json::to_value(&definition).unwrap();
    assert_eq!(
        encoded["nodes"][0]["config"]["body"]["nodes"][0]["inputs"],
        value["nodes"][0]["config"]["body"]["nodes"][0]["inputs"]
    );
    value["nodes"][0]["config"]["body"]["control_edges"] = json!([]);
    let invalid: WorkflowDefinition = serde_json::from_value(value).unwrap();
    assert!(invalid.nodes[0].flow_parameters(&["body"]).is_err());
}

#[test]
fn exclusive_example_describes_both_routes_and_executes_only_the_selected_value() {
    let mut modern: WorkflowDefinition =
        serde_json::from_str(include_str!("../../../examples/exclusive-inputs.json")).unwrap();
    let encoded = serde_json::to_value(&modern).unwrap();
    let consumer = encoded["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "consume")
        .unwrap();
    assert_eq!(
        consumer["inputs"]["input"],
        json!({"exclusive":[{"$ref":"ci.value"}, {"$ref":"local.value"}]})
    );
    assert_eq!(
        serde_json::from_value::<WorkflowDefinition>(encoded).unwrap(),
        modern
    );
    modern.dependencies.clear();
    let registry = NodeRegistry::from_inventory().unwrap();
    let modern = compile_definition(&modern, &registry).unwrap();
    let description = mf_compiler::describe_compiled(&modern).unwrap();
    assert_eq!(description.data_edges.len(), 2);
    for (edge, source) in description.data_edges.iter().zip(["ci", "local"]) {
        assert!(edge.exclusive);
        assert_eq!(
            (
                edge.from_node.as_str(),
                edge.from_output.as_str(),
                edge.to_node.as_str(),
                edge.to_input.as_str()
            ),
            (source, "value", "consume", "input")
        );
    }
    for ci in [false, true] {
        let arguments = mf_runtime::WorkflowArguments::try_from(json!({"params.ci":ci})).unwrap();
        let current =
            mf_compiler::execute_compiled_with_inputs(&modern, &registry, arguments, None).unwrap();
        assert_eq!(
            json!(current),
            json!({"result":{"mode":if ci { "ci" } else { "local" }}})
        );
    }
    let legacy = include_str!("fixtures/exclusive-inputs-legacy.json");
    assert!(WorkflowDefinition::from_json(legacy).is_err());
}

#[test]
fn unknown_versions_and_binding_diagnostics_identify_the_declaration() {
    let mut value = reference_workflow();
    value["version"] = json!("2099-01-01");
    let error = serde_json::from_value::<WorkflowDefinition>(value)
        .unwrap_err()
        .to_string();
    assert!(error.contains("2099-01-01"));
    let mut value = reference_workflow();
    value["nodes"][1]["inputs"]["input"] = json!({"from":{"node":"source.with.dots","port":null}});
    let error = serde_json::from_value::<WorkflowDefinition>(value)
        .unwrap_err()
        .to_string();
    assert!(error.contains("consumer/inputs/input"), "{error}");
}

#[cfg(feature = "codegen")]
#[test]
fn redundant_success_prerequisites_do_not_consume_typed_payloads() {
    let definition:WorkflowDefinition=serde_json::from_value(json!({
        "version":"2026-11-28","dependencies":{"core":{"package":"mfn-core","path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../builtin-nodes/core")}},
        "nodes":[{"id":"source","kind":"builtin.identity"},{"id":"target","kind":"builtin.identity","inputs":{"input":{"$ref":"source.value"}}}],
        "edges":[{"from_node":"source","to_node":"target"}],"outputs":[{"name":"result","node":"target","port":"value"}]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let artifacts = plan
        .generate_artifacts(&registry, Default::default())
        .unwrap();
    let report = artifacts.typed_plan;
    assert!(
        report.connections.iter().any(|edge| edge.source == "source"
            && edge.target == "target"
            && edge.fallback.is_none())
    );
    let result = mf_compiler::execute_compiled_with_inputs(
        &plan,
        &registry,
        mf_runtime::WorkflowArguments::try_from(json!({"source.input":42})).unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(json!(result), json!({"result":42}));
}

#[test]
fn serialization_rejects_unrepresentable_programmatic_graphs() {
    let original: WorkflowDefinition = serde_json::from_value(reference_workflow()).unwrap();
    let mut conflicting = original.clone();
    conflicting.edges.push(conflicting.edges[0].clone());
    assert!(
        serde_json::to_value(&conflicting)
            .unwrap_err()
            .to_string()
            .contains("conflicting bindings")
    );

    let mut missing = original.clone();
    missing.edges[0].to_node = "missing".into();
    assert!(
        serde_json::to_value(&missing)
            .unwrap_err()
            .to_string()
            .contains("unknown node")
    );

    let mut legacy = original;
    legacy.version = mf_compiler::WorkflowDefinitionVersion::V2026_10_03;
    legacy.order_edges.push(mf_compiler::OrderEdgeDefinition {
        from_node: "source.with.dots".into(),
        to_node: "consumer".into(),
    });
    assert!(
        serde_json::to_value(&legacy)
            .unwrap_err()
            .to_string()
            .contains("require version")
    );
    assert!(
        plan_definition(&legacy)
            .unwrap_err()
            .to_string()
            .contains("require version")
    );
}
