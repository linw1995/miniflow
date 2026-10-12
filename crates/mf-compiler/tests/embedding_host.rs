#![cfg(feature = "codegen")]

mod common;

use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn host(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    let mut manifest = String::from(
        "[package]\nname = \"mf-embedded-host\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\nresolver = \"3\"\n",
    );
    for section in ["dependencies", "build-dependencies"] {
        manifest.push_str(&format!("\n[{section}]\n"));
        for (alias, package, path) in [
            (
                "compiler",
                "mf-compiler",
                common::crates_dir().join("mf-compiler"),
            ),
            (
                "runtime",
                "mf-runtime",
                common::crates_dir().join("mf-runtime"),
            ),
            (
                "core_nodes",
                "mfn-core",
                common::crates_dir().join("builtin-nodes/core"),
            ),
            (
                "fixture_provider",
                "fixture-multi-nodes",
                common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes"),
            ),
        ] {
            let features = if package == "mf-compiler" && section == "build-dependencies" {
                ", features = [\"codegen\"]"
            } else {
                ""
            };
            manifest.push_str(&format!("{alias} = {{ package = {package:?}, path = {:?}, default-features = false{features} }}\n", path.to_str().unwrap()));
        }
    }
    manifest.push_str("\n[dependencies.json]\npackage = \"serde_json\"\nversion = \"1.0.151\"\n");
    fs::write(root.join("Cargo.toml"), manifest).unwrap();
    fs::write(root.join("build.rs"), r#"
use core_nodes as _;
use fixture_provider as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = runtime::NodeRegistry::from_inventory()?;
    compiler::embed_workflows(&[
        compiler::EmbedRequest::new("answer", "answer.json").with_provider("core", "core_nodes"),
        compiler::EmbedRequest::new("events", "events.json").with_provider("core", "core_nodes"),
        compiler::EmbedRequest::new("contract", "contract.json").with_provider("fixture", "fixture_provider"),
    ], &registry)?;
    Ok(())
}
"#).unwrap();
    fs::write(root.join("src/main.rs"), r#"
compiler::embed_workflow!(pub mod answer);
compiler::embed_workflow!(pub mod events);
compiler::embed_workflow!(pub mod contract);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = runtime::NodeRegistry::from_inventory()?;
    if std::env::var_os("MF_FIXTURE_INTERFACE_CHANGE").is_some() {
        let _ = contract::prepare_workflow(&registry, None)?;
        panic!("incompatible startup metadata was accepted");
    }
    let runtime = runtime::FlowRuntime::default();
    let flow = answer::prepare_workflow(&registry, None)?;
    let mut context = runtime::ExecutionContext::default();
    let outputs = runtime.execute_in_context(&flow, &mut context)?;
    assert!(context.output("source.value").is_ok());
    assert_eq!(runtime.execute(&flow)?, outputs);
    assert_eq!(runtime.execute(&answer::prepare_workflow(&registry, None)?)?, outputs);
    assert_eq!(answer::manifest()?.description.nodes.len(), 2);
    let prepared = events::prepare_stream(&registry)?;
    let options = runtime::StreamOptions {
        stdin: Some(runtime::TextInput::new(std::fs::File::open(std::env::args_os().nth(1).unwrap())?)),
        ..Default::default()
    };
    let stream = runtime.start_stream(prepared, options)?;
    let mut messages = Vec::new();
    while let Some(message) = stream.recv()? {
        messages.push(json::to_value(message.outputs)?);
    }
    stream.join()?;
    println!("{}", json::json!({"answer": outputs, "events": messages}));
    Ok(())
}
"#).unwrap();
    fs::write(root.join("answer.json"), json!({
        "version": "2026-10-03", "dependencies": {"core": {"package": "mfn-core", "version": "0.1.0"}},
        "nodes": [{"id":"source", "kind":"builtin.constant", "config":{"value":42}}, {"id":"echo", "kind":"builtin.identity"}],
        "edges": [{"from_node":"source", "from_output":"value", "to_node":"echo", "to_input":"input"}],
        "outputs": [{"name":"answer", "node":"echo", "port":"value"}]
    }).to_string()).unwrap();
    fs::write(root.join("events.json"), json!({
        "version": "2026-10-03", "execution":{"mode":"stream"}, "dependencies": {"core": {"package": "mfn-core", "version": "0.1.0"}},
        "nodes": [{"id":"source", "kind":"builtin.readline"}],
        "outputs": [{"name":"line", "node":"source", "port":"line"}]
    }).to_string()).unwrap();
    fs::write(root.join("contract.json"), json!({
        "version": "2026-10-03", "dependencies": {"fixture": {"package": "fixture-multi-nodes", "path":"unused-by-host"}},
        "nodes": [{"id":"source", "kind":"fixture.startup_contract"}],
        "outputs": [{"name":"result", "node":"source", "port":"value"}]
    }).to_string()).unwrap();
}

fn build(root: &Path) -> Output {
    mf_compiler::cargo_command(root)
        .args(["build", "--offline", "--release"])
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .unwrap()
}

fn executable(root: &Path) -> PathBuf {
    std::env::var_os("MF_TEST_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
        .join("release")
        .join(format!("mf-embedded-host{}", std::env::consts::EXE_SUFFIX))
}

#[test]
fn embedded_host_uses_renamed_crates_and_checks_contracts_before_execution() {
    let root = tempfile::tempdir().unwrap();
    host(root.path());
    let output = build(root.path());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        !stderr
            .lines()
            .any(|line| line.starts_with("warning:") || line.starts_with("warning[")),
        "{stderr}"
    );
    let input = root.path().join("lines.txt");
    fs::write(&input, "first\nsecond\n").unwrap();
    let executable = executable(root.path());
    let output = Command::new(&executable).arg(&input).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({
            "answer": {"answer":42}, "events":[{"line":"first"},{"line":"second"}]
        })
    );
    let marker = root.path().join("execution-marker");
    for mode in ["type", "required", "remove", "add", "stdin", "condition"] {
        let output = Command::new(&executable)
            .env("MF_FIXTURE_INTERFACE_CHANGE", mode)
            .env("MF_FIXTURE_EXECUTION_MARKER", &marker)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{mode}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("InterfaceDrift"),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!marker.exists());
    }
}
