mod common;
#[path = "fixtures/multi-nodes/src/context_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use serde_json::{Value, json};

fn graph() -> Value {
    json!({"version":"2026-09-26","dependencies": common::fixture_definition().dependencies,
        "nodes":[
            {"id":"a","kind":"fixture.context","config":{"ports":["value","off"],"outputs":{"value":null},"skipped":["off"]}},
            {"id":"b","kind":"fixture.context","config":{"ports":["value"],"read":"a.value"}},
            {"id":"c","kind":"fixture.context","config":{"ports":["value"],"fail":true}},
            {"id":"d","kind":"fixture.context","config":{"ports":["value"],"fail":true}}
        ],
        "control_edges":[
            {"from_node":"a","from_output":"value","to_node":"b"},
            {"from_node":"a","from_output":"off","to_node":"c"},
            {"from_node":"c","from_output":"value","to_node":"d"}
        ],"outputs":[{"name":"result","node":"b","port":"value"}]})
}
fn flow(value: Value) -> mf_compiler::Flow {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    instantiate_compiled(
        &compile_definition(&definition, &registry).unwrap(),
        &registry,
    )
    .unwrap()
}
#[test]
fn controls_check_availability_and_runs_have_fresh_contexts() {
    for value in [json!(null), json!(false), json!(42)] {
        let mut definition = graph();
        definition["nodes"][0]["config"]["outputs"]["value"] = value.clone();
        let flow = flow(definition);
        for _ in 0..2 {
            assert_eq!(flow.execute().unwrap()["result"], value);
        }
    }
}
#[test]
fn skips_fanout_and_checks_missing_data_before_skipping() {
    let mut definition = graph();
    definition["nodes"][1]["config"]["inputs"] = json!(["extra"]);
    definition["edges"] =
        json!([{"from_node":"a","from_output":"off","to_node":"b","to_input":"extra"}]);
    definition["outputs"] = json!([]);
    flow(definition.clone()).execute().unwrap();
    definition["nodes"][0]["config"]["outputs"] = json!({});
    let error = flow(definition.clone()).execute().unwrap_err().to_string();
    assert!(error.contains("a.value") && error.contains("b"));
    definition["control_edges"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert_eq!(flow(definition).execute().unwrap_err().to_string(), error);
}
#[test]
fn validates_explicit_skip_contract() {
    for change in ["overlap", "unknown", "required", "extra"] {
        let mut definition = graph();
        match change {
            "overlap" => definition["nodes"][0]["config"]["outputs"]["off"] = json!(true),
            "unknown" => definition["nodes"][0]["config"]["skipped"] = json!(["absent"]),
            "extra" => definition["nodes"][0]["config"]["outputs"]["extra"] = json!(true),
            _ => definition["nodes"][0]["config"]["required"] = json!("off"),
        }
        assert!(flow(definition).execute().is_err(), "{change}");
    }
}
#[test]
fn selected_outputs_distinguish_skips_null_and_missing() {
    let mut value = graph();
    value["outputs"] = json!([
        {"name":"null","node":"b","port":"value","optional":true},
        {"name":"port_skip","node":"a","port":"off","optional":true},
        {"name":"node_skip","node":"d","port":"value","optional":true}
    ]);
    assert_eq!(
        json!(flow(value.clone()).execute().unwrap()),
        json!({"null":null})
    );
    value["outputs"][1]["optional"] = json!(false);
    let error = flow(value.clone()).execute().unwrap_err().to_string();
    assert!(error.contains("port_skip") && error.contains("a.off") && error.contains("skipped"));
    value["outputs"] = json!([{"name":"only","node":"d","port":"value","optional":true}]);
    assert!(flow(value.clone()).execute().unwrap().is_empty());
    value["nodes"][0]["config"]["ports"]
        .as_array_mut()
        .unwrap()
        .push(json!("missing"));
    value["outputs"] = json!([{"name":"broken","node":"a","port":"missing","optional":true}]);
    let error = flow(value).execute().unwrap_err().to_string();
    assert!(error.contains("broken") && error.contains("a.missing"));
}

#[test]
fn optional_selection_defaults_round_trip_and_remain_strict() {
    let definition: WorkflowDefinition = serde_json::from_value(graph()).unwrap();
    assert!(!definition.outputs[0].optional);
    assert!(
        serde_json::to_value(&definition).unwrap()["outputs"][0]
            .get("optional")
            .is_none()
    );
    let mut value = graph();
    value["outputs"][0]["optional"] = json!("true");
    assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
    for change in ["unknown", "duplicate"] {
        let mut value = graph();
        value["outputs"][0]["optional"] = json!(true);
        if change == "unknown" {
            value["outputs"][0]["port"] = json!("unknown");
        } else {
            let duplicate = value["outputs"][0].clone();
            value["outputs"].as_array_mut().unwrap().push(duplicate);
        }
        let definition = serde_json::from_value(value).unwrap();
        assert!(compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).is_err());
    }
}

#[test]
fn snapshots_distinguish_null_skips_and_publication_failures() {
    use mf_runtime::{ExecutionContext, SnapshotOutcome, SnapshotRecorder};
    let recorder = SnapshotRecorder::memory();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(recorder.clone());
    flow(graph()).execute_in_context(&mut context).unwrap();
    let root = recorder.current();
    assert_eq!(root.node(&[], "b").unwrap().outputs["value"], json!(null));
    assert_eq!(
        root.node(&[], "c").unwrap().outcome,
        SnapshotOutcome::Skipped
    );
    assert_eq!(root.node(&[], "c").unwrap().skipped.as_ref(), &["value"]);
    assert!(
        root.node(&[], "c")
            .unwrap()
            .outputs
            .as_object()
            .unwrap()
            .is_empty()
    );

    let mut invalid = graph();
    invalid["nodes"][0]["config"]["outputs"]["extra"] = json!(true);
    let recorder = SnapshotRecorder::memory();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(recorder.clone());
    assert!(flow(invalid).execute_in_context(&mut context).is_err());
    let root = recorder.current();
    let failure = root.node(&[], "a").unwrap();
    assert_eq!(failure.outcome, SnapshotOutcome::Failed);
    assert_eq!(failure.outputs["extra"], json!(true));
    assert!(failure.error.as_ref().unwrap().contains("undeclared"));
    assert!(context.output("a.extra").is_err());
}
