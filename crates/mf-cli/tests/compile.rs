use serde_json::json;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFINITION: &str = r#"{
    "version":"2026-09-24",
    "nodes":[
        {"id":"source","kind":"builtin.constant","config":{"value":41}},
        {"id":"echo","kind":"builtin.identity"},
        {"id":"status","kind":"builtin.constant","config":{"value":"done"}}
    ],
    "edges":[
        {"from_node":"source","from_output":"value","to_node":"echo","to_input":"input"}
    ],
    "outputs":[
        {"name":"answer","node":"echo","port":"value"},
        {"name":"original","node":"source","port":"value"},
        {"name":"status","node":"status","port":"value"}
    ]
}"#;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for attempt in 0..128 {
            let path = std::env::temp_dir()
                .join(format!("mf cli {} {nonce} {attempt}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("could not create temporary directory: {error}"),
            }
        }
        panic!("could not allocate a unique temporary directory")
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn compile(definition: &Path, output: &Path, rustflags: Option<&str>) -> Output {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mf"));
    command
        .current_dir(workspace)
        .arg("compile")
        .arg(definition)
        .arg("--output")
        .arg(output);
    if let Some(rustflags) = rustflags {
        command.env("RUSTFLAGS", rustflags);
    }
    command.output().unwrap()
}

fn assert_rejected_definition(value: serde_json::Value, diagnostics: &[&str]) {
    let temporary = TemporaryDirectory::new();
    let definition = temporary.path().join("invalid-workflow.json");
    let target = temporary.path().join("workflow");
    fs::write(&definition, value.to_string()).unwrap();
    fs::write(&target, b"previous executable").unwrap();

    let result = compile(&definition, &target, None);
    assert!(!result.status.success());
    assert_eq!(fs::read(&target).unwrap(), b"previous executable");
    let stderr = String::from_utf8_lossy(&result.stderr);
    for diagnostic in diagnostics {
        assert!(
            stderr.contains(diagnostic),
            "missing `{diagnostic}` in: {stderr}"
        );
    }
    assert_eq!(
        fs::read_dir(temporary.path()).unwrap().count(),
        2,
        "validation failures should not create a runner project"
    );
}

#[test]
fn compiled_binary_replaces_target_and_runs_without_the_definition() {
    let temporary = TemporaryDirectory::new();
    let definition = temporary.path().join("workflow.json");
    let target = temporary.path().join("workflow");
    fs::write(&definition, DEFINITION).unwrap();
    fs::write(&target, b"previous executable").unwrap();

    let result = compile(&definition, &target, None);
    assert!(
        result.status.success(),
        "compile failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_ne!(fs::read(&target).unwrap(), b"previous executable");
    assert_eq!(
        fs::read_dir(temporary.path()).unwrap().count(),
        2,
        "successful builds should remove their temporary project"
    );

    fs::remove_file(&definition).unwrap();
    let output = Command::new(&target)
        .current_dir(temporary.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "workflow failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"answer": 41, "original": 41, "status": "done"})
    );
}

#[test]
fn cargo_failure_preserves_existing_target_and_forwards_diagnostics() {
    let temporary = TemporaryDirectory::new();
    let definition = temporary.path().join("workflow.json");
    let target = temporary.path().join("workflow");
    fs::write(&definition, DEFINITION).unwrap();
    fs::write(&target, b"previous executable").unwrap();

    let result = compile(
        &definition,
        &target,
        Some("-C definitely_not_a_rustc_option"),
    );
    assert!(!result.status.success());
    assert_eq!(fs::read(&target).unwrap(), b"previous executable");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("error"), "{stderr}");
    assert!(stderr.contains("definitely_not_a_rustc_option"), "{stderr}");
    assert!(stderr.contains("Cargo build failed"), "{stderr}");
    let saved_projects: Vec<_> = fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".mf-build-")
        })
        .collect();
    assert_eq!(saved_projects.len(), 1);
    assert!(saved_projects[0].join("project/Cargo.toml").is_file());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&saved_projects[0])
            .unwrap()
            .permissions()
            .mode()
            & 0o077,
        0
    );
}

#[test]
fn output_cannot_replace_the_source_definition() {
    let temporary = TemporaryDirectory::new();
    let definition = temporary.path().join("workflow.json");
    fs::write(&definition, DEFINITION).unwrap();

    let result = compile(&definition, &definition, None);
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(&definition).unwrap(), DEFINITION);
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("would overwrite the source definition")
    );
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
}

#[test]
fn unknown_plugin_reports_node_and_kind() {
    assert_rejected_definition(
        json!({
            "version": "2026-09-24",
            "nodes": [{"id": "fetch", "kind": "missing.fetch"}]
        }),
        &["fetch", "missing.fetch", "unknown kind"],
    );
}

#[test]
fn cyclic_workflow_reports_the_cycle_path() {
    assert_rejected_definition(
        json!({
            "version": "2026-09-24",
            "nodes": [
                {"id": "a", "kind": "builtin.identity"},
                {"id": "b", "kind": "builtin.identity"}
            ],
            "edges": [
                {"from_node": "a", "from_output": "value", "to_node": "b", "to_input": "input"},
                {"from_node": "b", "from_output": "value", "to_node": "a", "to_input": "input"}
            ]
        }),
        &["cycle", "a -> b -> a"],
    );
}

#[test]
fn invalid_output_port_reports_both_edge_endpoints() {
    assert_rejected_definition(
        json!({
            "version": "2026-09-24",
            "nodes": [
                {"id": "source", "kind": "builtin.constant", "config": {"value": 1}},
                {"id": "echo", "kind": "builtin.identity"}
            ],
            "edges": [
                {"from_node": "source", "from_output": "missing", "to_node": "echo", "to_input": "input"}
            ]
        }),
        &["source", "missing", "echo", "input", "missing output port"],
    );
}

#[test]
fn invalid_input_port_reports_both_edge_endpoints() {
    assert_rejected_definition(
        json!({
            "version": "2026-09-24",
            "nodes": [
                {"id": "source", "kind": "builtin.constant", "config": {"value": 1}},
                {"id": "echo", "kind": "builtin.identity"}
            ],
            "edges": [
                {"from_node": "source", "from_output": "value", "to_node": "echo", "to_input": "missing"}
            ]
        }),
        &["source", "value", "echo", "missing", "missing input port"],
    );
}
