#![allow(dead_code)]
use mf_compiler::{CompiledWorkflow, WorkflowDefinition};
use std::path::{Path, PathBuf};

pub fn runner_executable(project: &Path, profile: &str) -> PathBuf {
    std::env::var_os("MF_TEST_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project.join("target"))
        .join(profile)
        .join(format!(
            "mf-generated-workflow{}",
            std::env::consts::EXE_SUFFIX
        ))
}

pub fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}
pub fn fixture_definition() -> WorkflowDefinition {
    WorkflowDefinition::from_json(&serde_json::json!({
        "version":"2026-09-26", "dependencies": {"arbitrary.alias": {"package":"fixture-multi-nodes", "path": PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes"), "features":["double"], "default-features":false}},
        "nodes":[{"id":"a", "kind":"fixture.source"},{"id":"b", "kind":"fixture.echo"}],
        "edges":[{"from_node":"a","from_output":"value","to_node":"b","to_input":"input"}],
        "outputs":[{"name":"result","node":"b","port":"value"}]
    }).to_string()).unwrap()
}
pub fn fixture_plan() -> CompiledWorkflow {
    CompiledWorkflow {
        definition: fixture_definition(),
        execution_order: vec!["a".into(), "b".into()],
    }
}
