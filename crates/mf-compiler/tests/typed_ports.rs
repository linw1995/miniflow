mod common;
#[path = "fixtures/multi-nodes/src/typed_fixture.rs"]
mod fixture;
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowCompileError, WorkflowDefinition, compile_definition,
    instantiate_compiled, plan_definition,
};
use serde_json::{Value, json};
use std::process::Command;

fn graph(source_type: &str, value: Value, sink_type: &str) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version": "2026-09-26",
        "dependencies": common::fixture_definition().dependencies,
        "nodes": [
            {"id": "source", "kind": "fixture.typed_source", "config": {"type": source_type, "value": value}},
            {"id": "sink", "kind": "fixture.typed_echo", "config": {"type": sink_type}}
        ],
        "edges": [{"from_node": "source", "from_output": "value", "to_node": "sink", "to_input": "input"}],
        "outputs": [{"name": "result", "node": "sink", "port": "value"}]
    }))
    .unwrap()
}

fn skipped_graph() -> WorkflowDefinition {
    let mut dependencies = serde_json::to_value(common::fixture_definition().dependencies).unwrap();
    dependencies["core"] =
        json!({"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")});
    serde_json::from_value(json!({
        "version": "2026-09-26",
        "dependencies": dependencies,
        "nodes": [
            {"id": "source", "kind": "fixture.typed_source", "config": {"type": "any", "value": "wrong"}},
            {"id": "trigger", "kind": "fixture.typed_source", "config": {"type": "boolean", "value": false}},
            {"id": "route", "kind": "builtin.if_else", "config": {"branches": [{
                "id": "on", "condition": {"source": {"output": "trigger.value", "path": ""}, "operator": "eq", "value": true}
            }]}},
            {"id": "sink", "kind": "fixture.typed_echo", "config": {"type": "int64"}}
        ],
        "edges": [{"from_node": "source", "from_output": "value", "to_node": "sink", "to_input": "input"}],
        "control_edges": [
            {"from_node": "trigger", "from_output": "value", "to_node": "route"},
            {"from_node": "route", "from_output": "on", "to_node": "sink"}
        ],
        "outputs": [{"name": "result", "node": "sink", "port": "value", "optional": true}]
    }))
    .unwrap()
}

fn run_in_memory(definition: &WorkflowDefinition) -> Result<mf_compiler::FlowOutputs, String> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(definition, &registry).map_err(|error| error.to_string())?;
    instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .map_err(|error| error.to_string())
}

#[test]
fn classifies_refined_and_dynamic_connections() {
    let mut static_source = graph("int64", json!(7), "int64");
    static_source.nodes[0].kind = "fixture.integer_source".into();
    static_source.nodes[0].config = json!({});
    assert_eq!(run_in_memory(&static_source).unwrap()["result"], json!(7));

    for (source_type, value, sink_type, expected) in [
        ("int64", json!(7), "number", json!(7)),
        ("any", json!(7), "int64", json!(7)),
        ("list_number", json!([1, 2]), "list_int64", json!([1, 2])),
        (
            "object",
            json!({"count": 7}),
            "map_int64",
            json!({"count": 7}),
        ),
    ] {
        assert_eq!(
            run_in_memory(&graph(source_type, value, sink_type)).unwrap()["result"],
            expected
        );
    }

    for (source_type, sink_type) in [
        ("string", "int64"),
        ("float64", "int64"),
        ("list_string", "list_int64"),
    ] {
        let definition = graph(source_type, json!("unused"), sink_type);
        let error =
            compile_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap_err();
        assert!(matches!(
            error,
            WorkflowCompileError::IncompatiblePortTypes { .. }
        ));
        let message = error.to_string();
        assert!(message.contains("source") && message.contains("sink"));
    }

    let error = run_in_memory(&graph("array", json!([1, "wrong"]), "list_int64")).unwrap_err();
    assert!(error.contains("sink") && error.contains("/1"));
}

#[test]
fn generated_runner_matches_memory_for_checked_connections() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    for value in [json!(42), json!("wrong")] {
        let definition = graph("any", value, "int64");
        let expected = run_in_memory(&definition);
        let plan = plan_definition(&definition).unwrap();
        mf_compiler::write_dependency_project(
            &project,
            &plan,
            &mf_compiler::SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        )
        .unwrap();
        mf_compiler::resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
        let build = mf_compiler::cargo_command(&project)
            .args(["build", "--offline", "--locked"])
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let executable = common::runner_executable(&project, "debug");
        let validation = Command::new(&executable)
            .arg("--validate")
            .output()
            .unwrap();
        assert!(validation.status.success());
        let actual = Command::new(executable).output().unwrap();
        match expected {
            Ok(outputs) => {
                assert!(actual.status.success());
                assert_eq!(
                    serde_json::from_slice::<Value>(&actual.stdout).unwrap(),
                    serde_json::to_value(outputs).unwrap()
                );
            }
            Err(error) => {
                assert!(!actual.status.success());
                assert_eq!(String::from_utf8_lossy(&actual.stderr).trim(), error);
            }
        }
    }
}

#[test]
fn skipped_typed_inputs_match_in_memory_and_generated_execution() {
    let definition = skipped_graph();
    let expected = run_in_memory(&definition).unwrap();
    assert!(expected.is_empty());
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    let plan = plan_definition(&definition).unwrap();
    mf_compiler::write_dependency_project(
        &project,
        &plan,
        &mf_compiler::SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    mf_compiler::resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
    let build = mf_compiler::cargo_command(&project)
        .args(["build", "--offline", "--locked"])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let executable = common::runner_executable(&project, "debug");
    let result = Command::new(executable).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({})
    );
}
