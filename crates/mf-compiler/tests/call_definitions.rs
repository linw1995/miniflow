extern crate mfn_code as _;
extern crate mfn_core as _;
#[path = "fixtures/connected_tools.rs"]
mod connected;
#[path = "fixtures/multi-nodes/src/directed_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition};
use serde_json::{Value, json};

fn compile(
    value: Value,
) -> Result<mf_compiler::CompiledWorkflow, mf_compiler::WorkflowCompileError> {
    compile_definition(
        &serde_json::from_value(value).unwrap(),
        &NodeRegistry::from_inventory().unwrap(),
    )
}

#[test]
fn wire_calls_only_declare_identity_and_metadata_while_inputs_define_port_ownership() {
    let definition: WorkflowDefinition = serde_json::from_value(connected::document()).unwrap();
    let encoded = serde_json::to_value(&definition).unwrap();
    assert_eq!(
        encoded["nodes"][2]["inputs"]["operation.query"],
        json!({"from":{"node":"a_parent","port":"query"}})
    );
    assert!(
        encoded["calls"]["a_parent"]["search"]
            .get("ports")
            .is_none()
    );
    compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap();
}

#[test]
fn the_retired_parallel_port_mapping_is_rejected() {
    let mut value = connected::document();
    value["calls"]["a_parent"]["search"]["ports"] =
        json!([{"output":"query","input":"operation.query"}]);
    assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
}

#[test]
fn caller_can_only_supply_inputs_connected_to_its_output_ports() {
    let mut value = connected::document();
    value["nodes"][2]["inputs"]
        .as_object_mut()
        .unwrap()
        .remove("operation.query");
    assert!(
        compile(value)
            .unwrap_err()
            .to_string()
            .contains("operation.query")
    );
}

#[test]
fn ordinary_connection_validation_rejects_unknown_argument_ports() {
    let mut value = connected::document();
    value["nodes"][2]["inputs"]["operation.query"]["from"]["port"] = "unknown".into();
    assert!(compile(value).is_err());
}

#[test]
fn directed_targets_require_an_explicit_workflow_boundary() {
    let mut value = connected::document();
    value["nodes"][2] = json!({"id":"tool","kind":"builtin.constant","config":{"value":"hidden"}});
    let error = compile(value).unwrap_err();
    assert!(error.to_string().contains("workflow.call"), "{error}");
}

#[test]
fn internal_values_cannot_be_exported_by_an_implicit_target_result_mapping() {
    let mut value = connected::document();
    value["nodes"][2]["config"]["definition"]["outputs"][0]["node"] = "unknown".into();
    assert!(compile(value).is_err());
}

#[test]
fn ordinary_input_sources_cannot_be_replaced_by_default_argument_ownership() {
    let definition: WorkflowDefinition = serde_json::from_value(connected::document()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let flow = mf_compiler::instantiate_compiled(
        &compile_definition(&definition, &registry).unwrap(),
        &registry,
    )
    .unwrap();
    let binding = flow
        .prepared_calls()
        .find(|call| call.binding.name == "search")
        .unwrap();
    let interface = binding.binding.interface();
    assert_eq!(
        interface
            .outputs()
            .iter()
            .map(|port| port.name())
            .collect::<Vec<_>>(),
        ["query"]
    );
    assert!(interface.defaults().is_empty());
}

#[test]
fn outer_call_sites_do_not_waive_input_requirements_of_reused_ids_in_loop_bodies() {
    let mut value = connected::document();
    value["nodes"].as_array_mut().unwrap().push(
        json!({"id":"repeat","kind":"workflow.loop","loop":{
            "max_iterations":1,"variables":[{"name":"count","type":"int"}],
            "body":{"nodes":[{"id":"tool","kind":"builtin.identity"}]}
        }}),
    );
    let error = compile(value).unwrap_err().to_string();
    assert!(error.contains("tool") && error.contains("input"), "{error}");
}

#[test]
fn readiness_ignores_only_the_active_callers_argument_edges() {
    let mut value = connected::document();
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"other_parent","kind":"test.directed_caller"}));
    value["calls"]["other_parent"] = json!({"search":{"target":"tool"}});
    value["nodes"][2]["inputs"]["operation.token"] =
        json!({"from":{"node":"other_parent","port":"query"}});
    assert!(matches!(
        compile(value),
        Err(mf_compiler::WorkflowCompileError::Cycle { .. })
    ));
}
