use mf_runtime::{MAX_WORKFLOW_INPUT_BYTES, RunnerCommand, WorkflowArguments};

#[test]
fn parameter_sources_and_modes_are_unambiguous() {
    for arguments in [
        vec!["--inputs"],
        vec!["--inputs", "{}", "--inputs", "{}"],
        vec!["--inputs", "{}", "--inputs-file", "missing"],
        vec!["--validate", "--inputs-file", "missing"],
        vec!["--describe", "--describe-interface"],
        vec!["--unknown"],
        vec!["--inputs", "{\"a\":{},\"a\":{}}"],
    ] {
        assert!(
            RunnerCommand::parse(arguments.iter().map(Into::into)).is_err(),
            "{arguments:?}"
        );
    }
    assert!(matches!(
        RunnerCommand::parse(["--describe-interface".into()]).unwrap(),
        RunnerCommand::DescribeInterface
    ));
    assert!(
        matches!(RunnerCommand::parse([]).unwrap(), RunnerCommand::Execute(values) if values == WorkflowArguments::default())
    );
}

#[test]
fn argument_files_preserve_values_and_enforce_the_transport_bound() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("arguments.json");
    std::fs::write(&path, br#"{"read":{"path":"data.jsonl","optional":null}}"#).unwrap();
    let expected = WorkflowArguments::from_file(&path).unwrap();
    let command =
        RunnerCommand::parse(["--inputs-file".into(), path.clone().into_os_string()]).unwrap();
    assert!(matches!(command, RunnerCommand::Execute(values) if values == expected));
    std::fs::write(&path, vec![b' '; MAX_WORKFLOW_INPUT_BYTES + 1]).unwrap();
    assert!(
        WorkflowArguments::from_file(&path)
            .unwrap_err()
            .to_string()
            .contains("limit")
    );
}
