extern crate mfn_code as _;
extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{RuntimeOptions, ValueRef};
use serde_json::{Value, json};
use std::num::NonZeroUsize;

fn document(mode: &str) -> Value {
    json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"items","kind":"builtin.constant","config":{"value":[{"calculate.x":2},{"calculate.x":4}]}},
        {"id":"factor","kind":"builtin.constant","config":{"value":3}},
        {"id":"batch","kind":"builtin.iteration","config":{"mode":mode,"input_mode":"arguments","body":{
            "nodes":[
                {"id":"calculate","kind":"builtin.code","config":{"language":"cel","inputs":{"x":"int","factor":"int"},"code":{"result":"x * factor"}}},
                {"id":"format","kind":"builtin.code","config":{"language":"cel","inputs":{"value":"int"},"code":{"text":"'value:' + string(value)"}},"inputs":{"value":{"from":{"node":"calculate","port":"result"}}}}
            ],"result":{"node":"format","port":"text"}
        }},"inputs":{"items":{"from":{"node":"items","port":"value"}},"calculate.factor":{"from":{"node":"factor","port":"value"}}}}
    ],"outputs":[{"name":"results","node":"batch","port":"results"}]})
}
fn prepare(value: Value) -> mf_runtime::Flow {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    instantiate_compiled(&plan, &registry).unwrap()
}
#[test]
fn workflow_argument_batches_use_normal_connections_and_preserve_order_at_each_worker_limit() {
    for mode in ["sequential", "parallel"] {
        for workers in [1, 4] {
            let flow = prepare(document(mode));
            let output = flow
                .execute_with_options(RuntimeOptions {
                    max_parallel_domains: NonZeroUsize::new(workers).unwrap(),
                })
                .unwrap();
            assert_eq!(
                serde_json::to_value(&output["results"]).unwrap(),
                json!(["value:6", "value:12"])
            );
            let body = flow.workflow_graph().nodes[&"batch".into()]
                .subgraph
                .as_ref()
                .unwrap();
            assert!(
                body.inputs.contains_key("calculate.x")
                    && body.inputs.contains_key("calculate.factor")
            );
            assert!(
                body.connections
                    .iter()
                    .any(|edge| edge.from_node.as_str() == "calculate"
                        && edge.to_node.as_str() == "format")
            );
        }
    }
}
#[test]
fn per_item_arguments_override_shared_values_without_leaking_between_items() {
    let mut value = document("sequential");
    value["nodes"][0]["config"]["value"] =
        json!([{"calculate.x":2,"calculate.factor":5},{"calculate.x":4}]);
    assert_eq!(
        serde_json::to_value(&prepare(value).execute().unwrap()["results"]).unwrap(),
        json!(["value:10", "value:12"])
    );
}
#[test]
fn missing_unknown_and_malformed_item_arguments_fail_through_workflow_input_validation() {
    for items in [
        json!([{}]),
        json!([{"unknown":2}]),
        json!([1]),
        json!({"key":{"calculate.x":2}}),
    ] {
        let mut value = document("sequential");
        value["nodes"][0]["config"]["value"] = items;
        assert!(prepare(value).execute().is_err());
    }
}
#[test]
fn empty_argument_batches_execute_no_body_nodes() {
    let mut value = document("parallel");
    value["nodes"][0]["config"]["value"] = json!([]);
    value["nodes"][2]["config"]["body"]["nodes"][0]["config"]["code"]["result"] = "1 / 0".into();
    assert_eq!(
        prepare(value).execute().unwrap()["results"],
        ValueRef::array(Vec::new())
    );
}
