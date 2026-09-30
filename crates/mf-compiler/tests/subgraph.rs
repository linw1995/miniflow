mod common;

use mf_compiler::{CompileRequest, SupportPackages, compile_project};
use serde_json::json;
use std::{fs, process::Command};

#[test]
fn third_party_body_declarations_drive_both_backends_and_warm_builds() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("flow.json");
    let output = root.path().join("flow");
    let build = root.path().join("build");
    let definition = json!({
        "version": "2026-09-26",
        "dependencies": {"custom": {"package": "fixture-multi-nodes", "path": std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}},
        "nodes": [
            {"id": "source", "kind": "fixture.source", "config": {"print": true}},
            {"id": "container", "kind": "fixture.subgraph", "config": {
                "body": {
                    "nodes": [{"id": "echo", "kind": "fixture.echo"}],
                    "edges": [{"from_node": "@custom", "from_output": "input", "to_node": "echo", "to_input": "input"}]
                },
                "result": {"node": "echo", "port": "value"}
            }}
        ],
        "edges": [{"from_node": "source", "from_output": "value", "to_node": "container", "to_input": "input"}],
        "outputs": [{"name": "value", "node": "container", "port": "value"}]
    });
    fs::write(&path, definition.to_string()).unwrap();
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    let request = CompileRequest {
        definition: &path,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &support,
    };
    compile_project(&request).unwrap();
    let source = fs::read_to_string(build.join("src/workflow.rs")).unwrap();
    assert!(source.contains("@custom") && source.contains("fixture.echo"));
    assert!(!source.contains("execute_compiled"));
    let before = fs::metadata(build.join("src/workflow.rs"))
        .unwrap()
        .modified()
        .unwrap();
    compile_project(&request).unwrap();
    assert_eq!(
        before,
        fs::metadata(build.join("src/workflow.rs"))
            .unwrap()
            .modified()
            .unwrap()
    );
    fs::write(build.join("src/main.rs"), r#"
extern crate node_0 as _;
mod workflow;
fn main() {
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json")).unwrap();
    let memory = mf_compiler::execute_compiled(&plan, &registry, None).unwrap();
    let generated = workflow::run_workflow(&registry).unwrap();
    assert_eq!(memory, generated);
    assert_eq!(serde_json::to_value(memory).unwrap(), serde_json::json!({"value": 14}));
}
"#).unwrap();
    let built = mf_compiler::cargo_command(&build)
        .args(["build", "--release", "--locked", "--offline"])
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let result = Command::new(common::runner_executable(&build, "release"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
