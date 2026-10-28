extern crate mfn_core as _;
mod common;
#[path = "fixtures/multi-nodes/src/schema_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{CallArguments, Outputs, ValueRef};
use serde_json::{Value, json};

fn definition() -> WorkflowDefinition {
    serde_json::from_value(json!({"version":"2026-10-28","dependencies":{
        "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
        "custom":{"package":"fixture-multi-nodes","path":common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes")}
    },"nodes":[
        {"id":"parent","kind":"fixture.schema_caller"},
        {"id":"config","kind":"builtin.constant","config":{"value":"never-advertise-this-value"}},
        {"id":"tool","kind":"fixture.schema_tool","config":{"allow_drift":true},"inputs":{"token":{"from":{"node":"config","port":"value"}}}}
    ],"calls":{"parent":{"lookup":{"description":"Find a record","target":"tool","ports":[{"output":"request","input":"payload"}]}}},
    "outputs":[{"name":"result","node":"parent","port":"value"}]})).unwrap()
}

#[test]
fn resolved_schemas_are_projected_from_the_executable_scope_after_ownership() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let interfaces = flow.call_interfaces();
    let interface = &interfaces.for_caller(&"parent".into())[0];
    assert_eq!(interface.description(), Some("Find a record"));
    assert_eq!(interface.outputs().len(), 1);
    assert!(
        interface.outputs()[0]
            .schema_constraints()
            .iter()
            .all(|schema| schema.is_complete())
    );
    let public = serde_json::to_string(interface).unwrap();
    assert!(public.contains("Search text") && public.contains("thorough"));
    assert!(
        !public.contains("token")
            && !public.contains("never-advertise")
            && !public.contains("opaque")
    );
    for value in [
        json!({}),
        json!({"query":"find","mode":"bad"}),
        json!({"query":"find","mode":"fast","limit":null}),
    ] {
        assert!(
            interface
                .validate_decoded_arguments(&CallArguments {
                    outputs: Outputs::from([("request".into(), value.into())]),
                    ..Default::default()
                })
                .is_err()
        );
    }
    assert_eq!(flow.execute().unwrap()["result"], ValueRef::from("find"));
}

#[cfg(feature = "codegen")]
#[test]
fn generated_startup_detects_schema_drift_when_coarse_ports_are_unchanged() {
    use std::{fs, process::Command};
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
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
    assert!(String::from_utf8_lossy(&drift.stderr).contains("metadata differs"));
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
        json!({"result":"find"})
    );
}
