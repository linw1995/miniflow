use mf_runtime::WorkflowArguments;

#[test]
fn parameter_sources_are_unambiguous() {
    for arguments in [
        vec!["--inputs"],
        vec!["--inputs", "{}", "--inputs", "{}"],
        vec!["--inputs", "{}", "--inputs-file", "missing"],
        vec!["--unknown"],
        vec!["--inputs", "{\"a\":{},\"a\":{}}"],
    ] {
        assert!(
            WorkflowArguments::parse(arguments.iter().map(Into::into)).is_err(),
            "{arguments:?}"
        );
    }
    for flag in ["--validate", "--describe", "--describe-interface"] {
        let error = WorkflowArguments::parse([flag.into()]).unwrap_err();
        assert!(error.to_string().contains("unknown workflow argument"));
    }
    assert_eq!(
        WorkflowArguments::parse([]).unwrap(),
        WorkflowArguments::default()
    );
}
