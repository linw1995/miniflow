extern crate mfn_code as _;
extern crate mfn_core as _;
#[path = "fixtures/connected_tools.rs"]
mod connected;
#[path = "fixtures/multi-nodes/src/directed_fixture.rs"]
mod fixture;
use mf_compiler::{
    FlowBuilder, NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled,
};
use mf_runtime::{
    CallArguments, CallBinding, CallInputDefaults, CallInputSource, NodeId, Outputs, ValueRef,
};
use serde_json::{Value, json};

fn flow(value: Value) -> mf_runtime::Flow {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = serde_json::from_value(value).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    instantiate_compiled(&plan, &registry).unwrap()
}
fn binding(value: Value) -> mf_runtime::ResolvedCallBinding {
    flow(value)
        .prepared_calls()
        .find(|call| call.binding.name == "search")
        .unwrap()
        .binding
        .clone()
}
fn invalid(value: Value) -> mf_compiler::WorkflowCompileError {
    compile_definition(
        &serde_json::from_value(value).unwrap(),
        &NodeRegistry::from_inventory().unwrap(),
    )
    .unwrap_err()
}

#[test]
fn connected_ports_define_caller_arguments_and_external_values_remain_private() {
    let binding = binding(connected::document());
    assert!(matches!(
        binding.inputs[0].source,
        CallInputSource::Output(_)
    ));
    assert!(matches!(
        binding.inputs[1].source,
        CallInputSource::External
    ));
    let input = binding
        .bind_arguments(&CallArguments {
            outputs: Outputs::from([("query".into(), "request".into())]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(input["operation.query"].as_str(), Some("request"));
    assert!(!input.contains_key("operation.token"));
}

#[test]
fn remaining_inputs_never_replace_an_external_port_connection() {
    let mut value = connected::document();
    value["calls"]["a_parent"]["search"]["defaults"] = "remaining_inputs".into();
    let binding = binding(value);
    assert!(matches!(
        binding.inputs[1].source,
        CallInputSource::External
    ));
    assert!(binding.interface().defaults().is_empty());
}

#[test]
fn a_shared_source_is_projected_once_and_validated_against_each_destination() {
    let mut value = connected::document();
    value["nodes"][2]["inputs"]["operation.token"] =
        json!({"from":{"node":"a_parent","port":"query"}});
    let binding = binding(value);
    assert_eq!(binding.interface().outputs().len(), 1);
    let arguments = CallArguments {
        outputs: Outputs::from([("query".into(), "shared".into())]),
        ..Default::default()
    };
    let bound = binding.bind_arguments(&arguments).unwrap();
    assert!(bound["operation.query"].ptr_eq(&bound["operation.token"]));
}

#[test]
fn unknown_arguments_and_missing_required_values_fail_before_execution() {
    let binding = binding(connected::document());
    assert!(binding.bind_arguments(&CallArguments::default()).is_err());
    assert!(
        binding
            .bind_arguments(&CallArguments {
                outputs: Outputs::from([("unknown".into(), "request".into())]),
                ..Default::default()
            })
            .is_err()
    );
    assert!(
        binding
            .bind_arguments(&CallArguments {
                outputs: Outputs::from([("query".into(), ValueRef::null())]),
                ..Default::default()
            })
            .is_err()
    );
}

#[test]
fn ordinary_connection_inference_checks_caller_and_external_types() {
    for input in ["query", "token"] {
        let mut value = connected::document();
        value["nodes"][2]["config"]["definition"]["nodes"][0]["config"]["inputs"][input] =
            "int".into();
        value["nodes"][2]["config"]["definition"]["nodes"][0]["config"]["code"] =
            json!({"text":"'constant'"});
        assert!(invalid(value).to_string().contains("cannot connect"));
    }
}

#[test]
fn caller_owned_inputs_use_defaults_only_after_all_graph_connections_are_resolved() {
    let mut value = connected::document();
    value["nodes"][2]["inputs"]
        .as_object_mut()
        .unwrap()
        .remove("operation.query");
    assert!(
        invalid(value.clone())
            .to_string()
            .contains("operation.query")
    );
    value["calls"]["a_parent"]["search"]["defaults"] = "remaining_inputs".into();
    let binding = binding(value);
    assert!(matches!(binding.inputs[0].source, CallInputSource::Default));
    assert_eq!(binding.interface().defaults()[0].name(), "operation.query");
}

#[test]
fn prepared_host_bindings_resolve_the_same_graph_without_a_second_mapping() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(connected::document()).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let nodes = mf_compiler::resolve_nodes(&plan.definition, &registry).unwrap();
    let caller = NodeId::new(
        nodes
            .iter()
            .position(|node| node.definition_id.as_str() == "a_parent")
            .unwrap(),
    );
    let target = NodeId::new(
        nodes
            .iter()
            .position(|node| node.definition_id.as_str() == "tool")
            .unwrap(),
    );
    let builder = FlowBuilder::prepare(
        nodes,
        plan.definition.edges.clone(),
        plan.execution_order.clone(),
        plan.definition.outputs.clone(),
    )
    .unwrap();
    let bindings = builder
        .resolve_call_bindings(
            caller,
            &[CallBinding::new("manual", target, CallInputDefaults::ExplicitOnly).unwrap()],
        )
        .unwrap();
    assert_eq!(bindings[0].interface().outputs()[0].name(), "query");
    let duplicate = CallBinding::new("same", target, CallInputDefaults::ExplicitOnly).unwrap();
    assert!(matches!(
        builder.resolve_call_bindings(caller, &[duplicate.clone(), duplicate]),
        Err(mf_compiler::CallBindingError::DuplicateName { .. })
    ));
    assert!(matches!(
        builder.resolve_call_bindings(NodeId::new(100), &[]),
        Err(mf_compiler::CallBindingError::UnknownNode { .. })
    ));
}

#[test]
fn optional_workflow_inputs_remain_omitted_without_becoming_skipped_dependencies() {
    let mut value = connected::document();
    value["nodes"][0]["config"]["result"] = "value".into();
    value["nodes"][2]["config"]["definition"] = json!({"version":"2026-10-28","dependencies":{},
        "nodes":[{"id":"optional","kind":"fixture.optional_tool"}],
        "outputs":[{"name":"value","node":"optional","port":"value"}]});
    value["nodes"][2].as_object_mut().unwrap().remove("inputs");
    let prepared = flow(value.clone());
    let binding = &prepared
        .prepared_calls()
        .find(|call| call.binding.name == "search")
        .unwrap()
        .binding;
    assert!(matches!(binding.inputs[0].source, CallInputSource::Omitted));
    assert_eq!(
        prepared.execute().unwrap()["result"].as_str(),
        Some("fallback")
    );
}

#[test]
fn connected_optional_arguments_can_be_omitted_without_reading_the_callers_terminal_port() {
    let mut value = connected::document();
    value["nodes"][0]["config"] = json!({"result":"value","omit_optional":true});
    value["nodes"][2]["config"]["definition"] = json!({"version":"2026-10-28","dependencies":{},
        "nodes":[{"id":"optional","kind":"fixture.optional_tool"}],
        "outputs":[{"name":"value","node":"optional","port":"value"}]});
    value["nodes"][2]["inputs"] =
        json!({"optional.value":{"from":{"node":"a_parent","port":"query"}}});
    let prepared = flow(value);
    assert_eq!(
        prepared.execute().unwrap()["result"].as_str(),
        Some("fallback")
    );
}
