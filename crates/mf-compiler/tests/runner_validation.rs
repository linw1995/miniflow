mod common;
use mf_compiler::{SupportPackages, plan_definition, resolve_project, write_dependency_project};
use std::process::Command;

#[test]
fn build_validates_without_execution_and_reports_plugin_errors() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    let mut definition = common::fixture_definition();
    definition.nodes[0].config = serde_json::json!({"print":true});
    definition
        .dependencies
        .get_mut("arbitrary.alias")
        .unwrap()
        .features
        .push("fail-execution".into());
    let build = |definition: &mf_compiler::WorkflowDefinition| {
        let plan = plan_definition(definition).unwrap();
        write_dependency_project(
            &project,
            &plan,
            &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        )
        .unwrap();
        resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
        mf_compiler::cargo_command(&project)
            .args(["build", "--offline", "--release", "--locked"])
            .output()
            .unwrap()
    };
    let first = build(&definition);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let runner = common::runner_executable(&project, "release");
    let execution = Command::new(&runner).output().unwrap();
    assert!(!execution.status.success());
    assert!(String::from_utf8_lossy(&execution.stderr).contains("execution sentinel"));
    for invalid in ["config", "kind", "port", "duplicate"] {
        let mut invalid_definition = definition.clone();
        match invalid {
            "config" => invalid_definition.nodes[0].config = serde_json::json!(42),
            "kind" => invalid_definition.nodes[0].kind = "unknown.kind".into(),
            "port" => invalid_definition.edges[0].from_output = "missing".into(),
            _ => invalid_definition
                .dependencies
                .get_mut("arbitrary.alias")
                .unwrap()
                .features
                .push("duplicate-kind".into()),
        }
        let result = build(&invalid_definition);
        assert!(!result.status.success(), "{invalid} was accepted");
        assert!(!String::from_utf8_lossy(&result.stderr).contains("execution sentinel"));
    }
}

#[test]
fn structural_planning_does_not_need_a_plugin_registry() {
    let mut definition = common::fixture_definition();
    definition.nodes.reverse();
    assert_eq!(
        plan_definition(&definition).unwrap().execution_order,
        vec![mf_compiler::DefinitionId::from("a"), "b".into()]
    );
    definition.edges[0].from_node = "missing".into();
    assert!(
        plan_definition(&definition)
            .unwrap_err()
            .to_string()
            .contains("unknown source node")
    );
}
