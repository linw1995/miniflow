#![cfg(feature = "codegen")]
mod common;
#[path = "fixtures/multi-nodes/src/generated_fixture.rs"]
mod fixture;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::WorkflowArguments;
use serde_json::json;

fn text_definition() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-10-03",
        "dependencies":{"arbitrary.alias":{"package":"fixture-multi-nodes", "path": common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes")}},
        "nodes":[{"id":"a", "kind":"fixture.typed_text"}, {"id":"b", "kind":"fixture.typed_text"}, {"id":"c", "kind":"fixture.typed_text"}],
        "edges":[{"from_node":"a", "from_output":"text", "to_node":"b", "to_input":"text"}, {"from_node":"b", "from_output":"text", "to_node":"c", "to_input":"text"}],
        "outputs":[{"name":"result", "node":"c", "port":"text"}]
    })).unwrap()
}

#[test]
fn source_wires_private_fields_with_static_constructor_proofs() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&text_definition(), &registry).unwrap();
    let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
    let report: mf_compiler::TypedPlan = serde_json::from_str(&artifacts.typed_plan_json).unwrap();
    assert_eq!(report.segments[0].positions, [0, 1, 2]);
    let renamed = fixture::renamed(json!({})).unwrap().prepared();
    assert_eq!(renamed.metadata.ports.inputs[0].name, "other");
    let source = artifacts.rust_source;
    assert!(
        source.contains("node_0::generated_fixture::text"),
        "{source}"
    );
    assert!(source.contains("input_from_fields"));
    assert!(source.contains("verify_fields"));
    assert!(source.contains("PORT_NAMES"));
    assert!(source.contains("into_fields"));
    assert!(!source.contains("from_inputs"));
    assert_eq!(source.matches("GeneratedNodeResult::encoded").count(), 1);
    assert_eq!(source.matches("decode_inputs").count(), 1);
    assert!(
        !plan
            .generate_artifacts()
            .unwrap()
            .rust_source
            .contains("mf_prepare_generated_workflow!(")
    );
}

#[cfg(unix)]
#[test]
fn generated_owned_values_preserve_contracts_and_installation() {
    use mf_compiler::{
        CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options,
    };
    use std::{fs, process::Command};
    let root = tempfile::tempdir().unwrap();
    let definition_path = root.path().join("flow.json");
    let executable = root.path().join("runner");
    let build = root.path().join("build");
    let mut definition = text_definition();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let args = json!({"a":{"text":"owned payload"}});
    let expected = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(WorkflowArguments::try_from(args.clone()).unwrap())
        .unwrap();
    fs::write(&definition_path, serde_json::to_vec(&definition).unwrap()).unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &definition_path,
            output: &executable,
            locked: false,
            build_dir: Some(&build),
            support: &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    let output = Command::new(&executable)
        .args(["--inputs", &args.to_string()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let mut command = mf_compiler::cargo_command(&build);
    command.args(["build", "--release", "--offline"]);
    let diagnostics = command.output().unwrap();
    assert!(
        diagnostics.status.success(),
        "{}",
        String::from_utf8_lossy(&diagnostics.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&diagnostics.stderr).contains("warning:"),
        "{}",
        String::from_utf8_lossy(&diagnostics.stderr)
    );
    let drift = Command::new(&executable)
        .args(["--inputs", &args.to_string()])
        .env("MF_FIXTURE_TYPED_GENERATION_DRIFT", "1")
        .output()
        .unwrap();
    assert!(!drift.status.success());
    assert!(String::from_utf8_lossy(&drift.stderr).contains("prepared metadata differs"));

    definition.nodes[1].config = json!({"fail":true});
    fs::write(&definition_path, serde_json::to_vec(&definition).unwrap()).unwrap();
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    let request = || CompileRequest {
        definition: &definition_path,
        output: &executable,
        locked: false,
        build_dir: Some(&build),
        support: &support,
    };
    let reference = compile_definition(&definition, &registry).unwrap();
    let failure = instantiate_compiled(&reference, &registry)
        .unwrap()
        .execute_with_inputs(WorkflowArguments::try_from(args.clone()).unwrap())
        .unwrap_err();
    compile_project_with_options(&request(), &RunnerOptions { telemetry: false }).unwrap();
    let failed = Command::new(&executable)
        .args(["--inputs", &args.to_string()])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&failed.stderr).trim(),
        failure.to_string()
    );

    definition = definition_for_items();
    fs::write(&definition_path, serde_json::to_vec(&definition).unwrap()).unwrap();
    compile_project_with_options(&request(), &RunnerOptions { telemetry: true }).unwrap();
    let items = json!({"a":{"items":[1,-2,i64::MAX]}});
    let output = Command::new(&executable)
        .args(["--inputs", &items.to_string()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"result":[1,-2,i64::MAX]})
    );
    let installed = fs::read(&executable).unwrap();
    for flag in ["bad_constructor", "bad_names"] {
        let mut invalid = text_definition();
        invalid.nodes[0].config = json!({flag:true});
        fs::write(&definition_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(
            compile_project_with_options(&request(), &RunnerOptions { telemetry: false }).is_err()
        );
        assert_eq!(fs::read(&executable).unwrap(), installed);
    }
}

fn definition_for_items() -> WorkflowDefinition {
    let mut definition = text_definition();
    for node in &mut definition.nodes {
        node.kind = "fixture.typed_items".into();
    }
    for edge in &mut definition.edges {
        edge.from_output = "items".into();
        edge.to_input = "items".into();
    }
    definition.outputs[0].port = "items".into();
    definition
}
