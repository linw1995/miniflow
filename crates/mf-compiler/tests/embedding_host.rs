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
    let fixture = common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes");
    let provider = root.join("provider");
    copy_tree(&fixture.join("src"), &provider.join("src"));
    let manifest = fs::read_to_string(fixture.join("Cargo.toml"))
        .unwrap()
        .replace(
            "path = \"../../../../mf-runtime\"",
            &format!(
                "path = {:?}",
                common::crates_dir().join("mf-runtime").to_str().unwrap()
            ),
        );
    fs::write(provider.join("Cargo.toml"), manifest).unwrap();
    fs::write(
        provider.join("src/embedding_stateful.rs"),
        include_str!("fixtures/embedding_stateful.rs"),
    )
    .unwrap();
    let library = provider.join("src/lib.rs");
    let mut source = fs::read_to_string(&library).unwrap();
    source.push_str("\nmod embedding_stateful;\n");
    fs::write(library, source).unwrap();
    let mut manifest = String::from(
        "[package]\nname = \"mf-embedded-host\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\nresolver = \"3\"\nexclude = [\"provider\"]\n",
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
            ("fixture_provider", "fixture-multi-nodes", provider.clone()),
            (
                "code_nodes",
                "mfn-code",
                common::crates_dir().join("builtin-nodes/code"),
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
    manifest.push_str(&format!(
        "\n[dependencies.mf-telemetry]\npath = {:?}\ndefault-features = false\n",
        common::crates_dir().join("mf-telemetry").to_str().unwrap()
    ));
    manifest.push_str("\n[dependencies.opentelemetry]\nversion = \"0.33.0\"\ndefault-features = false\nfeatures = [\"logs\", \"trace\"]\n\n[dependencies.opentelemetry_sdk]\nversion = \"0.33.0\"\ndefault-features = false\nfeatures = [\"logs\", \"trace\", \"testing\"]\n");
    fs::write(root.join("Cargo.toml"), manifest).unwrap();
    fs::write(root.join("build.rs"), r#"
use core_nodes as _;
use fixture_provider as _;
use code_nodes as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let registry = runtime::NodeRegistry::from_inventory()?;
    compiler::embed_workflows(&[
        compiler::EmbedRequest::new("answer", "answer.json").with_provider("core", "core_nodes"),
        compiler::EmbedRequest::new("events", "events.json").with_provider("core", "core_nodes"),
        compiler::EmbedRequest::new("contract", "contract.json").with_provider("fixture", "fixture_provider"),
        compiler::EmbedRequest::new("contract_stream", "contract-stream.json").with_provider("fixture", "fixture_provider"),
        compiler::EmbedRequest::new("typed", "typed.json").with_provider("fixture", "fixture_provider"),
        compiler::EmbedRequest::new("counter", "counter.json").with_provider("fixture", "fixture_provider"),
        compiler::EmbedRequest::new("repeat", "repeat.json").with_provider("core", "core_nodes").with_provider("code", "code_nodes"),
        compiler::EmbedRequest::new("iteration", "iteration.json").with_provider("core", "core_nodes").with_provider("code", "code_nodes"),
    ], &registry)?;
    Ok(())
}
"#).unwrap();
    fs::write(root.join("src/main.rs"), r#"
compiler::embed_workflow!(pub mod answer);
compiler::embed_workflow!(pub mod events);
compiler::embed_workflow!(pub mod contract);
compiler::embed_workflow!(pub mod contract_stream);
compiler::embed_workflow!(pub mod typed);
compiler::embed_workflow!(pub mod counter);
compiler::embed_workflow!(pub mod repeat);
compiler::embed_workflow!(pub mod iteration);

mod capture {
    use json as serde_json;
    include!("capture.rs");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("MF_EMBEDDING_OUTPUT").is_some() {
        println!("{}", env!("OUT_DIR"));
        return Ok(());
    }
    let registry = runtime::NodeRegistry::from_inventory()?;
    if std::env::var_os("MF_EMBEDDING_DESCRIBE").is_some() {
        println!("{:?}", contract::manifest()?.interface.schema.inputs["source"]["path./~"].value_type);
        return Ok(());
    }
    if std::env::var_os("MF_FIXTURE_INTERFACE_CHANGE").is_some() {
        if std::env::var_os("MF_EMBEDDING_STREAM_DRIFT").is_some() {
            let _ = contract_stream::prepare_stream(&registry)?;
        } else {
            let _ = contract::prepare_workflow(&registry, None)?;
        }
        panic!("incompatible startup metadata was accepted");
    }
    let runtime = runtime::FlowRuntime::default();
    let counter = counter::prepare_workflow(&registry, None)?;
    assert_eq!(runtime.execute(&counter)?["count"].as_u64(), Some(0));
    assert_eq!(runtime.execute(&counter)?["count"].as_u64(), Some(1));
    assert_eq!(runtime.execute(&counter::prepare_workflow(&registry, None)?)?["count"].as_u64(), Some(0));
    assert_eq!(runtime.execute(&repeat::prepare_workflow(&registry, None)?)?["count"].as_i64(), Some(3));
    assert_eq!(json::to_value(runtime.execute(&iteration::prepare_workflow(&registry, None)?)?)?, json::json!({"results":[2,5,8]}));
    let mut typed_context = runtime::ExecutionContext::default();
    typed_context.set_workflow_arguments(runtime::WorkflowArguments::try_from(json::json!({"source.text":"hello"}))?);
    runtime.execute_in_context(&typed::prepare_workflow(&registry, None)?, &mut typed_context)?;
    match typed_context.output("source.text")? {
        runtime::ContextValue::Value(value) => assert_eq!(value.as_str(), Some("hello")),
        runtime::ContextValue::Skipped => panic!("unobserved intermediate typed output was skipped"),
    }
    let harness = capture::Harness::new(true);
    let manifest = typed::manifest()?;
    let observation = harness.observer().start_with_description(manifest.description.clone(), mf_telemetry::identity::RunId::new())?;
    let typed_outputs = runtime::ExecutionContext::run(Some(observation), |state| -> Result<_, Box<dyn std::error::Error>> {
        let flow = typed::prepare_workflow(&registry, state.observation_mut())?;
        state.set_workflow_arguments(runtime::WorkflowArguments::try_from(json::json!({"source.text":"hello"}))?);
        let result = runtime.execute_in_context(&flow, state)?;
        match state.output("source.text")? {
            runtime::ContextValue::Value(value) => assert_eq!(value.as_str(), Some("hello")),
            runtime::ContextValue::Skipped => panic!("intermediate typed output was skipped"),
        }
        Ok(result)
    })?;
    assert_eq!(typed_outputs["reply"].as_str(), Some("hello"));
    let records = harness.records();
    assert_eq!(records.len(), 6);
    assert_eq!(records.first().unwrap().event_name, "mf.workflow.started");
    assert_eq!(records.last().unwrap().event_name, "mf.workflow.finished");
    assert!(records.iter().all(|record| record.attributes["mf.workflow.id"] == json::json!(manifest.description.workflow_id.to_string())));
    assert_eq!(harness.spans.get_finished_spans()?.len(), 3);
    let flow = answer::prepare_workflow(&registry, None)?;
    let outputs = runtime.execute(&flow)?;
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
    fs::write(
        root.join("src/capture.rs"),
        include_str!("fixtures/observation_capture.rs"),
    )
    .unwrap();
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
    fs::write(root.join("contract-stream.json"), json!({
        "version": "2026-10-03", "execution": {"mode": "stream"},
        "dependencies": {"fixture": {"package": "fixture-multi-nodes", "path":"unused-by-host"}},
        "nodes": [{"id":"source", "kind":"fixture.startup_contract", "config":{"stream":true}}],
        "outputs": [{"name":"result", "node":"source", "port":"value"}]
    }).to_string()).unwrap();
    fs::write(root.join("typed.json"), json!({
        "version":"2026-10-03", "dependencies":{"fixture":{"package":"fixture-multi-nodes", "path":"unused-by-host"}},
        "nodes":[{"id":"source", "kind":"fixture.typed_text"},{"id":"echo", "kind":"fixture.typed_text"}],
        "edges":[{"from_node":"source", "from_output":"text", "to_node":"echo", "to_input":"text"}],
        "outputs":[{"name":"reply", "node":"echo", "port":"text"}]
    }).to_string()).unwrap();
    fs::write(root.join("counter.json"), json!({
        "version":"2026-10-03", "dependencies":{"fixture":{"package":"fixture-multi-nodes", "path":"unused-by-host"}},
        "nodes":[{"id":"source", "kind":"fixture.embedding_counter"}],
        "outputs":[{"name":"count", "node":"source", "port":"value"}]
    }).to_string()).unwrap();
    let examples = common::crates_dir().parent().unwrap().join("examples");
    fs::copy(examples.join("loop.json"), root.join("repeat.json")).unwrap();
    fs::copy(examples.join("iteration.json"), root.join("iteration.json")).unwrap();
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
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
fn embedded_host_preserves_execution_and_build_contracts() {
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
    for stream in [false, true] {
        let mut command = Command::new(&executable);
        command
            .env("MF_FIXTURE_INTERFACE_CHANGE", "type")
            .env("MF_FIXTURE_EXECUTION_MARKER", &marker);
        if stream {
            command.env("MF_EMBEDDING_STREAM_DRIFT", "1");
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "stream={stream}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("InterfaceDrift"),
            "stream={stream}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!marker.exists());
    }
    let answer = root.path().join("answer.json");
    let mut definition: Value = serde_json::from_slice(&fs::read(&answer).unwrap()).unwrap();
    definition["nodes"][0]["config"]["value"] = json!(43);
    fs::write(&answer, definition.to_string()).unwrap();
    assert_build_success(root.path());
    assert_eq!(run(&executable, &input)["answer"]["answer"], 43);
    let mut invalid = definition.clone();
    invalid["edges"][0]["from_output"] = json!("missing");
    fs::write(&answer, invalid.to_string()).unwrap();
    let output = build(root.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("UnknownOutputPort"));
    assert_eq!(run(&executable, &input)["answer"]["answer"], 43);
    fs::write(&answer, definition.to_string()).unwrap();
    let provider = root.path().join("provider/src/interface_fixture.rs");
    let source = fs::read_to_string(&provider).unwrap();
    fs::write(
        &provider,
        source.replace("ValueType::String, false", "ValueType::Boolean, false"),
    )
    .unwrap();
    assert_build_success(root.path());
    let described = Command::new(&executable)
        .env("MF_EMBEDDING_DESCRIBE", "1")
        .output()
        .unwrap();
    assert!(described.status.success());
    assert_eq!(String::from_utf8_lossy(&described.stdout).trim(), "Boolean");
    let portable = root.path().join("portable-host");
    fs::copy(&executable, &portable).unwrap();
    for entry in fs::read_dir(root.path()).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            fs::remove_file(path).unwrap();
        }
    }
    fs::remove_dir_all(root.path().join("src")).unwrap();
    fs::remove_dir_all(root.path().join("provider")).unwrap();
    let output = Command::new(&executable)
        .env("MF_EMBEDDING_OUTPUT", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let output_directory = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    fs::remove_dir_all(output_directory.join("mf-embedded")).unwrap();
    fs::remove_file(root.path().join("build.rs")).unwrap();
    let output = Command::new(&portable)
        .arg(&input)
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["answer"]["answer"],
        43
    );

    let example = tempfile::tempdir().unwrap();
    let source = common::crates_dir()
        .parent()
        .unwrap()
        .join("examples/embedded-workflows");
    for file in ["Cargo.toml", "build.rs", "answer.json"] {
        fs::copy(source.join(file), example.path().join(file)).unwrap();
    }
    copy_tree(&source.join("src"), &example.path().join("src"));
    let manifest = example.path().join("Cargo.toml");
    let content = fs::read_to_string(&manifest).unwrap().replace(
        "../../crates/",
        &format!("{}/", common::crates_dir().display()),
    );
    fs::write(manifest, content).unwrap();
    assert_build_success(example.path());
    let example_binary = executable.with_file_name(format!(
        "embedded-workflows{}",
        std::env::consts::EXE_SUFFIX
    ));
    let example_binary = if std::env::var_os("MF_TEST_TARGET_DIR").is_some() {
        example_binary
    } else {
        example
            .path()
            .join("target/release")
            .join(example_binary.file_name().unwrap())
    };
    let output = Command::new(example_binary).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"answer":42})
    );
}

fn assert_build_success(root: &Path) {
    let output = build(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run(executable: &Path, input: &Path) -> Value {
    let output = Command::new(executable).arg(input).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
