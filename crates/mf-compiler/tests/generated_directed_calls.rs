#![cfg(feature = "codegen")]
extern crate mfn_code as _;
extern crate mfn_core as _;
mod common;
#[path = "fixtures/connected_tools.rs"]
mod connected;
#[path = "fixtures/multi-nodes/src/directed_fixture.rs"]
mod fixture;
// Other constructors in this shared fixture are exercised by typed_fast_paths.
#[allow(dead_code)]
#[path = "fixtures/multi-nodes/src/generated_fixture.rs"]
mod typed_fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use serde_json::{Value, json};
use std::{fs, process::Command};

fn definition() -> WorkflowDefinition {
    let mut child = connected::document();
    child["nodes"][0]["config"]["allow_drift"] = true.into();
    child["edges"] = json!([{"from_node":"z_config","to_node":"tool"}]);
    let unused = child["nodes"][3].clone();
    serde_json::from_value(json!({"version":"2026-10-28","dependencies":{
        "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
        "code":{"package":"mfn-code","path":common::crates_dir().join("builtin-nodes/code")},
        "custom":{"package":"fixture-multi-nodes","path":common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes")}
    },"nodes":[
        {"id":"parent","kind":"test.directed_caller","config":{"repeat":2,"result":"result"}},
        {"id":"delegate","kind":"workflow.call","config":{"flow":"child","definition":child}},
        unused,
        {"id":"z_source","kind":"builtin.constant","config":{"value":"ordinary"}},
        {"id":"z_middle","kind":"fixture.typed_text","inputs":{"text":{"from":{"node":"z_source","port":"value"}}}},
        {"id":"z_sink","kind":"fixture.typed_text","inputs":{"text":{"from":{"node":"z_middle","port":"text"}}}}
    ],"calls":{"parent":{"search":{"target":"delegate"},"unused":{"target":"unused"}}},
    "outputs":[{"name":"result","node":"parent","port":"value"}]})).unwrap()
}

#[test]
fn generated_layouts_freeze_calls_and_preserve_ordinary_transfer_rules() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    for (standard, artifacts) in [
        (false, plan.generate_execution_plans(&registry).unwrap()),
        (
            true,
            plan.generate_runner_execution_plans(&registry).unwrap(),
        ),
    ] {
        assert!(artifacts.rust_source.contains("bind_calls_root"));
        assert!(artifacts.rust_source.contains("PreparedCall"));
        assert!(artifacts.rust_source.contains("verify_generated_metadata"));
        assert!(artifacts.rust_source.contains("success_guard_root_"));
        assert!(!artifacts.rust_source.contains("FlowBuilder"));
        let typed: mf_compiler::TypedPlan =
            serde_json::from_str(&artifacts.typed_plan_json).unwrap();
        let ordinary = typed
            .connections
            .iter()
            .find(|edge| edge.source == "z_middle" && edge.target == "z_sink")
            .unwrap();
        assert_eq!(
            ordinary.fallback,
            if standard {
                None
            } else {
                Some(mf_compiler::TypedFallbackReason::CustomRunner)
            }
        );
        assert_eq!(typed.segments.is_empty(), !standard);
        mf_runtime::WorkflowManifest::from_bytes(&artifacts.manifest_bytes).unwrap();
    }
}

#[test]
fn ordinary_transfer_rules_preserve_values_read_by_call_targets() {
    let mut value = serde_json::to_value(definition()).unwrap();
    value["nodes"][0]["config"]["result"] = "value".into();
    value["nodes"][1] = json!({"id":"delegate","kind":"workflow.call",
        "config":{"flow":"read","definition":{"version":"2026-10-28","dependencies":{},
            "nodes":[{"id":"read","kind":"builtin.identity"}],
            "outputs":[{"name":"value","node":"read","port":"value"}]}},
        "inputs":{"read.input":{"from":{"node":"z_middle","port":"text"}}}});
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
    let typed: mf_compiler::TypedPlan = serde_json::from_str(&artifacts.typed_plan_json).unwrap();
    let shared = typed
        .connections
        .iter()
        .find(|edge| edge.source == "z_middle" && edge.target == "z_sink")
        .unwrap();
    assert!(shared.fallback.is_some());
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(
        flow.execute().unwrap()["result"],
        mf_runtime::ValueRef::from("ordinary")
    );
}

#[test]
fn standalone_calls_match_memory_reject_metadata_drift_and_run_without_build_sources() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    let mut expected = serde_json::to_value(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap(),
    )
    .unwrap();
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
    let main = project.join("src/main.rs");
    let mut source = fs::read_to_string(&main).unwrap();
    source.push_str(
        r#"
#[test]
fn mismatched_call_scope_is_rejected_before_dispatch() {
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let flow = workflow::prepare_workflow(&registry, None).unwrap();
    let error = workflow::bind_calls_root(flow, "different-layout").err().unwrap();
    assert!(error.to_string().contains("scope identity differs"));
}
"#,
    );
    fs::write(main, source).unwrap();
    let built = mf_compiler::cargo_command(&project)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let checked = mf_compiler::cargo_command(&project)
        .args([
            "test",
            "--offline",
            "--bin",
            "mf-generated-workflow",
            "mismatched_call_scope_is_rejected_before_dispatch",
        ])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let executable = common::runner_executable(&project, "debug");
    let failed = Command::new(&executable)
        .env("MINIFLOW_TEST_CALL_PORT_DRIFT", "1")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(
        (String::from_utf8_lossy(&failed.stderr).contains("metadata differs")
            || String::from_utf8_lossy(&failed.stderr)
                .contains("cannot connect `a_parent`.`query`")),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    let first = Command::new(&executable).output().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&first.stdout).unwrap(),
        expected
    );
    let mut skipped = serde_json::to_value(definition()).unwrap();
    let body = &mut skipped["nodes"][1]["config"]["definition"];
    body["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"switch","kind":"builtin.constant","config":{"value":-1}}),
        json!({"id":"route","kind":"builtin.if_else","config":{"branches":[{"id":"positive","condition":{"source":{"output":"switch.value","path":""},"operator":"gt","value":0}}]}}),
    ]);
    body["edges"].as_array_mut().unwrap().extend([
        json!({"from_node":"switch","to_node":"route"}),
        json!({"from_node":"route","from_output":"positive","to_node":"z_config"}),
    ]);
    let skipped = compile_definition(&serde_json::from_value(skipped).unwrap(), &registry).unwrap();
    expected = serde_json::to_value(
        instantiate_compiled(&skipped, &registry)
            .unwrap()
            .execute()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(expected, json!({"result":"skipped"}));
    mf_compiler::write_dependency_project(
        &project,
        &skipped,
        &mf_compiler::SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    let rebuilt = mf_compiler::cargo_command(&project)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        rebuilt.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    let installed = root.path().join("runner");
    fs::copy(&executable, &installed).unwrap();
    fs::remove_dir_all(&project).unwrap();
    let output = Command::new(&installed)
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        expected
    );
}
