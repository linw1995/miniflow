#![cfg(feature = "codegen")]

mod common;

use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn write_json(root: &Path, name: &str, value: Value) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn graph(nodes: Value, outputs: Value) -> Value {
    json!({"version":"2026-10-25", "dependencies":{}, "nodes":nodes, "outputs":outputs})
}

fn leaf(value: i64) -> Value {
    graph(
        json!([{"id":"source","kind":"builtin.constant","config":{"value":value}}]),
        json!([{"name":"value","node":"source","port":"value"}]),
    )
}

fn host(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    let fixture = common::crates_dir().join("mf-compiler/tests/fixtures/embedding-resources");
    let manifest = fs::read_to_string(fixture.join("Cargo.template.toml"))
        .unwrap()
        .replace("@CRATES@", common::crates_dir().to_str().unwrap());
    fs::write(root.join("Cargo.toml"), manifest).unwrap();
    fs::copy(fixture.join("build.rs"), root.join("build.rs")).unwrap();
    fs::copy(fixture.join("main.rs"), root.join("src/main.rs")).unwrap();

    let mut entry = graph(
        json!([{"id":"invoke","kind":"host.invoke","config":{
            "body":{"path":"children/child.json"},
            "opaque":{
                "path":"does-not-exist.json",
                "$flow":{"path":"also-does-not-exist.json"},
                "body":{"path":"not-a-flow.json"}
            }
        }}]),
        json!([
            {"name":"value","node":"invoke","port":"value"},
            {"name":"host","node":"invoke","port":"host"}
        ]),
    );
    // An identically named outer alias must not shadow the child's lexical source.
    entry["flows"] = json!({"local":{"builtin":"mf-embedded-resources-host.answer"}});
    write_json(root, "flows/entry.json", entry);
    let mut child = graph(
        json!([
            {"id":"local","kind":"workflow.call","config":{"flow":"local"}},
            {"id":"host","kind":"workflow.call","config":{"flow":"host"}}
        ]),
        json!([
            {"name":"value","node":"local","port":"value"},
            {"name":"host","node":"host","port":"value"}
        ]),
    );
    child["flows"] = json!({
        "local":{"path":"nested/leaf.json"},
        "host":{"builtin":"mf-embedded-resources-host.answer"}
    });
    write_json(root, "flows/children/child.json", child);
    write_json(root, "flows/children/nested/leaf.json", leaf(42));
    // Explicit resources are bytes, not eagerly parsed Flow definitions.
    fs::write(root.join("unused-malformed.json"), "{").unwrap();

    write_json(
        root,
        "repeat.json",
        graph(
            json!([
                {"id":"seed","kind":"builtin.constant","config":{"value":0}},
                {"id":"repeat","kind":"workflow.loop","config":{
                    "max_iterations":2,
                    "variables":[{"name":"count","type":"int"}],
                    "until":{"variable":"count","operator":"gte","value":3},
                    "body":{"path":"flows/loop-body.json"}
                },"inputs":{"count":{"from":{"node":"seed","port":"value"}}}}
            ]),
            json!([{"name":"count","node":"repeat","port":"count"}]),
        ),
    );
    write_json(
        root,
        "flows/loop-body.json",
        graph(
            json!([
                {"id":"next","kind":"builtin.constant","config":{"value":3}},
                {"id":"assign","kind":"workflow.loop_assign","config":{"variable":"count"},
                 "inputs":{"value":{"from":{"node":"next","port":"value"}}}}
            ]),
            json!([]),
        ),
    );
    write_json(
        root,
        "iteration.json",
        graph(
            json!([
                {"id":"source","kind":"builtin.constant","config":{"value":[2,5,8]}},
                {"id":"each","kind":"builtin.iteration","config":{
                    "mode":"sequential", "body":{"path":"flows/iteration-body.json"}
                },"inputs":{"items":{"from":{"node":"source","port":"value"}}}}
            ]),
            json!([{"name":"results","node":"each","port":"results"}]),
        ),
    );
    write_json(
        root,
        "flows/iteration-body.json",
        graph(
            json!([{"id":"echo","kind":"builtin.identity",
                "inputs":{"input":{"from":{"node":"%iteration","port":"item"}}}}]),
            json!([{"name":"result","node":"echo","port":"value"}]),
        ),
    );
    write_json(
        root,
        "missing.json",
        graph(
            json!([{"id":"invoke","kind":"host.invoke","config":{
                "body":{"path":"unpacked.json"}, "opaque":{}
            }}]),
            json!([]),
        ),
    );
    // A filesystem fallback would find a valid definition and incorrectly accept it.
    write_json(root, "unpacked.json", leaf(999));
}

fn build(root: &Path, extra: Option<&Path>) -> Output {
    let mut command = mf_compiler::cargo_command(root);
    command
        .args(["build", "--offline", "--release"])
        .env("CARGO_TERM_COLOR", "never")
        .env_remove("MF_EMBED_RESOURCE_EXTRA");
    if std::env::var_os("MF_COVERAGE_BINARIES_DIR").is_some() {
        command.arg("--message-format=json-render-diagnostics");
    }
    if let Some(extra) = extra {
        command.env("MF_EMBED_RESOURCE_EXTRA", extra);
    }
    command.output().unwrap()
}

fn preserve_coverage(root: &Path, output: &Output) {
    let Some(coverage) = std::env::var_os("MF_COVERAGE_BINARIES_DIR") else {
        return;
    };
    assert_success(output);
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
        coverage.join("mf-embedding-resources-build-script"),
    )
    .unwrap();
    fs::copy(executable(root), coverage.join("mf-embedding-resources")).unwrap();

    let repository = common::crates_dir().parent().unwrap().to_owned();
    let mappings = [
        (
            "src/main.rs",
            "crates/mf-compiler/tests/fixtures/embedding-resources/main.rs",
        ),
        (
            "build.rs",
            "crates/mf-compiler/tests/fixtures/embedding-resources/build.rs",
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
    assert_success(&output);
    let libdir = String::from_utf8(output.stdout).unwrap();
    let llvm_cov = Path::new(libdir.trim())
        .parent()
        .unwrap()
        .join("bin")
        .join(format!("llvm-cov{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new(llvm_cov)
        .args(["export", "--empty-profile", "--summary-only"])
        .arg(coverage.join("mf-embedding-resources-build-script"))
        .output()
        .unwrap();
    assert_success(&output);
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
        "crates/mf-compiler/tests/fixtures/embedding-resources/build.rs",
    );
    fs::write(
        coverage.join("mf-embedding-resources.paths.json"),
        serde_json::to_vec(&mappings).unwrap(),
    )
    .unwrap();
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn executable(root: &Path) -> PathBuf {
    std::env::var_os("MF_TEST_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
        .join("release")
        .join(format!(
            "mf-embedded-resources-host{}",
            std::env::consts::EXE_SUFFIX
        ))
}

fn run(executable: &Path, cwd: &Path) -> Value {
    let output = Command::new(executable).current_dir(cwd).output().unwrap();
    assert_success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn expected(value: i64) -> Value {
    json!({
        "answer":{"value":value,"host":91},
        "repeat":{"count":3},
        "iteration":{"results":[2,5,8]}
    })
}

#[test]
fn resources_are_packed_without_providers_and_resolved_only_by_the_runtime_host() {
    let root = tempfile::tempdir().unwrap();
    host(root.path());
    assert_success(&build(root.path(), None));
    let executable = executable(root.path());

    let output = Command::new(&executable)
        .current_dir(root.path())
        .env("MF_EMBED_PLAN_ONLY", "1")
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "metadata only"
    );
    assert_eq!(run(&executable, root.path()), expected(42));
    let output = Command::new(&executable)
        .arg("missing")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "missing source rejected"
    );

    // Cargo must track every explicitly packed file, including transitive named Calls.
    write_json(root.path(), "flows/children/nested/leaf.json", leaf(43));
    assert_success(&build(root.path(), None));
    assert_eq!(run(&executable, root.path()), expected(43));

    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(outside.path(), "{}").unwrap();
    let relative_outside = PathBuf::from("..").join(outside.path().file_name().unwrap());
    for path in [outside.path(), relative_outside.as_path()] {
        let output = build(root.path(), Some(path));
        assert!(
            !output.status.success(),
            "accepted outside resource {path:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("OutsidePackage") || stderr.contains("escapes the manifest"),
            "{stderr}"
        );
    }
    let output = build(root.path(), Some(Path::new("flows")));
    assert!(
        !output.status.success(),
        "accepted a directory as a resource"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        // Both a file link and a link in an ancestor directory must be rejected,
        // even when their destinations remain inside the embedding package.
        symlink("unused-malformed.json", root.path().join("linked.json")).unwrap();
        symlink("flows/children", root.path().join("linked-directory")).unwrap();
        for path in ["linked.json", "linked-directory/child.json"] {
            let output = build(root.path(), Some(Path::new(path)));
            assert!(!output.status.success(), "accepted symlink resource {path}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("LinkedSource"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    let output = build(root.path(), None);
    assert_success(&output);
    preserve_coverage(root.path(), &output);

    let output = Command::new(&executable)
        .arg("output-directory")
        .output()
        .unwrap();
    assert_success(&output);
    let output_directory = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    let portable_directory = tempfile::tempdir().unwrap();
    let portable = portable_directory
        .path()
        .join(format!("portable-host{}", std::env::consts::EXE_SUFFIX));
    fs::copy(&executable, &portable).unwrap();
    fs::remove_dir_all(output_directory.join("mf-embedded")).unwrap();
    // Remove authored JSON, Rust sources, manifests, lockfile, build script and
    // package-local build artifacts. The copied binary is the only remaining input.
    root.close().unwrap();
    let output = Command::new(&portable)
        .current_dir(portable_directory.path())
        .env("PATH", "")
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        expected(43)
    );
}
