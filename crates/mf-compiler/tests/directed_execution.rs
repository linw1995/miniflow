extern crate mfn_code as _;
extern crate mfn_core as _;
#[path = "fixtures/connected_tools.rs"]
mod connected;
#[path = "fixtures/multi-nodes/src/directed_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{ContextValue, ExecutionContext, RuntimeOptions, ValueRef};
use serde_json::{Value, json};
use std::num::NonZeroUsize;

fn parse(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}
fn execute(value: Value) -> mf_runtime::FlowOutputs {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap()
}

#[test]
fn connected_arguments_compose_external_inputs_and_execute_the_complete_tool_flow() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(connected::document()), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert!(!flow.input_schema().inputs.contains_key("tool"));
    assert_eq!(
        flow.prepared_calls().next().unwrap().binding.inputs.len(),
        2
    );
    for workers in [1, 4] {
        let outputs = flow
            .execute_with_options(RuntimeOptions {
                max_parallel_domains: NonZeroUsize::new(workers).unwrap(),
            })
            .unwrap();
        assert_eq!(outputs["result"], ValueRef::from("reply:query:credential"));
    }
    let recovered = mf_compiler::CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    assert_eq!(
        instantiate_compiled(&recovered, &registry)
            .unwrap()
            .execute()
            .unwrap()["result"],
        ValueRef::from("reply:query:credential")
    );
}

#[test]
fn only_explicit_workflow_outputs_return_to_the_caller() {
    let mut value = connected::document();
    value["nodes"][0]["config"]["capture_outputs"] = true.into();
    let outputs = execute(value);
    assert_eq!(
        serde_json::to_value(&outputs["result"]).unwrap(),
        json!({"text":"reply:query:credential"})
    );
}

#[test]
fn downstream_processing_and_output_selection_follow_normal_flow_edges() {
    let mut value = connected::document();
    value["nodes"][2]["config"]["definition"]["nodes"][1]["config"]["code"]["message"] =
        "'formatted:' + value".into();
    assert_eq!(
        execute(value)["result"],
        ValueRef::from("formatted:query:credential")
    );
}

#[test]
fn unselected_flows_do_not_execute_and_external_configuration_remains_eager() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = connected::document();
    value["nodes"][0]["config"]["repeat"] = 0.into();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let mut context = ExecutionContext::default();
    assert_eq!(
        flow.execute_in_context(&mut context).unwrap()["result"],
        ValueRef::from("unselected")
    );
    assert!(matches!(
        context.output("z_config.value").unwrap(),
        ContextValue::Value(_)
    ));
    assert!(context.output("tool.text").is_err());
}

#[test]
fn remaining_inputs_are_derived_after_explicit_connections() {
    let mut value = connected::document();
    value["nodes"][2]["inputs"]
        .as_object_mut()
        .unwrap()
        .remove("operation.query");
    value["calls"]["a_parent"]["search"]["defaults"] = "remaining_inputs".into();
    assert_eq!(
        execute(value)["result"],
        ValueRef::from("reply:defaultcredential")
    );
}

#[test]
fn tool_flows_can_be_composed_inside_an_existing_flow_call() {
    let document = json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"outer","kind":"workflow.call","config":{"flow":"outer","definition":connected::document()}}
    ],"outputs":[{"name":"result","node":"outer","port":"result"}]});
    assert_eq!(
        execute(document)["result"],
        ValueRef::from("reply:query:credential")
    );
}

#[test]
fn tool_failures_preserve_the_normal_child_workflow_error_chain() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = connected::document();
    value["nodes"][2]["config"]["definition"]["nodes"][1]["config"]["code"]["message"] =
        "1 / 0".into();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let error = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap_err();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    let mut rendered = String::new();
    while let Some(error) = current {
        rendered.push_str(&error.to_string());
        current = error.source();
    }
    assert!(rendered.contains("render"), "{rendered}");
}

#[test]
fn explicitly_selected_zero_argument_flows_use_the_same_workflow_output_contract() {
    let mut value = connected::document();
    value["nodes"][0]["config"]["alias"] = "unused".into();
    value["nodes"][3]["config"]["definition"]["nodes"][0] =
        json!({"id":"unreachable","kind":"builtin.constant","config":{"value":"zero"}});
    value["nodes"][3]["config"]["definition"]["outputs"][0]["port"] = "value".into();
    assert_eq!(execute(value)["result"], ValueRef::from("zero"));
}
