use mf_compiler::{WorkflowDefinition, compile_definition, write_runner_project};
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mf runner {} {nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn generated_project_builds_and_directly_runs_linked_nodes() {
    let definition = WorkflowDefinition::from_json(
        r#"{
            "version": "2026-09-26", "dependencies": {},
            "nodes":[
                {"id":"source","kind":"builtin.constant","config":{"value":41}},
                {"id":"echo","kind":"builtin.identity"}
            ],
            "edges":[
                {"from_node":"source","from_output":"value","to_node":"echo","to_input":"input"}
            ],
            "outputs":[{"name":"answer","node":"echo","port":"value"}]
        }"#,
    )
    .unwrap();
    let registry = mf_bundle::registry().unwrap();
    let artifacts = compile_definition(&definition, &registry)
        .unwrap()
        .generate_artifacts()
        .unwrap();
    let temporary = TemporaryDirectory::new();
    let project = temporary.path().join("runner");
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    write_runner_project(
        &artifacts,
        &project,
        &crates_dir.join("mf-runtime"),
        &crates_dir.join("mf-bundle"),
    )
    .unwrap();

    let workflow_source = fs::read_to_string(project.join("src/workflow.rs")).unwrap();
    assert!(workflow_source.contains("mf_runtime::instantiate_node"));
    assert!(workflow_source.contains("mf_runtime::execute_node"));
    assert!(!workflow_source.contains("Flow::new"));
    assert!(fs::read_to_string(project.join("workflow-plan.json")).is_ok());
    assert!(fs::read_to_string(project.join("src/config_0.json")).is_ok());
    assert!(fs::read_to_string(project.join("src/config_1.json")).is_ok());

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let build = Command::new(cargo)
        .current_dir(&project)
        .env("CARGO_TARGET_DIR", project.join("target"))
        .arg("build")
        .arg("--offline")
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "generated runner build failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let binary_name = if cfg!(windows) {
        "mf-generated-workflow.exe"
    } else {
        "mf-generated-workflow"
    };
    let output = Command::new(project.join("target/debug").join(binary_name))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generated runner failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"answer": 41})
    );
}
