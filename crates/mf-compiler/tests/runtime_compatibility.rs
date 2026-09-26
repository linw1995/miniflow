use mf_compiler::validate_runtime_identity;
use serde_json::json;

#[test]
fn rejects_multiple_runtime_versions_and_sources_with_dependency_paths() {
    for other in [
        "registry+index#mf-runtime@0.2.0",
        "git+repo#mf-runtime@0.1.0",
    ] {
        let mut metadata = json!({
            "packages":[{"name":"mf-runtime","id":"registry+index#mf-runtime@0.1.0"}],
            "resolve":{"root":"runner", "nodes":[
                {"id":"runner","deps":[{"name":"mf_runtime","pkg":"registry+index#mf-runtime@0.1.0"},{"name":"node_0","pkg":"plugin"}]},
                {"id":"plugin","deps":[{"name":"mf_runtime","pkg":other}]}
            ]}
        });
        assert!(validate_runtime_identity(&metadata).is_ok());
        metadata["packages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"mf-runtime","id":other}));
        let error = validate_runtime_identity(&metadata)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("runner -> plugin -> {other}")),
            "{error}"
        );
    }
}
