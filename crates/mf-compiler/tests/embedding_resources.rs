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
    if let Some(extra) = extra {
        command.env("MF_EMBED_RESOURCE_EXTRA", extra);
    }
    command.output().unwrap()
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
    assert_success(&build(root.path(), None));

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
