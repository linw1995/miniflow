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
        "nodes":[{"id":"invoke","kind":"workflow.call","config":{"flow":"host.runner","body":body}}],
        "outputs":[{"name":"value","node":"invoke","port":"value"}]
    }))
    .unwrap()
}

#[test]
fn callback_authority_is_unique_scoped_and_absent_from_movable_children() {
    use mf_runtime::callback::{CallbackReply, HostCallback};
    let body = json!({
        "nodes": [{"id":"arg", "kind":"builtin.parameters", "config": {
            "parameters": {"s": {"type":"string", "default":"fallback"}}
        }}],
        "outputs": [{"name":"value", "node":"arg", "port":"s"}]
    });
    let call = |id: &str, name: &str| {
        json!({
            "id":id, "kind":"workflow.call", "config":{"flow":format!("callback:{name}"), "body":body}
        })
    };
    let doc: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-11-13", "dependencies":{},
        "nodes": [call("selected", "choose"), call("other", "ambiguous"), call("third", "ambiguous"), {
            "id":"tool", "kind":"builtin.constant", "config":{"type":"flow", "value":{
                "version":"2026-11-13", "dependencies":{}, "nodes":[call("inner", "choose")],
                "outputs":[{"name":"value", "node":"inner", "port":"value"}]
            }}
        }],
        "outputs":[{"name":"value", "node":"selected", "port":"value"},
                   {"name":"other", "node":"other", "port":"value"}]
    })).unwrap();
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_resolved_definition(&doc, &registry).unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    let mut invalid = manifest.clone();
    for path in [
        vec![],
        vec!["missing".to_owned()],
        vec!["selected".to_owned()],
    ] {
        invalid.descriptor.callbacks.insert("other".into(), path);
        assert!(invalid.validate().is_err());
    }
    let sites = manifest.descriptor.callbacks;
    assert_eq!(sites, [("choose".into(), vec!["selected".into()])].into());
    let callback = HostCallback::new(|args, _| {
        assert!(args.0["arg.s"].is_secret());
        Ok(CallbackReply::from(mf_runtime::Outputs::from([(
            "value".into(),
            "bound".into(),
        )])))
    });
    let bound = [("choose".into(), callback.clone())].into();
    assert!(
        registry
            .bind_callbacks(
                &sites,
                &[],
                &[
                    ("choose".into(), callback.clone()),
                    ("unbound".into(), callback)
                ]
                .into()
            )
            .is_err()
    );
    registry.bind_callbacks(&sites, &[], &bound).unwrap();
    assert!(registry.bind_callbacks(&sites, &[], &bound).is_err());
    let result = mf_compiler::execute_compiled(&plan, &registry, None).unwrap();
    assert_eq!(result["value"].as_str(), Some("bound"));
    assert_eq!(result["other"].as_str(), Some("fallback"));
}

#[test]
fn a_reserved_callback_declaration_needs_a_nonblank_binding_name() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-28", "dependencies":{}, "nodes":[{
            "id":"bad", "kind":"workflow.call", "config":{"flow":"callback: ", "body":{
                "nodes":[{"id":"literal","kind":"builtin.constant","config":{"value":1}}],
                "outputs":[{"name":"value","node":"literal","port":"value"}]
            }}
        }]
    }))
    .unwrap();
    let error = compile_resolved_definition(&definition, &registry).unwrap_err();
    assert!(
        error.to_string().contains("host callbacks require a name"),
        "{error}"
    );
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
    assert_eq!(
        plan.graph.children[&"invoke".into()]["body"]
            .relations
            .host
            .as_deref(),
        Some("host.runner")
    );
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

    // A host body does not grant a global exemption for its unlinked node kinds.
    // The same kind outside that body needs a provider before any factory runs.
    let mut outside = definition;
    outside.nodes.insert(
        0,
        serde_json::from_value(json!({
            "id": "a_before", "kind": "builtin.constant", "config": {}
        }))
        .unwrap(),
    );
    outside.nodes.push(
        serde_json::from_value(json!({
            "id": "z_missing", "kind": "unlinked.provider"
        }))
        .unwrap(),
    );
    let plan = mf_compiler::plan_resolved_definition(&outside, &registry).unwrap();
    let saved = plan.clone();
    assert_eq!(saved.graph.execution_order[0].as_str(), "a_before");
    // Constructing the earlier constant would fail on its missing value; instead
    // availability preflight must report the later unlinked provider first.
    assert!(matches!(
        mf_compiler::instantiate_compiled(&saved, &registry).err().unwrap(),
        mf_compiler::WorkflowCompileError::UnknownNodeKind { definition_id, kind }
        if definition_id.as_str() == "z_missing" && kind == "unlinked.provider"
    ));
}

#[test]
fn preparing_a_call_body_requires_a_root_call_with_an_authored_body() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"invoke", "kind":"workflow.call", "config":{"flow":"inline", "body":{
                "nodes":[{"id":"literal", "kind":"builtin.constant", "config":{"value":"inside"}}],
                "outputs":[{"name":"value", "node":"literal", "port":"value"}]
            }}},
            {"id":"literal", "kind":"builtin.constant", "config":{"value":"outside"}},
            {"id":"dynamic", "kind":"workflow.call", "config":{"inputs":[], "outputs":[]}}
        ],
        "outputs":[{"name":"value", "node":"invoke", "port":"value"}]
    }))
    .unwrap();
    let plan = mf_compiler::plan_resolved_definition(&definition, &registry).unwrap();
    let body = mf_compiler::prepare_call_body(&plan, &registry, "invoke").unwrap();
    let origin = body.origin().unwrap();
    assert_eq!(origin.workflow_id, plan.graph.workflow_id().unwrap());
    assert_eq!(origin.path, ["invoke"]);
    let outputs = body.execute_in_context(&mut Default::default()).unwrap();
    assert_eq!(outputs["value"].as_str(), Some("inside"));

    for (id, expected) in [
        ("literal", "expected a root Flow call"),
        ("dynamic", "expected an authored Flow call body"),
    ] {
        assert!(matches!(
            mf_compiler::prepare_call_body(&plan, &registry, id),
            Err(mf_compiler::WorkflowCompileError::InvalidGraph { definition_id, message })
                if definition_id.as_str() == id && message == expected
        ));
    }
}

#[cfg(feature = "codegen")]
#[test]
fn registered_host_calls_cannot_be_emitted_into_generated_runners() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let body = child();
    registry
        .register_flow(
            "host.runner",
            body.clone(),
            PreparedSubgraph::new(
                vec![],
                vec![PortSpec::new("value", ValueType::String, true)],
                |_| panic!("code generation must not execute a host closure"),
            ),
        )
        .unwrap();
    let plan = compile_resolved_definition(&caller(&body), &registry).unwrap();
    let error = plan
        .generate_artifacts(
            &registry,
            mf_compiler::CodegenOptions {
                preserve_intermediate_outputs: true,
            },
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("host Flow executors cannot be emitted"),
        "{error}"
    );
}

#[test]
fn registered_calls_compare_nested_bindings_against_the_actual_compiled_tree() {
    let mut body = caller(&child());
    body.nodes[0].id = "nested".into();
    body.outputs[0].node = "nested".into();
    let mut loop_body = child();
    loop_body.outputs.clear();
    body.nodes.push(
        serde_json::from_value(json!({
            "id":"repeat", "kind":"loop.run", "config":{
                "max_iterations":1, "variables":[{"name":"count", "type":"int"}],
                "body":loop_body
            }
        }))
        .unwrap(),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::plan_resolved_definition(&caller(&body), &registry).unwrap();
    let saved = plan.clone();

    for case in [
        "matching",
        "root inherited version",
        "stale raw body",
        "compiled child drift",
        "missing registered body",
    ] {
        let mut plan = saved.clone();
        let mut registered = body.clone();
        let host = plan
            .graph
            .children
            .get_mut(&"invoke".into())
            .unwrap()
            .get_mut("body")
            .unwrap();
        match case {
            "root inherited version" => {
                registered.version = mf_runtime::WorkflowDefinitionVersion::V2026_10_25;
                host.definition.version = registered.version;
                for node in &mut registered.nodes {
                    let body = node.config["body"].as_object_mut().unwrap();
                    body.remove("version");
                    body.remove("dependencies");
                }
                assert!(
                    host.children
                        .values()
                        .flat_map(|bindings| bindings.values())
                        .all(|child| child.definition.version == saved.graph.definition.version)
                );
            }
            "stale raw body" => {
                // Only the stored child tree is executable, not this inline copy.
                let nested = host
                    .definition
                    .nodes
                    .iter_mut()
                    .find(|node| node.id.as_str() == "nested")
                    .unwrap();
                nested.config["body"]["nodes"][0]["config"] = json!({"changed":true});
            }
            "compiled child drift" => {
                let nested = host
                    .children
                    .get_mut(&"nested".into())
                    .unwrap()
                    .get_mut("body")
                    .unwrap();
                nested.definition.nodes[0].config = json!({"changed":true});
            }
            "missing registered body" => {
                // The registered owner still exists, but has no raw body for the
                // actual nested binding selected by the compiled graph.
                registered.nodes[0]
                    .config
                    .as_object_mut()
                    .unwrap()
                    .remove("body");
            }
            _ => {}
        }
        // Authoring order and synthesized Loop sources are not definition drift.
        registered.nodes.reverse();
        let mut registry = NodeRegistry::from_inventory().unwrap();
        registry
            .register_flow(
                "host.runner",
                registered,
                PreparedSubgraph::new(
                    vec![],
                    vec![PortSpec::new("value", ValueType::String, true)],
                    |_| Ok([("value".into(), "host result".into())].into()),
                ),
            )
            .unwrap();
        let result = mf_compiler::instantiate_compiled(&plan, &registry);
        if matches!(
            case,
            "matching" | "root inherited version" | "stale raw body"
        ) {
            // Unknown providers in both nested bodies remain host-local exemptions.
            assert_eq!(
                result.unwrap().execute().unwrap()["value"],
                json!("host result")
            );
            continue;
        }
        let error = result.err().unwrap();
        let mf_compiler::WorkflowCompileError::CompiledGraph { source } = &error else {
            panic!("{case}: {error}");
        };
        let mf_compiler::WorkflowBuildError::HostFlowBinding {
            definition_id,
            source,
        } = source.as_ref()
        else {
            panic!("{case}: {source}");
        };
        assert_eq!(definition_id.as_str(), "invoke", "{case}");
        let mf_compiler::WorkflowCompileError::InvalidGraph {
            definition_id,
            message,
        } = source.as_ref()
        else {
            panic!("{case}: {source}");
        };
        let expected = if case == "missing registered body" {
            ("nested", "host Flow body does not match compiled bindings")
        } else {
            (
                "invoke",
                "host Flow binding differs from the frozen call definition",
            )
        };
        assert_eq!(
            (definition_id.as_str(), message.as_str()),
            expected,
            "{case}"
        );
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
