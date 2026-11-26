#[cfg(unix)]
#[path = "support/runner_fixture.rs"]
mod runner_fixture;
use std::process::Command;

#[test]
fn help_succeeds_without_executing_a_command() {
    for (args, expected) in [
        (&["--help"][..], "compile"),
        (&["-h"][..], "run"),
        (&["compile", "--help"][..], "--no-telemetry"),
        (&["run", "--help"][..], "--tui"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mf"))
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("Usage: mf"), "{args:?}: {stdout}");
        assert!(stdout.contains(expected), "{args:?}: {stdout}");
    }
}

#[test]
fn version_matches_the_package() {
    let output = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("mf {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn invalid_arguments_report_diagnostics_on_stderr() {
    for args in [
        &[][..],
        &["unknown"],
        &["compile"],
        &["compile", "flow.json"],
        &["compile", "--output", "flow"],
        &["compile", "flow.json", "--output"],
        &["compile", "flow.json", "--output", "--locked"],
        &["compile", "flow.json", "--output", "flow", "--build-dir"],
        &["compile", "flow.json", "--output", "flow", "--unknown"],
        &["compile", "flow.json", "--output", "flow", "extra"],
        &["run"],
        &["run", "--tui"],
        &["run", "flow", "--tui", "--tui"],
        &["run", "flow", "--tui", "extra"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mf"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("--help"),
            "{args:?}: {output:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn direct_run_preserves_arguments_business_output_and_exit_status_without_forwarding_stdin() {
    use serde_json::json;
    use std::{io::Write, process::Stdio};

    let root = tempfile::tempdir().unwrap();
    let runner = root.path().join("runner with spaces");
    let manifest = mf_runtime::WorkflowManifest::from_graph(&mf_runtime::CompiledGraph {
        definition: serde_json::from_value(json!({"version":"2026-10-03","dependencies":{},"nodes":[{"id":"read","kind":"test.read"}]})).unwrap(),
        execution_order: vec!["read".into()], relations: Default::default(), children: Default::default()
    }, serde_json::from_value(json!({"inputs":{"read":{"path":{"type":"string","required":false}}},"stdin":{}})).unwrap()).unwrap();
    runner_fixture::write(
        &runner,
        Some(&manifest.to_bytes().unwrap()),
        r#"
[ -z "${MF_RUNNER_RPC_V1+x}" ] || exit 71
if read -r unexpected; then exit 72; fi
if [ "$1" = --read.path=signal ]; then kill -TERM $$; fi
python3 -c 'import json,sys;print(json.dumps({"args":sys.argv[1:]}))' "$@"
printf 'runner diagnostic\n' >&2
exit 37
"#,
    );
    for (flags, expected) in [
        (
            vec!["--read.path", "file with spaces"],
            "--read.path\nfile with spaces\n",
        ),
        (vec!["--read.path", "-h"], "--read.path\n-h\n"),
        (vec!["--read.path=--help"], "--read.path=--help\n"),
        (vec!["-v", "--inputs", "{}"], "--verbose\n--inputs\n{}\n"),
        (
            vec!["-vv", "--read.path", "-vvv"],
            "--verbose\n--verbose\n--read.path\n-vvv\n",
        ),
        (
            vec!["-vvvv", "--inputs", "{}"],
            "--verbose\n--verbose\n--verbose\n--inputs\n{}\n",
        ),
        (
            vec!["--verbose", "--inputs-file", "parameters with spaces.json"],
            "--verbose\n--inputs-file\nparameters with spaces.json\n",
        ),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
            .arg("run")
            .arg(&runner)
            .args(flags)
            .env("MF_RUNNER_RPC_V1", "must not reach the child")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"piped input\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
            json!({"args":expected.lines().collect::<Vec<_>>()})
        );
        assert_eq!(output.stderr, b"runner diagnostic\n");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_mf"))
        .args(["-v", "run"])
        .arg(&runner)
        .arg("-vv")
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(37));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"args":["--verbose","--verbose","--verbose"]})
    );
    let output = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(&runner)
        .arg("--read.path=signal")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(143));
}

#[test]
fn direct_run_reports_launch_failure() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(root.path().join("missing-runner"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not inspect workflow"));
}
