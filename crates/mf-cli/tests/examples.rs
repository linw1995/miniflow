#![cfg(feature = "development-support")]

use serde_json::{Value, json};
use std::{fs, path::Path};

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

fn examples() -> [(&'static str, Value); 7] {
    [
        ("cel-list.json", json!({"doubled": [2, 4]})),
        ("cel-scalar.json", json!({"doubled": 42})),
        ("else-if.json", json!({"medium": {"amount": 500}})),
        ("hello-workflow.json", json!({"answer": 42})),
        ("if-else.json", json!({"accepted": {"amount": 150}})),
        ("iteration.json", json!({"results": [2, 5, 8]})),
        ("loop.json", json!({"count": 3})),
    ]
}

#[test]
fn every_example_has_an_expected_result() {
    let mut actual: Vec<_> = fs::read_dir(workspace().join("examples"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    let mut expected: Vec<_> = examples().into_iter().map(|(name, _)| name).collect();
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "update the example cases when adding, removing, or renaming examples/*.json"
    );
}

#[cfg(unix)]
#[test]
fn documented_examples_compile_and_produce_expected_outputs() {
    use std::{os::unix::fs::symlink, process::Command};

    let temporary = tempfile::Builder::new()
        .prefix("mf examples ")
        .tempdir()
        .unwrap();
    let definitions = temporary.path().join("examples");
    let runtime = temporary.path().join("runtime");
    fs::create_dir(&definitions).unwrap();
    fs::create_dir(&runtime).unwrap();
    // Preserve the examples' relative paths while keeping lock files out of the checkout.
    symlink(workspace().join("crates"), temporary.path().join("crates")).unwrap();

    for (name, expected) in examples() {
        let definition = definitions.join(name);
        fs::copy(workspace().join("examples").join(name), &definition).unwrap();
        let stem = Path::new(name).file_stem().unwrap();
        let executable = runtime.join(stem);
        let build = temporary.path().join("build").join(stem);
        let compiled = Command::new(env!("CARGO_BIN_EXE_mf"))
            .current_dir(&runtime)
            .env("MF_DEV_SUPPORT_ROOT", workspace().join("crates"))
            .env("CARGO_NET_OFFLINE", "true")
            .arg("compile")
            .arg(&definition)
            .arg("--output")
            .arg(&executable)
            .arg("--build-dir")
            .arg(&build)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{name}: compilation failed with {}:\n{}\n{}",
            compiled.status,
            String::from_utf8_lossy(&compiled.stdout),
            String::from_utf8_lossy(&compiled.stderr),
        );

        let output = Command::new(&executable)
            .current_dir(&runtime)
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: execution failed with {}:\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let actual: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{name}: invalid JSON output: {error}\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(actual, expected, "{name}: unexpected workflow output");
    }
}
