mod common;
extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    CompileRequest, NodeRegistry, SupportPackages, WorkflowDefinition, compile_definition,
    compile_project, instantiate_compiled,
};
use serde_json::{Value, json};
use std::{env, fs, process::Command};

fn graph(expression: &str) -> Value {
    json!({
        "version": "2026-09-26",
        "dependencies": {
            "core": {"package": "mfn-core", "path": common::crates_dir().join("builtin-nodes/core")},
            "code": {"package": "mfn-code", "path": common::crates_dir().join("builtin-nodes/code")}
        },
        "nodes": [
            {"id": "source", "kind": "builtin.constant", "config": {"value": 0}},
            {"id": "route", "kind": "builtin.if_else", "config": {"branches": [{
                "id": "on", "condition": {"source": {"output": "source.value", "path": ""}, "operator": "eq", "value": 1}
            }]}},
            {"id": "transform", "kind": "builtin.code", "config": {
                "language": "cel", "inputs": {"amount": "int"}, "code": {"result": expression}
            }}
        ],
        "edges": [{"from_node": "source", "from_output": "value", "to_node": "transform", "to_input": "amount"}],
        "control_edges": [
            {"from_node": "source", "from_output": "value", "to_node": "route"},
            {"from_node": "route", "from_output": "on", "to_node": "transform"}
        ],
        "outputs": [{"name": "result", "node": "transform", "port": "result", "optional": true}]
    })
}

#[test]
fn checks_inactive_code_and_known_input_conflicts() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let invalid: WorkflowDefinition = serde_json::from_value(graph("missing + 1")).unwrap();
    let error = compile_definition(&invalid, &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("transform") && error.contains("output `result`"));

    let valid: WorkflowDefinition = serde_json::from_value(graph("amount * 2")).unwrap();
    let plan = compile_definition(&valid, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert!(flow.execute().unwrap().is_empty());

    let mut active = graph("amount * 2");
    active["nodes"][0]["config"]["value"] = json!(21);
    active["nodes"][1]["config"]["branches"][0]["condition"]["value"] = json!(21);
    let definition: WorkflowDefinition = serde_json::from_value(active).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let result = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(result["result"], json!(42));

    let mut active_wrong = graph("amount * 2");
    active_wrong["nodes"][0]["config"]["value"] = json!("bad");
    active_wrong["nodes"][1]["config"]["branches"][0]["condition"]["value"] = json!("bad");
    let definition: WorkflowDefinition = serde_json::from_value(active_wrong).unwrap();
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("source") && error.contains("transform"));
    assert!(error.contains("string") && error.contains("int64"));
}

#[test]
fn runner_validation_rejects_inactive_code_without_replacing_output() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    let output = root.path().join("runner");
    let build = root.path().join("build");
    fs::write(&definition, graph("missing + 1").to_string()).unwrap();
    fs::write(&output, b"previous").unwrap();
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    let request = CompileRequest {
        definition: &definition,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &support,
    };
    let error = compile_project(&request).unwrap_err();
    assert_eq!(error.stage, "runner validation");
    assert_eq!(fs::read(&output).unwrap(), b"previous");

    let executable = build
        .join("target/release")
        .join(format!("mf-generated-workflow{}", env::consts::EXE_SUFFIX));
    let validation = Command::new(&executable)
        .arg("--validate")
        .output()
        .unwrap();
    assert!(!validation.status.success());
    let diagnostic = String::from_utf8_lossy(&validation.stderr);
    assert!(diagnostic.contains("transform") && diagnostic.contains("output `result`"));

    fs::write(&definition, graph("amount * 2").to_string()).unwrap();
    compile_project(&request).unwrap();
    let result = Command::new(&output).output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({})
    );

    let mut active = graph("amount * 2");
    active["nodes"][0]["config"]["value"] = json!(21);
    active["nodes"][1]["config"]["branches"][0]["condition"]["value"] = json!(21);
    fs::write(&definition, active.to_string()).unwrap();
    compile_project(&request).unwrap();
    let result = Command::new(&output).output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({"result": 42})
    );

    active["nodes"][2]["config"]["code"]["result"] = json!("amount * 3");
    fs::write(&definition, active.to_string()).unwrap();
    compile_project(&request).unwrap();
    let result = Command::new(&output).output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        json!({"result": 63})
    );

    let installed = fs::read(&output).unwrap();
    let lock = fs::read(definition.with_extension("lock")).unwrap();
    active["nodes"][2]["config"]["code"]["result"] = json!("missing + 1");
    fs::write(&definition, active.to_string()).unwrap();
    let error = compile_project(&request).unwrap_err();
    assert_eq!(error.stage, "runner validation");
    assert_eq!(fs::read(&output).unwrap(), installed);
    assert_eq!(fs::read(definition.with_extension("lock")).unwrap(), lock);
}
