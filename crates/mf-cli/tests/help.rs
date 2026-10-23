#![cfg(unix)]

use mf_runtime::WorkflowManifest;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn manifest() -> WorkflowManifest {
    serde_json::from_value(serde_json::json!({
        "version": "2026-10-16",
        "definition": {
            "version": "2026-10-03", "dependencies": {},
            "nodes": [
                {"id": "source", "kind": "fixture.source"},
                {"id": "transform", "kind": "fixture.transform"}
            ],
            "edges": [{"from_node": "source", "from_output": "value", "to_node": "transform", "to_input": "amount"}]
        },
        "execution_order": ["source", "transform"],
        "input_schema": {"inputs": {"source": {"path": {
            "type": "string", "required": true,
            "binding": {"env": "FLOW_PATH", "default": "hidden-default", "secret": true}
        }}}, "stdin": {}}
    }))
    .unwrap()
}

fn command(runner: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mf"));
    command.arg("run").arg(runner);
    command
}

fn write_runner(runner: &Path, payload: Option<&[u8]>) {
    let source = runner.with_extension("rs");
    let data = runner.with_extension("manifest");
    let embedded = if let Some(payload) = payload {
        fs::write(&data, payload).unwrap();
        format!(
            r#"
#[used]
#[cfg_attr(target_os = "linux", unsafe(link_section = ".mf_manifest"))]
#[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__mf_manifest"))]
static MANIFEST: [u8; include_bytes!({data:?}).len()] = *include_bytes!({data:?});
"#
        )
    } else {
        String::new()
    };
    let retain = if payload.is_some() {
        "std::hint::black_box(&MANIFEST);"
    } else {
        ""
    };
    fs::write(
        &source,
        format!(
            "{embedded}\nfn main() {{ {retain} panic!(\"help must not launch the runner\"); }}\n"
        ),
    )
    .unwrap();
    let output = Command::new("rustc")
        .arg(&source)
        .args(["--crate-name", "help_runner", "-o"])
        .arg(runner)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn combines_help_from_a_nonexecutable_binary_without_running_or_reading_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("runner with spaces");
    let payload = manifest().to_bytes().unwrap();
    write_runner(&runner, Some(&payload));
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o600)).unwrap();
    for flags in [
        vec!["--help"],
        vec!["-h"],
        vec!["--tui", "--help"],
        vec!["--chat", "--help"],
        vec!["--chat-db", "missing/chat.sqlite", "--help"],
        vec!["--inputs-file", "missing/inputs.json", "--help"],
        vec!["--inputs", "invalid-json", "--help"],
    ] {
        let output = command(&runner)
            .args(&flags)
            .env("FLOW_PATH", "hidden-environment")
            .output()
            .unwrap();
        assert!(output.status.success(), "{flags:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{flags:?}: {output:?}");
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.starts_with("CLI help:\n"), "{help}");
        assert!(help.contains("Usage: mf run"), "{help}");
        assert!(help.contains("--tui"), "{help}");
        let (_, runner_help) = help.split_once("Runner help:\n").unwrap();
        assert!(runner_help.contains("--source.path <VALUE>"), "{help}");
        assert!(runner_help.contains("env: FLOW_PATH"), "{help}");
        assert!(runner_help.contains("default configured; secret"), "{help}");
        assert!(!runner_help.contains("--transform.amount"), "{help}");
        assert!(!help.contains("hidden-default"), "{help}");
        assert!(!help.contains("hidden-environment"), "{help}");
    }
}

#[test]
fn chat_database_help_displays_the_resolved_path_without_creating_storage() {
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("runner");
    write_runner(&runner, Some(&manifest().to_bytes().unwrap()));
    let home = directory.path().join("home with spaces");
    let data = directory.path().join("custom data");
    for data_home in [
        None,
        Some(Path::new("")),
        Some(Path::new("relative")),
        Some(data.as_path()),
    ] {
        #[cfg(target_os = "macos")]
        let expected = home.join("Library/Application Support/miniflow/chat.sqlite");
        #[cfg(not(target_os = "macos"))]
        let expected = data_home
            .filter(|path| path.is_absolute())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("miniflow/chat.sqlite");
        for args in [
            vec!["run".as_ref(), "--help".as_ref()],
            vec!["run".as_ref(), "-h".as_ref()],
            vec!["help".as_ref(), "run".as_ref()],
            vec!["run".as_ref(), runner.as_os_str(), "--help".as_ref()],
        ] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_mf"));
            command
                .args(args)
                .env("HOME", &home)
                .env_remove("XDG_DATA_HOME");
            if let Some(data_home) = data_home {
                command.env("XDG_DATA_HOME", data_home);
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            let help = String::from_utf8(output.stdout).unwrap();
            assert!(
                help.contains(&format!("[default: {}]", expected.display())),
                "{help}"
            );
        }
    }
    assert!(!home.exists());
    assert!(!data.exists());
}

#[test]
fn chat_database_help_remains_available_without_a_user_data_directory() {
    let output = Command::new(env!("CARGO_BIN_EXE_mf"))
        .args(["run", "--help"])
        .env_remove("HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(
        help.contains("User data directory unavailable; specify --chat-db."),
        "{help}"
    );
    assert!(!help.contains("[default:"), "{help}");
}

#[test]
fn missing_or_invalid_manifests_fail_without_executing_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("runner");
    let marker = directory.path().join("executed");
    fs::write(
        &runner,
        "#!/bin/sh\ntouch executed\nprintf 'unexpected help\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();
    for case in ["script", "missing", "invalid"] {
        match case {
            "script" => {}
            "missing" => write_runner(&runner, None),
            "invalid" => write_runner(&runner, Some(b"invalid manifest")),
            _ => unreachable!(),
        }
        let output = command(&runner)
            .arg("--help")
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("could not inspect workflow"));
        assert!(!marker.exists());
    }
    let output = command(&directory.path().join("missing"))
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not inspect workflow"));
}

#[test]
fn explicit_separator_and_named_input_values_keep_their_forwarding_semantics() {
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("runner");
    fs::write(&runner, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();
    for (flags, expected) in [
        (vec!["--", "--help"], "--help\n"),
        (vec!["--read.path", "-h"], "--read.path\n-h\n"),
        (
            vec!["--node", "transform", "--help"],
            "--node\ntransform\n--help\n",
        ),
        (vec!["--node=transform", "-h"], "--node=transform\n-h\n"),
        (vec!["--read.path=--help"], "--read.path=--help\n"),
        (vec!["--inputs=--help"], "--inputs\n--help\n"),
        (vec!["--inputs-file=--help"], "--inputs-file\n--help\n"),
        (
            vec!["--", "--node", "transform", "--help"],
            "--node\ntransform\n--help\n",
        ),
    ] {
        let output = command(&runner).args(&flags).output().unwrap();
        assert!(output.status.success(), "{flags:?}: {output:?}");
        assert_eq!(output.stdout, expected.as_bytes(), "{flags:?}");
        assert!(output.stderr.is_empty());
    }
}
