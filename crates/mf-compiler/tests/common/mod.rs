#![allow(dead_code)]

#[cfg(feature = "codegen")]
pub mod embedding_coverage;

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
#[cfg(feature = "codegen")]
pub fn link_compiler_for_custom_host(project: &Path) {
    let path = project.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path).unwrap();
    let source = serde_json::to_string(crates_dir().join("mf-compiler").to_str().unwrap()).unwrap();
    let dependency = format!("mf-compiler = {{ path = {source}, default-features = false }}\n");
    std::fs::write(
        path,
        manifest.replacen(
            "[dependencies]\n",
            &format!("[dependencies]\n{dependency}"),
            1,
        ),
    )
    .unwrap();
}

pub fn fixture_definition() -> WorkflowDefinition {
    WorkflowDefinition::from_json(&serde_json::json!({
        "version":"2026-09-26", "dependencies": {"arbitrary.alias": {"package":"fixture-multi-nodes", "path": PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes"), "features":["double"], "default-features":false}},
        "nodes":[{"id":"a", "kind":"fixture.source"},{"id":"b", "kind":"fixture.echo"}],
        "edges":[{"from_node":"a","from_output":"value","to_node":"b","to_input":"input"}],
        "outputs":[{"name":"result","node":"b","port":"value"}]
    }).to_string()).unwrap()
}
pub fn assert_receiver_binding_preflight(error: &mf_compiler::WorkflowCompileError, owner: &str) {
    let mf_compiler::WorkflowCompileError::CompiledGraph { source } = error else {
        panic!("expected compiled receiver-binding preflight, got {error:?}");
    };
    let mf_compiler::WorkflowBuildError::InvalidDefinition {
        definition_id,
        message,
    } = source.as_ref()
    else {
        panic!("expected receiver-binding rejection before a factory, got {error:?}");
    };
    assert_eq!(definition_id.as_str(), owner);
    assert_eq!(
        message,
        "receiving Flow parameter declarations differ from the compiled binding set"
    );
}

pub fn fixture_plan() -> CompiledWorkflow {
    CompiledWorkflow {
        graph: mf_runtime::CompiledGraph {
            definition: fixture_definition(),
            execution_order: vec!["a".into(), "b".into()],
            relations: Default::default(),
            children: Default::default(),
        },
    }
}
