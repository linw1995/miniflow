#![cfg(feature = "development-support")]
use serde_json::json;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

const DEFINITION: &str = r#"{
    "version": "2026-09-26", "dependencies": {},
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

fn temporary_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("mf cli ")
        .tempdir()
        .unwrap()
}

fn compile_command(definition: &Path, output: &Path) -> Command {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mf"));
    command
        .current_dir(workspace)
        .env("MF_DEV_SUPPORT_ROOT", workspace.join("crates"))
        .env("CARGO_NET_OFFLINE", "true")
        .arg("compile")
        .arg(definition)
        .arg("--output")
        .arg(output)
        .arg("--build-dir")
        .arg(definition.parent().unwrap().join(".mf-build-test"));
    command
}

fn compile(definition: &Path, output: &Path, rustflags: Option<&str>) -> Output {
    let mut command = compile_command(definition, output);
    if let Some(rustflags) = rustflags {
        command.env("RUSTFLAGS", rustflags);
    }
    command.output().unwrap()
}

#[test]
fn development_support_uses_checkout_crates_without_override() {
    let temporary = temporary_directory();
    let definition = temporary.path().join("workflow.json");
    let output = temporary.path().join("workflow");
    fs::write(&definition, definition_json()).unwrap();

    let result = compile_command(&definition, &output)
        .env_remove("MF_DEV_SUPPORT_ROOT")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let execution = Command::new(output).output().unwrap();
    assert!(execution.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&execution.stdout).unwrap()["answer"],
        json!(41)
    );
}

fn assert_rejected_definition(mut value: serde_json::Value, diagnostics: &[&str]) {
    let temporary = temporary_directory();
    let definition = temporary.path().join("invalid-workflow.json");
    let target = temporary.path().join("workflow");
    add_dependencies(&mut value);
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
}

#[test]
fn compiled_binary_replaces_target_and_runs_without_the_definition() {
    let temporary = temporary_directory();
    let definition = temporary.path().join("workflow.json");
    let target = temporary.path().join("workflow");
    fs::write(&definition, definition_json()).unwrap();
    fs::write(&target, b"previous executable").unwrap();

    let result = compile(&definition, &target, None);
    assert!(
        result.status.success(),
        "compile failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_ne!(fs::read(&target).unwrap(), b"previous executable");

    fs::remove_file(&definition).unwrap();
    fs::remove_file(definition.with_extension("lock")).unwrap();
    fs::remove_dir_all(temporary.path().join(".mf-build-test")).unwrap();
    let validation = Command::new(&target)
        .arg("--validate")
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(validation.status.success());
    assert!(validation.stdout.is_empty());
    let output = Command::new(&target)
        .env("PATH", "")
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
    let temporary = temporary_directory();
    let definition = temporary.path().join("workflow.json");
    let target = temporary.path().join("workflow");
    fs::write(&definition, definition_json()).unwrap();
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
    assert!(stderr.contains("dependency resolution failed"), "{stderr}");
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
    assert!(saved_projects[0].join("Cargo.toml").is_file());
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
    let temporary = temporary_directory();
    let definition = temporary.path().join("workflow.json");
    fs::write(&definition, definition_json()).unwrap();

    let result = compile(&definition, &definition, None);
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(&definition).unwrap(), definition_json());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("would overwrite the source definition")
    );
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
}

#[test]
fn unknown_plugin_reports_node_and_kind() {
    assert_rejected_definition(
        json!({
            "version": "2026-09-26", "dependencies": {},
            "nodes": [{"id": "fetch", "kind": "missing.fetch"}]
        }),
        &["fetch", "missing.fetch", "unknown kind"],
    );
}

#[test]
fn cyclic_workflow_reports_the_cycle_path() {
    assert_rejected_definition(
        json!({
            "version": "2026-09-26", "dependencies": {},
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
            "version": "2026-09-26", "dependencies": {},
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
            "version": "2026-09-26", "dependencies": {},
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

fn add_dependencies(value: &mut serde_json::Value) {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    value["dependencies"] = json!({
        "core": {"package":"mfn-core","path":crates.join("builtin-nodes/core")}
    });
}
fn definition_json() -> String {
    let mut value = serde_json::from_str(DEFINITION).unwrap();
    add_dependencies(&mut value);
    value.to_string()
}

#[test]
fn failed_validation_preserves_prior_lock_and_executable() {
    let temporary = temporary_directory();
    let definition = temporary.path().join("flow.json");
    let target = temporary.path().join("flow");
    fs::write(&definition, definition_json()).unwrap();
    let first = compile(&definition, &target, None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let lock = fs::read(definition.with_extension("lock")).unwrap();
    let executable = fs::read(&target).unwrap();
    let mut invalid: serde_json::Value = serde_json::from_str(&definition_json()).unwrap();
    invalid["nodes"][0]["config"] = json!({});
    fs::write(&definition, invalid.to_string()).unwrap();
    let failed = compile(&definition, &target, None);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("runner validation"));
    assert_eq!(fs::read(definition.with_extension("lock")).unwrap(), lock);
    assert_eq!(fs::read(&target).unwrap(), executable);
}

#[test]
fn install_failure_reports_persisted_lock_and_preserves_existing_directory() {
    let temporary = temporary_directory();
    let definition = temporary.path().join("flow.json");
    let target = temporary.path().join("output");
    fs::write(&definition, definition_json()).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("sentinel"), "existing").unwrap();
    let result = compile(&definition, &target, None);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("dependency lock was updated"));
    assert!(definition.with_extension("lock").is_file());
    assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"existing");
}

#[cfg(unix)]
#[test]
fn missing_artifact_and_cargo_exit_cannot_install_a_stale_binary() {
    let temporary = temporary_directory();
    let definition = temporary.path().join("flow.json");
    let target = temporary.path().join("flow");
    fs::write(&definition, definition_json()).unwrap();
    fs::write(&target, "previous").unwrap();
    let wrapper = temporary.path().join("cargo-wrapper");
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for status in [0, 23] {
        fs::write(&wrapper, format!("#!/bin/sh\nif [ \"$1\" = metadata ]; then exec \"$MF_REAL_CARGO\" \"$@\"; fi\necho 'fixture Cargo diagnostics' >&2\nexit {status}\n")).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_mf"))
            .args(["compile"])
            .arg(&definition)
            .arg("--output")
            .arg(&target)
            .arg("--build-dir")
            .arg(temporary.path().join(".mf-build-test"))
            .env("MF_DEV_SUPPORT_ROOT", crates)
            .env("CARGO_NET_OFFLINE", "true")
            .env(
                "MF_REAL_CARGO",
                std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
            )
            .env("CARGO", &wrapper)
            .output()
            .unwrap();
        assert!(!result.status.success());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("fixture Cargo diagnostics"), "{stderr}");
        assert!(
            stderr.contains(if status == 0 {
                "did not produce a runner"
            } else {
                "Cargo build failed"
            }),
            "{stderr}"
        );
        assert_eq!(fs::read(&target).unwrap(), b"previous");
        assert!(!definition.with_extension("lock").exists());
    }
}

#[test]
fn warm_build_preserves_generated_inputs_and_reuses_compiled_runner() {
    let temporary = temporary_directory();
    let definition = temporary.path().join("flow.json");
    let target = temporary.path().join("flow");
    let source = definition_json();
    fs::write(&definition, &source).unwrap();
    let first = compile(&definition, &target, None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let project = temporary.path().join(".mf-build-test");
    let names = ["Cargo.toml", "Cargo.lock", "src/main.rs", "src/workflow.rs"];
    let times: Vec<_> = names
        .iter()
        .map(|name| {
            fs::metadata(project.join(name))
                .unwrap()
                .modified()
                .unwrap()
        })
        .collect();
    let second = compile(&definition, &temporary.path().join("another-output"), None);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(String::from_utf8_lossy(&second.stderr).contains("reused compiled runner"));
    for (name, time) in names.iter().zip(times) {
        assert_eq!(
            fs::metadata(project.join(name))
                .unwrap()
                .modified()
                .unwrap(),
            time,
            "{name} was rewritten"
        );
    }
    assert_eq!(fs::read_to_string(&definition).unwrap(), source);
}

#[test]
fn repairs_partial_projects_and_updates_configuration() {
    let temporary = temporary_directory();
    let definition = temporary.path().join("flow.json");
    let target = temporary.path().join("flow");
    fs::write(&definition, definition_json()).unwrap();
    assert!(compile(&definition, &target, None).status.success());
    let project = temporary.path().join(".mf-build-test");
    fs::remove_file(project.join("src/main.rs")).unwrap();
    fs::write(project.join("Cargo.lock"), "interrupted working state").unwrap();
    let mut changed: serde_json::Value = serde_json::from_str(&definition_json()).unwrap();
    changed["nodes"].as_array_mut().unwrap().pop();
    changed["outputs"].as_array_mut().unwrap().pop();
    changed["nodes"][0]["config"]["value"] = json!(99);
    fs::write(&definition, changed.to_string()).unwrap();
    let result = compile(&definition, &target, None);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(project.join("src/main.rs").is_file());
    let output = Command::new(&target).output().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"answer":99,"original":99})
    );
}

#[test]
fn cargo_invalidates_features_local_sources_and_flags() {
    let temporary = temporary_directory();
    let definition = temporary.path().join("flow.json");
    let target = temporary.path().join("flow");
    let local = temporary.path().join("nodes");
    fs::create_dir_all(local.join("src")).unwrap();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let fixture = crates.join("mf-compiler/tests/fixtures/multi-nodes");
    let manifest = fs::read_to_string(fixture.join("Cargo.toml"))
        .unwrap()
        .replace(
            "../../../../mf-runtime",
            crates.join("mf-runtime").to_str().unwrap(),
        );
    fs::write(local.join("Cargo.toml"), &manifest).unwrap();
    let plugin = fs::read_to_string(fixture.join("src/lib.rs")).unwrap();
    for entry in fs::read_dir(fixture.join("src")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), local.join("src").join(entry.file_name())).unwrap();
    }
    let mut value = json!({"version":"2026-09-26","dependencies":{"local":{"package":"fixture-multi-nodes","path":"nodes","features":["double"]}},"nodes":[{"id":"source","kind":"fixture.source"}],"outputs":[{"name":"value","node":"source","port":"value"}]});
    let check = |value: &serde_json::Value, expected: i64, flags: Option<&str>| {
        fs::write(&definition, value.to_string()).unwrap();
        let result = compile(&definition, &target, flags);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let run = Command::new(&target).output().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&run.stdout).unwrap(),
            json!({"value":expected})
        );
        result
    };
    check(&value, 14, None);
    value["dependencies"]["local"]["features"] = json!([]);
    assert!(
        !String::from_utf8_lossy(&check(&value, 7, None).stderr).contains("reused compiled runner")
    );
    value["dependencies"]["local"]["features"] = json!(["double"]);
    fs::write(local.join("src/lib.rs"), plugin.replace("14", "28")).unwrap();
    let changed = check(&value, 28, None);
    assert!(!String::from_utf8_lossy(&changed.stderr).contains("Compiling mf-runtime"));
    let built = fs::read(&target).unwrap();
    let invalid_flags = compile(
        &definition,
        &target,
        Some("-C definitely_not_a_rustc_option"),
    );
    assert!(!invalid_flags.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_flags.stderr).contains("definitely_not_a_rustc_option")
    );
    assert_eq!(fs::read(&target).unwrap(), built);
}

#[cfg(unix)]
#[test]
fn rejects_redirected_cache_directories_without_mutating_inputs() {
    for name in ["src", "target"] {
        let temporary = temporary_directory();
        let definition = temporary.path().join("workflow.rs");
        let target = temporary.path().join("flow");
        let original = definition_json();
        fs::write(&definition, &original).unwrap();
        fs::write(&target, "previous executable").unwrap();
        let directory = temporary.path().join(".mf-build-test");
        let canonical = fs::canonicalize(&definition).unwrap();
        drop(mf_compiler::BuildDirectory::open(&canonical, Some(&directory)).unwrap());
        std::os::unix::fs::symlink(temporary.path(), directory.join(name)).unwrap();
        let result = compile(&definition, &target, None);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("must not be a symbolic link"));
        assert_eq!(fs::read_to_string(&definition).unwrap(), original);
        assert_eq!(fs::read(&target).unwrap(), b"previous executable");
        assert!(!definition.with_extension("lock").exists());
        assert!(!directory.join("Cargo.toml").exists());
    }
}
