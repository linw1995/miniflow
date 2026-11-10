#![cfg(unix)]
extern crate mfn_core as _;

use mf_runtime::{PortSpec, ValueType, WorkflowArguments};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[test]
fn source_free_runner_passes_through_parameter_edges_under_observation() {
    let root = tempfile::tempdir().unwrap();
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let source = root.path().join("callee.json");
    fs::write(&source, json!({
        "version":"2026-10-28",
        "dependencies":{"core":{"package":"mfn-core", "path":crates.join("builtin-nodes/core")}},
        "nodes":[{"id":"params", "kind":"builtin.parameters", "config":{
            "parameters":{"text":{"type":"string", "secret":false}}
        }}],
        "outputs":[{"name":"answer", "node":"params", "port":"text"}]
    }).to_string()).unwrap();
    let registry = mf_compiler::NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    let plan = mf_compiler::plan_resolved_workflow(&resolved).unwrap();
    let build = root.path().join("build");
    mf_compiler::write_dependency_project(
        &build,
        &plan,
        &mf_compiler::SupportPackages::Local { crates_dir: crates },
    )
    .unwrap();
    let target = std::env::var_os("MF_TEST_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| build.join("target"));
    assert!(
        mf_compiler::cargo_command(&build)
            .args(["build", "--offline", "--target-dir"])
            .arg(&target)
            .status()
            .unwrap()
            .success()
    );
    let runner = root.path().join("runner");
    fs::copy(target.join("debug/mf-generated-workflow"), &runner).unwrap();
    fs::remove_file(source).unwrap();
    fs::remove_dir_all(build).unwrap();
    let observed = Arc::new(AtomicUsize::new(0));
    let observations = observed.clone();
    let ports = vec![PortSpec::new("answer", ValueType::String, true)];
    assert!(mf_tui::runner_flow::load(&runner, vec![]).is_err());
    let callable = mf_tui::runner_flow::load_with_observer(&runner, ports, move |_| {
        observations.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    let caller = mf_compiler::WorkflowDefinition::from_json(&json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{
                "parameters":{"policy":{"type":"flow", "secret":false}, "text":{"type":"string", "secret":false}}
            }},
            {"id":"apply", "kind":"workflow.call", "config":{
                "inputs":[{"name":"params.text", "value_type":"string", "required":true}],
                "outputs":[{"name":"answer", "value_type":"string", "required":true}]
            }, "inputs":{
                "flow":{"from":{"node":"params", "port":"policy"}},
                "params.text":{"from":{"node":"params", "port":"text"}}
            }}
        ],
        "outputs":[{"name":"answer", "node":"apply", "port":"answer"}]
    }).to_string()).unwrap();
    let plan = mf_compiler::compile_definition(&caller, &registry).unwrap();
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    // Snapshot observation must not invent child scopes in the calling artifact.
    for text in ["first", "second"] {
        let description = mf_compiler::describe_compiled(&plan).unwrap();
        let observation = mf_telemetry::observation::Observer::stderr(0)
            .start_with_description(description, mf_telemetry::identity::RunId::new())
            .unwrap();
        let result = mf_runtime::ExecutionContext::run(Some(observation), |context| {
            context.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(|event| {
                assert!(event.scope.is_empty());
            }));
            context.set_workflow_arguments(WorkflowArguments(BTreeMap::from([
                ("params.policy".into(), callable.clone().into()),
                ("params.text".into(), text.into()),
            ])));
            flow.execute_in_context(context)
        })
        .unwrap();
        assert_eq!(result["answer"].as_str(), Some(text));
        assert!(result["answer"].is_secret());
    }
    assert_eq!(observed.load(Ordering::SeqCst), 2);
    let invalid = WorkflowArguments::try_from(json!({"params.text":42})).unwrap();
    assert!(matches!(
        callable.invoke("apply", invalid, &mut Default::default()),
        Err(mf_runtime::WorkflowRunError::WorkflowInputs { .. })
    ));
    assert_eq!(
        observed.load(Ordering::SeqCst),
        2,
        "invalid inputs must not launch a runner"
    );
    let wrong = mf_tui::runner_flow::load(
        &runner,
        vec![PortSpec::new("answer", ValueType::Int64, true)],
    )
    .unwrap();
    assert!(
        wrong
            .invoke(
                "apply",
                WorkflowArguments::try_from(json!({"params.text":"wrong type"})).unwrap(),
                &mut Default::default()
            )
            .is_err()
    );
}
