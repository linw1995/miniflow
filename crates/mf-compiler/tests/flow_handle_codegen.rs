#![cfg(unix)]

mod common;

use serde_json::{Value, json};

#[test]
fn passed_finite_task_stream_keeps_incremental_and_terminal_exports_separate() {
    use std::{fs, process::Command};
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("flow.json");
    let output = root.path().join("runner");
    let build = root.path().join("build");
    let body = json!({
        "version":"2026-11-01", "dependencies":{},
        "nodes":[{"id":"step", "kind":"fixture.task_stream"}],
        "outputs":[{"name":"increment", "node":"step", "port":"delta"},
                   {"name":"count", "node":"step", "port":"total"}]
    });
    let value = json!({
        "version":"2026-11-01",
        "dependencies":{
            "core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")},
            "fixture":{"package":"fixture-multi-nodes", "path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}
        },
        "flow_arguments":{"child":{"definition":body,"description":null,"parameters":{}}},
        "nodes":[
            {"id":"select", "kind":"builtin.constant", "config":{"value":"child"}},
            {"id":"args", "kind":"builtin.constant", "config":{"value":{"step.count":2}}},
            {"id":"invoke", "kind":"workflow.call", "config":{
                "flow_ref":{"$ref":"select.value"}, "arguments_object":true, "capture_errors":true
            }, "inputs":{"arguments":{"from":{"node":"args", "port":"value"}}}}
        ],
        "edges":[{"from_node":"select", "to_node":"invoke"}],
        "outputs":[{"name":"increment", "node":"invoke", "port":"increment"},
                   {"name":"completion", "node":"invoke", "port":"result"}]
    });
    fs::write(&source, value.to_string()).unwrap();
    mf_compiler::compile_project_with_options(
        &mf_compiler::CompileRequest {
            definition: &source,
            output: &output,
            locked: false,
            build_dir: Some(&build),
            support: &mf_compiler::SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &mf_compiler::RunnerOptions { telemetry: false },
    )
    .unwrap();
    fs::remove_file(source).unwrap();
    fs::remove_dir_all(build).unwrap();
    let result = Command::new(output)
        .env_clear()
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let values: Vec<Value> = serde_json::Deserializer::from_slice(&result.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(values.len(), 3);
    assert!(values.contains(&json!({"increment":0})));
    assert!(values.contains(&json!({"increment":1})));
    assert!(values.contains(&json!({"completion":{"outputs":{"count":2}}})));
}
