mod common;
#[path = "fixtures/multi-nodes/src/typed_fixture.rs"]
mod fixture;
extern crate mfn_core as _;

use mf_compiler::{
    CompileRequest, NodeRegistry, SupportPackages, WorkflowDefinition, compile_definition,
    compile_project, instantiate_compiled,
};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn definition() -> Value {
    json!({
        "version":"2026-09-26",
        "dependencies":{
            "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
            "fixture":{"package":"fixture-multi-nodes","path":Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}
        },
        "nodes":[
            {"id":"source","kind":"builtin.constant","config":{"value":[{"count":1},{"count":2}]}},
            {"id":"identity","kind":"builtin.identity"},
            {"id":"sink","kind":"fixture.typed_echo","config":{"type":"list_map_int64"}}
        ],
        "edges":[
            {"from_node":"source","from_output":"value","to_node":"identity","to_input":"input"},
            {"from_node":"identity","from_output":"value","to_node":"sink","to_input":"input"}
        ],
        "outputs":[{"name":"result","node":"sink","port":"value"}]
    })
}

fn in_memory(definition: &Value) -> Result<Value, String> {
    let definition: WorkflowDefinition = serde_json::from_value(definition.clone()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).map_err(|error| error.to_string())?;
    instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .map(|outputs| json!(outputs))
        .map_err(|error| error.to_string())
}

#[test]
fn generated_runner_matches_inference_and_preserves_installed_binary_on_conflict() {
    let root = tempfile::tempdir().unwrap();
    let definition_path = root.path().join("flow.json");
    let output = root.path().join("flow");
    let build = root.path().join("build");
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    let request = CompileRequest {
        definition: &definition_path,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &support,
    };

    let valid = definition();
    fs::write(&definition_path, valid.to_string()).unwrap();
    compile_project(&request).unwrap();
    let runner = common::runner_executable(&build, "release");
    assert_eq!(fs::read(&runner).unwrap(), fs::read(&output).unwrap());
    let initial_binary = fs::read(&output).unwrap();
    let initial_lock = fs::read(definition_path.with_extension("lock")).unwrap();
    let actual = Command::new(&output).output().unwrap();
    assert!(actual.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&actual.stdout).unwrap(),
        in_memory(&valid).unwrap()
    );

    let mut inactive_conflict = valid.clone();
    inactive_conflict["nodes"][0]["config"]["value"] = json!([{"count":1},{"count":"wrong"}]);
    inactive_conflict["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"trigger","kind":"builtin.constant","config":{"value":false}}),
        json!({"id":"route","kind":"builtin.if_else","config":{"branches":[{
            "id":"on","condition":{"source":{"output":"trigger.value","path":""},"operator":"eq","value":true}
        }]}}),
    ]);
    inactive_conflict["control_edges"] = json!([
        {"from_node":"trigger","from_output":"value","to_node":"route"},
        {"from_node":"route","from_output":"on","to_node":"sink"}
    ]);
    fs::write(&definition_path, inactive_conflict.to_string()).unwrap();
    let error = compile_project(&request).unwrap_err();
    assert_eq!(error.stage, "runner validation");
    let validation = Command::new(&runner).arg("--validate").output().unwrap();
    assert!(!validation.status.success());
    let diagnostic = String::from_utf8_lossy(&validation.stderr);
    assert!(
        diagnostic.contains("sink") && diagnostic.contains("/1/count"),
        "{diagnostic}"
    );
    assert_eq!(fs::read(&output).unwrap(), initial_binary);
    assert_eq!(
        fs::read(definition_path.with_extension("lock")).unwrap(),
        initial_lock
    );

    let mut unknown = valid;
    unknown["nodes"][0]["kind"] = json!("fixture.typed_source");
    unknown["nodes"][0]["config"] = json!({
        "type":"any","value":[{"count":1},{"count":"wrong"}]
    });
    fs::write(&definition_path, unknown.to_string()).unwrap();
    compile_project(&request).unwrap();
    let validation = Command::new(&output).arg("--validate").output().unwrap();
    assert!(validation.status.success());
    let actual = Command::new(&output).output().unwrap();
    assert!(!actual.status.success());
    assert_eq!(
        String::from_utf8_lossy(&actual.stderr).trim(),
        in_memory(&unknown).unwrap_err()
    );
}

#[test]
fn generated_runner_rejects_a_false_output_derivation() {
    let root = tempfile::tempdir().unwrap();
    let definition_path = root.path().join("false-derivation.json");
    let output = root.path().join("flow");
    let build = root.path().join("build");
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    let definition = json!({
        "version":"2026-09-26",
        "dependencies":{
            "fixture":{"package":"fixture-multi-nodes","path":Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}
        },
        "nodes":[
            {"id":"source","kind":"fixture.typed_source","config":{"type":"int64","value":7}},
            {"id":"forward","kind":"fixture.dishonest_forward"}
        ],
        "edges":[{"from_node":"source","from_output":"value","to_node":"forward","to_input":"input"}],
        "outputs":[{"name":"result","node":"forward","port":"value"}]
    });
    let expected = in_memory(&definition).unwrap_err();
    assert!(expected.contains("forward") && expected.contains("int64"));
    fs::write(&definition_path, definition.to_string()).unwrap();
    compile_project(&CompileRequest {
        definition: &definition_path,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &support,
    })
    .unwrap();
    let actual = Command::new(&output).output().unwrap();
    assert!(!actual.status.success());
    assert_eq!(String::from_utf8_lossy(&actual.stderr).trim(), expected);
}
