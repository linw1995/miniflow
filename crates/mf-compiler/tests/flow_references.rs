extern crate mfn_core as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{ExecutionContext, WorkflowArguments};
use mf_telemetry::{
    event::{Event, ScopePathEntry},
    identity::RunId,
};
use serde_json::{Value, json};

fn child(content: &str) -> Value {
    json!({
        "version":"2026-11-01", "dependencies":{},
        "nodes":[{"id":"leaf", "kind":"builtin.constant", "config":{"value":content}}],
        "outputs":[{"name":"content", "node":"leaf", "port":"value"}]
    })
}

fn caller() -> Value {
    json!({
        "version":"2026-11-01", "dependencies":{},
        "flow_arguments":{
            "first":{"definition":child("one"), "description":null, "parameters":{}},
            "second":{"definition":child("two"), "description":null, "parameters":{}}
        },
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{"target":{"type":"any", "secret":false}}}},
            {"id":"arguments", "kind":"builtin.constant", "config":{"value":{}}},
            {"id":"invoke", "kind":"workflow.call", "config":{"flow_ref":{"$ref":"params.target"}, "arguments_object":true,"capture_errors":true},
             "inputs":{"arguments":{"from":{"node":"arguments", "port":"value"}}}}
        ],
        "edges":[{"from_node":"params", "to_node":"invoke"}],
        "outputs":[{"name":"result", "node":"invoke", "port":"result"}]
    })
}

#[test]
fn passed_references_select_frozen_bodies_and_keep_binding_observations_distinct() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = WorkflowDefinition::from_json(&caller().to_string()).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(
        graph.nodes.len(),
        3,
        "bindings are bodies, not wrapper tasks"
    );
    assert_eq!(graph.injected_bodies.len(), 2);
    let frozen =
        WorkflowDefinition::from_json(&serde_json::to_string(&plan.definition).unwrap()).unwrap();
    let rebuilt = compile_definition(&frozen, &registry).unwrap();
    assert_eq!(rebuilt.definition, plan.definition);
    let harness = capture::Harness::new(true);
    for (name, content) in [("first", "one"), ("second", "two")] {
        let observation = harness
            .observer()
            .start_with_description(graph.clone(), RunId::new())
            .unwrap();
        let outputs = mf_compiler::execute_compiled_with_inputs(
            &rebuilt,
            &registry,
            WorkflowArguments::try_from(json!({"params.target":name})).unwrap(),
            Some(observation),
        )
        .unwrap();
        assert_eq!(
            outputs["result"]["outputs"]["content"].as_str(),
            Some(content)
        );
    }
    let mut bindings = std::collections::BTreeSet::new();
    for record in harness.records() {
        let event = record.decode().unwrap();
        event.validate_for(&graph).unwrap();
        if let Event::NodeStarted { node, .. } = event.event
            && node.id == "leaf"
        {
            let [ScopePathEntry::CallBinding { call_id, binding }] = node.path.as_slice() else {
                panic!("missing binding identity")
            };
            assert_eq!(call_id, "invoke");
            bindings.insert(binding.clone());
        }
    }
    assert_eq!(bindings, ["first".to_owned(), "second".to_owned()].into());
    assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
}

#[test]
fn selectors_are_data_and_cannot_execute_json_or_global_host_bindings() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    registry
        .register_flow(
            "host-only",
            serde_json::from_value(child("host")).unwrap(),
            mf_runtime::PreparedSubgraph::new(vec![], vec![], |_| {
                panic!("global binding must never execute")
            }),
        )
        .unwrap();
    let definition = WorkflowDefinition::from_json(&caller().to_string()).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for target in [
        json!("host-only"),
        json!("unknown"),
        child("forged"),
        json!({"definition":child("forged")}),
        json!(null),
    ] {
        let mut context = ExecutionContext::default();
        context.set_workflow_arguments(
            WorkflowArguments::try_from(json!({"params.target":target})).unwrap(),
        );
        assert!(flow.execute_in_context(&mut context).is_err());
    }
}

#[test]
fn references_require_explicit_prerequisites_and_do_not_inherit_across_calls() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = caller();
    value["edges"] = json!([]);
    let error = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap_err();
    assert!(error.to_string().contains("explicit dependency"), "{error}");

    let nested = caller();
    let mut nested = nested;
    nested.as_object_mut().unwrap().remove("flow_arguments");
    let mut value = caller();
    value["nodes"][2]["config"] = json!({"flow":"nested", "definition":nested, "arguments_object":true,"capture_errors":true});
    value["nodes"][1]["config"]["value"] = json!({"params.target":"first"});
    let definition = serde_json::from_value(value).unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let outputs = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(WorkflowArguments::try_from(json!({"params.target":"first"})).unwrap())
        .unwrap();
    assert!(
        outputs["result"]["error"]
            .as_str()
            .unwrap()
            .contains("unknown passed Flow parameter")
    );
}

fn host_definition() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"remote", "kind":"unlinked.provider"}],
        "outputs":[{"name":"value", "node":"remote", "port":"value"}]
    }))
    .unwrap()
}

fn host_caller() -> Value {
    let mut value = caller();
    value["flow_arguments"]["first"]["definition"] =
        serde_json::to_value(host_definition()).unwrap();
    value["nodes"][0]["config"]["parameters"]["arguments"] =
        json!({"type":"object", "secret":false});
    value["nodes"][2]["inputs"]["arguments"] =
        json!({"from":{"node":"params", "port":"arguments"}});
    value
}

#[test]
fn explicitly_passed_host_binding_uses_its_executor_and_input_contract_without_child_providers() {
    use mf_runtime::{PortSpec, PreparedSubgraph, ValueRef, ValueType};
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let definition = serde_json::from_value(host_caller()).unwrap();
    assert!(compile_definition(&definition, &registry).is_err());
    let credential = ValueRef::from("credential").into_secret();
    let expected = credential.clone();
    let executions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = executions.clone();
    registry
        .register_flow(
            "first",
            host_definition(),
            PreparedSubgraph::new(
                vec![],
                vec![PortSpec::new("value", ValueType::String, true)],
                move |context| {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    assert_eq!(
                        context.invocation_path(),
                        [ScopePathEntry::CallBinding {
                            call_id: "invoke".into(),
                            binding: "first".into()
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
        "prompt":{"type":"string", "required":true},
        "key":{"type":"string", "required":true,"binding":{"secret":true}}
    }}, "stdin":{}}))
                .unwrap(),
            ),
        )
        .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let frozen =
        WorkflowDefinition::from_json(&serde_json::to_string(&plan.definition).unwrap()).unwrap();
    let rebuilt = compile_definition(&frozen, &registry).unwrap();
    let flow = instantiate_compiled(&rebuilt, &registry).unwrap();
    assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 0);
    let outputs = flow
        .execute_with_inputs(WorkflowArguments(
            [
                ("params.target".into(), "first".into()),
                (
                    "params.arguments".into(),
                    ValueRef::object([
                        ("remote.prompt".into(), "hello host".into()),
                        ("remote.key".into(), credential),
                    ]),
                ),
            ]
            .into(),
        ))
        .unwrap();
    assert_eq!(
        outputs["result"]["outputs"]["value"].as_str(),
        Some("hello host")
    );
    assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(outputs["result"]["outputs"]["value"].is_secret());
    #[cfg(feature = "codegen")]
    {
        let error = plan.generate_execution_plans(&registry).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("host Flow executors cannot be emitted"),
            "{error}"
        );
        let static_definition = serde_json::from_value(json!({
            "version":"2026-10-28", "dependencies":{},
            "nodes":[{"id":"invoke","kind":"workflow.call","config":{"flow":"first","definition":host_definition()}}],
            "outputs":[{"name":"value","node":"invoke","port":"value"}]
        })).unwrap();
        let static_plan = compile_definition(&static_definition, &registry).unwrap();
        let error = static_plan.generate_execution_plans(&registry).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("host Flow executors cannot be emitted"),
            "{error}"
        );
    }
}

#[test]
fn passed_host_definition_drift_is_rejected_for_selected_and_unselected_bindings() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    for name in ["first", "second"] {
        registry
            .register_flow(
                name,
                host_definition(),
                mf_runtime::PreparedSubgraph::new(vec![], vec![], |_| {
                    panic!("preparation must never invoke a host executor")
                }),
            )
            .unwrap();
    }
    for changed in ["first", "second"] {
        let mut value = caller();
        for name in ["first", "second"] {
            value["flow_arguments"][name]["definition"] =
                serde_json::to_value(host_definition()).unwrap();
        }
        value["nodes"][0]["config"]["parameters"]["target"]["default"] = json!("first");
        value["flow_arguments"][changed]["definition"]["nodes"][0]["config"] =
            json!({"changed":true});
        let error =
            compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("host Flow binding differs from the frozen call definition"),
            "changed {changed}: {error}"
        );
    }
}

#[test]
fn passed_host_executors_still_obey_terminal_reference_call_contracts() {
    for name in ["first", "second"] {
        let mut registry = NodeRegistry::from_inventory().unwrap();
        registry
            .register_flow(
                name,
                host_definition(),
                mf_runtime::PreparedSubgraph::streaming(
                    vec![],
                    vec![],
                    vec![mf_runtime::PortSpec::new(
                        "value",
                        mf_runtime::ValueType::String,
                        true,
                    )],
                    |_, _| panic!("invalid host body must not execute"),
                ),
            )
            .unwrap();
        let mut value = caller();
        value["nodes"][0]["config"]["parameters"]["target"]["default"] = json!("first");
        value["flow_arguments"][name]["definition"] =
            serde_json::to_value(host_definition()).unwrap();
        let error =
            compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("terminal children without stdin"),
            "{error}"
        );
    }
}

#[test]
fn every_passed_body_is_prepared_even_when_the_selected_binding_is_valid() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = caller();
    value["flow_arguments"]["second"]["definition"]["nodes"][0]["kind"] = json!("missing.provider");
    let error = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap_err();
    assert!(error.to_string().contains("missing.provider"), "{error}");
    for extra in [
        json!({"$template":"{{params.target}}"}),
        json!({"$ref":"params.target", "definition":child("forged")}),
    ] {
        let mut value = caller();
        value["nodes"][2]["config"]["flow_ref"] = extra;
        assert!(compile_definition(&serde_json::from_value(value).unwrap(), &registry).is_err());
    }
}
