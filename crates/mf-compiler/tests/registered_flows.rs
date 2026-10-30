extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_resolved_definition};
use mf_runtime::{PortSpec, PreparedSubgraph, ValueType, WorkflowArguments};
use serde_json::json;

fn child() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"remote","kind":"unlinked.provider"}],
        "outputs":[{"name":"value","node":"remote","port":"value"}]
    }))
    .unwrap()
}

fn caller(body: &WorkflowDefinition) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"invoke","kind":"workflow.call","config":{"flow":"host.runner","definition":body}}],
        "outputs":[{"name":"value","node":"invoke","port":"value"}]
    }))
    .unwrap()
}

#[test]
fn registered_calls_use_host_execution_without_loading_child_providers() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let body = child();
    let definition = caller(&body);
    assert!(compile_resolved_definition(&definition, &registry).is_err());
    let credential = mf_runtime::ValueRef::from("credential").into_secret();
    let expected = credential.clone();
    registry
        .register_flow(
            "host.runner",
            body,
            PreparedSubgraph::new(
                vec![],
                vec![PortSpec::new("value", ValueType::String, true)],
                move |context| {
                    assert_eq!(
                        context.invocation_path(),
                        [mf_telemetry::event::ScopePathEntry::Call {
                            call_id: "invoke".into()
                        }]
                    );
                    let arguments = context.workflow_arguments();
                    assert!(arguments.0["remote.key"].is_secret());
                    assert!(arguments.0["remote.key"].ptr_eq(&expected));
                    Ok([("value".into(), arguments.0["remote.prompt"].clone())].into())
                },
            )
            .with_input_schema(
                serde_json::from_value(json!({"inputs":{"remote":{
                    "prompt":{"type":"string","required":true},
                    "key":{"type":"string","required":true,"binding":{"secret":true}}
                }},"stdin":{}}))
                .unwrap(),
            ),
        )
        .unwrap();
    let plan = compile_resolved_definition(&definition, &registry).unwrap();
    for prompt in ["first", "second"] {
        let arguments = WorkflowArguments(
            [
                ("invoke.remote.prompt".into(), prompt.into()),
                ("invoke.remote.key".into(), credential.clone()),
            ]
            .into(),
        );
        let result =
            mf_compiler::execute_compiled_with_inputs(&plan, &registry, arguments, None).unwrap();
        assert_eq!(result["value"].as_str(), Some(prompt));
    }
}

#[test]
fn registered_calls_reject_definition_drift_and_duplicate_names() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let body = child();
    let prepared = || PreparedSubgraph::new(vec![], vec![], |_| Ok(Default::default()));
    registry
        .register_flow("host.runner", body.clone(), prepared())
        .unwrap();
    assert!(matches!(
        registry.register_flow("host.runner", body.clone(), prepared()),
        Err(mf_runtime::NodeRegistryError::DuplicateFlow { .. })
    ));
    assert!(matches!(
        registry.register_flow(" ", body.clone(), prepared()),
        Err(mf_runtime::NodeRegistryError::BlankFlowName)
    ));
    let mut changed = body;
    changed.nodes[0].config = json!({"changed":true});
    let error = compile_resolved_definition(&caller(&changed), &registry).unwrap_err();
    assert!(error.to_string().contains("binding differs"), "{error}");
}
