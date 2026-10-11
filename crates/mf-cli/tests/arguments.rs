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
fn direct_run_preserves_stdin_arguments_output_and_exit_status() {
    use std::{fs, io::Write, os::unix::fs::PermissionsExt, process::Stdio};

    let root = tempfile::tempdir().unwrap();
    let runner = root.path().join("runner with spaces");
    fs::write(&runner, "#!/bin/sh\nIFS= read -r line\nprintf '%s\\n' \"$line\" \"$@\"\nprintf 'runner diagnostic\\n' >&2\nexit 37\n").unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();
    for (flags, expected) in [
        (
            vec!["--read.path", "file with spaces"],
            "--read.path\nfile with spaces\n",
        ),
        (vec!["-v", "--inputs", "{}"], "--verbose\n--inputs\n{}\n"),
        (
            vec!["--verbose", "--inputs-file", "parameters with spaces.json"],
            "--verbose\n--inputs-file\nparameters with spaces.json\n",
        ),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
            .arg("run")
            .arg(&runner)
            .args(flags)
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
            String::from_utf8(output.stdout).unwrap(),
            format!("piped input\n{expected}")
        );
        assert_eq!(output.stderr, b"runner diagnostic\n");
    }
    fs::write(&runner, "#!/bin/sh\nkill -TERM $$\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(&runner)
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not run workflow"));
}
