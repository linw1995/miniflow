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
        &["run", "flow"],
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
