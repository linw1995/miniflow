#![cfg(unix)]
mod common;

extern crate mfn_lsp as _;
extern crate mfn_test_affected as _;

use common::*;
use mf_compiler::{
    CompileRequest, NodeRegistry, RunnerOptions, SupportPackages, WorkflowDefinition,
    compile_definition, compile_project_with_options, instantiate_stream,
};
use mf_runtime::{FlowRuntime, StreamOptions, WorkflowArguments};
use mfn_test_affected::TestPlan;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn definition(root: &Path) -> Value {
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut value: Value = serde_json::from_str(
        &fs::read_to_string(package.join("../../flows/test-affected.json")).unwrap(),
    )
    .unwrap();
    value["dependencies"]["lsp"]["path"] = json!(package.join("../lsp").canonicalize().unwrap());
    value["dependencies"]["tests"]["path"] = json!(package.canonicalize().unwrap());
    value["nodes"][1]["config"] = json!({
        "command":"python3", "profile":"standard", "extensions":["rs"], "deadline_ms":5000,
        "args":[package.join("../lsp/tests/fixtures/server.py").canonicalize().unwrap(), "normal", root.join("target/server")]
    });
    value
}

#[test]
fn generated_flow_matches_memory_drains_once_and_rejects_cross_domain_bypass() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    let head = commit(root);
    let value = definition(root);
    let registry = NodeRegistry::from_inventory().unwrap();
    let workflow: WorkflowDefinition = serde_json::from_value(value.clone()).unwrap();
    let compiled = compile_definition(&workflow, &registry).unwrap();
    let prepared = instantiate_stream(&compiled, &registry).unwrap();
    assert!(!root.join("target/server.pid").exists());
    let arguments = json!({"changes.repository":root,"changes.base":base,"changes.head":head});
    let instance = FlowRuntime::default()
        .start_stream(
            prepared,
            StreamOptions {
                arguments: WorkflowArguments::try_from(arguments.clone()).unwrap(),
                ..Default::default()
            },
        )
        .unwrap();
    let output = instance.recv().unwrap().unwrap();
    let memory: TestPlan =
        serde_json::from_value(serde_json::to_value(&output.outputs["plan"]).unwrap()).unwrap();
    assert!(instance.recv().unwrap().is_none());
    let summary = instance.join().unwrap();
    assert_eq!(summary.emitted_messages, 1);
    assert_eq!(memory.tests.len(), 1);
    assert_eq!(memory.tests[0].name, "tests::changed");
    assert_eq!(memory.analysis_status, mfn_lsp::Status::Analyzed);

    let mut bypass = value.clone();
    bypass["edges"][3]["from_node"] = json!("changes");
    let workflow: WorkflowDefinition = serde_json::from_value(bypass).unwrap();
    let result =
        compile_definition(&workflow, &registry).and_then(|p| instantiate_stream(&p, &registry));
    assert!(result.is_err());

    let flow = root.join("target/test.flow.json");
    fs::write(&flow, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let executable = root.join("target/test-flow");
    let build = root.join("target/build");
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../crates")
        .canonicalize()
        .unwrap();
    let prior_requests = fs::read(root.join("target/server")).unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &flow,
            output: &executable,
            locked: false,
            build_dir: Some(&build),
            support: &SupportPackages::Local { crates_dir: crates },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    let manifest = mf_tui::manifest::read_manifest(&executable).unwrap();
    assert_eq!(manifest.definition.nodes.len(), 3);
    assert_eq!(
        fs::read(root.join("target/server")).unwrap(),
        prior_requests
    );
    let output = Command::new(&executable)
        .current_dir(std::env::temp_dir())
        .args(["--inputs", &arguments.to_string()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 1);
    let generated: TestPlan = serde_json::from_value(records[0]["plan"].clone()).unwrap();
    assert_eq!(generated.tests, memory.tests);
    assert_eq!(generated.packages, memory.packages);
    assert_eq!(generated.head, memory.head);
    let plan_path = root.join("target/generated-plan.json");
    fs::write(&plan_path, serde_json::to_vec(&records[0]).unwrap()).unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../run-tests.py");
    let output = Command::new("python3")
        .arg(script)
        .arg(&plan_path)
        // CI shares a target directory, but these repositories have identical package names.
        .env("CARGO_TARGET_DIR", root.join("target/fixture-build"))
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"selected_count\": 1"));

    // Even an unavailable server must not run when the chosen extension has no inputs.
    let mut empty = value;
    empty["nodes"][1]["config"] =
        json!({"command":"/missing-language-server", "extensions":["unused"]});
    let compiled = compile_definition(&serde_json::from_value(empty).unwrap(), &registry).unwrap();
    let instance = FlowRuntime::default()
        .start_stream(
            instantiate_stream(&compiled, &registry).unwrap(),
            StreamOptions {
                arguments: WorkflowArguments::try_from(arguments).unwrap(),
                ..Default::default()
            },
        )
        .unwrap();
    let output = instance.recv().unwrap().unwrap();
    let plan: TestPlan =
        serde_json::from_value(serde_json::to_value(&output.outputs["plan"]).unwrap()).unwrap();
    assert_eq!(plan.analysis_status, mfn_lsp::Status::NotRequested);
    assert_eq!(plan.tests, memory.tests);
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
}
