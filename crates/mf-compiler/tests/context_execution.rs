mod common;
#[path = "fixtures/multi-nodes/src/context_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use serde_json::{Value, json};
use std::{fs, process::Command};

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
fn validates_explicit_skip_contract_and_declared_reads() {
    for change in ["overlap", "unknown", "required", "undeclared"] {
        let mut definition = graph();
        match change {
            "overlap" => definition["nodes"][0]["config"]["outputs"]["off"] = json!(true),
            "unknown" => definition["nodes"][0]["config"]["skipped"] = json!(["absent"]),
            "required" => definition["nodes"][0]["config"]["required"] = json!("off"),
            _ => {
                definition["nodes"][1]["config"]
                    .as_object_mut()
                    .unwrap()
                    .remove("read");
                definition["nodes"][1]["config"]["read_undeclared"] = json!("a.value");
            }
        }
        assert!(flow(definition).execute().is_err(), "{change}");
    }
}
#[test]
fn generated_binary_matches_context_values_and_execution_trace() {
    let root = tempfile::tempdir().unwrap();
    let trace = root.path().join("trace");
    let mut value = graph();
    for node in value["nodes"].as_array_mut().unwrap() {
        node["config"]["trace"] = json!(trace);
        node["config"]["name"] = node["id"].clone();
    }
    let expected = flow(value.clone()).execute().unwrap();
    let expected_trace = fs::read_to_string(&trace).unwrap();
    assert_eq!(expected_trace, "a\nb\n");
    fs::remove_file(&trace).unwrap();
    let definition = serde_json::from_value(value).unwrap();
    let plan = mf_compiler::plan_definition(&definition).unwrap();
    let project = root.path().join("build");
    mf_compiler::write_dependency_project(
        &project,
        &plan,
        &mf_compiler::SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    mf_compiler::resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
    let build = mf_compiler::pipeline::cargo_command(&project)
        .args(["build", "--offline", "--locked"])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let executable = project.join("target/debug/mf-generated-workflow");
    let validate = Command::new(&executable)
        .arg("--validate")
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "{}",
        String::from_utf8_lossy(&validate.stderr)
    );
    assert!(!trace.exists());
    let result = Command::new(&executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!(expected)
    );
    assert_eq!(fs::read_to_string(trace).unwrap(), expected_trace);
}
