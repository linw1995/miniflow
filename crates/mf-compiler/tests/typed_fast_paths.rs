#![cfg(feature = "codegen")]
extern crate mfn_core as _;
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

fn identity_definition() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-10-03",
        "dependencies":{"arbitrary.core":{"package":"mfn-core", "path": common::crates_dir().join("builtin-nodes/core")}},
        "nodes":[{"id":"a", "kind":"builtin.identity"}, {"id":"b", "kind":"builtin.identity"}, {"id":"c", "kind":"builtin.identity"}],
        "edges":[{"from_node":"a", "from_output":"value", "to_node":"b", "to_input":"input"}, {"from_node":"b", "from_output":"value", "to_node":"c", "to_input":"input"}],
        "outputs":[{"name":"result", "node":"c", "port":"value"}]
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
    assert_eq!(source.matches("GeneratedNodeResult::encoded").count(), 1);
    assert_eq!(source.matches("decode_inputs").count(), 1);
    assert!(
        !plan
            .generate_artifacts()
            .unwrap()
            .rust_source
            .contains("mf_prepare_generated_workflow!(")
    );
    let builtins = compile_definition(&identity_definition(), &registry).unwrap();
    let artifacts = builtins.generate_runner_execution_plans(&registry).unwrap();
    let report: mf_compiler::TypedPlan = serde_json::from_str(&artifacts.typed_plan_json).unwrap();
    assert_eq!(report.segments[0].positions, [0, 1, 2]);
    assert!(
        report
            .connections
            .iter()
            .all(|edge| edge.fallback.is_none())
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
    let shared = json!({"nested":[null,true,42,"shared payload"]});
    for refined in [false, true] {
        definition = identity_definition();
        let args =
            if refined {
                definition.nodes.insert(
                    0,
                    serde_json::from_value(json!({
                        "id":"constant", "kind":"builtin.constant", "config":{"value":shared}
                    }))
                    .unwrap(),
                );
                definition.edges.push(serde_json::from_value(json!({
                "from_node":"constant", "from_output":"value", "to_node":"a", "to_input":"input"
            })).unwrap());
                json!({})
            } else {
                json!({"a":{"input":shared}})
            };
        let plan = compile_definition(&definition, &registry).unwrap();
        let report: mf_compiler::TypedPlan = serde_json::from_str(
            &plan
                .generate_runner_execution_plans(&registry)
                .unwrap()
                .typed_plan_json,
        )
        .unwrap();
        assert_eq!(report.segments.is_empty(), refined);
        if refined {
            assert!(
                report.connections.iter().any(|edge| edge.fallback
                    == Some(mf_compiler::TypedFallbackReason::UnprovenRefinement))
            );
        }
        let expected = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(WorkflowArguments::try_from(args.clone()).unwrap())
            .unwrap();
        fs::write(&definition_path, serde_json::to_vec(&definition).unwrap()).unwrap();
        compile_project_with_options(&request(), &RunnerOptions { telemetry: false }).unwrap();
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
    }
    for (next, args, expected, has_segment) in [
        (
            definition_for_extended(),
            json!({"a":{"count":u64::MAX,"size":7,"ratio":-0.0,"payload":[null,"shared"]}}),
            json!({"count":u64::MAX,"size":7,"ratio":-0.0,"payload":[null,"shared"]}),
            true,
        ),
        (
            definition_for_extended(),
            json!({"a":{"count":u64::MAX,"size":7.0,"ratio":42,"payload":[null,"shared"]}}),
            json!({"count":u64::MAX,"size":7,"ratio":42.0,"payload":[null,"shared"]}),
            true,
        ),
        (
            definition_for_defaulted(),
            json!({"a":{"text":"defaults"}}),
            json!({"result":"defaults!!!"}),
            false,
        ),
    ] {
        definition = next;
        let plan = compile_definition(&definition, &registry).unwrap();
        let report: mf_compiler::TypedPlan = serde_json::from_str(
            &plan
                .generate_runner_execution_plans(&registry)
                .unwrap()
                .typed_plan_json,
        )
        .unwrap();
        if has_segment {
            assert!(!report.segments.is_empty());
        }
        let reference = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(WorkflowArguments::try_from(args.clone()).unwrap())
            .unwrap();
        assert_eq!(serde_json::to_value(reference).unwrap(), expected);
        fs::write(&definition_path, serde_json::to_vec(&definition).unwrap()).unwrap();
        compile_project_with_options(&request(), &RunnerOptions { telemetry: false }).unwrap();
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
            expected
        );
    }
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

fn definition_for_extended() -> WorkflowDefinition {
    let mut value = serde_json::to_value(text_definition()).unwrap();
    for node in value["nodes"].as_array_mut().unwrap() {
        node["kind"] = json!("fixture.typed_extended");
    }
    value["edges"] = json!([]);
    value["outputs"] = json!([]);
    for field in ["count", "size", "ratio", "payload"] {
        for (source, target) in [("a", "b"), ("b", "c")] {
            value["edges"].as_array_mut().unwrap().push(json!({
                "from_node":source,"from_output":field,"to_node":target,"to_input":field
            }));
        }
        value["outputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":field,"node":"c","port":field}));
    }
    serde_json::from_value(value).unwrap()
}

fn definition_for_defaulted() -> WorkflowDefinition {
    let mut definition = text_definition();
    for node in &mut definition.nodes {
        node.kind = "fixture.typed_defaulted".into();
    }
    definition
}
