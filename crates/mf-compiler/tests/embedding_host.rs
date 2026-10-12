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
    let fixture = common::crates_dir().join("mf-compiler/tests/fixtures/embedding-host");
    for file in [
        "Cargo.toml.in",
        "build.rs",
        "answer.json",
        "events.json",
        "contract.json",
        "contract-stream.json",
        "typed.json",
        "counter.json",
    ] {
        fs::copy(
            fixture.join(file),
            root.join(file.strip_suffix(".in").unwrap_or(file)),
        )
        .unwrap();
    }
    copy_tree(&fixture.join("src"), &root.join("src"));
    fs::copy(
        common::crates_dir().join("mf-compiler/tests/fixtures/observation_capture.rs"),
        root.join("src/capture.rs"),
    )
    .unwrap();
    let manifest = root.join("Cargo.toml");
    let source = fs::read_to_string(&manifest)
        .unwrap()
        .replace(
            "../../../../",
            &format!("{}/", common::crates_dir().display()),
        )
        .replace("../multi-nodes", "provider");
    fs::write(manifest, source).unwrap();
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
