//! Iteration shares ordinary immutable ports without serializing native capabilities.
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{
    FlowValue, Outputs, PortSpec, PreparedSubgraph, ValueRef, ValueType, WorkflowArguments,
};
use serde_json::{Value, json};

fn definition(mode: &str, result: &str) -> Value {
    json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                "flow":{"type":"flow", "secret":true},
                "label":{"type":"string", "secret":false},
                "settings":{"type":"object", "secret":true},
                "items":{"type":"array", "secret":true}
            }}},
            {"id":"each", "kind":"builtin.iteration", "config":{
                "mode":mode,
                "inputs":{"flow_0":"flow", "label":"string", "settings":"object"},
                "body":{"version":"2026-10-28", "dependencies":{}, "nodes":[],
                    "outputs":[{"name":"result", "node":"%iteration", "port":result}]
                }
            }, "inputs":{
                "items":{"from":{"node":"params", "port":"items"}},
                "flow_0":{"from":{"node":"params", "port":"flow"}},
                "label":{"from":{"node":"params", "port":"label"}},
                "settings":{"from":{"node":"params", "port":"settings"}}
            }}
        ],
        "outputs":[
            {"name":"results", "node":"each", "port":"results"},
            {"name":"flow", "node":"params", "port":"flow"},
            {"name":"label", "node":"params", "port":"label"},
            {"name":"settings", "node":"params", "port":"settings"},
            {"name":"items", "node":"params", "port":"items"}
        ]
    })
}

fn arguments() -> WorkflowArguments {
    let capability = FlowValue::new(PreparedSubgraph::new(
        Vec::new(),
        vec![PortSpec::new("answer", ValueType::String, true)],
        |_| Ok(Outputs::from([("answer".into(), "native answer".into())])),
    ))
    .unwrap();
    WorkflowArguments(Outputs::from([
        (
            "params.flow".into(),
            ValueRef::from(capability).into_secret(),
        ),
        ("params.label".into(), "shared string".into()),
        (
            "params.settings".into(),
            ValueRef::from(json!({"token":"secret"})).into_secret(),
        ),
        (
            "params.items".into(),
            ValueRef::from(json!([{"position":1}, {"position":2}])).into_secret(),
        ),
    ]))
}

#[test]
fn compiled_iterations_share_strings_native_flows_and_secret_objects_without_copying_payloads() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for mode in ["sequential", "parallel"] {
        for port in ["flow_0", "label", "settings", "item"] {
            let definition: WorkflowDefinition =
                serde_json::from_value(definition(mode, port)).unwrap();
            let plan = compile_definition(&definition, &registry).unwrap();
            // Reloading a compiled plan must retain the configured scope types.
            let plan = mf_compiler::CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
            let flow = instantiate_compiled(&plan, &registry).unwrap();
            let outputs = flow.execute_with_inputs(arguments()).unwrap();
            let results = outputs["results"].as_array().unwrap();
            assert_eq!(results.len(), 2);
            for (index, value) in results.iter().enumerate() {
                let expected = match port {
                    "flow_0" => &outputs["flow"],
                    "item" => &outputs["items"][index],
                    name => &outputs[name],
                };
                assert!(value.ptr_eq(expected), "{mode}: {port}");
                assert_eq!(value.is_secret(), port != "label");
                if port == "item" {
                    assert!(value["position"].is_secret());
                }
                if port == "settings" {
                    assert!(value["token"].is_secret());
                }
                if port == "flow_0" {
                    let capability = value.as_flow().unwrap();
                    assert_eq!(
                        capability
                            .invoke(
                                "check",
                                WorkflowArguments::default(),
                                &mut Default::default()
                            )
                            .unwrap()["answer"]
                            .as_str(),
                        Some("native answer")
                    );
                }
            }
        }
    }
}

#[test]
fn compiled_iteration_can_invoke_a_shared_flow_from_each_item_scope() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for mode in ["sequential", "parallel"] {
        let mut value = definition(mode, "flow_0");
        value["nodes"][1]["config"]["body"] = json!({
            "version":"2026-10-28", "dependencies":{},
            "nodes":[{"id":"invoke", "kind":"workflow.call", "config":{
                "inputs":[], "outputs":[{"name":"answer", "value_type":"string", "required":true}]
            }, "inputs":{"flow":{"from":{"node":"%iteration", "port":"flow_0"}}}}],
            "outputs":[{"name":"result", "node":"invoke", "port":"answer"}]
        });
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let outputs = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(arguments())
            .unwrap();
        assert_eq!(
            outputs["results"],
            json!(["native answer", "native answer"])
        );
    }
}

#[test]
fn iteration_shared_input_configuration_is_strict_and_required() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for inputs in [
        json!(null),
        json!([]),
        json!({"bad":false}),
        json!({"bad":"not_a_type"}),
        json!({"":"string"}),
        json!({" ":"any"}),
        json!({"item":"any"}),
        json!({"key":"any"}),
        json!({"items":"any"}),
        json!({"index":"any"}),
    ] {
        let mut value = definition("sequential", "item");
        value["nodes"][1]["config"]["inputs"] = inputs.clone();
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        assert!(
            compile_definition(&definition, &registry).is_err(),
            "{inputs}"
        );
    }
    let mut value = definition("sequential", "label");
    value["nodes"][1]["inputs"]
        .as_object_mut()
        .unwrap()
        .remove("label");
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    assert!(compile_definition(&definition, &registry).is_err());
}

#[test]
fn iteration_shared_inputs_reject_writeback() {
    let mut owner = definition("sequential", "label")["nodes"][1].clone();
    owner["config"]["body"]["nodes"] = json!([
        {"id":"write", "kind":"workflow.assign", "config":{"variable":"label"},
         "inputs":{"value":{"from":{"node":"%iteration", "port":"label"}}}}
    ]);
    let mut value = definition("sequential", "label");
    value["nodes"][1] = owner;
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    assert!(compile_definition(&definition, &registry).is_err());
}
