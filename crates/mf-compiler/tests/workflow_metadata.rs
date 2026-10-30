extern crate mfn_core as _;
mod common;
#[path = "fixtures/multi-nodes/src/schema_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use serde_json::{Value, json};
fn document() -> WorkflowDefinition {
    serde_json::from_value(json!({"version":"2026-10-28","dependencies":{
        "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
        "custom":{"package":"fixture-multi-nodes","path":common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes")}
    },"nodes":[
        {"id":"items","kind":"builtin.constant","config":{"value":[{"operation.payload":{"query":"find","mode":"fast"}}]}},
        {"id":"credential","kind":"builtin.constant","config":{"value":"private-value"}},
        {"id":"batch","kind":"builtin.iteration","config":{"input_mode":"arguments","body":{
            "nodes":[{"id":"operation","kind":"fixture.schema_tool","config":{"allow_drift":true}}],
            "result":{"node":"operation","port":"value"}
        }},"inputs":{"items":{"from":{"node":"items","port":"value"}},"operation.token":{"from":{"node":"credential","port":"value"}}}}
    ],"outputs":[{"name":"results","node":"batch","port":"results"}]})).unwrap()
}
#[test]
fn graph_reflection_keeps_decoded_contracts_without_runtime_values() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&document(), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let body = flow.workflow_graph().nodes[&"batch".into()]
        .subgraph
        .as_ref()
        .unwrap();
    let schema = body.inputs["operation.payload"].schema.as_ref().unwrap();
    assert!(schema.is_complete());
    let encoded = serde_json::to_string(flow.workflow_graph()).unwrap();
    assert!(encoded.contains("Search text") && encoded.contains("thorough"));
    assert!(!encoded.contains("private-value"));
    assert_eq!(
        serde_json::to_value(&flow.execute().unwrap()["results"]).unwrap(),
        json!(["find"])
    );
}
#[cfg(feature = "codegen")]
#[test]
fn generated_scopes_reject_schema_only_drift_before_execution_and_run_without_sources() {
    use std::{fs, process::Command};
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&document(), &registry).unwrap();
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    mf_compiler::write_dependency_project(
        &project,
        &plan,
        &mf_compiler::SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    let built = mf_compiler::cargo_command(&project)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let binary = common::runner_executable(&project, "debug");
    let drift = Command::new(&binary)
        .env("MINIFLOW_TEST_SCHEMA_DRIFT", "1")
        .output()
        .unwrap();
    assert!(!drift.status.success());
    assert!(
        String::from_utf8_lossy(&drift.stderr).contains("metadata differs"),
        "{}",
        String::from_utf8_lossy(&drift.stderr)
    );
    let installed = root.path().join("runner");
    fs::copy(binary, &installed).unwrap();
    fs::remove_dir_all(project).unwrap();
    let output = Command::new(installed).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"results":["find"]})
    );
}
