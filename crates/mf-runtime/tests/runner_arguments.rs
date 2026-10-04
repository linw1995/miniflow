use mf_runtime::{RunnerCommand, WorkflowArguments};

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
