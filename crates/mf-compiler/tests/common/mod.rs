#![allow(dead_code)]
use mf_compiler::{CompiledWorkflow, WorkflowDefinition};
use std::{fs, path::PathBuf};

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mf-project-{}-{}-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
