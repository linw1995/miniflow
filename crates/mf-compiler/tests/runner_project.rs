mod common;
use mf_compiler::{CompileRequest, SupportPackages, compile_project};
use std::{fs, process::Command};

#[test]
fn generated_project_builds_and_directly_runs_selected_nodes() {
    let root = common::Directory::new();
    let source = root.0.join("flow.json");
    let output = root.0.join("flow");
    fs::write(
        &source,
        serde_json::to_string(&common::fixture_definition()).unwrap(),
    )
    .unwrap();
    let build = root.0.join("build");
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    compile_project(&CompileRequest {
        definition: &source,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &support,
    })
    .unwrap();
    let generated = fs::read_to_string(build.join("src/workflow.rs")).unwrap();
    assert!(generated.contains("mf_runtime::execute_node"));
    assert!(!generated.contains("Flow::new"));
    fs::remove_file(source).unwrap();
    let result = Command::new(output).env("PATH", "").output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap(),
        serde_json::json!({"result":14})
    );
}
