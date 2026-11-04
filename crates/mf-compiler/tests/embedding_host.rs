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
    fs::copy(fixture.join("Cargo.template.toml"), root.join("Cargo.toml")).unwrap();
    for file in [
        "build.rs",
        "answer.json",
        "events.json",
        "contract.json",
        "contract-stream.json",
        "typed.json",
        "counter.json",
    ] {
        fs::copy(fixture.join(file), root.join(file)).unwrap();
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
    let mut command = mf_compiler::cargo_command(root);
    command
        .args(["build", "--offline", "--release"])
        .env("CARGO_TERM_COLOR", "never");
    if std::env::var_os("MF_COVERAGE_BINARIES_DIR").is_some() {
        command.arg("--message-format=json-render-diagnostics");
    }
    command.output().unwrap()
}

fn preserve_coverage(root: &Path, output: &Output) {
    let Some(coverage) = std::env::var_os("MF_COVERAGE_BINARIES_DIR") else {
        return;
    };
    assert!(output.status.success());
    let coverage = Path::new(&coverage);
    let build_script = root.join("build.rs").canonicalize().unwrap();
    // Cargo identifies the exact build-script executable, including fresh artifacts.
    // Scanning the shared target directory could retain a stale script instead.
    let artifacts: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| {
            message["reason"] == "compiler-artifact"
                && message["target"]["kind"] == json!(["custom-build"])
                && message["target"]["src_path"]
                    .as_str()
                    .is_some_and(|source| root.join(source).canonicalize().unwrap() == build_script)
        })
        .collect();
    assert_eq!(artifacts.len(), 1, "missing host build-script artifact");
    let artifact = &artifacts[0];
    let script_binary = artifact["executable"].as_str().unwrap_or_else(|| {
        let filenames = artifact["filenames"].as_array().unwrap();
        assert_eq!(filenames.len(), 1, "ambiguous build-script artifact");
        filenames[0].as_str().unwrap()
    });
    let script_binary = Path::new(script_binary);
    let script_directory = script_binary.parent().unwrap();
    let launcher = script_directory.join(".kache-launch");
    // Ignore a stale record if this artifact is no longer a Kache launcher.
    let launcher_marker = b"KACHE_BUILD_SCRIPT_PATH=";
    let is_launcher = launcher.is_file()
        && fs::read(script_binary)
            .unwrap()
            .windows(launcher_marker.len())
            .any(|window| window == launcher_marker);
    let script_binary = if is_launcher {
        // Kache publishes a launcher at Cargo's artifact path. Its NUL-delimited
        // record names the preserved Rust executable that owns the coverage maps.
        let record = fs::read(launcher).unwrap();
        let preserved = record.split(|byte| *byte == 0).next().unwrap();
        script_directory.join(std::str::from_utf8(preserved).unwrap())
    } else {
        script_binary.to_owned()
    };
    fs::copy(
        script_binary,
        coverage.join("mf-embedding-host-build-script"),
    )
    .unwrap();
    fs::copy(executable(root), coverage.join("mf-embedding-host")).unwrap();

    let repository = common::crates_dir().parent().unwrap().to_owned();
    let mappings = [
        (
            "src/main.rs",
            "crates/mf-compiler/tests/fixtures/embedding-host/src/main.rs",
        ),
        (
            "build.rs",
            "crates/mf-compiler/tests/fixtures/embedding-host/build.rs",
        ),
    ]
    .map(|(source, fixture)| {
        let source = root.join(source).canonicalize().unwrap();
        // Remaps are only valid when the compiled and tracked sources share lines.
        assert_eq!(
            fs::read(&source).unwrap(),
            fs::read(repository.join(fixture)).unwrap()
        );
        (source.to_str().unwrap().to_owned(), fixture)
    });
    let mut mappings = std::collections::BTreeMap::from(mappings);
    // A reused script can still name its original, now-deleted source root.
    // Ask its coverage map rather than guessing a cache-wide source alias.
    let output = Command::new("rustc")
        .args(["--print", "target-libdir"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let libdir = String::from_utf8(output.stdout).unwrap();
    let llvm_cov = Path::new(libdir.trim())
        .parent()
        .unwrap()
        .join("bin")
        .join(format!("llvm-cov{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new(llvm_cov)
        .args(["export", "--empty-profile", "--summary-only"])
        .arg(coverage.join("mf-embedding-host-build-script"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let sources: Vec<&str> = report["data"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|data| data["files"].as_array().unwrap())
        .filter_map(|file| file["filename"].as_str())
        .filter(|source| {
            Path::new(source)
                .file_name()
                .is_some_and(|name| name == "build.rs")
        })
        .collect();
    assert_eq!(sources.len(), 1, "ambiguous build-script coverage source");
    mappings.insert(
        sources[0].to_owned(),
        "crates/mf-compiler/tests/fixtures/embedding-host/build.rs",
    );
    fs::write(
        coverage.join("mf-embedding-host.paths.json"),
        serde_json::to_vec(&mappings).unwrap(),
    )
    .unwrap();
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
            .env("MF_FIXTURE_INTERFACE_CHANGE", "initialization")
            .env("MF_EMBEDDING_VALIDATE_CONTRACT", "1")
            .env("MF_FIXTURE_EXECUTION_MARKER", &marker);
        if stream {
            command.env("MF_EMBEDDING_VALIDATE_STREAM", "1");
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "stream={stream}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("NodeConstruction"),
            "stream={stream}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!marker.exists());
    }
    let described = Command::new(&executable)
        .env("MF_EMBEDDING_DESCRIBE", "1")
        .env("MF_FIXTURE_INTERFACE_CHANGE", "type")
        .output()
        .unwrap();
    assert!(
        described.status.success(),
        "{}",
        String::from_utf8_lossy(&described.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&described.stdout).trim(),
        "Int64/Int64"
    );
    let answer = root.path().join("answer.json");
    let mut definition: Value = serde_json::from_slice(&fs::read(&answer).unwrap()).unwrap();
    definition["nodes"][0]["config"]["value"] = json!(43);
    fs::write(&answer, definition.to_string()).unwrap();
    assert_build_success(root.path());
    assert_eq!(run(&executable, &input)["answer"]["answer"], 43);
    for (mutation, expected) in [
        ("kind", "UnknownNodeKind"),
        ("config", "NodeConstruction"),
        ("port", "UnknownOutputPort"),
    ] {
        let mut invalid = definition.clone();
        match mutation {
            "kind" => invalid["nodes"][0]["kind"] = json!("missing.kind"),
            "config" => invalid["nodes"][0]["config"] = json!({}),
            "port" => invalid["edges"][0]["from_output"] = json!("missing"),
            _ => unreachable!(),
        }
        fs::write(&answer, invalid.to_string()).unwrap();
        assert_build_success(root.path());
        let output = Command::new(&executable)
            .env("MF_EMBEDDING_VALIDATE", "1")
            .output()
            .unwrap();
        assert!(!output.status.success(), "{mutation}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{mutation}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::write(&answer, definition.to_string()).unwrap();
    assert_build_success(root.path());
    let mut invalid = definition.clone();
    invalid["edges"][0]["to_node"] = json!("source");
    fs::write(&answer, invalid.to_string()).unwrap();
    let output = build(root.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cycle"));
    assert_eq!(run(&executable, &input)["answer"]["answer"], 43);
    fs::write(&answer, definition.to_string()).unwrap();
    let provider = root.path().join("provider/src/interface_fixture.rs");
    let source = fs::read_to_string(&provider).unwrap();
    fs::write(
        &provider,
        source.replace("ValueType::String, false", "ValueType::Boolean, false"),
    )
    .unwrap();
    let output = assert_build_success(root.path());
    preserve_coverage(root.path(), &output);
    let described = Command::new(&executable)
        .env("MF_EMBEDDING_DESCRIBE", "1")
        .output()
        .unwrap();
    assert!(described.status.success());
    assert_eq!(
        String::from_utf8_lossy(&described.stdout).trim(),
        "Boolean/Boolean"
    );
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
    fs::remove_file(root.path().join("Cargo.toml")).unwrap();
    fs::remove_file(root.path().join("Cargo.lock")).unwrap();
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

fn assert_build_success(root: &Path) -> Output {
    let output = build(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
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
